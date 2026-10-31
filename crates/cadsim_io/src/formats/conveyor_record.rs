//! ワーク搬送の設定をプロジェクトファイル (.cadsim) に書く形 (Bevy に依存しない)
//!
//! 部品 (ベルト・投入口・排出口) は木の中の位置 (`[ルートの番号, 子の番号, …]`) で指す。
//! グリッパのチャックはタイミングチャートのアクチュエータの id で指す (チャートと一緒に保存する)。
//! ワーク種類・投入口・コンベアどうしは設定の中の id で指す。

use cadsim_core::conveyor::{
    BeltAxis, Conveyor, ConveyorConfig, ConveyorEnd, Gripper, Sink, Source, SourceOutput, WorkKind,
};
use serde::{Deserialize, Serialize};

type NodePath = Vec<usize>;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ConveyorsRecord {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<WorkKindRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sinks: Vec<SinkRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conveyors: Vec<ConveyorRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grippers: Vec<GripperRecord>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WorkKindRecord {
    pub id: u32,
    pub name: String,
    pub size: [f64; 3],
    pub color: [f32; 3],
}

/// 投入口がワークを出す先
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum SourceOutputRecord {
    /// コンベア (id) の始端
    Conveyor { conveyor: u32 },
    /// 投入口の部品の上面
    OnPart,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SourceRecord {
    pub id: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<NodePath>,
    pub output: SourceOutputRecord,
    pub interval: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SinkRecord {
    pub id: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<NodePath>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeltAxisRecord {
    X,
    Z,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConveyorEndRecord {
    Stopper,
    Discharge,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ConveyorRecord {
    pub id: u32,
    pub name: String,
    /// ベルトにする部品の木の中の位置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<NodePath>,
    pub axis: BeltAxisRecord,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reversed: bool,
    pub speed: f64,
    pub end: ConveyorEndRecord,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GripperRecord {
    pub id: u32,
    pub name: String,
    /// 開閉に使うアクチュエータの id
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chuck: Option<u32>,
    pub tcp: [f64; 3],
    pub reach: f64,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// 保存する形にする。何もなければ None。`part_path` はノード ID を木の中の位置にする。
pub fn to_record(
    config: &ConveyorConfig,
    part_path: impl Fn(u32) -> Option<NodePath>,
) -> Option<ConveyorsRecord> {
    if *config == ConveyorConfig::default() {
        return None;
    }
    let path = |part: Option<u32>| part.and_then(&part_path);
    Some(ConveyorsRecord {
        kinds: config
            .kinds
            .iter()
            .map(|k| WorkKindRecord {
                id: k.id,
                name: k.name.clone(),
                size: k.size,
                color: k.color,
            })
            .collect(),
        sources: config
            .sources
            .iter()
            .map(|s| SourceRecord {
                id: s.id,
                name: s.name.clone(),
                part: path(s.part),
                output: match s.output {
                    SourceOutput::Conveyor(conveyor) => SourceOutputRecord::Conveyor { conveyor },
                    SourceOutput::OnPart => SourceOutputRecord::OnPart,
                },
                interval: s.interval,
                kinds: s.kinds.clone(),
                limit: s.limit,
            })
            .collect(),
        sinks: config
            .sinks
            .iter()
            .map(|s| SinkRecord {
                id: s.id,
                name: s.name.clone(),
                part: path(s.part),
            })
            .collect(),
        conveyors: config
            .conveyors
            .iter()
            .map(|c| ConveyorRecord {
                id: c.id,
                name: c.name.clone(),
                part: path(c.part),
                axis: match c.axis {
                    BeltAxis::X => BeltAxisRecord::X,
                    BeltAxis::Z => BeltAxisRecord::Z,
                },
                reversed: c.reversed,
                speed: c.speed,
                end: match c.end {
                    ConveyorEnd::Stopper => ConveyorEndRecord::Stopper,
                    ConveyorEnd::Discharge => ConveyorEndRecord::Discharge,
                },
            })
            .collect(),
        grippers: config
            .grippers
            .iter()
            .map(|g| GripperRecord {
                id: g.id,
                name: g.name.clone(),
                chuck: g.chuck,
                tcp: g.tcp,
                reach: g.reach,
            })
            .collect(),
    })
}

/// ファイルから読む。2つ目は部品が見つからなかったものの注意。
pub fn from_record(
    record: Option<&ConveyorsRecord>,
    find_part: impl Fn(&[usize]) -> Option<u32>,
) -> (ConveyorConfig, Vec<String>) {
    let Some(record) = record else {
        return Default::default();
    };
    let mut warnings = Vec::new();
    let mut part = |name: &str, path: &Option<NodePath>| {
        let path = path.as_ref()?;
        let found = find_part(path);
        if found.is_none() {
            warnings.push(format!(
                "ワーク搬送の「{name}」の部品が見つからないため、対応付けを外しました"
            ));
        }
        found
    };
    let conveyors: Vec<Conveyor> = record
        .conveyors
        .iter()
        .map(|r| Conveyor {
            id: r.id,
            name: r.name.clone(),
            part: part(&r.name, &r.part),
            axis: match r.axis {
                BeltAxisRecord::X => BeltAxis::X,
                BeltAxisRecord::Z => BeltAxis::Z,
            },
            reversed: r.reversed,
            speed: r.speed,
            end: match r.end {
                ConveyorEndRecord::Stopper => ConveyorEnd::Stopper,
                ConveyorEndRecord::Discharge => ConveyorEnd::Discharge,
            },
        })
        .collect();
    let kinds: Vec<WorkKind> = record
        .kinds
        .iter()
        .map(|k| WorkKind {
            id: k.id,
            name: k.name.clone(),
            size: k.size,
            color: k.color,
        })
        .collect();
    let sources: Vec<Source> = record
        .sources
        .iter()
        .map(|r| Source {
            id: r.id,
            name: r.name.clone(),
            part: part(&r.name, &r.part),
            output: match r.output {
                // 消えたコンベアへ出していたら部品の上に出す
                SourceOutputRecord::Conveyor { conveyor }
                    if conveyors.iter().any(|c| c.id == conveyor) =>
                {
                    SourceOutput::Conveyor(conveyor)
                }
                _ => SourceOutput::OnPart,
            },
            interval: r.interval,
            kinds: r
                .kinds
                .iter()
                .copied()
                .filter(|id| kinds.iter().any(|k| k.id == *id))
                .collect(),
            limit: r.limit,
        })
        .collect();
    let sinks: Vec<Sink> = record
        .sinks
        .iter()
        .map(|r| Sink {
            id: r.id,
            name: r.name.clone(),
            part: part(&r.name, &r.part),
        })
        .collect();
    let grippers: Vec<Gripper> = record
        .grippers
        .iter()
        .map(|r| Gripper {
            id: r.id,
            name: r.name.clone(),
            chuck: r.chuck,
            tcp: r.tcp,
            reach: r.reach,
        })
        .collect();
    let next_id = kinds
        .iter()
        .map(|k| k.id)
        .chain(sources.iter().map(|s| s.id))
        .chain(sinks.iter().map(|s| s.id))
        .chain(conveyors.iter().map(|c| c.id))
        .chain(grippers.iter().map(|g| g.id))
        .max()
        .map_or(1, |id| id + 1);
    (
        ConveyorConfig {
            kinds,
            sources,
            sinks,
            conveyors,
            grippers,
            next_id,
        },
        warnings,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ConveyorConfig {
        let mut config = ConveyorConfig::default();
        let belt = {
            let c = config.add_conveyor();
            c.part = Some(42);
            c.end = ConveyorEnd::Discharge;
            c.id
        };
        let kind = config.add_kind().id;
        let s = config.add_source();
        s.output = SourceOutput::Conveyor(belt);
        s.kinds = vec![kind];
        s.limit = Some(10);
        config.add_sink().part = Some(42);
        let g = config.add_gripper();
        g.chuck = Some(3);
        g.tcp = [1.0, 2.0, 3.0];
        config
    }

    #[test]
    fn round_trip() {
        let config = sample();
        let record = to_record(&config, |id| (id == 42).then(|| vec![0, 1])).unwrap();
        let json = serde_json::to_string(&record).unwrap();
        let parsed: ConveyorsRecord = serde_json::from_str(&json).unwrap();
        let (restored, warnings) = from_record(Some(&parsed), |p| (p == [0, 1]).then_some(42));
        assert!(warnings.is_empty());
        assert_eq!(restored, config);
        assert_eq!(to_record(&ConveyorConfig::default(), |_| None), None);
    }

    #[test]
    fn missing_parts_and_references_are_unlinked() {
        let mut record = to_record(&sample(), |_| Some(vec![9])).unwrap();
        record.conveyors.clear();
        let (restored, warnings) = from_record(Some(&record), |_| None);
        assert_eq!(restored.sinks[0].part, None);
        assert_eq!(warnings.len(), 1);
        assert_eq!(restored.sources[0].output, SourceOutput::OnPart);
    }
}
