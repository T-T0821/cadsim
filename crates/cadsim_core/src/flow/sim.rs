//! 工程モデルの離散事象シミュレーション (Bevy に依存しない)
//!
//! 「ワークが届いた」「加工が終わった」「故障した」「直った」の事象を時刻順に処理し、
//! 事象のたびに、送り出せるワークを下流へ動かせるだけ動かす。時刻は事象から事象へ飛ぶので、
//! 数時間分のラインでも一瞬で回せる。[`Simulation::advance`] で少しずつ進めれば、
//! 3D ビューで状態の色を見ながら早送りもできる。
//!
//! 乱数は種から決まる自前の SplitMix64 なので、同じ種なら必ず同じ結果になる。
//!
//! ライン連携 ([`Simulation::link`]) をすると、ワークの移動を [`FlowEvent`] として記録し、
//! AGV で運ぶ要素は自分では送り出さずに運び手を待つ ([`Simulation::pick_up`] /
//! [`Simulation::deliver`])。
//!
//! 人が要る工程 ([`Labor`]) は、ワークが届くと「人待ち」になり、その工程ができて手の空いている
//! 出勤中の作業者を呼ぶ。来られる人が複数いれば、割り当てのルール ([`AssignRule`]) で選ぶ
//! (速い人 / いちばん長く手待ちしている人 / 持ち場の人)。
//! 作業者は手が空くと、人待ちの工程のうちいちばん長く待っているところへ行く
//! (「持ち場の人が先」なら自分の持ち場を先に見る)。
//! 別の工程へ移るときは歩行時間がかかる。シフトの終わりや休憩に入るときは、
//! 今の作業を終えてから抜ける (作業の途中では抜けない)。

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, VecDeque};

use super::model::{ElementId, ElementKind, FlowModel, Labor, RunSettings};
use crate::staffing::model::{AssignRule, WorkerId};

/// 設備の状態
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    /// 加工中 (生成は使わない。バッファはワークがあるとき)
    Working,
    /// ワーク待ち (前の工程から来ない・生成の間隔待ち)
    Waiting,
    /// 詰まり (加工は終わったが、送り先がいっぱいで出せない)
    Blocked,
    /// 故障で停止中
    Down,
    /// 人待ち (ワークはあるが、作業者が来ない)
    NoOperator,
}

impl State {
    pub const ALL: [State; 5] = [
        State::Working,
        State::Waiting,
        State::Blocked,
        State::Down,
        State::NoOperator,
    ];

    pub fn label(self) -> &'static str {
        match self {
            State::Working => "稼働",
            State::Waiting => "待ち",
            State::Blocked => "詰まり",
            State::Down => "故障",
            State::NoOperator => "人待ち",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// 状態ごとの時間 (秒)。[`State::ALL`] の順。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StateTimes(pub [f64; 5]);

impl StateTimes {
    pub fn get(&self, state: State) -> f64 {
        self.0[state.index()]
    }

    pub fn total(&self) -> f64 {
        self.0.iter().sum()
    }

    /// 全体に対する割合 (0〜1)。時間が 0 なら 0。
    pub fn ratio(&self, state: State) -> f64 {
        let total = self.total();
        if total > 0.0 {
            self.get(state) / total
        } else {
            0.0
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Work {
    /// 生成された時刻
    created: f64,
}

/// 実行中の要素1つ分
#[derive(Clone, Debug)]
struct Runtime {
    state: State,
    since: f64,
    times: StateTimes,
    /// 工程: 機械の中のワーク。生成: 出せずに持っているワーク。
    slot: Option<Work>,
    /// 工程: 加工が終わっていて送り出すのを待っている
    done: bool,
    /// 工程: 故障で止まったときの残りの加工時間
    remaining: f64,
    /// 工程: 次の故障までの加工時間
    time_to_failure: f64,
    /// バッファ・搬送 (搬送は運んでいるワーク)
    queue: VecDeque<Work>,
    /// 搬送: `queue` の各ワークが届く時刻 (先頭ほど早い)
    ready: VecDeque<f64>,
    /// 生成した数 (上限の判定用。立ち上がりも含む)
    created: u32,
    /// 集計期間に送り出した数 (排出は受け取った数)
    count: u32,
    failures: u32,
    /// 中身の数 × 時間の積分と最大 (バッファ・工程)
    content_area: f64,
    content_since: f64,
    content_max: usize,
    /// 排出: 生成から排出までの時間の合計
    lead_sum: f64,
    /// 人が要る工程: 付いている (向かっている) 作業者 (作業者の並びの添字)
    operator: Option<usize>,
    /// 人が要る工程: 人が来たら始める加工時間 (秒、人の速さで割る前)
    pending: Option<f64>,
}

impl Runtime {
    fn new() -> Self {
        Self {
            state: State::Waiting,
            since: 0.0,
            times: StateTimes::default(),
            slot: None,
            done: false,
            remaining: 0.0,
            time_to_failure: f64::INFINITY,
            queue: VecDeque::new(),
            ready: VecDeque::new(),
            created: 0,
            count: 0,
            failures: 0,
            content_area: 0.0,
            content_since: 0.0,
            content_max: 0,
            lead_sum: 0.0,
            operator: None,
            pending: None,
        }
    }

    fn content(&self) -> usize {
        self.queue.len() + usize::from(self.slot.is_some())
    }
}

/// 作業者の状態
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkerState {
    /// 工程で作業中
    Working,
    /// 工程へ歩いている
    Walking,
    /// 出勤しているが手が空いている
    Idle,
    /// 勤務時間外・休憩中
    OffDuty,
}

impl WorkerState {
    pub const ALL: [WorkerState; 4] = [
        WorkerState::Working,
        WorkerState::Walking,
        WorkerState::Idle,
        WorkerState::OffDuty,
    ];

    pub fn label(self) -> &'static str {
        match self {
            WorkerState::Working => "作業",
            WorkerState::Walking => "歩行",
            WorkerState::Idle => "手待ち",
            WorkerState::OffDuty => "休み",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// 作業者の状態ごとの時間 (秒)。[`WorkerState::ALL`] の順。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WorkerTimes(pub [f64; 4]);

impl WorkerTimes {
    pub fn get(&self, state: WorkerState) -> f64 {
        self.0[state.index()]
    }

    /// 出勤している時間 (休み以外)
    pub fn on_duty(&self) -> f64 {
        self.0.iter().sum::<f64>() - self.get(WorkerState::OffDuty)
    }

    /// 出勤している時間に対する割合 (0〜1)
    pub fn ratio(&self, state: WorkerState) -> f64 {
        let on = self.on_duty();
        if on > 0.0 && state != WorkerState::OffDuty {
            self.get(state) / on
        } else {
            0.0
        }
    }
}

/// 実行中の作業者1人分 (欠勤の人は入れない)
#[derive(Clone, Debug)]
struct WorkerRt {
    id: WorkerId,
    name: String,
    state: WorkerState,
    since: f64,
    times: WorkerTimes,
    /// シフト上、今働く時間か
    on_duty: bool,
    /// 最後にいた工程 (要素の添字)。歩行時間の判定に使う。
    at: Option<usize>,
    /// シフトがあれば次の切り替えを入れる
    shift: Option<crate::staffing::model::Shift>,
    /// 要素ごとの作業の速さ (できない工程は None)
    speed: Vec<Option<f64>>,
    /// 持ち場 (要素の添字)
    primary: Option<usize>,
    /// 持ち場のライン。ほかのラインの作業は応援として数える
    home_line: Option<super::model::LineId>,
    /// 集計期間に終えた作業の数
    tasks: u32,
    /// そのうち、持ち場のライン以外での作業の数
    support_tasks: u32,
    /// 集計期間に歩いた回数
    walks: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EventKind {
    /// 生成がワークを出す
    Arrival,
    /// 工程の加工が終わる
    Done,
    /// 工程が故障する
    Fail,
    /// 工程の修理が終わる
    Repair,
    /// 作業者 (`element` は作業者の添字) のシフト・休憩が切り替わる
    WorkerShift,
    /// 工程に作業者が歩いて着く
    WorkerArrive,
    /// 投入・段取りが終わって、工程の作業者の手が空く
    OperatorRelease,
    /// 搬送しているワークが届く
    TransportArrive,
}

#[derive(Clone, Copy, Debug)]
struct Event {
    time: f64,
    /// 同じ時刻の事象は入れた順に処理する
    seq: u64,
    element: usize,
    kind: EventKind,
}

impl PartialEq for Event {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Event {}
impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Event {
    /// BinaryHeap は大きい方から出すので、早い事象ほど大きくする
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .time
            .total_cmp(&self.time)
            .then(other.seq.cmp(&self.seq))
    }
}

/// 種から決まる乱数 (SplitMix64)
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// [0, 1) の一様乱数
    fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// 平均 `mean` の指数分布
    fn exponential(&mut self, mean: f64) -> f64 {
        -mean * (1.0 - self.uniform()).ln()
    }

    /// `mean` の ± `spread` 割合の一様分布
    fn around(&mut self, mean: f64, spread: f64) -> f64 {
        if spread <= 0.0 {
            mean
        } else {
            mean * (1.0 + spread * (2.0 * self.uniform() - 1.0))
        }
    }
}

/// ライン連携で記録する出来事
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlowEvent {
    /// ワークが `from` から `to` へ移った。`carried` は AGV が運んで降ろしたもの。
    Moved {
        time: f64,
        from: ElementId,
        to: ElementId,
        carried: bool,
    },
    /// AGV で送り出す要素に、運んでもらうワークが1個できた
    CarrierNeeded { time: f64, element: ElementId },
}

/// AGV に積まれて運ばれている途中のワークの荷札
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LoadId(pub u64);

/// ライン連携の状態
#[derive(Clone, Debug, Default)]
struct Link {
    /// 送り出しを外 (AGV) に任せる要素 (要素の並びの添字)
    external: Vec<bool>,
    /// 運び手を頼んだが、まだ積まれていない数
    requested: Vec<usize>,
    /// 運ばれている途中のワーク: 荷札 → (積んだ要素, ワーク)
    transit: BTreeMap<LoadId, (usize, Work)>,
    next_load: u64,
    /// 降ろされたが、送り先が空かずにまだ入れていないワーク
    deliveries: VecDeque<(usize, Work)>,
    events: Vec<FlowEvent>,
    /// 投入を止めている生成
    held_sources: Vec<bool>,
    /// 投入を止めている間に来た「出す時刻」(再開したらすぐ出す)
    held_arrivals: Vec<bool>,
}

/// グラフ用に一定間隔で記録する値
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub time: f64,
    /// ライン内のワーク数 (仕掛数)
    pub wip: usize,
    /// 開始からの排出数の合計
    pub output: u32,
    /// 要素ごとの状態 (要素の並び順)
    pub states: Vec<State>,
}

/// 記録する点の数
const SAMPLE_COUNT: usize = 240;

/// 実行中のシミュレーション
#[derive(Clone, Debug)]
pub struct Simulation {
    model: FlowModel,
    /// 要素ごとの送り先 (要素の並びの添字)
    next: Vec<Vec<usize>>,
    runtime: Vec<Runtime>,
    events: BinaryHeap<Event>,
    seq: u64,
    rng: Rng,
    now: f64,
    /// ライン内のワーク数と、その時間積分 (集計期間のみ)
    wip: usize,
    wip_area: f64,
    wip_since: f64,
    wip_max: usize,
    output_total: u32,
    samples: Vec<Sample>,
    sample_interval: f64,
    next_sample: f64,
    /// ライン連携 (しないときは None)
    link: Option<Link>,
    /// 出勤する作業者
    workers: Vec<WorkerRt>,
}

impl Simulation {
    /// モデルから作る。回せないモデルなら理由を返す。
    pub fn new(model: &FlowModel) -> Result<Self, Vec<String>> {
        let problems = model.problems();
        if !problems.is_empty() {
            return Err(problems);
        }
        let next = model
            .elements
            .iter()
            .map(|e| e.next.iter().filter_map(|&n| model.index_of(n)).collect())
            .collect();
        let settings = model.settings;
        let mut sim = Self {
            model: model.clone(),
            next,
            runtime: vec![Runtime::new(); model.elements.len()],
            events: BinaryHeap::new(),
            seq: 0,
            rng: Rng(settings.seed),
            now: 0.0,
            wip: 0,
            wip_area: 0.0,
            wip_since: 0.0,
            wip_max: 0,
            output_total: 0,
            samples: Vec::new(),
            sample_interval: settings.duration / SAMPLE_COUNT as f64,
            next_sample: 0.0,
            link: None,
            workers: Vec::new(),
        };
        sim.start_workers();
        for i in 0..sim.runtime.len() {
            match sim.model.elements[i].kind {
                ElementKind::Source { .. } => sim.schedule(0.0, i, EventKind::Arrival),
                ElementKind::Process {
                    failure: Some(f), ..
                } => sim.runtime[i].time_to_failure = sim.rng.exponential(f.mtbf),
                _ => {}
            }
        }
        Ok(sim)
    }

    pub fn settings(&self) -> RunSettings {
        self.model.settings
    }

    pub fn now(&self) -> f64 {
        self.now
    }

    pub fn finished(&self) -> bool {
        self.now >= self.model.settings.duration
    }

    /// 要素の今の状態 (要素の並び順)
    pub fn state_of(&self, id: ElementId) -> Option<State> {
        let i = self.model.index_of(id)?;
        Some(self.runtime[i].state)
    }

    /// 要素の中にあるワーク数
    pub fn content_of(&self, id: ElementId) -> Option<usize> {
        let i = self.model.index_of(id)?;
        Some(self.runtime[i].content())
    }

    /// 時刻 `until` (回す時間で頭打ち) まで進める。1回に処理する事象は `max_events` まで。
    /// 届いた時刻を返す (事象が多すぎて途中で止めたときは `until` より前)。
    pub fn advance(&mut self, until: f64, max_events: usize) -> f64 {
        let until = until.min(self.model.settings.duration);
        let mut handled = 0;
        while let Some(event) = self.events.peek().copied() {
            if event.time > until {
                break;
            }
            if handled >= max_events {
                self.record_samples(event.time);
                self.now = self.now.max(event.time);
                return self.now;
            }
            self.events.pop();
            self.record_samples(event.time);
            self.now = event.time;
            self.handle(event);
            self.settle();
            handled += 1;
        }
        self.record_samples(until);
        self.now = until;
        if self.finished() {
            // 最後の点 (回す時間ちょうど) も記録する
            self.next_sample = self.next_sample.min(until);
            self.push_sample(until);
        }
        self.now
    }

    // --- ライン連携 -------------------------------------------------------------

    /// ライン連携を始める。以後ワークの移動を記録し、`external` の要素は
    /// 送り出すワークができても自分では動かさず、[`FlowEvent::CarrierNeeded`] を出して
    /// [`Simulation::pick_up`] を待つ。
    pub fn link(&mut self, external: impl IntoIterator<Item = ElementId>) {
        let n = self.runtime.len();
        let mut link = Link {
            external: vec![false; n],
            requested: vec![0; n],
            held_sources: vec![false; n],
            held_arrivals: vec![false; n],
            ..Link::default()
        };
        for id in external {
            if let Some(i) = self.model.index_of(id)
                && !matches!(self.model.elements[i].kind, ElementKind::Sink)
            {
                link.external[i] = true;
            }
        }
        self.link = Some(link);
        self.settle();
    }

    /// ライン連携しているか
    pub fn is_linked(&self) -> bool {
        self.link.is_some()
    }

    /// 前回からの出来事を取り出す (ライン連携していなければ空)
    pub fn take_events(&mut self) -> Vec<FlowEvent> {
        self.link
            .as_mut()
            .map(|l| std::mem::take(&mut l.events))
            .unwrap_or_default()
    }

    /// AGV で送り出す要素から、ワークを1個積む。積めるワークがなければ None。
    /// 空いた所には上流からすぐワークが入る。
    pub fn pick_up(&mut self, element: ElementId) -> Option<LoadId> {
        let i = self.model.index_of(element)?;
        if !self.is_external(i) || self.outgoing_count(i) == 0 {
            return None;
        }
        let work = self.take(i);
        let link = self.link.as_mut().expect("linked");
        link.requested[i] = link.requested[i].saturating_sub(1);
        let id = LoadId(link.next_load);
        link.next_load += 1;
        link.transit.insert(id, (i, work));
        self.settle();
        Some(id)
    }

    /// 積んだワークを降ろし、積んだ要素の送り先へ入れる。送り先が空いていなければ
    /// 空くまでその場で待つ。知らない荷札なら false。
    pub fn deliver(&mut self, load: LoadId) -> bool {
        let Some(link) = self.link.as_mut() else {
            return false;
        };
        let Some(entry) = link.transit.remove(&load) else {
            return false;
        };
        link.deliveries.push_back(entry);
        self.settle();
        true
    }

    /// 生成の投入を許す・止める (PLC の投入許可)。止めている間は次のワークを出さず、
    /// 許したときに出す時刻を過ぎていればすぐ出す。ライン連携していなければ何もしない。
    pub fn set_source_enabled(&mut self, element: ElementId, enabled: bool) {
        let Some(i) = self.model.index_of(element) else {
            return;
        };
        if !matches!(self.model.elements[i].kind, ElementKind::Source { .. }) {
            return;
        }
        let Some(link) = self.link.as_mut() else {
            return;
        };
        link.held_sources[i] = !enabled;
        if enabled && std::mem::take(&mut link.held_arrivals[i]) {
            self.schedule(self.now, i, EventKind::Arrival);
        }
    }

    /// 生成の投入が許されているか
    pub fn source_enabled(&self, element: ElementId) -> bool {
        let Some(i) = self.model.index_of(element) else {
            return false;
        };
        !self.link.as_ref().is_some_and(|l| l.held_sources[i])
    }

    /// これまでに排出したワークの数 (立ち上がりも含む)
    pub fn output_total(&self) -> u32 {
        self.output_total
    }

    /// ライン内のワーク数
    pub fn wip(&self) -> usize {
        self.wip
    }

    /// 運ばれている途中と、降ろされて送り先を待っているワークの数
    pub fn in_transit(&self) -> usize {
        self.link
            .as_ref()
            .map_or(0, |l| l.transit.len() + l.deliveries.len())
    }

    fn is_external(&self, i: usize) -> bool {
        self.link.as_ref().is_some_and(|l| l.external[i])
    }

    /// 送り出せるワークの数
    fn outgoing_count(&self, i: usize) -> usize {
        let rt = &self.runtime[i];
        match self.model.elements[i].kind {
            ElementKind::Source { .. } => usize::from(rt.slot.is_some()),
            ElementKind::Process { .. } => usize::from(rt.done),
            ElementKind::Buffer { .. } => rt.queue.len(),
            ElementKind::Transport { .. } => rt.ready.iter().filter(|&&t| t <= self.now).count(),
            ElementKind::Sink => 0,
        }
    }

    fn log(&mut self, event: FlowEvent) {
        if let Some(link) = self.link.as_mut() {
            link.events.push(event);
        }
    }

    /// 降ろされたワークを、空いた送り先へ入れる。1個でも入れたら true。
    fn place_deliveries(&mut self) -> bool {
        let mut moved = false;
        let mut k = 0;
        while let Some(&(from, _)) = self.link.as_ref().and_then(|l| l.deliveries.get(k)) {
            let target = self.next[from]
                .iter()
                .copied()
                .find(|&t| self.can_accept(t));
            let Some(target) = target else {
                k += 1;
                continue;
            };
            let (_, work) = self
                .link
                .as_mut()
                .and_then(|l| l.deliveries.remove(k))
                .expect("delivery exists");
            self.accept(target, work);
            self.log(FlowEvent::Moved {
                time: self.now,
                from: self.model.elements[from].id,
                to: self.model.elements[target].id,
                carried: true,
            });
            moved = true;
        }
        moved
    }

    /// AGV で送り出す要素に新しく送り出せるワークができていたら、運び手を頼む
    fn request_carriers(&mut self) {
        let Some(link) = self.link.as_ref() else {
            return;
        };
        let mut needed = Vec::new();
        for i in 0..self.runtime.len() {
            if !link.external[i] {
                continue;
            }
            let out = self.outgoing_count(i);
            for _ in link.requested[i]..out {
                needed.push(i);
            }
        }
        for i in needed {
            self.link.as_mut().expect("linked").requested[i] += 1;
            self.log(FlowEvent::CarrierNeeded {
                time: self.now,
                element: self.model.elements[i].id,
            });
        }
    }

    /// 最後まで回して結果を返す
    pub fn run(model: &FlowModel) -> Result<Report, Vec<String>> {
        let mut sim = Self::new(model)?;
        sim.advance(f64::INFINITY, usize::MAX);
        Ok(sim.report())
    }

    // --- 事象の処理 ------------------------------------------------------------

    fn schedule(&mut self, time: f64, element: usize, kind: EventKind) {
        self.seq += 1;
        self.events.push(Event {
            time,
            seq: self.seq,
            element,
            kind,
        });
    }

    fn handle(&mut self, event: Event) {
        let i = event.element;
        match event.kind {
            EventKind::Arrival => {
                if let Some(link) = self.link.as_mut()
                    && link.held_sources[i]
                {
                    link.held_arrivals[i] = true;
                    return;
                }
                self.runtime[i].slot = Some(Work { created: self.now });
                self.runtime[i].created += 1;
                // 下流に送れなければそのまま詰まり。送れたら settle で待ちに戻る。
                self.set_state(i, State::Blocked);
            }
            EventKind::Done => {
                self.runtime[i].done = true;
                self.set_state(i, State::Blocked);
                if self.model.elements[i].labor == Labor::Full {
                    self.release_operator(i);
                }
            }
            EventKind::Fail => {
                if self.measuring() {
                    self.runtime[i].failures += 1;
                }
                self.set_state(i, State::Down);
                let mttr = self.failure_of(i).map_or(0.0, |f| f.mttr);
                let repair = self.rng.exponential(mttr);
                self.schedule(self.now + repair, i, EventKind::Repair);
            }
            EventKind::Repair => {
                if let Some(f) = self.failure_of(i) {
                    self.runtime[i].time_to_failure = self.rng.exponential(f.mtbf);
                }
                let remaining = self.runtime[i].remaining;
                self.start_work(i, remaining);
            }
            EventKind::WorkerShift => self.shift_change(i),
            EventKind::WorkerArrive => {
                if let Some(w) = self.runtime[i].operator {
                    self.workers[w].at = Some(i);
                    self.begin_task(i);
                }
            }
            EventKind::OperatorRelease => self.release_operator(i),
            EventKind::TransportArrive => self.update_transport_state(i),
        }
    }

    fn failure_of(&self, i: usize) -> Option<super::model::Failure> {
        match self.model.elements[i].kind {
            ElementKind::Process { failure, .. } => failure,
            _ => None,
        }
    }

    /// 工程で `duration` 秒の加工を始める (故障するならその時刻に故障を入れる)
    fn start_work(&mut self, i: usize, duration: f64) {
        self.set_state(i, State::Working);
        let rt = &mut self.runtime[i];
        if rt.time_to_failure < duration {
            let at = self.now + rt.time_to_failure;
            rt.remaining = duration - rt.time_to_failure;
            rt.time_to_failure = 0.0;
            self.schedule(at, i, EventKind::Fail);
        } else {
            rt.time_to_failure -= duration;
            self.schedule(self.now + duration, i, EventKind::Done);
        }
    }

    /// 送り出せるワークを、動かなくなるまで下流へ動かす
    fn settle(&mut self) {
        loop {
            let mut moved = self.place_deliveries();
            for i in 0..self.runtime.len() {
                if !self.has_outgoing(i) || self.is_external(i) {
                    continue;
                }
                let Some(target) = self.next[i].iter().copied().find(|&t| self.can_accept(t))
                else {
                    continue;
                };
                let work = self.take(i);
                self.accept(target, work);
                if self.link.is_some() {
                    self.log(FlowEvent::Moved {
                        time: self.now,
                        from: self.model.elements[i].id,
                        to: self.model.elements[target].id,
                        carried: false,
                    });
                }
                moved = true;
            }
            if !moved {
                break;
            }
        }
        self.request_carriers();
    }

    fn has_outgoing(&self, i: usize) -> bool {
        let rt = &self.runtime[i];
        match self.model.elements[i].kind {
            ElementKind::Source { .. } => rt.slot.is_some(),
            ElementKind::Process { .. } => rt.done,
            ElementKind::Buffer { .. } => !rt.queue.is_empty(),
            ElementKind::Transport { .. } => rt.ready.front().is_some_and(|&t| t <= self.now),
            ElementKind::Sink => false,
        }
    }

    /// 搬送の状態: 空なら待ち、先頭が届いていて送れなければ詰まり、それ以外は運搬中 (稼働)
    fn update_transport_state(&mut self, i: usize) {
        let state = match self.runtime[i].ready.front() {
            None => State::Waiting,
            Some(&t) if t <= self.now => State::Blocked,
            Some(_) => State::Working,
        };
        if self.runtime[i].state != state {
            self.set_state(i, state);
        }
    }

    fn can_accept(&self, i: usize) -> bool {
        let rt = &self.runtime[i];
        match self.model.elements[i].kind {
            ElementKind::Source { .. } => false,
            ElementKind::Process { .. } => rt.slot.is_none(),
            ElementKind::Buffer { capacity } | ElementKind::Transport { capacity, .. } => {
                rt.queue.len() < capacity as usize
            }
            ElementKind::Sink => true,
        }
    }

    fn take(&mut self, i: usize) -> Work {
        let measuring = self.measuring();
        self.track_content(i);
        let kind = self.model.elements[i].kind.clone();
        let rt = &mut self.runtime[i];
        if measuring {
            rt.count += 1;
        }
        match kind {
            ElementKind::Source {
                interval,
                spread,
                limit,
            } => {
                let work = rt.slot.take().expect("source holds a work");
                let more = limit.is_none_or(|l| rt.created < l);
                self.set_state(i, State::Waiting);
                // ワークがラインに入った
                self.change_wip(1);
                if more {
                    let wait = self.rng.around(interval, spread);
                    self.schedule(self.now + wait, i, EventKind::Arrival);
                }
                work
            }
            ElementKind::Process { .. } => {
                rt.done = false;
                let work = rt.slot.take().expect("process holds a work");
                self.set_state(i, State::Waiting);
                work
            }
            ElementKind::Buffer { .. } => {
                let work = rt.queue.pop_front().expect("buffer is not empty");
                if rt.queue.is_empty() {
                    self.set_state(i, State::Waiting);
                }
                work
            }
            ElementKind::Transport { .. } => {
                let work = rt.queue.pop_front().expect("transport carries a work");
                rt.ready.pop_front();
                self.update_transport_state(i);
                work
            }
            ElementKind::Sink => unreachable!("sink never sends"),
        }
    }

    fn accept(&mut self, i: usize, work: Work) {
        self.track_content(i);
        let measuring = self.measuring();
        match self.model.elements[i].kind.clone() {
            ElementKind::Process {
                cycle_time, spread, ..
            } => {
                self.runtime[i].slot = Some(work);
                let duration = self.rng.around(cycle_time, spread);
                if self.model.elements[i].labor.needs_operator() {
                    self.runtime[i].pending = Some(duration);
                    self.set_state(i, State::NoOperator);
                    self.call_operator(i);
                } else {
                    self.start_work(i, duration);
                }
            }
            ElementKind::Buffer { .. } => {
                self.runtime[i].queue.push_back(work);
                self.set_state(i, State::Working);
            }
            ElementKind::Transport { travel_time, .. } => {
                let at = self.now + travel_time;
                self.runtime[i].queue.push_back(work);
                self.runtime[i].ready.push_back(at);
                self.schedule(at, i, EventKind::TransportArrive);
                self.update_transport_state(i);
            }
            ElementKind::Sink => {
                self.output_total += 1;
                self.change_wip(-1);
                if measuring {
                    let rt = &mut self.runtime[i];
                    rt.count += 1;
                    rt.lead_sum += self.now - work.created;
                }
            }
            ElementKind::Source { .. } => unreachable!("source never receives"),
        }
        let rt = &mut self.runtime[i];
        rt.content_max = rt.content_max.max(rt.content());
    }

    // --- 作業者 --------------------------------------------------------------

    /// 出勤する作業者を並べ、シフトの切り替えを入れる
    fn start_workers(&mut self) {
        let staffing = self.model.staffing.clone();
        let clock = staffing.start_clock;
        for w in staffing.workers.iter().filter(|w| !w.absent) {
            let shift = w.shift.and_then(|s| staffing.shift(s)).cloned();
            let on_duty = shift.as_ref().is_none_or(|s| s.on_duty_at(clock));
            let speed = self
                .model
                .elements
                .iter()
                .map(|e| w.speed_for(e.id))
                .collect();
            self.workers.push(WorkerRt {
                id: w.id,
                name: w.name.clone(),
                state: if on_duty {
                    WorkerState::Idle
                } else {
                    WorkerState::OffDuty
                },
                since: 0.0,
                times: WorkerTimes::default(),
                on_duty,
                at: None,
                shift,
                speed,
                primary: w.primary.and_then(|p| self.model.index_of(p)),
                home_line: w.primary.and_then(|p| self.model.get(p)?.line),
                tasks: 0,
                support_tasks: 0,
                walks: 0,
            });
        }
        for w in 0..self.workers.len() {
            self.schedule_shift_change(w);
        }
    }

    fn clock(&self) -> f64 {
        self.model.staffing.start_clock + self.now
    }

    fn schedule_shift_change(&mut self, w: usize) {
        if let Some(shift) = &self.workers[w].shift {
            let after = shift.next_change_after(self.clock());
            self.schedule(self.now + after, w, EventKind::WorkerShift);
        }
    }

    fn set_worker_state(&mut self, w: usize, state: WorkerState) {
        let span = self.measured_span(self.workers[w].since);
        let wr = &mut self.workers[w];
        wr.times.0[wr.state.index()] += span;
        wr.state = state;
        wr.since = self.now;
    }

    /// シフト・休憩の切り替え。作業中・歩行中なら、その作業が終わったときに抜ける。
    fn shift_change(&mut self, w: usize) {
        let clock = self.clock();
        let on_duty = self.workers[w]
            .shift
            .as_ref()
            .is_none_or(|s| s.on_duty_at(clock));
        self.workers[w].on_duty = on_duty;
        match (on_duty, self.workers[w].state) {
            (true, WorkerState::OffDuty) => {
                self.set_worker_state(w, WorkerState::Idle);
                self.find_work(w);
            }
            (false, WorkerState::Idle) => self.set_worker_state(w, WorkerState::OffDuty),
            _ => {}
        }
        self.schedule_shift_change(w);
    }

    /// 人待ちになった工程に作業者を呼ぶ。来られる人がいなければ人待ちのまま。
    fn call_operator(&mut self, i: usize) {
        let rule = self.model.staffing.rule;
        let workers = &self.workers;
        let best = (0..workers.len())
            .filter(|&w| workers[w].state == WorkerState::Idle)
            .filter_map(|w| Some((w, workers[w].speed[i]?)))
            .max_by(|&(a, sa), &(b, sb)| {
                let (wa, wb) = (&workers[a], &workers[b]);
                let first = match rule {
                    AssignRule::Fastest => std::cmp::Ordering::Equal,
                    // 手待ちに入った時刻が早い人ほど優先
                    AssignRule::LongestIdle => wb.since.total_cmp(&wa.since),
                    AssignRule::Primary => (wa.primary == Some(i)).cmp(&(wb.primary == Some(i))),
                };
                first
                    .then(sa.total_cmp(&sb))
                    // 同じなら、今その工程にいる人 → 名簿の上の人
                    .then((wa.at == Some(i)).cmp(&(wb.at == Some(i))))
                    .then(b.cmp(&a))
            })
            .map(|(w, _)| w);
        if let Some(w) = best {
            self.assign(w, i);
        }
    }

    /// 手の空いた作業者に、いちばん長く人待ちしている工程を割り当てる
    fn find_work(&mut self, w: usize) {
        let waiting = |i: usize| {
            let rt = &self.runtime[i];
            rt.state == State::NoOperator
                && rt.operator.is_none()
                && self.workers[w].speed[i].is_some()
        };
        let own = self.workers[w]
            .primary
            .filter(|&p| self.model.staffing.rule == AssignRule::Primary && waiting(p));
        let target = own.or_else(|| {
            (0..self.runtime.len())
                .filter(|&i| waiting(i))
                .min_by(|&a, &b| self.runtime[a].since.total_cmp(&self.runtime[b].since))
        });
        if let Some(i) = target {
            self.assign(w, i);
        }
    }

    fn assign(&mut self, w: usize, i: usize) {
        self.runtime[i].operator = Some(w);
        let walk = self.model.staffing.walk_seconds;
        if self.workers[w].at == Some(i) || walk <= 0.0 {
            self.workers[w].at = Some(i);
            self.begin_task(i);
        } else {
            if self.measuring() {
                self.workers[w].walks += 1;
            }
            self.set_worker_state(w, WorkerState::Walking);
            self.schedule(self.now + walk, i, EventKind::WorkerArrive);
        }
    }

    /// 作業者が工程に着いたので加工を始める
    fn begin_task(&mut self, i: usize) {
        let Some(w) = self.runtime[i].operator else {
            return;
        };
        let Some(duration) = self.runtime[i].pending.take() else {
            return;
        };
        let speed = self.workers[w].speed[i].unwrap_or(1.0);
        self.set_worker_state(w, WorkerState::Working);
        match self.model.elements[i].labor {
            Labor::Full => self.start_work(i, duration / speed),
            Labor::Load { seconds } => {
                // 最初の `seconds` 秒が人の作業。人が速ければその分だけ短くなる。
                let hand = seconds.min(duration);
                let manual = hand / speed;
                self.schedule(self.now + manual, i, EventKind::OperatorRelease);
                self.start_work(i, duration - hand + manual);
            }
            Labor::None => {
                self.start_work(i, duration);
                self.release_operator(i);
            }
        }
    }

    /// 工程の作業者の手が空いた。次の人待ちへ行くか、勤務時間外なら抜ける。
    fn release_operator(&mut self, i: usize) {
        let Some(w) = self.runtime[i].operator.take() else {
            return;
        };
        if self.measuring() {
            let line = self.model.elements[i].line;
            let wr = &mut self.workers[w];
            wr.tasks += 1;
            if wr.home_line.is_some() && line != wr.home_line {
                wr.support_tasks += 1;
            }
        }
        if self.workers[w].on_duty {
            self.set_worker_state(w, WorkerState::Idle);
            self.find_work(w);
        } else {
            self.set_worker_state(w, WorkerState::OffDuty);
        }
    }

    // --- 集計 ---------------------------------------------------------------

    fn measuring(&self) -> bool {
        self.now >= self.model.settings.warmup
    }

    /// 集計期間に入っている時間 (`from` から今まで)
    fn measured_span(&self, from: f64) -> f64 {
        (self.now - from.max(self.model.settings.warmup)).max(0.0)
    }

    fn set_state(&mut self, i: usize, state: State) {
        let span = self.measured_span(self.runtime[i].since);
        let rt = &mut self.runtime[i];
        rt.times.0[rt.state.index()] += span;
        rt.state = state;
        rt.since = self.now;
    }

    /// 中身の数が変わる直前に呼び、それまでの積分を足す
    fn track_content(&mut self, i: usize) {
        let span = self.measured_span(self.runtime[i].content_since);
        let rt = &mut self.runtime[i];
        rt.content_area += rt.content() as f64 * span;
        rt.content_since = self.now;
    }

    fn change_wip(&mut self, delta: isize) {
        self.wip_area += self.wip as f64 * self.measured_span(self.wip_since);
        self.wip_since = self.now;
        self.wip = self.wip.saturating_add_signed(delta);
        if self.measuring() {
            self.wip_max = self.wip_max.max(self.wip);
        }
    }

    fn record_samples(&mut self, upto: f64) {
        while self.next_sample < upto && self.next_sample < self.model.settings.duration {
            self.push_sample(self.next_sample);
        }
    }

    fn push_sample(&mut self, time: f64) {
        if self.samples.last().is_some_and(|s| s.time >= time) {
            return;
        }
        self.samples.push(Sample {
            time,
            wip: self.wip,
            output: self.output_total,
            states: self.runtime.iter().map(|r| r.state).collect(),
        });
        self.next_sample = time + self.sample_interval;
    }

    /// 今の時刻までの集計
    pub fn report(&self) -> Report {
        let settings = self.model.settings;
        let measured = (self.now - settings.warmup).max(0.0);
        let elements: Vec<ElementStats> = self
            .model
            .elements
            .iter()
            .zip(&self.runtime)
            .map(|(e, rt)| {
                let mut times = rt.times;
                times.0[rt.state.index()] += self.measured_span(rt.since);
                let content_area =
                    rt.content_area + rt.content() as f64 * self.measured_span(rt.content_since);
                let hours = |s: f64| s / 3600.0;
                let working = times.get(State::Working);
                let energy_kwh = e.energy.rated_kw * hours(working)
                    + e.energy.standby_kw * hours(times.total() - working);
                let air_m3 = e.energy.air_l_per_min * working / 60.0 / 1000.0;
                ElementStats {
                    id: e.id,
                    name: e.name.clone(),
                    kind: e.kind.tag(),
                    part: e.part,
                    times,
                    count: rt.count,
                    failures: rt.failures,
                    average_cycle: (rt.count > 0 && matches!(e.kind, ElementKind::Process { .. }))
                        .then(|| working / rt.count as f64),
                    content_average: if measured > 0.0 {
                        content_area / measured
                    } else {
                        0.0
                    },
                    content_max: rt.content_max,
                    lead_time: (rt.count > 0 && matches!(e.kind, ElementKind::Sink))
                        .then(|| rt.lead_sum / rt.count as f64),
                    energy_kwh,
                    air_m3,
                }
            })
            .collect();
        let output: u32 = elements
            .iter()
            .filter(|e| e.kind == super::model::KindTag::Sink)
            .map(|e| e.count)
            .sum();
        let lead_sum: f64 = elements
            .iter()
            .filter_map(|e| Some(e.lead_time? * e.count as f64))
            .sum();
        let workers = self
            .workers
            .iter()
            .map(|w| {
                let mut times = w.times;
                times.0[w.state.index()] += self.measured_span(w.since);
                WorkerStats {
                    id: w.id,
                    name: w.name.clone(),
                    times,
                    tasks: w.tasks,
                    support_tasks: w.support_tasks,
                    walks: w.walks,
                }
            })
            .collect();
        let wip_area = self.wip_area + self.wip as f64 * self.measured_span(self.wip_since);
        let energy_kwh = elements.iter().map(|e| e.energy_kwh).sum();
        let air_m3 = elements.iter().map(|e| e.air_m3).sum();
        Report {
            time: self.now,
            measured,
            output,
            throughput_per_hour: if measured > 0.0 {
                output as f64 * 3600.0 / measured
            } else {
                0.0
            },
            cycle_time: (output > 0).then(|| measured / output as f64),
            lead_time: (output > 0).then(|| lead_sum / output as f64),
            wip_average: if measured > 0.0 {
                wip_area / measured
            } else {
                0.0
            },
            wip_max: self.wip_max,
            bottleneck: bottleneck(&elements),
            energy_kwh,
            air_m3,
            energy_per_work_kwh: (output > 0).then(|| energy_kwh / output as f64),
            elements,
            workers,
            samples: self.samples.clone(),
        }
    }
}

/// 作業者1人分の集計
#[derive(Clone, Debug, PartialEq)]
pub struct WorkerStats {
    pub id: WorkerId,
    pub name: String,
    /// 集計期間の状態ごとの時間
    pub times: WorkerTimes,
    /// 終えた作業の数
    pub tasks: u32,
    /// そのうち、持ち場のライン以外 (応援) での作業の数
    pub support_tasks: u32,
    /// 別の工程へ歩いた回数
    pub walks: u32,
}

impl WorkerStats {
    /// 稼働率: 出勤している時間のうち、作業と歩行の割合
    pub fn utilization(&self) -> f64 {
        self.times.ratio(WorkerState::Working) + self.times.ratio(WorkerState::Walking)
    }
}

/// 要素1つ分の集計
#[derive(Clone, Debug, PartialEq)]
pub struct ElementStats {
    pub id: ElementId,
    pub name: String,
    pub kind: super::model::KindTag,
    pub part: Option<u32>,
    /// 集計期間の状態ごとの時間
    pub times: StateTimes,
    /// 送り出した数 (排出は受け取った数)
    pub count: u32,
    pub failures: u32,
    /// 工程: 1個あたりの実際の加工時間の平均 (秒)
    pub average_cycle: Option<f64>,
    /// 中のワーク数の時間平均と最大
    pub content_average: f64,
    pub content_max: usize,
    /// 排出: 生成から排出までの平均時間 (秒)
    pub lead_time: Option<f64>,
    pub energy_kwh: f64,
    pub air_m3: f64,
}

impl ElementStats {
    /// 稼働と故障を合わせた割合。いちばん高い工程がボトルネック。
    pub fn busy_ratio(&self) -> f64 {
        self.times.ratio(State::Working) + self.times.ratio(State::Down)
    }
}

/// 集計結果
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// どこまで回したか (秒)
    pub time: f64,
    /// 集計した時間 (立ち上がりを除く, 秒)
    pub measured: f64,
    /// 集計期間の排出数
    pub output: u32,
    pub throughput_per_hour: f64,
    /// ラインのサイクルタイム (集計時間 / 排出数, 秒)
    pub cycle_time: Option<f64>,
    /// 生成から排出までの平均時間 (秒)
    pub lead_time: Option<f64>,
    /// 仕掛数 (ライン内のワーク数) の時間平均と最大
    pub wip_average: f64,
    pub wip_max: usize,
    pub bottleneck: Option<ElementId>,
    pub energy_kwh: f64,
    pub air_m3: f64,
    pub energy_per_work_kwh: Option<f64>,
    pub elements: Vec<ElementStats>,
    /// 出勤した作業者 (欠勤の人は入らない)
    pub workers: Vec<WorkerStats>,
    pub samples: Vec<Sample>,
}

impl Report {
    pub fn element(&self, id: ElementId) -> Option<&ElementStats> {
        self.elements.iter().find(|e| e.id == id)
    }

    /// 作業者の稼働率の平均。作業者がいなければ None。
    pub fn worker_utilization(&self) -> Option<f64> {
        (!self.workers.is_empty()).then(|| {
            self.workers
                .iter()
                .map(WorkerStats::utilization)
                .sum::<f64>()
                / self.workers.len() as f64
        })
    }

    /// 人が要る工程の「人待ち」の割合の平均。人が要る工程がなければ None。
    pub fn no_operator_ratio(&self, model: &FlowModel) -> Option<f64> {
        let ratios: Vec<f64> = self
            .elements
            .iter()
            .filter(|e| model.get(e.id).is_some_and(|m| m.labor.needs_operator()))
            .map(|e| e.times.ratio(State::NoOperator))
            .collect();
        (!ratios.is_empty()).then(|| ratios.iter().sum::<f64>() / ratios.len() as f64)
    }
}

/// 稼働 + 故障の割合がいちばん高い工程。どれも 0 なら None。
fn bottleneck(elements: &[ElementStats]) -> Option<ElementId> {
    elements
        .iter()
        .filter(|e| e.kind == super::model::KindTag::Process && e.busy_ratio() > 0.0)
        .max_by(|a, b| a.busy_ratio().total_cmp(&b.busy_ratio()))
        .map(|e| e.id)
}

#[cfg(test)]
mod tests {
    use super::super::model::{Failure, KindTag, sample_line};
    use super::*;

    /// 生成 → 工程 × n → 排出 (ばらつきなし)
    fn line(interval: f64, cycles: &[f64]) -> FlowModel {
        let mut m = FlowModel::default();
        let s = m.append(KindTag::Source);
        if let ElementKind::Source { interval: i, .. } = &mut m.get_mut(s).unwrap().kind {
            *i = interval;
        }
        for &c in cycles {
            let p = m.append(KindTag::Process);
            if let ElementKind::Process { cycle_time, .. } = &mut m.get_mut(p).unwrap().kind {
                *cycle_time = c;
            }
        }
        m.append(KindTag::Sink);
        m.settings.duration = 3600.0;
        m
    }

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn slowest_process_sets_the_pace_and_is_the_bottleneck() {
        // 10 秒ごとに投入、20 秒 → 30 秒 → 5 秒の工程。30 秒の工程が律速する。
        let m = line(10.0, &[20.0, 30.0, 5.0]);
        let r = Simulation::run(&m).unwrap();
        // 最初の1個は 55 秒で出て、あとは 30 秒ごと
        assert_eq!(r.output, 1 + ((3600.0 - 55.0) / 30.0) as u32);
        let slow = m.elements[2].id;
        assert_eq!(r.bottleneck, Some(slow));
        let stats = r.element(slow).unwrap();
        assert!(stats.times.ratio(State::Working) > 0.98);
        // 前の工程は詰まり、後ろの工程は待つ
        let before = r.element(m.elements[1].id).unwrap();
        assert!(before.times.ratio(State::Blocked) > 0.3, "{before:?}");
        let after = r.element(m.elements[3].id).unwrap();
        assert!(after.times.ratio(State::Waiting) > 0.8, "{after:?}");
        assert!(close(r.cycle_time.unwrap(), 30.0, 0.5));
        // 生成も詰まる
        let source = r.element(m.elements[0].id).unwrap();
        assert!(source.times.ratio(State::Blocked) > 0.5);
    }

    #[test]
    fn a_fast_line_follows_the_source() {
        let m = line(60.0, &[10.0]);
        let r = Simulation::run(&m).unwrap();
        // 0, 60, ..., 3540 秒に投入。3600 秒の投入は 10 秒後に終わらないので数えない。
        assert_eq!(r.output, 60);
        let p = r.element(m.elements[1].id).unwrap();
        assert!(close(p.times.ratio(State::Working), 1.0 / 6.0, 0.01));
        assert!(close(p.average_cycle.unwrap(), 10.0, 1e-9));
        assert!(close(r.lead_time.unwrap(), 10.0, 1e-9));
        // 仕掛はほとんどの時間 0 か 1
        assert!(close(r.wip_average, 1.0 / 6.0, 0.01));
        assert_eq!(r.wip_max, 1);
    }

    #[test]
    fn source_limit_stops_generation() {
        let mut m = line(1.0, &[1.0]);
        if let ElementKind::Source { limit, .. } = &mut m.elements[0].kind {
            *limit = Some(7);
        }
        let r = Simulation::run(&m).unwrap();
        assert_eq!(r.output, 7);
    }

    #[test]
    fn buffer_absorbs_and_counts_contents() {
        // 生成 5 秒、工程 10 秒の前に容量 3 のバッファ。バッファはすぐ満杯になる。
        let mut m = FlowModel::default();
        m.append(KindTag::Source);
        let b = m.append(KindTag::Buffer);
        m.append(KindTag::Process);
        m.append(KindTag::Sink);
        if let ElementKind::Source { interval, .. } = &mut m.elements[0].kind {
            *interval = 5.0;
        }
        if let ElementKind::Buffer { capacity } = &mut m.get_mut(b).unwrap().kind {
            *capacity = 3;
        }
        if let ElementKind::Process { cycle_time, .. } = &mut m.elements[2].kind {
            *cycle_time = 10.0;
        }
        m.settings.duration = 3600.0;
        let r = Simulation::run(&m).unwrap();
        let buffer = r.element(b).unwrap();
        assert_eq!(buffer.content_max, 3);
        assert!(buffer.content_average > 2.9, "{buffer:?}");
        // 工程の中の1個 + バッファの3個 (+ 生成が持っている1個は数えない)
        assert_eq!(r.wip_max, 4);
    }

    #[test]
    fn failures_stop_the_process_for_about_the_mttr_ratio() {
        let mut m = line(1.0, &[10.0]);
        m.settings.duration = 200.0 * 3600.0;
        if let ElementKind::Process { failure, .. } = &mut m.elements[1].kind {
            *failure = Some(Failure {
                mtbf: 900.0,
                mttr: 100.0,
            });
        }
        let r = Simulation::run(&m).unwrap();
        let p = r.element(m.elements[1].id).unwrap();
        // 稼働 900 : 故障 100 の割合に近づく
        assert!(close(p.times.ratio(State::Down), 0.1, 0.01), "{p:?}");
        assert!(p.failures > 500);
        assert!(close(r.throughput_per_hour, 360.0 * 0.9, 5.0));
    }

    #[test]
    fn same_seed_gives_the_same_result() {
        let m = sample_line();
        assert_eq!(Simulation::run(&m), Simulation::run(&m));
        let mut other = m.clone();
        other.settings.seed = 2;
        assert_ne!(
            Simulation::run(&m).unwrap().output,
            Simulation::run(&other).unwrap().output
        );
    }

    #[test]
    fn warmup_is_excluded_from_the_statistics() {
        let mut m = line(10.0, &[20.0]);
        m.settings.warmup = 600.0;
        let r = Simulation::run(&m).unwrap();
        assert!(close(r.measured, 3000.0, 1e-9));
        // 600, 620, …, 3600 秒に排出 (両端を含む)
        assert_eq!(r.output, 151);
        let p = r.element(m.elements[1].id).unwrap();
        assert!(close(p.times.total(), 3000.0, 1e-6));
    }

    #[test]
    fn stepping_matches_a_full_run() {
        let m = sample_line();
        let full = Simulation::run(&m).unwrap();
        let mut sim = Simulation::new(&m).unwrap();
        let mut t = 0.0;
        while !sim.finished() {
            t = sim.advance(t + 123.4, 50);
        }
        assert_eq!(sim.report(), full);
        assert_eq!(full.samples.len(), SAMPLE_COUNT + 1);
        assert_eq!(full.samples.last().unwrap().time, m.settings.duration);
    }

    #[test]
    fn energy_uses_rated_power_while_working() {
        let mut m = line(60.0, &[30.0]);
        m.elements[1].energy = super::super::model::Energy {
            rated_kw: 2.0,
            standby_kw: 1.0,
            air_l_per_min: 100.0,
        };
        let r = Simulation::run(&m).unwrap();
        // 1 時間の半分が稼働: 2 kW × 0.5 h + 1 kW × 0.5 h
        assert!(close(r.energy_kwh, 1.5, 0.01), "{}", r.energy_kwh);
        // 100 L/min × 30 分 = 3 m³
        assert!(close(r.air_m3, 3.0, 0.05), "{}", r.air_m3);
        assert!(close(r.energy_per_work_kwh.unwrap(), 1.5 / 60.0, 0.001));
    }

    #[test]
    fn invalid_models_are_rejected() {
        assert!(Simulation::new(&FlowModel::default()).is_err());
    }

    // --- 人員配置 ---------------------------------------------------------------

    use crate::staffing::model::{Break, Shift};

    /// 工程ごとに人の要否を決め、`workers` 人 (全員が全工程をできる、速さ 1.0) を入れる
    fn staffed(mut m: FlowModel, labor: &[Labor], workers: usize) -> FlowModel {
        let processes: Vec<ElementId> = m
            .elements
            .iter()
            .filter(|e| e.kind.tag() == KindTag::Process)
            .map(|e| e.id)
            .collect();
        for (&id, &l) in processes.iter().zip(labor) {
            m.get_mut(id).unwrap().labor = l;
        }
        for _ in 0..workers {
            let w = m.staffing.add_worker();
            for &p in &processes {
                m.staffing.worker_mut(w).unwrap().set_skill(p, true);
            }
        }
        m
    }

    #[test]
    fn one_person_for_two_manual_processes_halves_the_pace() {
        // 1 秒ごとに投入、10 秒の手作業が 2 つ。1 人だと 20 秒に 1 個、2 人なら 10 秒に 1 個。
        let base = line(1.0, &[10.0, 10.0]);
        let one = Simulation::run(&staffed(base.clone(), &[Labor::Full, Labor::Full], 1)).unwrap();
        let two = Simulation::run(&staffed(base, &[Labor::Full, Labor::Full], 2)).unwrap();
        assert!(
            close(one.throughput_per_hour, 180.0, 3.0),
            "{}",
            one.throughput_per_hour
        );
        assert!(
            close(two.throughput_per_hour, 360.0, 3.0),
            "{}",
            two.throughput_per_hour
        );
        // 1 人のときはどちらの工程も半分くらい人待ち
        let p = one.element(one.elements[1].id).unwrap();
        assert!(p.times.ratio(State::NoOperator) > 0.3, "{p:?}");
        assert!(one.workers[0].utilization() > 0.99);
        assert_eq!(two.workers.len(), 2);
    }

    #[test]
    fn load_only_frees_the_operator_while_the_machine_runs() {
        // 投入 5 秒 + 自動 25 秒の機械が直列に 2 台。1 人で両方の投入を回せるが、
        // 2 台が同時に空くので片方は 5 秒人待ちになり、35 秒に 1 個になる。
        let m = staffed(
            line(1.0, &[30.0, 30.0]),
            &[Labor::Load { seconds: 5.0 }, Labor::Load { seconds: 5.0 }],
            1,
        );
        let r = Simulation::run(&m).unwrap();
        assert!(
            close(r.throughput_per_hour, 3600.0 / 35.0, 2.0),
            "{}",
            r.throughput_per_hour
        );
        let w = &r.workers[0];
        // 35 秒ごとに 5 秒 × 2 台の作業
        assert!(
            close(w.times.ratio(WorkerState::Working), 10.0 / 35.0, 0.03),
            "{w:?}"
        );
        // 人が加工中ずっと付く場合 (30 秒 × 2 台を 1 人) よりずっと速い
        let full = staffed(line(1.0, &[30.0, 30.0]), &[Labor::Full, Labor::Full], 1);
        assert!(Simulation::run(&full).unwrap().throughput_per_hour < 61.0);
    }

    #[test]
    fn faster_workers_shorten_manual_work() {
        let mut m = staffed(line(1.0, &[10.0]), &[Labor::Full], 1);
        let p = m.elements[1].id;
        m.staffing.workers[0].skills[0] = crate::staffing::model::Skill {
            element: p,
            speed: 2.0,
        };
        let r = Simulation::run(&m).unwrap();
        assert!(
            close(r.throughput_per_hour, 720.0, 3.0),
            "{}",
            r.throughput_per_hour
        );
    }

    #[test]
    fn walking_between_stations_costs_time() {
        let mut m = staffed(line(1.0, &[10.0, 10.0]), &[Labor::Full, Labor::Full], 1);
        m.staffing.walk_seconds = 5.0;
        let r = Simulation::run(&m).unwrap();
        // 10 秒 + 歩行 5 秒 を 2 回で 30 秒に 1 個
        assert!(
            close(r.throughput_per_hour, 120.0, 3.0),
            "{}",
            r.throughput_per_hour
        );
        let w = &r.workers[0];
        assert!(
            close(w.times.ratio(WorkerState::Walking), 1.0 / 3.0, 0.03),
            "{w:?}"
        );
        assert!(w.walks > 200);
    }

    #[test]
    fn lunch_break_stops_manual_work() {
        // 8:00 から 4 時間回す。11:00〜12:00 は休憩なので 3 時間分しか作れない。
        let mut m = staffed(line(1.0, &[10.0]), &[Labor::Full], 1);
        m.settings.duration = 4.0 * 3600.0;
        let shift = m.staffing.insert_shift(Shift {
            id: 0,
            name: "日勤".into(),
            start: 8.0 * 3600.0,
            end: 17.0 * 3600.0,
            breaks: vec![Break {
                start: 11.0 * 3600.0,
                end: 12.0 * 3600.0,
            }],
        });
        m.staffing.workers[0].shift = Some(shift);
        let r = Simulation::run(&m).unwrap();
        assert!((1078..=1081).contains(&r.output), "{}", r.output);
        let w = &r.workers[0];
        assert!(
            close(w.times.get(WorkerState::OffDuty), 3600.0, 10.0),
            "{w:?}"
        );
        assert!(close(w.utilization(), 1.0, 0.01));
    }

    #[test]
    fn absent_workers_do_not_come() {
        let mut m = staffed(line(1.0, &[10.0, 10.0]), &[Labor::Full, Labor::Full], 2);
        m.staffing.workers[1].absent = true;
        let r = Simulation::run(&m).unwrap();
        assert_eq!(r.workers.len(), 1);
        assert!(close(r.throughput_per_hour, 180.0, 3.0));
    }

    #[test]
    fn longest_idle_spreads_work_evenly() {
        // 3 人とも同じ速さで 1 工程だけ。速い人ルールでは名簿の上の人に偏るが、
        // 手待ちの長い人ルールでは均等になる。
        let base = staffed(line(15.0, &[10.0]), &[Labor::Full], 3);
        let fastest = Simulation::run(&base).unwrap();
        let mut even = base.clone();
        even.staffing.rule = AssignRule::LongestIdle;
        let even = Simulation::run(&even).unwrap();
        let tasks = |r: &Report| r.workers.iter().map(|w| w.tasks).collect::<Vec<_>>();
        assert_eq!(fastest.output, even.output);
        let f = tasks(&fastest);
        assert!(f[0] > f[1] + f[2], "{f:?}");
        let e = tasks(&even);
        let (lo, hi) = (e.iter().min().unwrap(), e.iter().max().unwrap());
        assert!(hi - lo <= 1, "{e:?}");
    }

    #[test]
    fn primary_rule_keeps_people_at_their_stations() {
        // 2 人とも 2 工程できるが、持ち場を分けると歩かなくて済む
        let mut m = staffed(line(1.0, &[10.0, 10.0]), &[Labor::Full, Labor::Full], 2);
        m.staffing.walk_seconds = 5.0;
        let (p1, p2) = (m.elements[1].id, m.elements[2].id);
        let walks_fastest: u32 = Simulation::run(&m)
            .unwrap()
            .workers
            .iter()
            .map(|w| w.walks)
            .sum();
        m.staffing.rule = AssignRule::Primary;
        m.staffing.workers[0].primary = Some(p1);
        m.staffing.workers[1].primary = Some(p2);
        let r = Simulation::run(&m).unwrap();
        let walks: u32 = r.workers.iter().map(|w| w.walks).sum();
        assert!(walks <= 2, "{walks}");
        assert!(walks_fastest > walks, "{walks_fastest}");
        assert!(close(r.throughput_per_hour, 360.0, 3.0));
    }

    #[test]
    fn staffed_sample_is_deterministic_and_steppable() {
        let mut m = sample_line();
        crate::flow::model::apply_sample_staffing(&mut m);
        let full = Simulation::run(&m).unwrap();
        let mut sim = Simulation::new(&m).unwrap();
        let mut t = 0.0;
        while !sim.finished() {
            t = sim.advance(t + 77.7, 40);
        }
        assert_eq!(sim.report(), full);
        assert_eq!(full.workers.len(), 3);
        // 昼休みがあるので、人のいないラインより少ない
        let unstaffed = Simulation::run(&sample_line()).unwrap();
        assert!(
            full.output < unstaffed.output,
            "{} {}",
            full.output,
            unstaffed.output
        );
    }
}
