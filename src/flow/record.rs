//! 工程モデルをプロジェクトファイル (.cadsim) に書く形 (Bevy に依存しない)
//!
//! 3D の部品はノード ID ではなく木の中の位置 (`[ルートの番号, 子の番号, …]`) で指す。
//! ノード ID は開き直すと変わるため。位置との変換は呼び出し側が渡す。

use serde::{Deserialize, Serialize};

use super::model::{Element, ElementKind, Energy, Failure, FlowModel, RunSettings};

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
            })
            .collect(),
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
        });
        if !inserted {
            warnings.push(format!(
                "工程の ID {} が重なっているため読み飛ばしました",
                r.id
            ));
        }
    }
    (model, warnings)
}

#[cfg(test)]
mod tests {
    use super::super::model::sample_line;
    use super::*;

    #[test]
    fn round_trip_keeps_the_line() {
        let mut model = sample_line();
        model.elements[1].part = Some(42);
        let record = to_record(&model, |id| (id == 42).then(|| vec![0, 3]));
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"type\":\"Process\""), "{json}");
        let parsed: FlowRecord = serde_json::from_str(&json).unwrap();
        let (restored, warnings) = from_record(&parsed, |p| (p == [0, 3]).then_some(42));
        assert!(warnings.is_empty());
        assert_eq!(restored, model);
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
