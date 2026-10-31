//! 工程モデル: 生成・工程・バッファ・排出をつないだライン (Bevy に依存しない)
//!
//! - **生成** は一定間隔でワークを出す。下流が詰まっていると出せずに待つ。
//! - **工程** はワークを1個ずつ受け取り、サイクルタイムだけ加工して下流へ送る。
//!   加工中に故障 (MTBF / MTTR を平均とする指数分布の乱数) で止まることがある。
//! - **バッファ** は容量まで先入れ先出しでワークをためる。
//! - **排出** はワークを受け取って数える。
//!
//! 各要素は送り先を複数持てる。送るときは並び順に見て、最初に受け取れる先へ送る。
//!
//! 工程には「人が要るか」([`Labor`]) を付けられる。人が要る工程は、名簿 ([`Staffing`]) の
//! 作業者のうち、その工程ができて手の空いている人が来るまで「人待ち」で止まる。

pub use crate::staffing::model::{Labor, Staffing};

/// 要素の ID。プロジェクト内で一意。
pub type ElementId = u32;

/// ライン全体
#[derive(Clone, Debug, PartialEq)]
pub struct FlowModel {
    pub elements: Vec<Element>,
    pub settings: RunSettings,
    /// 作業者の名簿とシフト
    pub staffing: Staffing,
    next_id: ElementId,
}

impl Default for FlowModel {
    fn default() -> Self {
        Self {
            elements: Vec::new(),
            settings: RunSettings::default(),
            staffing: Staffing::default(),
            next_id: 1,
        }
    }
}

/// 早回しの条件
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunSettings {
    /// 回す時間 (秒)
    pub duration: f64,
    /// 立ち上がりとして集計から外す時間 (秒)
    pub warmup: f64,
    /// 乱数の種。同じ種なら同じ結果になる。
    pub seed: u64,
}

impl Default for RunSettings {
    fn default() -> Self {
        Self {
            duration: 8.0 * 3600.0,
            warmup: 0.0,
            seed: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub id: ElementId,
    pub name: String,
    pub kind: ElementKind,
    /// 送り先 (先頭ほど優先)
    pub next: Vec<ElementId>,
    /// 対応する 3D の部品・アセンブリのノード ID。色分けに使う。
    pub part: Option<u32>,
    /// 消費電力とエア
    pub energy: Energy,
    /// ライン連携で、送り出しを AGV に運ばせる。None ならすぐ下流へ送る。
    /// 工程シミュレーションのウィンドウだけで回すときは使わない。
    pub carrier: Option<AgvCall>,
    /// ライン連携で、この工程が加工を始めるたびにタイミングチャートを頭から再生する
    pub starts_chart: bool,
    /// 工程に人が要るか (工程以外は使わない)
    pub labor: Labor,
}

/// 送り出したワークを運ぶ AGV の搬送: `from` のステーションで積み、`to` で降ろす。
/// 番号は AGV の経路の点の id。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgvCall {
    pub from: u32,
    pub to: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ElementKind {
    Source {
        /// ワークを出す間隔 (秒)
        interval: f64,
        /// 間隔のばらつき (± 割合, 0〜1)。0 なら一定間隔。
        spread: f64,
        /// 出す個数の上限。None なら無制限。
        limit: Option<u32>,
    },
    Process {
        /// 1個あたりの加工時間 (秒)
        cycle_time: f64,
        /// 加工時間のばらつき (± 割合, 0〜1)
        spread: f64,
        failure: Option<Failure>,
    },
    Buffer {
        capacity: u32,
    },
    Sink,
}

/// 故障の入れ方。加工している時間で数える。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Failure {
    /// 平均故障間隔 (秒)
    pub mtbf: f64,
    /// 平均修理時間 (秒)
    pub mttr: f64,
}

/// 設備の電力とエア。稼働中は定格、それ以外 (待ち・詰まり・故障) は待機電力で数える。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Energy {
    /// 定格電力 (kW)
    pub rated_kw: f64,
    /// 待機電力 (kW)
    pub standby_kw: f64,
    /// 稼働中のエア消費量 (L/min, ANR)
    pub air_l_per_min: f64,
}

/// 要素の種類 (追加ボタン用)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindTag {
    Source,
    Process,
    Buffer,
    Sink,
}

impl KindTag {
    pub const ALL: [KindTag; 4] = [
        KindTag::Source,
        KindTag::Process,
        KindTag::Buffer,
        KindTag::Sink,
    ];

    pub fn label(self) -> &'static str {
        match self {
            KindTag::Source => "生成",
            KindTag::Process => "工程",
            KindTag::Buffer => "バッファ",
            KindTag::Sink => "排出",
        }
    }

    /// 追加したときの既定値
    pub fn default_kind(self) -> ElementKind {
        match self {
            KindTag::Source => ElementKind::Source {
                interval: 30.0,
                spread: 0.0,
                limit: None,
            },
            KindTag::Process => ElementKind::Process {
                cycle_time: 30.0,
                spread: 0.0,
                failure: None,
            },
            KindTag::Buffer => ElementKind::Buffer { capacity: 5 },
            KindTag::Sink => ElementKind::Sink,
        }
    }
}

impl ElementKind {
    pub fn tag(&self) -> KindTag {
        match self {
            ElementKind::Source { .. } => KindTag::Source,
            ElementKind::Process { .. } => KindTag::Process,
            ElementKind::Buffer { .. } => KindTag::Buffer,
            ElementKind::Sink => KindTag::Sink,
        }
    }
}

impl FlowModel {
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn get(&self, id: ElementId) -> Option<&Element> {
        self.elements.iter().find(|e| e.id == id)
    }

    pub fn get_mut(&mut self, id: ElementId) -> Option<&mut Element> {
        self.elements.iter_mut().find(|e| e.id == id)
    }

    pub fn index_of(&self, id: ElementId) -> Option<usize> {
        self.elements.iter().position(|e| e.id == id)
    }

    /// 要素を末尾に足す。名前は「工程1」のように種類と番号で付ける。
    pub fn add(&mut self, tag: KindTag) -> ElementId {
        let number = self.elements.iter().filter(|e| e.kind.tag() == tag).count() + 1;
        self.insert(Element {
            id: 0,
            name: format!("{}{number}", tag.label()),
            kind: tag.default_kind(),
            next: Vec::new(),
            part: None,
            energy: Energy::default(),
            carrier: None,
            starts_chart: false,
            labor: Labor::None,
        })
    }

    /// 要素を足して、末尾の要素 (排出以外) から新しい要素へつなぐ。
    /// ノーコードで「並べた順に流れる」ラインを作るための追加。
    pub fn append(&mut self, tag: KindTag) -> ElementId {
        let previous = self
            .elements
            .iter()
            .rev()
            .find(|e| e.kind.tag() != KindTag::Sink && e.next.is_empty())
            .map(|e| e.id);
        let id = self.add(tag);
        if let Some(previous) = previous
            && tag != KindTag::Source
        {
            self.get_mut(previous).unwrap().next.push(id);
        }
        id
    }

    /// 要素をそのまま入れる。ID は振り直す。ファイルからの読み込みでは
    /// [`FlowModel::insert_with_id`] を使う。
    pub fn insert(&mut self, mut element: Element) -> ElementId {
        element.id = self.next_id;
        self.next_id += 1;
        let id = element.id;
        self.elements.push(element);
        id
    }

    /// ID を保ったまま入れる (読み込み用)。同じ ID がすでにあれば false。
    pub fn insert_with_id(&mut self, element: Element) -> bool {
        if self.get(element.id).is_some() {
            return false;
        }
        self.next_id = self.next_id.max(element.id + 1);
        self.elements.push(element);
        true
    }

    /// 要素を消し、それを指していた送り先も消す。
    pub fn remove(&mut self, id: ElementId) {
        self.elements.retain(|e| e.id != id);
        for e in &mut self.elements {
            e.next.retain(|&n| n != id);
        }
        self.staffing.forget_element(id);
    }

    /// 並び順を1つ動かす (`up` なら前へ)
    pub fn move_element(&mut self, id: ElementId, up: bool) {
        let Some(i) = self.index_of(id) else { return };
        let j = if up { i.checked_sub(1) } else { Some(i + 1) };
        if let Some(j) = j.filter(|&j| j < self.elements.len()) {
            self.elements.swap(i, j);
        }
    }

    /// 回せない理由の一覧。空なら回せる。
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let has = |tag| self.elements.iter().any(|e| e.kind.tag() == tag);
        if !has(KindTag::Source) {
            out.push("「生成」がありません".into());
        }
        if !has(KindTag::Sink) {
            out.push("「排出」がありません".into());
        }
        for e in &self.elements {
            let name = &e.name;
            if e.kind.tag() != KindTag::Sink && e.next.is_empty() {
                out.push(format!("{name}: 送り先がありません"));
            }
            for n in &e.next {
                match self.get(*n) {
                    None => out.push(format!("{name}: 送り先 #{n} がありません")),
                    Some(t) if t.kind.tag() == KindTag::Source => {
                        out.push(format!("{name}: 「生成」には送れません"))
                    }
                    Some(_) if *n == e.id => out.push(format!("{name}: 自分には送れません")),
                    _ => {}
                }
            }
            match &e.kind {
                ElementKind::Source {
                    interval, spread, ..
                } => {
                    if interval.is_nan() || *interval < MIN_TIME {
                        out.push(format!("{name}: 間隔は {MIN_TIME} 秒以上にしてください"));
                    }
                    if !(0.0..=1.0).contains(spread) {
                        out.push(format!("{name}: ばらつきは 0〜100% にしてください"));
                    }
                }
                ElementKind::Process {
                    cycle_time,
                    spread,
                    failure,
                } => {
                    if cycle_time.is_nan() || *cycle_time < MIN_TIME {
                        out.push(format!(
                            "{name}: サイクルタイムは {MIN_TIME} 秒以上にしてください"
                        ));
                    }
                    if !(0.0..=1.0).contains(spread) {
                        out.push(format!("{name}: ばらつきは 0〜100% にしてください"));
                    }
                    if let Some(f) = failure
                        && (f.mtbf.is_nan()
                            || f.mttr.is_nan()
                            || f.mtbf < MIN_TIME
                            || f.mttr < MIN_TIME)
                    {
                        out.push(format!("{name}: MTBF・MTTR は正の値にしてください"));
                    }
                }
                ElementKind::Buffer { capacity } => {
                    if *capacity == 0 {
                        out.push(format!("{name}: 容量は 1 以上にしてください"));
                    }
                }
                ElementKind::Sink => {
                    if e.carrier.is_some() {
                        out.push(format!("{name}: 「排出」は AGV で送り出せません"));
                    }
                }
            }
            if e.kind.tag() == KindTag::Process {
                match e.labor {
                    Labor::None => {}
                    Labor::Load { seconds } if !(seconds >= 0.0 && seconds.is_finite()) => {
                        out.push(format!("{name}: 人が付く時間は 0 秒以上にしてください"));
                    }
                    _ => {
                        let skilled = self
                            .staffing
                            .workers
                            .iter()
                            .any(|w| w.speed_for(e.id).is_some());
                        if !skilled {
                            out.push(format!(
                                "{name}: 人が要る工程ですが、できる作業者が名簿にいません"
                            ));
                        }
                    }
                }
            }
            let energy = [
                e.energy.rated_kw,
                e.energy.standby_kw,
                e.energy.air_l_per_min,
            ];
            if energy.iter().any(|v| !(v.is_finite() && *v >= 0.0)) {
                out.push(format!("{name}: 電力・エアは 0 以上にしてください"));
            }
        }
        let s = &self.settings;
        if !(s.duration > 0.0 && s.duration <= MAX_DURATION) {
            out.push("回す時間は 0 より長く、1000 時間以内にしてください".into());
        }
        if !(s.warmup >= 0.0 && s.warmup < s.duration) {
            out.push("立ち上がり時間は回す時間より短くしてください".into());
        }
        out.extend(self.staffing.problems());
        out
    }
}

/// 時間の下限 (秒)。0 秒の工程が輪になると時間が進まなくなるため。
pub const MIN_TIME: f64 = 0.01;
/// 回す時間の上限 (秒)
pub const MAX_DURATION: f64 = 1000.0 * 3600.0;

/// 動作確認用のサンプルライン: 投入 → 加工 → バッファ → 組立 (故障あり) → 検査 → 排出
pub fn sample_line() -> FlowModel {
    let mut m = FlowModel::default();
    let source = m.append(KindTag::Source);
    let machining = m.append(KindTag::Process);
    let buffer = m.append(KindTag::Buffer);
    let assembly = m.append(KindTag::Process);
    let inspection = m.append(KindTag::Process);
    let sink = m.append(KindTag::Sink);
    let set = |m: &mut FlowModel, id, name: &str, kind, energy| {
        let e = m.get_mut(id).unwrap();
        e.name = name.into();
        e.kind = kind;
        e.energy = energy;
    };
    set(
        &mut m,
        source,
        "投入",
        ElementKind::Source {
            interval: 25.0,
            spread: 0.2,
            limit: None,
        },
        Energy::default(),
    );
    set(
        &mut m,
        machining,
        "加工",
        ElementKind::Process {
            cycle_time: 30.0,
            spread: 0.1,
            failure: None,
        },
        Energy {
            rated_kw: 7.5,
            standby_kw: 1.2,
            air_l_per_min: 50.0,
        },
    );
    set(
        &mut m,
        buffer,
        "中間バッファ",
        ElementKind::Buffer { capacity: 5 },
        Energy::default(),
    );
    set(
        &mut m,
        assembly,
        "組立",
        ElementKind::Process {
            cycle_time: 27.0,
            spread: 0.1,
            failure: Some(Failure {
                mtbf: 3600.0,
                mttr: 300.0,
            }),
        },
        Energy {
            rated_kw: 3.0,
            standby_kw: 0.5,
            air_l_per_min: 120.0,
        },
    );
    set(
        &mut m,
        inspection,
        "検査",
        ElementKind::Process {
            cycle_time: 10.0,
            spread: 0.0,
            failure: None,
        },
        Energy {
            rated_kw: 0.8,
            standby_kw: 0.3,
            air_l_per_min: 0.0,
        },
    );
    set(&mut m, sink, "排出", ElementKind::Sink, Energy::default());
    m
}

/// サンプルラインに人員を入れる: 加工は投入だけ人、組立と検査は手作業。
/// 日勤 (8:00〜17:00、休憩 10:00〜10:10・12:00〜13:00・15:00〜15:10) の 3 人。
/// 工程の名前で探すので、[`sample_line`] のあとに呼ぶ。
pub fn apply_sample_staffing(m: &mut FlowModel) {
    use crate::staffing::model::{Break, Shift, Skill, Worker};
    const H: f64 = 3600.0;
    let find = |m: &FlowModel, name: &str| m.elements.iter().find(|e| e.name == name).map(|e| e.id);
    let (Some(machining), Some(assembly), Some(inspection)) =
        (find(m, "加工"), find(m, "組立"), find(m, "検査"))
    else {
        return;
    };
    m.get_mut(machining).unwrap().labor = Labor::Load { seconds: 8.0 };
    m.get_mut(assembly).unwrap().labor = Labor::Full;
    m.get_mut(inspection).unwrap().labor = Labor::Full;
    let mut staffing = Staffing::default();
    staffing.walk_seconds = 5.0;
    let day = staffing.insert_shift(Shift {
        id: 0,
        name: "日勤".into(),
        start: 8.0 * H,
        end: 17.0 * H,
        breaks: vec![
            Break {
                start: 10.0 * H,
                end: 10.0 * H + 600.0,
            },
            Break {
                start: 12.0 * H,
                end: 13.0 * H,
            },
            Break {
                start: 15.0 * H,
                end: 15.0 * H + 600.0,
            },
        ],
    });
    let skill = |element, speed| Skill { element, speed };
    for (name, skills) in [
        ("Aさん", vec![skill(assembly, 1.1), skill(machining, 1.0)]),
        ("Bさん", vec![skill(assembly, 1.0), skill(inspection, 1.0)]),
        ("Cさん", vec![skill(inspection, 1.2), skill(machining, 1.0)]),
    ] {
        staffing.insert_worker(Worker {
            id: 0,
            name: name.into(),
            shift: Some(day),
            absent: false,
            skills,
        });
    }
    m.staffing = staffing;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_links_elements_in_order() {
        let mut m = FlowModel::default();
        let a = m.append(KindTag::Source);
        let b = m.append(KindTag::Process);
        let c = m.append(KindTag::Sink);
        assert_eq!(m.get(a).unwrap().next, vec![b]);
        assert_eq!(m.get(b).unwrap().next, vec![c]);
        assert!(m.get(c).unwrap().next.is_empty());
        assert_eq!(m.get(b).unwrap().name, "工程1");
        assert!(m.problems().is_empty(), "{:?}", m.problems());
    }

    #[test]
    fn remove_drops_links_to_the_element() {
        let mut m = FlowModel::default();
        let a = m.append(KindTag::Source);
        let b = m.append(KindTag::Process);
        m.append(KindTag::Sink);
        m.remove(b);
        assert!(m.get(a).unwrap().next.is_empty());
        assert!(!m.problems().is_empty());
    }

    #[test]
    fn problems_name_missing_parts_and_bad_values() {
        let mut m = FlowModel::default();
        let p = m.add(KindTag::Process);
        if let ElementKind::Process { cycle_time, .. } = &mut m.get_mut(p).unwrap().kind {
            *cycle_time = 0.0;
        }
        let problems = m.problems().join("\n");
        assert!(problems.contains("「生成」がありません"), "{problems}");
        assert!(problems.contains("「排出」がありません"), "{problems}");
        assert!(problems.contains("工程1: 送り先がありません"), "{problems}");
        assert!(problems.contains("サイクルタイム"), "{problems}");
    }

    #[test]
    fn sample_line_is_runnable() {
        assert!(sample_line().problems().is_empty());
        let mut staffed = sample_line();
        apply_sample_staffing(&mut staffed);
        assert!(staffed.problems().is_empty(), "{:?}", staffed.problems());
        assert_eq!(staffed.staffing.workers.len(), 3);
    }

    #[test]
    fn labor_needs_a_skilled_worker() {
        let mut m = sample_line();
        let assembly = m.elements[3].id;
        m.get_mut(assembly).unwrap().labor = Labor::Full;
        let problems = m.problems().join("\n");
        assert!(problems.contains("組立: 人が要る工程"), "{problems}");
        let w = m.staffing.add_worker();
        m.staffing.worker_mut(w).unwrap().set_skill(assembly, true);
        assert!(m.problems().is_empty(), "{:?}", m.problems());
        // 工程を消すとスキルも消える
        m.remove(assembly);
        assert!(m.staffing.worker(w).unwrap().skills.is_empty());
    }
}
