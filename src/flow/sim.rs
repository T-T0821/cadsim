//! 工程モデルの離散事象シミュレーション (Bevy に依存しない)
//!
//! 「ワークが届いた」「加工が終わった」「故障した」「直った」の事象を時刻順に処理し、
//! 事象のたびに、送り出せるワークを下流へ動かせるだけ動かす。時刻は事象から事象へ飛ぶので、
//! 数時間分のラインでも一瞬で回せる。[`Simulation::advance`] で少しずつ進めれば、
//! 3D ビューで状態の色を見ながら早送りもできる。
//!
//! 乱数は種から決まる自前の SplitMix64 なので、同じ種なら必ず同じ結果になる。

use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};

use super::model::{ElementId, ElementKind, FlowModel, RunSettings};

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
}

impl State {
    pub const ALL: [State; 4] = [State::Working, State::Waiting, State::Blocked, State::Down];

    pub fn label(self) -> &'static str {
        match self {
            State::Working => "稼働",
            State::Waiting => "待ち",
            State::Blocked => "詰まり",
            State::Down => "故障",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// 状態ごとの時間 (秒)。[`State::ALL`] の順。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StateTimes(pub [f64; 4]);

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
    /// バッファ
    queue: VecDeque<Work>,
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
            created: 0,
            count: 0,
            failures: 0,
            content_area: 0.0,
            content_since: 0.0,
            content_max: 0,
            lead_sum: 0.0,
        }
    }

    fn content(&self) -> usize {
        self.queue.len() + usize::from(self.slot.is_some())
    }
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
        };
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
                self.runtime[i].slot = Some(Work { created: self.now });
                self.runtime[i].created += 1;
                // 下流に送れなければそのまま詰まり。送れたら settle で待ちに戻る。
                self.set_state(i, State::Blocked);
            }
            EventKind::Done => {
                self.runtime[i].done = true;
                self.set_state(i, State::Blocked);
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
            let mut moved = false;
            for i in 0..self.runtime.len() {
                if !self.has_outgoing(i) {
                    continue;
                }
                let Some(target) = self.next[i].iter().copied().find(|&t| self.can_accept(t))
                else {
                    continue;
                };
                let work = self.take(i);
                self.accept(target, work);
                moved = true;
            }
            if !moved {
                break;
            }
        }
    }

    fn has_outgoing(&self, i: usize) -> bool {
        let rt = &self.runtime[i];
        match self.model.elements[i].kind {
            ElementKind::Source { .. } => rt.slot.is_some(),
            ElementKind::Process { .. } => rt.done,
            ElementKind::Buffer { .. } => !rt.queue.is_empty(),
            ElementKind::Sink => false,
        }
    }

    fn can_accept(&self, i: usize) -> bool {
        let rt = &self.runtime[i];
        match self.model.elements[i].kind {
            ElementKind::Source { .. } => false,
            ElementKind::Process { .. } => rt.slot.is_none(),
            ElementKind::Buffer { capacity } => rt.queue.len() < capacity as usize,
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
        let work = match kind {
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
            ElementKind::Sink => unreachable!("sink never sends"),
        };
        work
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
                self.start_work(i, duration);
            }
            ElementKind::Buffer { .. } => {
                self.runtime[i].queue.push_back(work);
                self.set_state(i, State::Working);
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
            samples: self.samples.clone(),
        }
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
    pub samples: Vec<Sample>,
}

impl Report {
    pub fn element(&self, id: ElementId) -> Option<&ElementStats> {
        self.elements.iter().find(|e| e.id == id)
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
}
