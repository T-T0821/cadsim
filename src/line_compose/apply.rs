//! 下書きをモデル・タイミングチャート・ワーク搬送の設定に置く
//!
//! Bevy の ECS や egui には依存しない (`Model` と `TimingChart` を直接書き換える)。
//! 設備はどれも箱で作り、1つのアセンブリ「ライン下書き: …」にまとめる。
//!
//! - コンベア: 本体の箱。ワーク搬送のコンベアとしても登録する (+X に流れる)。
//! - ロボット: 土台・腕・ハンドの箱。腕とハンドを回す回転テーブル「旋回」と、
//!   ハンドを上下させるシリンダ「上下」をチャートに足す。取る・置くは
//!   「旋回 → 下降 → 上昇」の3ステップに展開する。最後に旋回したままなら「原点へ戻る」を足す。
//! - シリンダ: 本体と押し板の箱。押し板を動かすシリンダをチャートに足す。
//! - センサー: 柱の箱と、コンベアを横切る光電センサー。ワークがなくても再生が止まらないよう、
//!   強制 ON にしておく。
//! - 作業台: 箱。

use std::collections::HashMap;

use bevy::math::DMat4;
use cadsim_core::conveyor::ConveyorConfig;
use cadsim_core::line_draft::layout::{Placed, work_point};
use cadsim_core::line_draft::{EquipmentKind, Verb};
use cadsim_core::usecase::compose_line::ComposedLine;

use crate::model::{BoxPart, Model, NodeId, NodeKind};
use crate::motion::TimingChart;
use crate::motion::chart::{Action, ActuatorKind, Axis, Interlock, Sensor, SensorKind, Step};

/// ロボットの腕の高さ (下面)
const ARM_HEIGHT: f64 = 1000.0;
const ARM_SECTION: [f64; 2] = [120.0, 140.0];
const HAND_SIZE: [f64; 3] = [90.0, 150.0, 90.0];
/// 動く部品と、その土台・本体とのすき間 (干渉チェックで「接触」と出ないように少しあける)
const GAP: f64 = 2.0;
/// 原点へ戻すステップの時間 (秒)
const HOME_SECONDS: f64 = 0.8;
/// ロボットの取る・置くの時間の配分 (旋回・下降・上昇)
const PICK_PHASES: [f64; 3] = [0.4, 0.3, 0.3];

const COLOR_CONVEYOR: [f32; 3] = [0.35, 0.40, 0.48];
const COLOR_ROBOT: [f32; 3] = [0.95, 0.55, 0.15];
const COLOR_CYLINDER: [f32; 3] = [0.30, 0.55, 0.85];
const COLOR_SENSOR: [f32; 3] = [0.90, 0.80, 0.20];
const COLOR_TABLE: [f32; 3] = [0.62, 0.52, 0.40];

/// 置いた結果
#[derive(Debug, Default)]
pub struct Applied {
    /// まとめたアセンブリ
    pub root: NodeId,
    pub actuators: usize,
    pub sensors: usize,
    pub steps: usize,
}

/// 設備ごとにチャートへ足したもの
#[derive(Clone, Copy, Default)]
struct Rig {
    /// ロボットの旋回・上下 (アクチュエータの id)
    turn: Option<u32>,
    lift: Option<u32>,
    /// ロボットの旋回の中心 (モデル座標)
    pivot: [f64; 3],
    /// シリンダ
    cylinder: Option<u32>,
    /// センサー
    sensor: Option<u32>,
}

/// 下書きを置く。`offset` は床の上で全体をずらす量 [x, z]。
/// チャートのステップは今あるステップの後ろに足す。
pub fn apply(
    model: &mut Model,
    chart: &mut TimingChart,
    conveyors: &mut ConveyorConfig,
    line: &ComposedLine,
    offset: [f64; 2],
) -> Applied {
    let draft = &line.draft;
    let root = model.add_node(
        None,
        format!("ライン下書き: {}", draft.title),
        DMat4::IDENTITY,
        NodeKind::Assembly { product: None },
    );
    let world = |p: &Placed| [p.center[0] + offset[0], p.center[1] + offset[1]];
    let mut applied = Applied {
        root,
        ..Default::default()
    };

    // ロボットの腕の長さは、取る・置く場所まで届く長さにする
    let mut reach: HashMap<usize, f64> = HashMap::new();
    for action in draft.steps.iter().flat_map(|s| &s.actions) {
        let (Some(r), Some(t)) = (
            draft.index_of(&action.equipment),
            action.target.as_deref().and_then(|t| draft.index_of(t)),
        ) else {
            continue;
        };
        let point = work_point(&line.placed[t], draft.equipment[t].kind, action.verb);
        let center = line.placed[r].center;
        let d = ((point[0] - center[0]).powi(2) + (point[2] - center[1]).powi(2)).sqrt();
        let e = reach.entry(r).or_insert(0.0);
        *e = e.max(d);
    }

    let mut rigs = vec![Rig::default(); draft.equipment.len()];
    for (i, (e, p)) in draft.equipment.iter().zip(&line.placed).enumerate() {
        let [x, z] = world(p);
        let add_box = |model: &mut Model, name: String, size: [f64; 3], center: [f64; 3], color| {
            let id = model.add_node(
                Some(root),
                name,
                DMat4::IDENTITY,
                NodeKind::Box(BoxPart {
                    size,
                    position: center,
                    rotation_deg: [0.0; 3],
                }),
            );
            if let Some(node) = model.get_mut(id) {
                node.color = Some(color);
            }
            id
        };
        match e.kind {
            EquipmentKind::Conveyor => {
                let id = add_box(
                    model,
                    e.name.clone(),
                    p.size,
                    [x, p.size[1] / 2.0, z],
                    COLOR_CONVEYOR,
                );
                let c = conveyors.add_conveyor();
                c.name = e.name.clone();
                c.part = Some(id);
            }
            EquipmentKind::Table => {
                add_box(
                    model,
                    e.name.clone(),
                    p.size,
                    [x, p.size[1] / 2.0, z],
                    COLOR_TABLE,
                );
            }
            EquipmentKind::Robot => {
                let base = p.size;
                add_box(
                    model,
                    format!("{} 土台", e.name),
                    base,
                    [x, base[1] / 2.0, z],
                    COLOR_ROBOT,
                );
                let length = reach.get(&i).copied().unwrap_or(700.0).clamp(500.0, 1800.0);
                let arm = add_box(
                    model,
                    format!("{} 腕", e.name),
                    [length + HAND_SIZE[0] / 2.0, ARM_SECTION[0], ARM_SECTION[1]],
                    [
                        x + (length + HAND_SIZE[0] / 2.0) / 2.0,
                        ARM_HEIGHT + GAP + ARM_SECTION[0] / 2.0,
                        z,
                    ],
                    COLOR_ROBOT,
                );
                let hand = add_box(
                    model,
                    format!("{} ハンド", e.name),
                    HAND_SIZE,
                    [x + length, ARM_HEIGHT - HAND_SIZE[1] / 2.0, z],
                    [0.25, 0.25, 0.28],
                );
                let pivot = [x, 0.0, z];
                let turn = chart.add_actuator(ActuatorKind::RotaryTable);
                if let Some(a) = chart.actuator_mut(turn) {
                    a.name = format!("{} 旋回", e.name);
                    a.axis = Axis::Y;
                    a.pivot = pivot;
                    a.parts = vec![arm, hand];
                }
                // ハンドの下面を搬送面 (作業台の天板) の少し上まで下ろす
                let hand_bottom = ARM_HEIGHT - HAND_SIZE[1];
                let lift = chart.add_actuator(ActuatorKind::Cylinder);
                if let Some(a) = chart.actuator_mut(lift) {
                    a.name = format!("{} 上下", e.name);
                    a.axis = Axis::Y;
                    a.reversed = true;
                    a.stroke = (hand_bottom - (line_top(line) + 10.0)).max(20.0);
                    a.mounted_on = Some(turn);
                    a.parts = vec![hand];
                }
                applied.actuators += 2;
                rigs[i].turn = Some(turn);
                rigs[i].lift = Some(lift);
                rigs[i].pivot = pivot;
            }
            EquipmentKind::Cylinder => {
                let y = cylinder_height(line, p);
                add_box(model, e.name.clone(), p.size, [x, y, z], COLOR_CYLINDER);
                // 押し板は本体のコンベア側の面に付ける
                let toward = if p.on_far_side { -1.0 } else { 1.0 };
                let plate_size = [100.0, 80.0, 30.0];
                let plate = add_box(
                    model,
                    format!("{} 押し板", e.name),
                    plate_size,
                    [
                        x,
                        y,
                        z + toward * (p.size[2] / 2.0 + GAP + plate_size[2] / 2.0),
                    ],
                    [0.75, 0.78, 0.82],
                );
                let id = chart.add_actuator(ActuatorKind::Cylinder);
                if let Some(a) = chart.actuator_mut(id) {
                    a.name = e.name.clone();
                    a.axis = Axis::Z;
                    a.reversed = p.on_far_side;
                    a.stroke = 150.0;
                    a.parts = vec![plate];
                }
                applied.actuators += 1;
                rigs[i].cylinder = Some(id);
            }
            EquipmentKind::Sensor => {
                add_box(
                    model,
                    e.name.clone(),
                    p.size,
                    [x, p.size[1] / 2.0, z],
                    COLOR_SENSOR,
                );
                let range = p
                    .attached_to
                    .map_or(300.0, |c| line.placed[c].size[2] + 120.0);
                let id = chart.add_sensor(SensorKind::Photoelectric);
                if let Some(s) = chart.sensor_mut(id) {
                    *s = Sensor {
                        name: e.name.clone(),
                        position: [x, p.size[1] - 50.0, z],
                        axis: Axis::Z,
                        reversed: p.on_far_side,
                        range,
                        force: Some(true),
                        ..s.clone()
                    };
                }
                applied.sensors += 1;
                rigs[i].sensor = Some(id);
            }
        }
    }

    // ステップ
    for step in &draft.steps {
        let robot_actions: Vec<_> = step
            .actions
            .iter()
            .filter(|a| a.verb.needs_target())
            .collect();
        let phases = if robot_actions.is_empty() { 1 } else { 3 };
        let mut new_steps: Vec<Step> = (0..phases)
            .map(|k| Step {
                comment: if phases == 1 {
                    step.comment.clone()
                } else {
                    format!("{}: {}", step.comment, ["旋回", "下降", "上昇"][k])
                },
                ..Default::default()
            })
            .collect();
        // 開始条件は最初の1つに付ける
        if let Some((sensor, on)) = &step.wait_for
            && let Some(id) = draft.index_of(sensor).and_then(|i| rigs[i].sensor)
        {
            new_steps[0].interlocks.push(Interlock {
                sensor: id,
                on: *on,
            });
        }
        // コンベア・シリンダは最初の1つで動かす
        let mut conveyor_time: f64 = 0.0;
        for a in step.actions.iter().filter(|a| !a.verb.needs_target()) {
            let Some(i) = draft.index_of(&a.equipment) else {
                continue;
            };
            match a.verb {
                Verb::Run => conveyor_time = conveyor_time.max(a.seconds),
                Verb::Extend | Verb::Retract => {
                    if let Some(id) = rigs[i].cylinder {
                        new_steps[0].actions.push(Action {
                            actuator: id,
                            target: if a.verb == Verb::Extend { 1.0 } else { 0.0 },
                            duration: a.seconds,
                        });
                    }
                }
                Verb::Pick | Verb::Place => {}
            }
        }
        // ロボットは旋回 → 下降 → 上昇
        for a in robot_actions {
            let (Some(r), Some(t)) = (
                draft.index_of(&a.equipment),
                a.target.as_deref().and_then(|t| draft.index_of(t)),
            ) else {
                continue;
            };
            let rig = rigs[r];
            let (Some(turn), Some(lift)) = (rig.turn, rig.lift) else {
                continue;
            };
            let mut point = work_point(&line.placed[t], draft.equipment[t].kind, a.verb);
            point[0] += offset[0];
            point[2] += offset[1];
            let angle = (-(point[2] - rig.pivot[2]))
                .atan2(point[0] - rig.pivot[0])
                .to_degrees();
            let targets = [(turn, angle), (lift, 1.0), (lift, 0.0)];
            for (k, (actuator, target)) in targets.into_iter().enumerate() {
                new_steps[k].actions.push(Action {
                    actuator,
                    target,
                    duration: a.seconds * PICK_PHASES[k],
                });
            }
        }
        // コンベアの搬送時間は、動作がそれより短ければ待ち時間で足す
        let first = &mut new_steps[0];
        let motion = first.duration();
        if conveyor_time > motion {
            first.dwell = conveyor_time - motion;
        }
        if let Some(last) = new_steps.last_mut() {
            last.dwell += step.dwell;
        }
        applied.steps += new_steps.len();
        chart.steps.extend(new_steps);
    }

    // 1サイクルの終わりに原点へ戻っていない (旋回したまま・出たまま) 設備を戻す
    let ours: Vec<u32> = rigs
        .iter()
        .flat_map(|r| [r.turn, r.lift, r.cylinder])
        .flatten()
        .collect();
    let home: Vec<Action> = ours
        .iter()
        .filter_map(|&id| {
            let last = chart
                .steps
                .iter()
                .rev()
                .find_map(|s| s.action(id))
                .map_or(0.0, |a| a.target);
            (last.abs() > 1e-9).then_some(Action {
                actuator: id,
                target: 0.0,
                duration: HOME_SECONDS,
            })
        })
        .collect();
    if !home.is_empty() {
        chart.steps.push(Step {
            comment: "原点へ戻る".into(),
            actions: home,
            ..Default::default()
        });
        applied.steps += 1;
    }

    applied
}

/// 取る・置く場所の天板の高さ (いちばん高いもの)
fn line_top(line: &ComposedLine) -> f64 {
    line.draft
        .equipment
        .iter()
        .zip(&line.placed)
        .filter(|(e, _)| matches!(e.kind, EquipmentKind::Conveyor | EquipmentKind::Table))
        .map(|(_, p)| p.top())
        .fold(0.0, f64::max)
}

/// シリンダの本体の中心の高さ: 付けたコンベアの搬送面の少し上
fn cylinder_height(line: &ComposedLine, p: &Placed) -> f64 {
    let base = p.attached_to.map_or(0.0, |c| line.placed[c].top());
    base + p.size[1] / 2.0 + 20.0
}

/// 既存のものと重ならないよう、全体の右 (+X) に置くためのずらし量
pub fn free_offset(model: &Model, line: &ComposedLine) -> [f64; 2] {
    let Some(bounds) = model.scene_bounds() else {
        return [0.0, 0.0];
    };
    let min_x = line
        .placed
        .iter()
        .map(|p| p.center[0] - p.size[0] / 2.0)
        .fold(f64::INFINITY, f64::min);
    let min_x = if min_x.is_finite() { min_x } else { 0.0 };
    [bounds.max.x + 1000.0 - min_x, 0.0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadsim_core::usecase::compose_line::compose_line;

    fn composed(text: &str) -> ComposedLine {
        compose_line(text, None).unwrap()
    }

    #[test]
    fn packing_line_builds_parts_actuators_and_steps() {
        let line = composed("コンベア2本とロボット1台、センサー1個、ストッパ1個で箱詰め");
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        let mut conveyors = ConveyorConfig::default();
        let applied = apply(&mut model, &mut chart, &mut conveyors, &line, [0.0, 0.0]);

        let root = model.get(applied.root).unwrap();
        assert!(root.name.starts_with("ライン下書き"));
        // コンベア2 + ロボット3 + ストッパ2 + センサー1
        assert_eq!(root.children.len(), 8);
        assert_eq!(conveyors.conveyors.len(), 2);
        // 旋回・上下・ストッパ
        assert_eq!(applied.actuators, 3);
        assert_eq!(chart.actuators.len(), 3);
        assert_eq!(chart.sensors.len(), 1);
        assert_eq!(chart.sensors[0].force, Some(true));
        // 送る / 位置決め / 取る×3 / 置く×3 / 送り出す / 原点へ戻る (旋回)
        assert_eq!(chart.steps.len(), 10);
        assert_eq!(chart.steps[9].actions.len(), 1);
        assert_eq!(chart.steps[9].actions[0].target, 0.0);
        assert_eq!(chart.steps[1].interlocks.len(), 1);
        assert!(chart.steps[2].comment.ends_with("旋回"));

        // 製品コンベアは下流の端 (+X, z = 0)、箱コンベアは +Z 側にある。
        // ロボットは間 (z = 800) にいるので、取るときは -Z 側 (+角度)、置くときは +Z 側 (-角度) を向く
        let turn = chart.actuators[0].id;
        let pick_angle = chart.steps[2].action(turn).unwrap().target;
        let place_angle = chart.steps[5].action(turn).unwrap().target;
        assert!(pick_angle > 0.0, "{pick_angle}");
        assert!(place_angle < 0.0, "{place_angle}");

        // スケジュールにできて、インターロックで止まらない (センサーは強制 ON)
        let schedule = chart.schedule();
        assert!(schedule.stop.is_none());
    }

    #[test]
    fn conveyor_run_becomes_a_timed_step() {
        let line = composed("コンベア1本で搬送");
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        let mut conveyors = ConveyorConfig::default();
        apply(&mut model, &mut chart, &mut conveyors, &line, [500.0, 0.0]);
        assert_eq!(chart.steps.len(), 1);
        assert_eq!(chart.steps[0].duration(), 3.0);
        // ずらし量が効いている
        let bounds = model.scene_bounds().unwrap();
        assert!((bounds.center().x - 500.0).abs() < 1e-6);
    }

    #[test]
    fn steps_are_appended_after_existing_ones() {
        let line = composed("コンベア1本で搬送");
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        chart.steps.push(Step::default());
        let mut conveyors = ConveyorConfig::default();
        apply(&mut model, &mut chart, &mut conveyors, &line, [0.0, 0.0]);
        assert_eq!(chart.steps.len(), 2);
    }
}
