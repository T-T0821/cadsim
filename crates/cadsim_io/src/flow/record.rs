//! 工程モデルをプロジェクトファイル (.plantwright) に書く形 (Bevy に依存しない)
//!
//! 3D の部品はノード ID ではなく木の中の位置 (`[ルートの番号, 子の番号, …]`) で指す。
//! ノード ID は開き直すと変わるため。位置との変換は呼び出し側が渡す。

use serde::{Deserialize, Serialize};

use cadsim_core::staffing::model::{AssignRule, Break, Shift, Skill, Staffing, Worker};

use cadsim_core::flow::model::{
    AgvCall, Element, ElementKind, Energy, Failure, FlowModel, Labor, Line, Order, Product,
    ProductTime, RunSettings,
};
use cadsim_core::flow::scenario::Scenario;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FlowRecord {
    /// 回す時間 (秒)
    pub duration: f64,
    /// 立ち上がり (秒)
    #[serde(default)]
    pub warmup: f64,
    #[serde(default = "default_seed")]
    pub seed: u64,
    pub elements: Vec<ElementRecord>,
    /// 作業者の名簿とシフト (なければ書かない)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staffing: Option<StaffingRecord>,
    /// ラインのまとまり (なければ書かない)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<LineRecord>,
    /// 品種 (なければ書かない)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub products: Vec<ProductRecord>,
    /// 生産計画の注文 (なければ書かない)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orders: Vec<OrderRecord>,
    /// 比べるために取っておいた案 (なければ書かない)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenarios: Vec<ScenarioRecord>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LineRecord {
    pub id: u32,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProductRecord {
    pub id: u32,
    pub name: String,
}

/// 注文。`due` は開始からの秒、`source` は投入する生成の id (なければいちばん上の生成)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OrderRecord {
    pub name: String,
    pub product: u32,
    pub quantity: u32,
    pub due: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct ProductTimeRecord {
    pub product: u32,
    pub cycle_time: f64,
}

/// 名前つきの設定の写し。中身は工程モデルと同じ形 (案の中に案は持たない)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ScenarioRecord {
    pub name: String,
    pub flow: FlowRecord,
}

fn default_seed() -> u64 {
    1
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ElementRecord {
    pub id: u32,
    pub name: String,
    #[serde(flatten)]
    pub kind: KindRecord,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<u32>,
    /// 対応する 3D の部品の木の中の位置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<Vec<usize>>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rated_kw: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub standby_kw: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub air_l_per_min: f64,
    /// ライン連携: 送り出しを運ぶ AGV の積み込み・荷降ろしの点の id
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agv: Option<AgvCallRecord>,
    /// ライン連携: 送り出しを流すワーク搬送の投入口の id
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conveyor: Option<u32>,
    /// ライン連携: 加工を始めるたびにタイミングチャートを頭から再生する
    #[serde(default, skip_serializing_if = "is_false")]
    pub starts_chart: bool,
    /// 工程に人が要るか (要らなければ書かない)
    #[serde(default, skip_serializing_if = "LaborRecord::is_none")]
    pub labor: LaborRecord,
    /// 属するラインの id (なければ書かない)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// 生成: 注文がないときに作る品種の id
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<u32>,
    /// 工程: 品種ごとの加工時間 (秒)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub product_times: Vec<ProductTimeRecord>,
    /// 工程: 品種が変わるときの段取り替えの時間 (秒)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub setup_time: f64,
    /// 受け取る品種の id (空なら何でも)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepts: Vec<u32>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct AgvCallRecord {
    pub from: u32,
    pub to: u32,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// 工程に人が要るか。JSON では `"labor": {"type": "Load", "seconds": 8}` のように書く。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(tag = "type")]
pub enum LaborRecord {
    #[default]
    None,
    Full,
    Load {
        seconds: f64,
    },
}

impl LaborRecord {
    fn is_none(&self) -> bool {
        *self == LaborRecord::None
    }
}

impl From<Labor> for LaborRecord {
    fn from(l: Labor) -> Self {
        match l {
            Labor::None => LaborRecord::None,
            Labor::Full => LaborRecord::Full,
            Labor::Load { seconds } => LaborRecord::Load { seconds },
        }
    }
}

impl From<LaborRecord> for Labor {
    fn from(l: LaborRecord) -> Self {
        match l {
            LaborRecord::None => Labor::None,
            LaborRecord::Full => Labor::Full,
            LaborRecord::Load { seconds } => Labor::Load { seconds },
        }
    }
}

/// 作業者の名簿とシフト。時刻は 0:00 からの秒、時間は秒。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StaffingRecord {
    pub start_clock: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub walk_seconds: f64,
    /// 割り当てのルール (`Fastest` / `LongestIdle` / `Primary`)。既定の `Fastest` なら書かない
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shifts: Vec<ShiftRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workers: Vec<WorkerRecord>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShiftRecord {
    pub id: u32,
    pub name: String,
    pub start: f64,
    pub end: f64,
    /// 休憩 `[開始, 終了]` の並び
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breaks: Vec<[f64; 2]>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WorkerRecord {
    pub id: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shift: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub absent: bool,
    /// できる工程 `[工程の ID, 速さ]` の並び (速さ 1.0 が標準)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<(u32, f64)>,
    /// 持ち場の工程の ID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<u32>,
}

fn staffing_record(s: &Staffing) -> Option<StaffingRecord> {
    (!s.is_empty()).then(|| StaffingRecord {
        start_clock: s.start_clock,
        walk_seconds: s.walk_seconds,
        rule: (s.rule != AssignRule::Fastest).then(|| s.rule.key().to_owned()),
        shifts: s
            .shifts
            .iter()
            .map(|sh| ShiftRecord {
                id: sh.id,
                name: sh.name.clone(),
                start: sh.start,
                end: sh.end,
                breaks: sh.breaks.iter().map(|b| [b.start, b.end]).collect(),
            })
            .collect(),
        workers: s
            .workers
            .iter()
            .map(|w| WorkerRecord {
                id: w.id,
                name: w.name.clone(),
                shift: w.shift,
                absent: w.absent,
                skills: w.skills.iter().map(|k| (k.element, k.speed)).collect(),
                primary: w.primary,
            })
            .collect(),
    })
}

fn staffing_from_record(r: &StaffingRecord, warnings: &mut Vec<String>) -> Staffing {
    let mut s = Staffing::default();
    s.start_clock = r.start_clock;
    s.walk_seconds = r.walk_seconds;
    if let Some(key) = &r.rule {
        match AssignRule::from_key(key) {
            Some(rule) => s.rule = rule,
            None => warnings.push(format!(
                "割り当てのルール「{key}」が分からないため「速い人が行く」にしました"
            )),
        }
    }
    for sh in &r.shifts {
        let ok = s.insert_shift_with_id(Shift {
            id: sh.id,
            name: sh.name.clone(),
            start: sh.start,
            end: sh.end,
            breaks: sh
                .breaks
                .iter()
                .map(|[start, end]| Break {
                    start: *start,
                    end: *end,
                })
                .collect(),
        });
        if !ok {
            warnings.push(format!(
                "シフトの ID {} が重なっているため読み飛ばしました",
                sh.id
            ));
        }
    }
    for w in &r.workers {
        let ok = s.insert_worker_with_id(Worker {
            id: w.id,
            name: w.name.clone(),
            shift: w.shift,
            absent: w.absent,
            skills: w
                .skills
                .iter()
                .map(|&(element, speed)| Skill { element, speed })
                .collect(),
            primary: w.primary,
        });
        if !ok {
            warnings.push(format!(
                "作業者の ID {} が重なっているため読み飛ばしました",
                w.id
            ));
        }
    }
    s
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// 要素の種類。JSON では `"type"` で見分ける。時間はすべて秒。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum KindRecord {
    Source {
        interval: f64,
        #[serde(default, skip_serializing_if = "is_zero")]
        spread: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    Process {
        cycle_time: f64,
        #[serde(default, skip_serializing_if = "is_zero")]
        spread: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mtbf: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mttr: Option<f64>,
    },
    Buffer {
        capacity: u32,
    },
    Sink,
    Transport {
        travel_time: f64,
        capacity: u32,
    },
}

/// 保存する形にする。`part_path` はノード ID を木の中の位置にする (部品が消えていれば None)。
pub fn to_record(model: &FlowModel, part_path: impl Fn(u32) -> Option<Vec<usize>>) -> FlowRecord {
    let mut record = model_record(model, &part_path);
    record.scenarios = model
        .scenarios
        .iter()
        .map(|s| ScenarioRecord {
            name: s.name.clone(),
            flow: model_record(&s.model, &part_path),
        })
        .collect();
    record
}

fn model_record(model: &FlowModel, part_path: &dyn Fn(u32) -> Option<Vec<usize>>) -> FlowRecord {
    FlowRecord {
        duration: model.settings.duration,
        warmup: model.settings.warmup,
        seed: model.settings.seed,
        elements: model
            .elements
            .iter()
            .map(|e| ElementRecord {
                id: e.id,
                name: e.name.clone(),
                kind: match &e.kind {
                    ElementKind::Source {
                        interval,
                        spread,
                        limit,
                    } => KindRecord::Source {
                        interval: *interval,
                        spread: *spread,
                        limit: *limit,
                    },
                    ElementKind::Process {
                        cycle_time,
                        spread,
                        failure,
                    } => KindRecord::Process {
                        cycle_time: *cycle_time,
                        spread: *spread,
                        mtbf: failure.map(|f| f.mtbf),
                        mttr: failure.map(|f| f.mttr),
                    },
                    ElementKind::Buffer { capacity } => KindRecord::Buffer {
                        capacity: *capacity,
                    },
                    ElementKind::Sink => KindRecord::Sink,
                    ElementKind::Transport {
                        travel_time,
                        capacity,
                    } => KindRecord::Transport {
                        travel_time: *travel_time,
                        capacity: *capacity,
                    },
                },
                next: e.next.clone(),
                part: e.part.and_then(part_path),
                rated_kw: e.energy.rated_kw,
                standby_kw: e.energy.standby_kw,
                air_l_per_min: e.energy.air_l_per_min,
                agv: e.carrier.map(|c| AgvCallRecord {
                    from: c.from,
                    to: c.to,
                }),
                conveyor: e.conveyor,
                starts_chart: e.starts_chart,
                labor: e.labor.into(),
                line: e.line,
                product: e.product,
                product_times: e
                    .product_times
                    .iter()
                    .map(|t| ProductTimeRecord {
                        product: t.product,
                        cycle_time: t.cycle_time,
                    })
                    .collect(),
                setup_time: e.setup_time,
                accepts: e.accepts.clone(),
            })
            .collect(),
        products: model
            .products
            .iter()
            .map(|p| ProductRecord {
                id: p.id,
                name: p.name.clone(),
            })
            .collect(),
        orders: model
            .orders
            .iter()
            .map(|o| OrderRecord {
                name: o.name.clone(),
                product: o.product,
                quantity: o.quantity,
                due: o.due,
                source: o.source,
            })
            .collect(),
        staffing: staffing_record(&model.staffing),
        lines: model
            .lines
            .iter()
            .map(|l| LineRecord {
                id: l.id,
                name: l.name.clone(),
            })
            .collect(),
        scenarios: Vec::new(),
    }
}

/// ファイルから読む。`find_part` は木の中の位置をノード ID にする。
/// 戻り値の2つ目は、部品が見つからなかった・ID が重なっていた要素の注意。
pub fn from_record(
    record: &FlowRecord,
    find_part: impl Fn(&[usize]) -> Option<u32>,
) -> (FlowModel, Vec<String>) {
    let (mut model, mut warnings) = model_from_record(record, &find_part);
    for s in &record.scenarios {
        let (scenario, scenario_warnings) = model_from_record(&s.flow, &find_part);
        warnings.extend(
            scenario_warnings
                .into_iter()
                .map(|w| format!("案「{}」: {w}", s.name)),
        );
        model.scenarios.push(Scenario {
            name: s.name.clone(),
            model: scenario,
        });
    }
    (model, warnings)
}

fn model_from_record(
    record: &FlowRecord,
    find_part: &dyn Fn(&[usize]) -> Option<u32>,
) -> (FlowModel, Vec<String>) {
    let mut model = FlowModel::default();
    model.settings = RunSettings {
        duration: record.duration,
        warmup: record.warmup,
        seed: record.seed,
    };
    let mut warnings = Vec::new();
    let known_product = |id: u32| record.products.iter().any(|p| p.id == id);
    for r in &record.elements {
        let part = r.part.as_ref().and_then(|path| {
            let found = find_part(path);
            if found.is_none() {
                warnings.push(format!(
                    "工程「{}」の 3D 部品が見つからないため、対応付けを外しました",
                    r.name
                ));
            }
            found
        });
        let kind = match &r.kind {
            KindRecord::Source {
                interval,
                spread,
                limit,
            } => ElementKind::Source {
                interval: *interval,
                spread: *spread,
                limit: *limit,
            },
            KindRecord::Process {
                cycle_time,
                spread,
                mtbf,
                mttr,
            } => ElementKind::Process {
                cycle_time: *cycle_time,
                spread: *spread,
                failure: match (mtbf, mttr) {
                    (Some(mtbf), Some(mttr)) => Some(Failure {
                        mtbf: *mtbf,
                        mttr: *mttr,
                    }),
                    _ => None,
                },
            },
            KindRecord::Buffer { capacity } => ElementKind::Buffer {
                capacity: *capacity,
            },
            KindRecord::Sink => ElementKind::Sink,
            KindRecord::Transport {
                travel_time,
                capacity,
            } => ElementKind::Transport {
                travel_time: *travel_time,
                capacity: *capacity,
            },
        };
        let inserted = model.insert_with_id(Element {
            id: r.id,
            name: r.name.clone(),
            kind,
            next: r.next.clone(),
            part,
            energy: Energy {
                rated_kw: r.rated_kw,
                standby_kw: r.standby_kw,
                air_l_per_min: r.air_l_per_min,
            },
            carrier: r.agv.map(|c| AgvCall {
                from: c.from,
                to: c.to,
            }),
            conveyor: r.conveyor,
            starts_chart: r.starts_chart,
            labor: r.labor.into(),
            line: r.line.filter(|l| record.lines.iter().any(|k| k.id == *l)),
            product: r.product.filter(|p| known_product(*p)),
            product_times: r
                .product_times
                .iter()
                .filter(|t| known_product(t.product))
                .map(|t| ProductTime {
                    product: t.product,
                    cycle_time: t.cycle_time,
                })
                .collect(),
            setup_time: r.setup_time,
            accepts: r
                .accepts
                .iter()
                .copied()
                .filter(|p| known_product(*p))
                .collect(),
        });
        if !inserted {
            warnings.push(format!(
                "工程の ID {} が重なっているため読み飛ばしました",
                r.id
            ));
        }
    }
    if let Some(staffing) = &record.staffing {
        model.staffing = staffing_from_record(staffing, &mut warnings);
    }
    model.products = record
        .products
        .iter()
        .map(|p| Product {
            id: p.id,
            name: p.name.clone(),
        })
        .collect();
    model.orders = record
        .orders
        .iter()
        .filter(|o| known_product(o.product))
        .map(|o| Order {
            name: o.name.clone(),
            product: o.product,
            quantity: o.quantity,
            due: o.due,
            source: o.source,
        })
        .collect();
    model.lines = record
        .lines
        .iter()
        .map(|l| Line {
            id: l.id,
            name: l.name.clone(),
        })
        .collect();
    (model, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadsim_core::flow::model::sample_line;

    #[test]
    fn round_trip_keeps_the_line() {
        let mut model = sample_line();
        model.elements[1].part = Some(42);
        model.elements[1].carrier = Some(AgvCall { from: 3, to: 5 });
        model.elements[1].starts_chart = true;
        model.elements[2].conveyor = Some(7);
        let record = to_record(&model, |id| (id == 42).then(|| vec![0, 3]));
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"type\":\"Process\""), "{json}");
        let parsed: FlowRecord = serde_json::from_str(&json).unwrap();
        let (restored, warnings) = from_record(&parsed, |p| (p == [0, 3]).then_some(42));
        assert!(warnings.is_empty());
        assert_eq!(restored, model);
    }

    #[test]
    fn round_trip_keeps_the_staffing() {
        let mut model = sample_line();
        cadsim_core::flow::model::apply_sample_staffing(&mut model);
        model.staffing.workers[2].absent = true;
        let record = to_record(&model, |_| None);
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"labor\":{\"type\":\"Full\"}"), "{json}");
        let parsed: FlowRecord = serde_json::from_str(&json).unwrap();
        let (restored, warnings) = from_record(&parsed, |_| None);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(restored, model);
    }

    #[test]
    fn round_trip_keeps_the_scenarios() {
        let mut model = sample_line();
        model.elements[1].part = Some(42);
        model.save_scenario("今のまま");
        if let ElementKind::Process { cycle_time, .. } = &mut model.elements[1].kind {
            *cycle_time = 20.0;
        }
        cadsim_core::flow::model::apply_sample_staffing(&mut model);
        model.save_scenario("速くして人を置く");
        let record = to_record(&model, |id| (id == 42).then(|| vec![0, 3]));
        let json = serde_json::to_string(&record).unwrap();
        let parsed: FlowRecord = serde_json::from_str(&json).unwrap();
        let (restored, warnings) = from_record(&parsed, |p| (p == [0, 3]).then_some(42));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(restored, model);
        assert_eq!(restored.scenarios[1].name, "速くして人を置く");
        // 案の中の部品が見つからなければ、案の名前を付けて知らせる
        let (_, warnings) = from_record(&parsed, |_| None);
        assert!(
            warnings.iter().any(|w| w.starts_with("案「今のまま」")),
            "{warnings:?}"
        );
    }

    #[test]
    fn round_trip_keeps_the_factory() {
        let model = cadsim_core::flow::factory::sample_factory();
        let record = to_record(&model, |_| None);
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"type\":\"Transport\""), "{json}");
        assert!(json.contains("\"lines\""), "{json}");
        let parsed: FlowRecord = serde_json::from_str(&json).unwrap();
        let (restored, warnings) = from_record(&parsed, |_| None);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(restored, model);
    }

    #[test]
    fn round_trip_keeps_the_plan() {
        let mut model = cadsim_core::flow::factory::sample_factory();
        cadsim_core::flow::plan::apply_sample_plan(&mut model);
        let accepts = model.products[0].id;
        model.elements[2].accepts = vec![accepts];
        model.elements[0].product = Some(accepts);
        let record = to_record(&model, |_| None);
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"orders\""), "{json}");
        assert!(json.contains("\"setup_time\":600"), "{json}");
        let parsed: FlowRecord = serde_json::from_str(&json).unwrap();
        let (restored, warnings) = from_record(&parsed, |_| None);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(restored, model);
    }

    #[test]
    fn old_files_without_staffing_still_load() {
        let json = r#"{"duration":3600,"elements":[{"id":1,"name":"工程1","type":"Process","cycle_time":10}]}"#;
        let parsed: FlowRecord = serde_json::from_str(json).unwrap();
        let (restored, _) = from_record(&parsed, |_| None);
        assert_eq!(restored.elements[0].labor, Labor::None);
        assert!(restored.staffing.is_empty());
    }

    #[test]
    fn missing_parts_are_unlinked_with_a_warning() {
        let mut model = sample_line();
        model.elements[1].part = Some(42);
        let record = to_record(&model, |_| Some(vec![9]));
        let (restored, warnings) = from_record(&record, |_| None);
        assert_eq!(restored.elements[1].part, None);
        assert_eq!(warnings.len(), 1);
    }
}
