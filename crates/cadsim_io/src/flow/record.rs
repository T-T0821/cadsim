//! 工程モデルをプロジェクトファイル (.cadsim) に書く形 (Bevy に依存しない)
//!
//! 3D の部品はノード ID ではなく木の中の位置 (`[ルートの番号, 子の番号, …]`) で指す。
//! ノード ID は開き直すと変わるため。位置との変換は呼び出し側が渡す。

use serde::{Deserialize, Serialize};

use cadsim_core::staffing::model::{AssignRule, Break, Shift, Skill, Staffing, Worker};

use cadsim_core::flow::model::{
    AgvCall, Element, ElementKind, Energy, Failure, FlowModel, Labor, RunSettings,
};

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
    /// ライン連携: 加工を始めるたびにタイミングチャートを頭から再生する
    #[serde(default, skip_serializing_if = "is_false")]
    pub starts_chart: bool,
    /// 工程に人が要るか (要らなければ書かない)
    #[serde(default, skip_serializing_if = "LaborRecord::is_none")]
    pub labor: LaborRecord,
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
}

/// 保存する形にする。`part_path` はノード ID を木の中の位置にする (部品が消えていれば None)。
pub fn to_record(model: &FlowModel, part_path: impl Fn(u32) -> Option<Vec<usize>>) -> FlowRecord {
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
                },
                next: e.next.clone(),
                part: e.part.and_then(&part_path),
                rated_kw: e.energy.rated_kw,
                standby_kw: e.energy.standby_kw,
                air_l_per_min: e.energy.air_l_per_min,
                agv: e.carrier.map(|c| AgvCallRecord {
                    from: c.from,
                    to: c.to,
                }),
                starts_chart: e.starts_chart,
                labor: e.labor.into(),
            })
            .collect(),
        staffing: staffing_record(&model.staffing),
    }
}

/// ファイルから読む。`find_part` は木の中の位置をノード ID にする。
/// 戻り値の2つ目は、部品が見つからなかった・ID が重なっていた要素の注意。
pub fn from_record(
    record: &FlowRecord,
    find_part: impl Fn(&[usize]) -> Option<u32>,
) -> (FlowModel, Vec<String>) {
    let mut model = FlowModel::default();
    model.settings = RunSettings {
        duration: record.duration,
        warmup: record.warmup,
        seed: record.seed,
    };
    let mut warnings = Vec::new();
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
            starts_chart: r.starts_chart,
            labor: r.labor.into(),
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
