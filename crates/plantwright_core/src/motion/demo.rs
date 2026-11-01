//! 動作確認用のサンプル装置
//!
//! ## 供給コンベア → ガントリーのピック & プレース → 排出コンベア ([`load_demo`])
//!
//! ```text
//!  S1 ワーク到着待ち  S2 リフタ下降  S3 チャック閉(把持)  S4 リフタ上昇
//!  S5 X軸 → 排出コンベア上  S6 リフタ下降  S7 チャック開(解放)  S8 リフタ上昇  S9 X軸 原点復帰
//! ```
//!
//! 供給コンベアはストッパ付きで、流れてきたワークは終端 (ガントリーの真下) で詰まって並ぶ。
//! 排出コンベアに置いたワークは Z 方向へ流れて消える。
//!
//! センサーとインターロック:
//! - 近接センサー「リフタ上昇端」(X軸キャリッジに取り付け): リフタロッドが上がっていると ON。
//!   X軸を動かすステップ (S5, S9) は、これが ON になるまで始めない。
//! - 近接センサー「チャック閉確認」(リフタに取り付け): 可動爪が閉じていると ON。
//!   掴んで上げるステップ (S4) は ON、離して上げるステップ (S8) は OFF になるまで始めない。
//!
//! ## 4軸ロボット ([`load_robot_demo`])
//!
//! 旋回 (J1) + 肩・肘・手首 (J2〜J4) のアームと、手首に載ったグリッパのシリンダ。
//! コンベア上の教示点から置き台の上の教示点へ、工具を下向きにしたまま運ぶ。

use super::sensor::SensorKind;
use crate::conveyor::{BeltAxis, ConveyorConfig, ConveyorEnd, SourceOutput};
use crate::motion::chart::Action;
use crate::motion::chart::ActuatorKind;
use crate::motion::chart::Axis;
use crate::motion::chart::Interlock;
use crate::motion::chart::MoveKind;
use crate::motion::chart::RobotMove;
use crate::motion::chart::Step;
use crate::motion::chart::TimingChart;

use crate::model::Model;

/// 寸法は mm。下の数値は 100 mm 単位で考えたものを `add` などで掛けている。
const S: f64 = 100.0;

/// ガントリーの原点 (ワークを掴む位置) の X 座標
const PICK_X: f64 = -0.5;
/// 排出コンベアまでの X 軸の移動量
const PLACE_TRAVEL: f64 = 4.0;
/// ワークの寸法
const WORK: [f64; 3] = [0.3, 0.3, 0.3];
/// コンベアのベルト上面の高さ
const BELT_TOP: f64 = 0.8;
/// リフタのストローク
const LIFT: f64 = 0.6;

/// モデル・チャート・搬送設定をサンプル装置に置き換える
pub fn load_demo(model: &mut Model, chart: &mut TimingChart, conveyors: &mut ConveyorConfig) {
    model.clear();
    model.unit = "mm".into();
    *chart = TimingChart::default();
    *conveyors = ConveyorConfig::default();

    let mut add = |name: &str, size: [f64; 3], position: [f64; 3]| {
        let id = model.add_box(size.map(|v| v * S), position.map(|v| v * S));
        if let Some(node) = model.get_mut(id) {
            node.name = name.into();
        }
        id
    };
    // 下げたときにチャックの下面がワークの上面に来る高さ (設計位置はそこから LIFT 上)
    let plate_bottom = BELT_TOP + WORK[1] + LIFT;
    let finger_gap = WORK[0] / 2.0 + 0.03;

    add("ガントリー梁", [6.2, 0.2, 0.3], [1.5, 3.3, -0.45]);
    let infeed = add(
        "供給コンベア",
        [3.0, BELT_TOP, 0.8],
        [PICK_X - 1.35, BELT_TOP / 2.0, 0.0],
    );
    let outfeed = add(
        "排出コンベア",
        [0.8, BELT_TOP, 3.0],
        [PICK_X + PLACE_TRAVEL, BELT_TOP / 2.0, 1.0],
    );
    // 投入口 (供給コンベアの始端の手前) と排出口 (排出コンベアの終端の先の受け箱)
    let feeder = add("投入シュート", [0.4, 1.4, 0.8], [PICK_X - 3.1, 0.7, 0.0]);
    let bin = add(
        "排出箱",
        [0.8, BELT_TOP, 0.4],
        [PICK_X + PLACE_TRAVEL, BELT_TOP / 2.0, 2.7],
    );
    let carriage = add("X軸キャリッジ", [0.6, 0.3, 0.6], [PICK_X, 3.3, 0.0]);
    let rod_top = 3.15;
    let rod_bottom = plate_bottom + 0.1;
    let rod = add(
        "リフタロッド",
        [0.15, rod_top - rod_bottom, 0.15],
        [PICK_X, (rod_top + rod_bottom) / 2.0, 0.0],
    );
    let plate = add(
        "チャック本体",
        [0.5, 0.1, 0.4],
        [PICK_X, plate_bottom + 0.05, 0.0],
    );
    let fixed_finger = add(
        "固定爪",
        [0.06, 0.25, 0.3],
        [PICK_X - finger_gap, plate_bottom - 0.125, 0.0],
    );
    // 可動爪は開いた位置で置き、チャックが閉じると -X に 0.1 動いてワークを挟む
    let moving_finger = add(
        "可動爪",
        [0.06, 0.25, 0.3],
        [PICK_X + finger_gap + 0.1, plate_bottom - 0.125, 0.0],
    );

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
        a.stroke = LIFT * S;
        a.mounted_on = Some(x_axis);
        a.parts = vec![rod, plate, fixed_finger];
    }
    let chuck = chart.add_actuator(ActuatorKind::Cylinder);
    {
        let a = chart.actuator_mut(chuck).unwrap();
        a.name = "チャック".into();
        a.axis = Axis::X;
        a.reversed = true;
        a.stroke = 0.1 * S;
        a.mounted_on = Some(lifter);
        a.parts = vec![moving_finger];
        a.default_duration = 0.2;
    }

    let lifter_up = chart.add_sensor(SensorKind::Proximity);
    {
        let s = chart.sensor_mut(lifter_up).unwrap();
        s.name = "リフタ上昇端".into();
        // ロッドの横 (Z+ 側) に置く。ロッドが下がると上端が検出距離から外れる
        s.position = [PICK_X * S, 2.9 * S, 0.2 * S];
        s.range = 0.15 * S;
        s.mounted_on = Some(x_axis);
    }
    let chuck_closed = chart.add_sensor(SensorKind::Proximity);
    {
        let s = chart.sensor_mut(chuck_closed).unwrap();
        s.name = "チャック閉確認".into();
        // 閉じた可動爪のすぐ外側 (X- 寄り・Z+ 側) に置く
        s.position = [
            (PICK_X + finger_gap - 0.04) * S,
            (plate_bottom - 0.125) * S,
            0.2 * S,
        ];
        s.range = 0.08 * S;
        s.mounted_on = Some(lifter);
    }
    let wait_for = |sensor, on| vec![Interlock { sensor, on }];

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
        // センサーで待ち合わせるまでの仮: 1個目のワークがストッパに届くまでの時間だけ待つ
        step("ワーク到着待ち", vec![], 2.0),
        step("下降", vec![act(lifter, 1.0, 0.4)], 0.0),
        step("チャック閉(把持)", vec![act(chuck, 1.0, 0.2)], 0.1),
        with_interlocks(
            step("上昇", vec![act(lifter, 0.0, 0.4)], 0.0),
            wait_for(chuck_closed, true),
        ),
        with_interlocks(
            step(
                "排出コンベア上へ移動",
                vec![act(x_axis, PLACE_TRAVEL * S, 1.0)],
                0.0,
            ),
            wait_for(lifter_up, true),
        ),
        step("下降", vec![act(lifter, 1.0, 0.4)], 0.0),
        step("チャック開(解放)", vec![act(chuck, 0.0, 0.2)], 0.1),
        with_interlocks(
            step("上昇", vec![act(lifter, 0.0, 0.4)], 0.0),
            wait_for(chuck_closed, false),
        ),
        with_interlocks(
            step("原点復帰", vec![act(x_axis, 0.0, 1.0)], 0.0),
            wait_for(lifter_up, true),
        ),
    ];

    // ワークは2種類を交互に流す
    let tall = {
        let k = conveyors.add_kind();
        k.name = "ワーク A".into();
        k.size = WORK.map(|v| v * S);
        k.id
    };
    let flat = {
        let k = conveyors.add_kind();
        k.name = "ワーク B".into();
        k.size = [WORK[0], WORK[1] * 0.6, WORK[2]].map(|v| v * S);
        k.id
    };
    let infeed_id = {
        let c = conveyors.add_conveyor();
        c.name = "供給コンベア".into();
        c.part = Some(infeed);
        c.axis = BeltAxis::X;
        c.speed = 1.2 * S;
        c.end = ConveyorEnd::Stopper;
        c.id
    };
    {
        let c = conveyors.add_conveyor();
        c.name = "排出コンベア".into();
        c.part = Some(outfeed);
        c.axis = BeltAxis::Z;
        c.speed = 0.8 * S;
        c.end = ConveyorEnd::Discharge;
    }
    {
        let source = conveyors.add_source();
        source.name = "投入シュート".into();
        source.part = Some(feeder);
        source.output = SourceOutput::Conveyor(infeed_id);
        source.interval = 5.0;
        source.kinds = vec![tall, flat];
    }
    {
        let sink = conveyors.add_sink();
        sink.name = "排出箱".into();
        sink.part = Some(bin);
    }
    {
        let g = conveyors.add_gripper();
        g.name = "ガントリーチャック".into();
        g.chuck = Some(chuck);
        g.tcp = [PICK_X, BELT_TOP + WORK[1] / 2.0 + LIFT, 0.0].map(|v| v * S);
        g.reach = 0.2 * S;
    }
}

/// モデルとチャートを4軸ロボットのサンプルに置き換える
pub fn load_robot_demo(model: &mut Model, chart: &mut TimingChart) {
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
    // 設計位置 (全関節 0) ではアームはまっすぐ上を向いている
    add("架台", [0.8, 0.6, 0.8], [0.0, 0.3, 0.0]);
    add("コンベア", [1.4, 0.6, 1.0], [2.0, 0.3, 0.0]);
    add("置き台", [1.0, 0.5, 1.0], [0.0, 0.25, -1.9]);
    let turret = add("旋回ベース", [0.7, 0.3, 0.7], [0.0, 0.75, 0.0]);
    let shoulder = add("肩", [0.4, 0.4, 0.5], [0.0, 1.1, 0.0]);
    let upper_arm = add("上腕", [0.22, 0.9, 0.22], [0.0, 1.75, 0.0]);
    let forearm = add("前腕", [0.18, 0.9, 0.18], [0.0, 2.8, 0.0]);
    let wrist = add("手首", [0.24, 0.3, 0.24], [0.0, 3.45, 0.0]);
    let hand = add("ハンド", [0.4, 0.08, 0.2], [0.0, 3.64, 0.0]);
    let fixed_finger = add("固定爪", [0.04, 0.2, 0.16], [-0.18, 3.78, 0.0]);
    let moving_finger = add("可動爪", [0.04, 0.2, 0.16], [0.18, 3.78, 0.0]);

    let mut joint = |name: &str, axis: Axis, pivot: [f64; 3], parent, parts, limit: f64| {
        let id = chart.add_actuator(ActuatorKind::RotaryTable);
        let a = chart.actuator_mut(id).unwrap();
        a.name = name.into();
        a.axis = axis;
        a.pivot = pivot.map(|v| v * S);
        a.mounted_on = parent;
        a.parts = parts;
        a.limits = Some([-limit, limit]);
        id
    };
    let j1 = joint(
        "J1 旋回",
        Axis::Y,
        [0.0; 3],
        None,
        vec![turret, shoulder],
        170.0,
    );
    let j2 = joint(
        "J2 肩",
        Axis::Z,
        [0.0, 1.1, 0.0],
        Some(j1),
        vec![upper_arm],
        120.0,
    );
    let j3 = joint(
        "J3 肘",
        Axis::Z,
        [0.0, 2.3, 0.0],
        Some(j2),
        vec![forearm],
        150.0,
    );
    let j4 = joint(
        "J4 手首",
        Axis::Z,
        [0.0, 3.3, 0.0],
        Some(j3),
        vec![wrist, hand, fixed_finger],
        135.0,
    );
    let gripper = chart.add_actuator(ActuatorKind::Cylinder);
    {
        let a = chart.actuator_mut(gripper).unwrap();
        a.name = "グリッパ".into();
        a.axis = Axis::X;
        a.reversed = true;
        a.stroke = 0.2 * S;
        a.mounted_on = Some(j4);
        a.parts = vec![moving_finger];
        a.default_duration = 0.3;
    }

    let robot = chart.add_robot(j4);
    let down = Some([0.0, -1.0, 0.0]);
    let [pick_above, pick, place_above, place] = {
        let r = chart.robot_mut(robot).unwrap();
        r.name = "4軸アーム".into();
        r.tcp = [0.0, 3.85 * S, 0.0];
        let mut point = |name: &str, p: [f64; 3]| r.add_point(name.into(), p.map(|v| v * S), down);
        [
            point("ピック上", [1.9, 1.5, 0.0]),
            point("ピック", [1.9, 0.85, 0.0]),
            point("プレース上", [0.0, 1.4, -1.9]),
            point("プレース", [0.0, 0.75, -1.9]),
        ]
    };

    // 大きく動くところは関節補間、上下の出し入れは直線補間
    let to = |point, duration| RobotMove {
        robot,
        point,
        duration,
        kind: MoveKind::Joint,
    };
    let straight = |point, duration| RobotMove {
        robot,
        point,
        duration,
        kind: MoveKind::Linear,
    };
    let step = |comment: &str, moves: Vec<RobotMove>, actions: Vec<Action>, dwell: f64| Step {
        comment: comment.into(),
        actions,
        dwell,
        moves,
        ..Default::default()
    };
    let act = |actuator, target, duration| Action {
        actuator,
        target,
        duration,
    };
    let home: Vec<Action> = [j1, j2, j3, j4]
        .into_iter()
        .map(|j| act(j, 0.0, 1.4))
        .collect();
    chart.steps = vec![
        step("ピック上へ", vec![to(pick_above, 1.4)], vec![], 0.0),
        step("下降", vec![straight(pick, 0.6)], vec![], 0.0),
        step("把持", vec![], vec![act(gripper, 1.0, 0.3)], 0.2),
        step("上昇", vec![straight(pick_above, 0.6)], vec![], 0.0),
        step("プレース上へ", vec![to(place_above, 1.4)], vec![], 0.0),
        step("下降", vec![straight(place, 0.6)], vec![], 0.0),
        step("解放", vec![], vec![act(gripper, 0.0, 0.3)], 0.2),
        step("上昇", vec![straight(place_above, 0.6)], vec![], 0.0),
        step("原点復帰", vec![], home, 0.0),
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robot_demo_reaches_every_point_and_returns_home() {
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        load_robot_demo(&mut model, &mut chart);
        let schedule = chart.schedule();
        assert_eq!(schedule.moves.len(), 6);
        for m in &schedule.moves {
            assert!(m.ok(), "{m:?}");
        }
        for actuator in &chart.actuators {
            assert!(
                schedule.final_value(actuator).abs() < 1e-9,
                "{}",
                actuator.name
            );
        }
    }
}
