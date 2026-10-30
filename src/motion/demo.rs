//! 動作確認用のサンプル装置: ガントリーのピック & プレース + 回転テーブル
//!
//! ```text
//!  S1 リフタ下降(把持)  S2 リフタ上昇  S3 X軸 → テーブル上  S4 リフタ下降(置く)
//!  S5 リフタ上昇        S6 X軸 → 原点 と テーブル 90° 割出し  S7 テーブル戻り
//! ```

use bevy::math::Vec3;

use super::chart::{Action, ActuatorKind, Axis, Step, TimingChart};
use crate::{Model, OrbitCamera};

/// モデルとチャートをサンプル装置に置き換え、カメラを装置全体が見える位置にする
pub fn load_demo(model: &mut Model, chart: &mut TimingChart, orbit: &mut OrbitCamera) {
    // 下のタイミングチャートに隠れないよう、注視点を低めにして少し上から見る
    orbit.focus = Vec3::new(1.5, 0.0, 0.0);
    orbit.yaw = 0.6;
    orbit.pitch = 0.45;
    orbit.distance = 10.0;

    for part in model.parts.drain(..) {
        model.dirty.insert(part.id);
    }
    *chart = TimingChart::default();

    let mut add = |name: &str, size: [f64; 3], position: [f64; 3]| {
        let id = model.add_box(size, position);
        if let Some(part) = model.part_mut(id) {
            part.name = name.into();
        }
        id
    };
    add("ガントリー梁", [5.6, 0.2, 0.3], [1.5, 3.3, -0.45]);
    add("コンベア", [1.6, 0.8, 1.2], [-0.5, 0.4, 0.0]);
    add("テーブル架台", [1.0, 0.6, 1.0], [3.5, 0.3, 0.0]);
    let carriage = add("X軸キャリッジ", [0.6, 0.3, 0.6], [-0.5, 3.3, 0.0]);
    let rod = add("リフタロッド", [0.15, 1.6, 0.15], [-0.5, 2.4, 0.0]);
    let gripper = add("グリッパ", [0.5, 0.2, 0.5], [-0.5, 1.5, 0.0]);
    let table = add("回転テーブル", [1.4, 0.2, 1.4], [3.5, 0.7, 0.0]);
    let fixture = add("治具", [0.6, 0.3, 0.25], [3.5, 0.95, 0.45]);

    let x_axis = chart.add_actuator(ActuatorKind::LinearAxis);
    {
        let a = chart.actuator_mut(x_axis).unwrap();
        a.name = "X軸".into();
        a.axis = Axis::X;
        a.parts = vec![carriage];
    }
    let lifter = chart.add_actuator(ActuatorKind::Cylinder);
    {
        let a = chart.actuator_mut(lifter).unwrap();
        a.name = "リフタ".into();
        a.axis = Axis::Y;
        a.reversed = true;
        a.stroke = 0.6;
        a.mounted_on = Some(x_axis);
        a.parts = vec![rod, gripper];
    }
    let rotary = chart.add_actuator(ActuatorKind::RotaryTable);
    {
        let a = chart.actuator_mut(rotary).unwrap();
        a.name = "割出しテーブル".into();
        a.axis = Axis::Y;
        a.pivot = [3.5, 0.0, 0.0];
        a.parts = vec![table, fixture];
    }

    let step = |comment: &str, actions: Vec<Action>, dwell: f64| Step {
        comment: comment.into(),
        actions,
        dwell,
    };
    let act = |actuator, target, duration| Action {
        actuator,
        target,
        duration,
    };
    chart.steps = vec![
        step("下降(把持)", vec![act(lifter, 1.0, 0.4)], 0.2),
        step("上昇", vec![act(lifter, 0.0, 0.4)], 0.0),
        step("テーブル上へ移動", vec![act(x_axis, 4.0, 1.0)], 0.0),
        step("下降(置く)", vec![act(lifter, 1.0, 0.4)], 0.2),
        step("上昇", vec![act(lifter, 0.0, 0.4)], 0.0),
        step(
            "原点復帰 + 90°割出し",
            vec![act(x_axis, 0.0, 1.0), act(rotary, 90.0, 0.8)],
            0.0,
        ),
        step("テーブル戻り", vec![act(rotary, 0.0, 0.8)], 0.0),
    ];
}
