//! タイミングチャートのロボットの動きを、三菱電機のロボット言語 MELFA-BASIC VI のプログラムにする
//!
//! - 位置はロボットの根元の座標系 (根元の関節の回転中心が原点、根元の回転軸が +Z) の mm。
//!   姿勢 (A, B, C) は工具の向き (工具の +Z) を、X → Y → Z の順の固定角 (度) で表す。
//! - 移動は関節補間を `Mov`、直線補間を `Mvs`。チャートの動作時間に合わせて、
//!   関節補間は `JOvrd` (関節速度の上限に対する割合)、直線補間は `Spd` (mm/s) を付ける。
//!   全体の速度 `Ovrd` は先頭で 100 にする。
//! - ロボットに載ったシリンダ (グリッパ) は `HClose` / `HOpen`、それ以外のアクチュエータは
//!   `M_Out` の出力、センサーの開始条件は `Wait M_In` にする。信号の番号はプログラムの先頭に書く。
//! - 関節に直接置いた動作 (原点復帰など) は、関節の値の `J` 変数への `Mov` にする。
//!
//! 構造フラグ (FL1, FL2) は姿勢から決められないので、仮の値 (7, 0) を書く。

use std::collections::HashMap;
use std::fmt::Write;

use bevy::math::{DQuat, DVec3, EulerRot};

use super::kinematics;
use super::pose;
use super::robot_time;
use plantwright_core::motion::chart::ActuatorKind;
use plantwright_core::motion::chart::MoveKind;
use plantwright_core::motion::chart::Robot;
use plantwright_core::motion::chart::Schedule;
use plantwright_core::motion::chart::TimingChart;

/// 出力信号の先頭番号 (ロボット以外のアクチュエータ)
const OUTPUT_BASE: usize = 10;
/// 入力信号の先頭番号 (センサー)
const INPUT_BASE: usize = 10;

/// ロボットの根元の座標系: 原点と、モデル座標からの回転
#[derive(Clone, Copy, Debug)]
pub struct BaseFrame {
    pub origin: DVec3,
    /// モデル座標の向き → ロボット座標の向き
    pub to_base: DQuat,
}

impl BaseFrame {
    /// モデル座標の点をロボット座標にする
    pub fn point(&self, p: DVec3) -> DVec3 {
        self.to_base * (p - self.origin)
    }
}

/// 根元の関節の回転中心を原点、根元の回転軸 (直動なら上 = モデルの +Y) を +Z にした座標系。
/// +X はモデルの +X (Z と平行ならモデルの +Z) を Z に直交させた向き。
pub fn base_frame(chart: &TimingChart, robot: &Robot) -> BaseFrame {
    let root = kinematics::chain(chart, robot.tip)
        .first()
        .and_then(|&id| chart.actuator(id));
    let (origin, z) = match root {
        Some(a) if a.kind == ActuatorKind::RotaryTable => {
            (DVec3::from_array(a.pivot), pose::direction(a).normalize())
        }
        Some(a) => (DVec3::from_array(a.pivot), DVec3::Y),
        None => (DVec3::ZERO, DVec3::Y),
    };
    let x = [DVec3::X, DVec3::Z]
        .into_iter()
        .map(|v| v - z * v.dot(z))
        .find(|v| v.length() > 1e-6)
        .unwrap_or(DVec3::X)
        .normalize();
    let y = z.cross(x);
    // 列が (x, y, z) の行列はロボット → モデル。その逆 (転置) がモデル → ロボット。
    let to_model = DQuat::from_mat3(&bevy::math::DMat3::from_cols(x, y, z));
    BaseFrame {
        origin,
        to_base: to_model.inverse(),
    }
}

/// 工具の +Z を向き `approach` (ロボット座標) に向ける姿勢 (A, B, C) [度]。
/// 真下向きは (180, 0, 0) になる。
pub fn tool_angles(approach: DVec3) -> [f64; 3] {
    let Some(approach) = approach.try_normalize() else {
        return [180.0, 0.0, 0.0];
    };
    let rotation = if approach.dot(DVec3::NEG_Z) > 1.0 - 1e-12 {
        DQuat::from_rotation_x(std::f64::consts::PI)
    } else {
        // 真下向き (180, 0, 0) から最小の回転で傾ける (工具の X の向きをなるべく保つ)
        let down = DQuat::from_rotation_x(std::f64::consts::PI);
        DQuat::from_rotation_arc(DVec3::NEG_Z, approach) * down
    };
    // R = Rz(C) Ry(B) Rx(A)
    let (c, b, a) = rotation.to_euler(EulerRot::ZYX);
    [a, b, c].map(|v| clean(v.to_degrees()))
}

/// -0.000 や 179.9999… を見やすくする
fn clean(v: f64) -> f64 {
    let v = (v * 1000.0).round() / 1000.0;
    if v == 0.0 { 0.0 } else { v }
}

/// 書き出しの設定
#[derive(Clone, Copy, Debug)]
pub struct MelfaOptions {
    /// モデル単位 1 が何 mm か
    pub unit_to_mm: f64,
}

/// ロボット `robot` の動きを MELFA-BASIC VI のプログラムにする
pub fn melfa_program(
    chart: &TimingChart,
    schedule: &Schedule,
    robot: &Robot,
    options: MelfaOptions,
) -> String {
    let frame = base_frame(chart, robot);
    let joints = kinematics::joints(chart, robot);
    let mm = options.unit_to_mm;
    let robot_actuators = chart.robot_actuators(robot);
    // ロボットに載ったシリンダはハンド、それ以外のアクチュエータは出力信号
    let hands: Vec<u32> = chart
        .actuators
        .iter()
        .filter(|a| a.kind == ActuatorKind::Cylinder && robot_actuators.contains(&a.id))
        .map(|a| a.id)
        .collect();
    let outputs: Vec<u32> = chart
        .actuators
        .iter()
        .filter(|a| !robot_actuators.contains(&a.id))
        .map(|a| a.id)
        .collect();
    let point_vars: HashMap<u32, String> = robot
        .points
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id, format!("P{}", i + 1)))
        .collect();
    let estimate = robot_time::estimate_cycle(chart, schedule);

    let mut out = String::new();
    let line = |out: &mut String, text: String| {
        out.push_str(&text);
        out.push_str("\r\n");
    };
    line(
        &mut out,
        format!("' {} (plantwright から書き出し)", robot.name),
    );
    line(
        &mut out,
        "' 位置: 根元の関節の回転中心が原点、根元の回転軸が +Z の mm。姿勢 A, B, C は度".into(),
    );
    line(
        &mut out,
        "' 構造フラグ (7,0) は仮の値です。実機の姿勢に合わせて直してください".into(),
    );
    line(
        &mut out,
        format!(
            "' サイクルタイム: チャート {:.2} s / 速度上限での見積もり {:.2} s (条件待ちを除く)",
            estimate.current(),
            estimate.fastest()
        ),
    );
    for (i, id) in hands.iter().enumerate() {
        if let Some(a) = chart.actuator(*id) {
            line(&mut out, format!("' ハンド {}: {}", i + 1, a.name));
        }
    }
    for (i, id) in outputs.iter().enumerate() {
        if let Some(a) = chart.actuator(*id) {
            line(
                &mut out,
                format!("' M_Out({}): {}", OUTPUT_BASE + i, a.name),
            );
        }
    }
    for (i, s) in chart.sensors.iter().enumerate() {
        line(&mut out, format!("' M_In({}): {}", INPUT_BASE + i, s.name));
    }

    line(&mut out, "Ovrd 100".into());
    let mut joint_vars: Vec<(String, Vec<f64>)> = Vec::new();
    for (index, step) in chart.steps.iter().enumerate() {
        let title = if step.comment.is_empty() {
            format!("S{}", index + 1)
        } else {
            format!("S{} {}", index + 1, step.comment)
        };
        line(&mut out, format!("' {title} ({:.2} s)", step.duration()));
        for interlock in &step.interlocks {
            if let Some(i) = chart.sensors.iter().position(|s| s.id == interlock.sensor) {
                line(
                    &mut out,
                    format!("Wait M_In({})={}", INPUT_BASE + i, u8::from(interlock.on)),
                );
            }
        }
        // ロボットの移動
        if let Some(m) = step.robot_move(robot.id)
            && let Some(var) = point_vars.get(&m.point)
        {
            let solved = schedule.solved_move(index, robot.id);
            match m.kind {
                MoveKind::Joint => {
                    // 速度上限で動いたときの時間との比を JOvrd (%) にする
                    if let Some(solved) = solved {
                        let fastest = robot_time::min_move_time(chart, robot, solved);
                        let percent = speed_percent(fastest, m.duration);
                        line(&mut out, format!("JOvrd {percent}"));
                    }
                    line(&mut out, format!("Mov {var}"));
                }
                MoveKind::Linear => {
                    if let Some(solved) = solved
                        && m.duration > 0.0
                    {
                        let speed = robot_time::path_length(solved) * mm / m.duration;
                        line(&mut out, format!("Spd {:.0}", speed.max(1.0)));
                    }
                    line(&mut out, format!("Mvs {var}"));
                }
            }
        } else {
            // 関節に直接置いた動作は J 変数への関節補間にする
            let direct: Vec<_> = step
                .actions
                .iter()
                .filter(|a| joints.contains(&a.actuator))
                .collect();
            if !direct.is_empty() {
                let end = schedule
                    .steps
                    .get(index)
                    .map_or(0.0, |s| s.action_start + step.duration() - step.dwell);
                let values: Vec<f64> = joints
                    .iter()
                    .map(|id| {
                        let a = chart.actuator(*id).expect("関節はチャートにある");
                        let v = schedule.value_at(a, end);
                        if a.kind == ActuatorKind::RotaryTable {
                            clean(v)
                        } else {
                            clean(v * mm)
                        }
                    })
                    .collect();
                let name = format!("JS{}", index + 1);
                line(&mut out, "JOvrd 100".into());
                line(&mut out, format!("Mov {name}"));
                joint_vars.push((name, values));
            }
        }
        // ハンドと出力信号
        for action in &step.actions {
            if let Some(i) = hands.iter().position(|&h| h == action.actuator) {
                let command = if action.target >= 0.5 {
                    "HClose"
                } else {
                    "HOpen"
                };
                line(&mut out, format!("{command} {}", i + 1));
            } else if let Some(i) = outputs.iter().position(|&o| o == action.actuator) {
                let on = chart
                    .actuator(action.actuator)
                    .is_some_and(|a| a.kind != ActuatorKind::Cylinder || action.target >= 0.5);
                line(
                    &mut out,
                    format!("M_Out({})={}", OUTPUT_BASE + i, u8::from(on)),
                );
            }
        }
        // ハンドや信号の動作が終わるのを待つ (ロボットの移動は完了まで次の行に進まない)
        let wait = step
            .actions
            .iter()
            .filter(|a| !joints.contains(&a.actuator))
            .map(|a| a.duration.max(0.0))
            .fold(0.0, f64::max)
            + step.dwell.max(0.0);
        if wait > 0.0 {
            line(&mut out, format!("Dly {:.2}", wait));
        }
    }
    line(&mut out, "End".into());

    // 位置変数
    for point in &robot.points {
        let var = &point_vars[&point.id];
        let p = frame.point(DVec3::from_array(point.position)) * mm;
        let approach = point
            .approach
            .map_or(DVec3::NEG_Z, |a| frame.to_base * DVec3::from_array(a));
        let [a, b, c] = tool_angles(approach);
        let note = if point.approach.is_none() {
            " (向きの指定なし: 下向きで書き出し)"
        } else {
            ""
        };
        line(
            &mut out,
            format!(
                "{var}=({:.3},{:.3},{:.3},{a:.3},{b:.3},{c:.3})(7,0) ' {}{note}",
                clean(p.x),
                clean(p.y),
                clean(p.z),
                point.name
            ),
        );
    }
    for (name, values) in joint_vars {
        let mut text = format!("{name}=(");
        for (i, v) in values
            .iter()
            .chain(std::iter::repeat(&0.0))
            .take(6)
            .enumerate()
        {
            if i > 0 {
                text.push(',');
            }
            let _ = write!(text, "{v:.3}");
        }
        text.push(')');
        line(&mut out, text);
    }
    out
}

/// 速度上限で `fastest` 秒の移動を `duration` 秒で動かすときの速度の割合 (1〜100 %)
fn speed_percent(fastest: f64, duration: f64) -> u32 {
    if duration <= 0.0 || fastest <= 0.0 {
        return 100;
    }
    ((fastest / duration * 100.0).ceil() as u32).clamp(1, 100)
}

/// モデルの単位名から、1 単位が何 mm か
pub fn unit_to_mm(unit: &str) -> f64 {
    match unit.trim().to_ascii_lowercase().as_str() {
        "m" => 1000.0,
        "cm" => 10.0,
        "in" | "inch" => 25.4,
        "ft" | "foot" => 304.8,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::demo;
    use plantwright_core::model::Model;

    #[test]
    fn downward_tool_is_180_0_0() {
        assert_eq!(tool_angles(DVec3::NEG_Z), [180.0, 0.0, 0.0]);
        // 工具を +X に向けると B = 90 になる (下向きから Y 軸まわりに倒す)
        let [_, b, _] = tool_angles(DVec3::X);
        assert!((b.abs() - 90.0).abs() < 1e-6, "{b}");
    }

    #[test]
    fn base_frame_turns_model_up_into_robot_z() {
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        demo::load_robot_demo(&mut model, &mut chart);
        let frame = base_frame(&chart, &chart.robots[0]);
        // J1 はモデルの +Y まわりに回るので、モデルの上はロボットの +Z
        assert!((frame.to_base * DVec3::Y - DVec3::Z).length() < 1e-12);
        assert!((frame.to_base * DVec3::X - DVec3::X).length() < 1e-12);
    }

    #[test]
    fn demo_program_has_moves_hands_and_positions() {
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        demo::load_robot_demo(&mut model, &mut chart);
        let schedule = chart.schedule();
        let program = melfa_program(
            &chart,
            &schedule,
            &chart.robots[0],
            MelfaOptions { unit_to_mm: 1.0 },
        );
        let lines: Vec<&str> = program.lines().collect();
        assert!(lines.contains(&"Mov P1"), "{program}");
        assert!(lines.contains(&"Mvs P2"), "{program}");
        assert!(lines.contains(&"HClose 1"), "{program}");
        assert!(lines.contains(&"HOpen 1"), "{program}");
        assert!(lines.contains(&"Mov JS9"), "{program}");
        assert!(lines.contains(&"End"));
        // ピック上 (モデル (190, 150, 0)) は、根元から見て X=190 に、下向きの工具で書き出す
        let p1 = lines.iter().find(|l| l.starts_with("P1=")).unwrap();
        assert!(p1.contains("(7,0)"), "{p1}");
        assert!(p1.contains(",180.000,0.000,0.000)"), "{p1}");
        assert!(p1.starts_with("P1=(190.000,"), "{p1}");
        // 原点復帰は全関節 0
        let js = lines.iter().find(|l| l.starts_with("JS9=")).unwrap();
        assert_eq!(*js, "JS9=(0.000,0.000,0.000,0.000,0.000,0.000)");
    }

    #[test]
    fn speed_percent_is_clamped() {
        assert_eq!(speed_percent(0.5, 1.0), 50);
        assert_eq!(speed_percent(2.0, 1.0), 100);
        assert_eq!(speed_percent(0.0, 1.0), 100);
    }
}
