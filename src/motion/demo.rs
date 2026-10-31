//! 動作確認用のサンプル装置: ガントリーのピック & プレース + 回転テーブル
//!
//! ```text
//!  S1 リフタ下降(把持)  S2 リフタ上昇  S3 X軸 → テーブル上  S4 リフタ下降(置く)
//!  S5 リフタ上昇        S6 X軸 → 原点 と テーブル 90° 割出し  S7 テーブル戻り
//! ```
//!
//! センサーとインターロック:
//! - 近接センサー「リフタ上昇端」(X軸キャリッジに取り付け): リフタロッドが上がっていると ON。
//!   X軸を動かすステップ (S3, S6) は、これが ON になるまで始めない。
//! - 光電センサー「治具確認」: テーブルが原点 (0°) にあると、治具が光軸を遮って ON。
//!   ワークを置くステップ (S4) は、これが ON になるまで始めない。

use super::chart::{Action, ActuatorKind, Axis, Interlock, Step, TimingChart};
use super::sensor::SensorKind;
use crate::model::Model;

/// 寸法は mm。下の数値は 100 mm 単位で考えたものを掛けている。
const S: f64 = 100.0;

/// モデルとチャートをサンプル装置に置き換える
pub fn load_demo(model: &mut Model, chart: &mut TimingChart) {
    model.clear();
    model.unit = "mm".into();
    *chart = TimingChart::default();

    let mut add = |name: &str, size: [f64; 3], position: [f64; 3]| {
        let id = model.add_box(size.map(|v| v * S), position.map(|v| v * S));
        if let Some(node) = model.get_mut(id) {
            node.name = name.into();
        }
        id
    };
    add("ガントリー梁", [5.6, 0.2, 0.3], [1.5, 3.3, -0.45]);
    add("コンベア", [1.6, 0.8, 1.2], [-0.5, 0.4, 0.0]);
    add("テーブル架台", [1.0, 0.6, 1.0], [3.5, 0.3, 0.0]);
    let carriage = add("X軸キャリッジ", [0.6, 0.3, 0.6], [-0.5, 3.3, 0.0]);
    let rod = add("リフタロッド", [0.15, 1.55, 0.15], [-0.5, 2.375, 0.0]);
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
        a.stroke = 0.6 * S;
        a.mounted_on = Some(x_axis);
        a.parts = vec![rod, gripper];
    }
    let rotary = chart.add_actuator(ActuatorKind::RotaryTable);
    {
        let a = chart.actuator_mut(rotary).unwrap();
        a.name = "割出しテーブル".into();
        a.axis = Axis::Y;
        a.pivot = [3.5 * S, 0.0, 0.0];
        a.parts = vec![table, fixture];
    }

    let lifter_up = chart.add_sensor(SensorKind::Proximity);
    {
        let s = chart.sensor_mut(lifter_up).unwrap();
        s.name = "リフタ上昇端".into();
        // ロッドの横 (Z+ 側) に置く。ロッドが 0.6 下がると検出距離から外れる
        s.position = [-0.5 * S, 2.9 * S, 0.2 * S];
        s.range = 0.15 * S;
        s.mounted_on = Some(x_axis);
    }
    let fixture_check = chart.add_sensor(SensorKind::Photoelectric);
    {
        let s = chart.sensor_mut(fixture_check).unwrap();
        s.name = "治具確認".into();
        // テーブルの外から治具の高さに X- 向きの光軸を出す
        s.position = [4.6 * S, 1.0 * S, 0.45 * S];
        s.axis = Axis::X;
        s.reversed = true;
        s.range = 1.0 * S;
    }
    let wait_on = |sensor| vec![Interlock { sensor, on: true }];

    let step = |comment: &str, actions: Vec<Action>, dwell: f64| Step {
        comment: comment.into(),
        actions,
        dwell,
        ..Default::default()
    };
    let with_interlocks = |step: Step, interlocks| Step { interlocks, ..step };
    let act = |actuator, target, duration| Action {
        actuator,
        target,
        duration,
    };
    chart.steps = vec![
        step("下降(把持)", vec![act(lifter, 1.0, 0.4)], 0.2),
        step("上昇", vec![act(lifter, 0.0, 0.4)], 0.0),
        with_interlocks(
            step("テーブル上へ移動", vec![act(x_axis, 4.0 * S, 1.0)], 0.0),
            wait_on(lifter_up),
        ),
        with_interlocks(
            step("下降(置く)", vec![act(lifter, 1.0, 0.4)], 0.2),
            wait_on(fixture_check),
        ),
        step("上昇", vec![act(lifter, 0.0, 0.4)], 0.0),
        with_interlocks(
            step(
                "原点復帰 + 90°割出し",
                vec![act(x_axis, 0.0, 1.0), act(rotary, 90.0, 0.8)],
                0.0,
            ),
            wait_on(lifter_up),
        ),
        step("テーブル戻り", vec![act(rotary, 0.0, 0.8)], 0.0),
    ];
}
