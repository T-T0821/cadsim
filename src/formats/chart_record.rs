//! タイミングチャートと、取り込んだアニメーションをプロジェクトファイル (.cadsim) に書く形
//!
//! ドメインの型 (`TimingChart`・アニメーションのクリップ) には serde を付けず、ここで写す。
//! 3D の部品はノード ID ではなく木の中の位置 (`[ルートの番号, 子の番号, …]`) で指す。
//! ノード ID は開き直すと変わるため。位置との変換は呼び出し側が渡す。

use glam::{DQuat, DVec3};
use serde::{Deserialize, Serialize};

use crate::interchange::{NodeTransform, Track};
use crate::motion::{Action, Actuator, ActuatorKind, Axis, Easing, Step, TimingChart};

type NodePath = Vec<usize>;

// ---------------------------------------------------------------------------
// タイミングチャート
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChartRecord {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actuators: Vec<ActuatorRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<StepRecord>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActuatorKindRecord {
    Cylinder,
    LinearAxis,
    RotaryTable,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisRecord {
    X,
    Y,
    Z,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EasingRecord {
    Linear,
    Smooth,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ActuatorRecord {
    pub id: u32,
    pub name: String,
    pub kind: ActuatorKindRecord,
    pub axis: AxisRecord,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reversed: bool,
    #[serde(default)]
    pub stroke: f64,
    #[serde(default)]
    pub pivot: [f64; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mounted_on: Option<u32>,
    /// 動かす部品の木の中の位置
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<NodePath>,
    pub easing: EasingRecord,
    pub default_duration: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StepRecord {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ActionRecord>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dwell: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ActionRecord {
    pub actuator: u32,
    pub target: f64,
    pub duration: f64,
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// 保存する形にする。`part_path` はノード ID を木の中の位置にする (部品が消えていれば None)。
pub fn chart_to_record(
    chart: &TimingChart,
    part_path: impl Fn(u32) -> Option<NodePath>,
) -> ChartRecord {
    ChartRecord {
        actuators: chart
            .actuators
            .iter()
            .map(|a| ActuatorRecord {
                id: a.id,
                name: a.name.clone(),
                kind: match a.kind {
                    ActuatorKind::Cylinder => ActuatorKindRecord::Cylinder,
                    ActuatorKind::LinearAxis => ActuatorKindRecord::LinearAxis,
                    ActuatorKind::RotaryTable => ActuatorKindRecord::RotaryTable,
                },
                axis: match a.axis {
                    Axis::X => AxisRecord::X,
                    Axis::Y => AxisRecord::Y,
                    Axis::Z => AxisRecord::Z,
                },
                reversed: a.reversed,
                stroke: a.stroke,
                pivot: a.pivot,
                mounted_on: a.mounted_on,
                parts: a.parts.iter().filter_map(|&id| part_path(id)).collect(),
                easing: match a.easing {
                    Easing::Linear => EasingRecord::Linear,
                    Easing::Smooth => EasingRecord::Smooth,
                },
                default_duration: a.default_duration,
            })
            .collect(),
        steps: chart
            .steps
            .iter()
            .map(|s| StepRecord {
                comment: s.comment.clone(),
                actions: s
                    .actions
                    .iter()
                    .map(|a| ActionRecord {
                        actuator: a.actuator,
                        target: a.target,
                        duration: a.duration,
                    })
                    .collect(),
                dwell: s.dwell,
            })
            .collect(),
    }
}

/// ファイルから読む。`find_part` は木の中の位置をノード ID にする。
/// 戻り値の2つ目は、部品が見つからなかったアクチュエータの注意。
pub fn chart_from_record(
    record: &ChartRecord,
    find_part: impl Fn(&[usize]) -> Option<u32>,
) -> (TimingChart, Vec<String>) {
    let mut warnings = Vec::new();
    let actuators: Vec<Actuator> = record
        .actuators
        .iter()
        .map(|r| {
            let kind = match r.kind {
                ActuatorKindRecord::Cylinder => ActuatorKind::Cylinder,
                ActuatorKindRecord::LinearAxis => ActuatorKind::LinearAxis,
                ActuatorKindRecord::RotaryTable => ActuatorKind::RotaryTable,
            };
            let parts: Vec<u32> = r.parts.iter().filter_map(|p| find_part(p)).collect();
            if parts.len() < r.parts.len() {
                warnings.push(format!(
                    "アクチュエータ「{}」の部品が {} 個見つからないため、割り当てを外しました",
                    r.name,
                    r.parts.len() - parts.len()
                ));
            }
            Actuator {
                name: r.name.clone(),
                axis: match r.axis {
                    AxisRecord::X => Axis::X,
                    AxisRecord::Y => Axis::Y,
                    AxisRecord::Z => Axis::Z,
                },
                reversed: r.reversed,
                stroke: r.stroke,
                pivot: r.pivot,
                mounted_on: r.mounted_on,
                parts,
                easing: match r.easing {
                    EasingRecord::Linear => Easing::Linear,
                    EasingRecord::Smooth => Easing::Smooth,
                },
                default_duration: r.default_duration,
                ..Actuator::new(r.id, kind)
            }
        })
        .collect();
    let ids: Vec<u32> = actuators.iter().map(|a| a.id).collect();
    let mut actuators = actuators;
    // 消えた親に載っている設定は外す (壊れたファイルで循環しないように)
    for actuator in &mut actuators {
        if actuator
            .mounted_on
            .is_some_and(|p| !ids.contains(&p) || p == actuator.id)
        {
            actuator.mounted_on = None;
        }
    }
    let steps = record
        .steps
        .iter()
        .map(|s| Step {
            comment: s.comment.clone(),
            actions: s
                .actions
                .iter()
                .filter(|a| ids.contains(&a.actuator))
                .map(|a| Action {
                    actuator: a.actuator,
                    target: a.target,
                    duration: a.duration,
                })
                .collect(),
            dwell: s.dwell,
        })
        .collect();
    let chart = TimingChart {
        next_actuator_id: ids.iter().map(|id| id + 1).max().unwrap_or(1),
        actuators,
        steps,
    };
    (chart, warnings)
}

// ---------------------------------------------------------------------------
// 取り込んだアニメーション (glTF のノードの動き)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AnimationRecord {
    pub name: String,
    pub tracks: Vec<TrackRecord>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TrackRecord {
    /// 動かすノードの木の中の位置
    pub node: NodePath,
    pub keys: Vec<KeyRecord>,
}

/// 1つのキー。親から見た配置で、位置はモデルの単位
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct KeyRecord {
    /// 時刻 (秒)
    pub time: f64,
    pub position: [f64; 3],
    /// 回転の四元数 [x, y, z, w]
    pub rotation: [f64; 4],
    #[serde(default = "unit_scale")]
    pub scale: [f64; 3],
}

fn unit_scale() -> [f64; 3] {
    [1.0; 3]
}

/// クリップ1つ分の名前とトラック (ノード ID と、そのノードのキー)
pub struct ClipData<'a> {
    pub name: &'a str,
    pub tracks: Vec<(u32, &'a Track)>,
}

/// 保存する形にする。消えたノードのトラックと、空になったクリップは書かない。
pub fn animations_to_records<'a>(
    clips: impl IntoIterator<Item = ClipData<'a>>,
    node_path: impl Fn(u32) -> Option<NodePath>,
) -> Vec<AnimationRecord> {
    clips
        .into_iter()
        .map(|clip| AnimationRecord {
            name: clip.name.to_string(),
            tracks: clip
                .tracks
                .iter()
                .filter_map(|(id, track)| {
                    Some(TrackRecord {
                        node: node_path(*id)?,
                        keys: track
                            .keys
                            .iter()
                            .map(|(time, t)| KeyRecord {
                                time: *time,
                                position: t.translation.to_array(),
                                rotation: t.rotation.to_array(),
                                scale: t.scale.to_array(),
                            })
                            .collect(),
                    })
                })
                .collect(),
        })
        .filter(|a| !a.tracks.is_empty())
        .collect()
}

/// ファイルから読む。クリップごとに (名前, [(ノード ID, トラック)]) を返す。
/// 見つからないノードのトラックは捨て、件数を2つ目に返す。
#[allow(clippy::type_complexity)]
pub fn animations_from_records(
    records: &[AnimationRecord],
    find_node: impl Fn(&[usize]) -> Option<u32>,
) -> (Vec<(String, Vec<(u32, Track)>)>, usize) {
    let mut missing = 0;
    let clips = records
        .iter()
        .map(|r| {
            let tracks = r
                .tracks
                .iter()
                .filter_map(|t| {
                    let Some(id) = find_node(&t.node) else {
                        missing += 1;
                        return None;
                    };
                    let keys = t
                        .keys
                        .iter()
                        .map(|k| {
                            (
                                k.time,
                                NodeTransform {
                                    translation: DVec3::from_array(k.position),
                                    rotation: DQuat::from_array(k.rotation).normalize(),
                                    scale: DVec3::from_array(k.scale),
                                },
                            )
                        })
                        .collect();
                    Some((id, Track { node: 0, keys }))
                })
                .collect();
            (r.name.clone(), tracks)
        })
        .collect();
    (clips, missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_chart() -> TimingChart {
        let mut chart = TimingChart::default();
        let table = chart.add_actuator(ActuatorKind::RotaryTable);
        let cylinder = chart.add_actuator(ActuatorKind::Cylinder);
        {
            let c = chart.actuator_mut(cylinder).unwrap();
            c.mounted_on = Some(table);
            c.parts = vec![10, 11];
            c.reversed = true;
            c.axis = Axis::Z;
            c.easing = Easing::Linear;
        }
        chart.actuator_mut(table).unwrap().pivot = [1.0, 2.0, 3.0];
        chart.steps = vec![
            Step {
                comment: "回す".into(),
                actions: vec![Action {
                    actuator: table,
                    target: 90.0,
                    duration: 0.8,
                }],
                dwell: 0.2,
            },
            Step {
                comment: String::new(),
                actions: vec![Action {
                    actuator: cylinder,
                    target: 1.0,
                    duration: 0.4,
                }],
                dwell: 0.0,
            },
        ];
        chart
    }

    #[test]
    fn chart_round_trip() {
        let chart = sample_chart();
        let record = chart_to_record(&chart, |id| Some(vec![0, id as usize]));
        let json = serde_json::to_string(&record).unwrap();
        let parsed: ChartRecord = serde_json::from_str(&json).unwrap();
        let (restored, warnings) = chart_from_record(&parsed, |p| Some(p[1] as u32));
        assert!(warnings.is_empty());
        assert_eq!(restored, chart);
    }

    #[test]
    fn missing_parts_and_broken_references_are_dropped() {
        let chart = sample_chart();
        let mut record = chart_to_record(&chart, |id| Some(vec![id as usize]));
        // 親のアクチュエータを消したファイル
        record.actuators.remove(0);
        let (restored, warnings) = chart_from_record(&record, |p| (p[0] == 10).then_some(10));
        assert_eq!(warnings.len(), 1);
        let cylinder = &restored.actuators[0];
        assert_eq!(cylinder.parts, vec![10]);
        assert_eq!(cylinder.mounted_on, None);
        // 消えたアクチュエータへの動作も消える
        assert!(restored.steps[0].actions.is_empty());
        assert_eq!(restored.steps[1].actions.len(), 1);
        assert_eq!(restored.next_actuator_id, 3);
    }

    #[test]
    fn animation_round_trip() {
        let track = Track {
            node: 5,
            keys: vec![
                (0.0, NodeTransform::default()),
                (
                    1.5,
                    NodeTransform {
                        translation: DVec3::new(0.0, 2000.0, 0.0),
                        rotation: DQuat::from_rotation_y(0.5),
                        scale: DVec3::ONE,
                    },
                ),
            ],
        };
        let records = animations_to_records(
            [ClipData {
                name: "lift",
                tracks: vec![(7, &track), (8, &track)],
            }],
            |id| (id == 7).then(|| vec![0, 2]),
        );
        assert_eq!(records[0].tracks.len(), 1);
        let json = serde_json::to_string(&records).unwrap();
        let parsed: Vec<AnimationRecord> = serde_json::from_str(&json).unwrap();
        let (clips, missing) = animations_from_records(&parsed, |p| (p == [0, 2]).then_some(7));
        assert_eq!(missing, 0);
        assert_eq!(clips[0].0, "lift");
        let (id, restored) = &clips[0].1[0];
        assert_eq!(*id, 7);
        assert_eq!(restored.keys.len(), 2);
        let key = restored.sample(1.5).unwrap();
        assert!((key.translation.y - 2000.0).abs() < 1e-9);
        assert!(key.rotation.abs_diff_eq(DQuat::from_rotation_y(0.5), 1e-12));
    }
}
