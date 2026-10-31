//! ロボットの画面 (設定・ジョグ・教示点) と、3D ビューへの TCP・関節・教示点の表示
//!
//! - ジョグ: 関節ごとのスライダか、TCP を X / Y / Z に少しずつ動かすボタン (逆運動学) で
//!   一時的に姿勢を動かす。チャートは変えず、再生やスクラブでチャートの位置に戻る。
//! - 教示点: 今の TCP を登録するか、3D ビューで部品の面をクリックして置くか、座標を直接入力する。
//!   選択中のステップに移動を置ける。
//! - サイクルタイム: 速度の上限から移動の最短時間を見積もる。MELFA-BASIC VI のプログラムに書き出せる。
//! - 経路の確認: 届かない移動・特異点の近くを通る移動の一覧と、移動中の干渉チェック。
//!   3D ビューには TCP の通り道を描き、問題のあるところに色を付ける。

use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::egui;

use super::chart::{MoveKind, Robot, RobotMove, TeachPoint, TimingChart};
use super::kinematics::{self, Tcp};
use super::melfa;
use super::path_interference::{PathBody, PathHit, robot_path_interference};
use super::robot_path::SINGULAR_DEXTERITY;
use super::robot_time;
use super::teach::{self, point_on_surface};
use super::ui::MotionUi;
use super::ui::{ChartUiState, ERROR_COLOR, WARN_COLOR};
use super::{ChartSchedule, JogPose, SimulationPose};
use crate::model::Model;
use crate::simulation::{InterferenceCheck, SimBody};
use crate::viewport::PartPicker;

/// 移動中の干渉を調べる時刻の間隔 (秒)
const INTERFERENCE_INTERVAL: f64 = 0.05;

/// 工具を向ける方向の候補 (教示点の設定用)
const APPROACHES: [(&str, Option<[f64; 3]>); 7] = [
    ("指定なし (位置だけ)", None),
    ("下向き (-Y)", Some([0.0, -1.0, 0.0])),
    ("上向き (+Y)", Some([0.0, 1.0, 0.0])),
    ("+X", Some([1.0, 0.0, 0.0])),
    ("-X", Some([-1.0, 0.0, 0.0])),
    ("+Z", Some([0.0, 0.0, 1.0])),
    ("-Z", Some([0.0, 0.0, -1.0])),
];

#[derive(Resource)]
pub struct RobotUiState {
    /// TCP ジョグの1回の移動量 (モデル単位)
    jog_step: f64,
    /// TCP ジョグで工具の向きを保つ
    keep_orientation: bool,
    /// 直前のジョグや教示点への移動が届かなかったときの表示
    message: Option<String>,
    /// 次のフレームで、このロボットの移動中の干渉を調べる
    interference_request: Option<u32>,
    /// 直前に調べた移動中の干渉 (ロボット, 結果)。チャートを変えたら消す。
    interference: Option<(u32, Vec<PathHit>)>,
    /// 3D ビューのクリックで教示点を置くロボット (置くモード中だけ Some)
    placing: Option<u32>,
    /// 置くとき、クリックした面から教示点を離す距離 (モデル単位)
    standoff: f64,
    /// 置くモード中、カーソルの下の面から作る教示点
    hover: Option<teach::SurfacePoint>,
    /// 次のフレームで、このロボットのプログラムを書き出す
    export_request: Option<u32>,
    /// 書き出しの結果
    export_message: Option<String>,
    /// 実機 (UR) とのつながり
    pub(super) ur: super::ur_ui::UrLinkState,
}

impl Default for RobotUiState {
    fn default() -> Self {
        Self {
            jog_step: 10.0,
            keep_orientation: true,
            message: None,
            interference_request: None,
            interference: None,
            placing: None,
            standoff: 0.0,
            hover: None,
            export_request: None,
            export_message: None,
            ur: Default::default(),
        }
    }
}

/// 右のプロパティ欄の下に出す「ロボット」の欄。関節 (回転テーブル・直動軸) がないうちは出さない。
pub fn show_robot_section(ui: &mut egui::Ui, motion: &mut MotionUi) {
    if !motion.chart.actuators.iter().any(|a| a.is_joint()) {
        return;
    }
    let mut edited = motion.chart.clone();
    let mut new_jog = None;
    egui::CollapsingHeader::new("ロボット")
        .id_salt("robot_section")
        .default_open(true)
        .show(ui, |ui| {
            new_jog = robot_panel(
                ui,
                &mut edited,
                &motion.schedule,
                &motion.pose,
                &motion.jog,
                &mut motion.state,
                &mut motion.robot,
            );
        });
    if edited != *motion.chart {
        *motion.chart = edited;
    }
    if let Some(values) = new_jog {
        motion.jog.set_if_neq(JogPose { values });
    }
}

/// ジョグで関節の値を変えたら、新しい値を返す (空ならジョグをやめる)
fn robot_panel(
    ui: &mut egui::Ui,
    chart: &mut TimingChart,
    schedule: &ChartSchedule,
    pose: &SimulationPose,
    jog: &JogPose,
    chart_state: &mut ChartUiState,
    state: &mut RobotUiState,
) -> Option<std::collections::HashMap<u32, f64>> {
    let mut new_jog = None;
    let joint_candidates: Vec<(u32, String)> = chart
        .actuators
        .iter()
        .filter(|a| a.is_joint())
        .map(|a| (a.id, a.name.clone()))
        .collect();

    ui.horizontal(|ui| {
        let selected = chart_state.selected_robot.and_then(|id| chart.robot(id));
        egui::ComboBox::from_id_salt("robot_select")
            .width(150.0)
            .selected_text(selected.map_or("(ロボットを選択)", |r| r.name.as_str()))
            .show_ui(ui, |ui| {
                for robot in &chart.robots {
                    ui.selectable_value(
                        &mut chart_state.selected_robot,
                        Some(robot.id),
                        &robot.name,
                    );
                }
            });
        ui.add_enabled_ui(!joint_candidates.is_empty(), |ui| {
            ui.menu_button("+ ロボット", |ui| {
                ui.label("先端 (手首) の関節:");
                for (id, name) in &joint_candidates {
                    if ui.button(name).clicked() {
                        chart_state.selected_robot = Some(chart.add_robot(*id));
                    }
                }
            })
            .response
            .on_disabled_hover_text("先に回転テーブルか直動軸を関節として追加してください");
        });
    });

    let Some(robot) = chart_state
        .selected_robot
        .and_then(|id| chart.robot(id))
        .cloned()
    else {
        ui.label(
            "回転テーブル・直動軸を「載せ先」でつなぐと関節の直列リンクになります。\n\
             先端の関節を選んでロボットにすると、教示点への移動を逆運動学で\n\
             各関節の動きに直してタイミングチャートで動かせます。\n\
             「ロボットのサンプル」で4軸アームの例を読み込めます。",
        );
        return None;
    };

    let joints = kinematics::joints(chart, &robot);
    let reach = kinematics::reach(chart, &robot);
    let tcp = kinematics::tcp(&robot, &pose.actuators);

    egui::CollapsingHeader::new("設定")
        .id_salt("robot_settings")
        .default_open(false)
        .show(ui, |ui| {
            robot_settings(ui, chart, &robot, &joint_candidates, chart_state);
        });

    ui.separator();
    ui.horizontal(|ui| {
        ui.strong("ジョグ");
        if !jog.values.is_empty() {
            let label = if state.ur.following(robot.id) {
                "(実機の角度で表示中)"
            } else {
                "(ジョグ中)"
            };
            ui.colored_label(egui::Color32::from_rgb(120, 200, 255), label);
            if ui.button("チャートの位置に戻す").clicked() {
                new_jog = Some(Default::default());
                state.message = None;
            }
        }
    });

    // 関節ごとのスライダ
    let current = |id: u32| pose.values.get(&id).copied().unwrap_or(0.0);
    egui::Grid::new("joint_jog").num_columns(2).show(ui, |ui| {
        for &id in &joints {
            let Some(actuator) = chart.actuator(id) else {
                continue;
            };
            ui.label(&actuator.name);
            let mut value = current(id);
            let range = actuator.limits.unwrap_or(match actuator.kind {
                super::chart::ActuatorKind::RotaryTable => [-180.0, 180.0],
                _ => [-2.0 * reach, 2.0 * reach],
            });
            let slider = egui::Slider::new(&mut value, range[0]..=range[1])
                .suffix(actuator.kind.unit())
                .fixed_decimals(if actuator.kind.unit().is_empty() {
                    3
                } else {
                    1
                });
            // 表示用の丸めで値が変わっただけのときはジョグにしない (触ったときだけ)
            let response = ui.add(slider);
            if response.changed()
                && (response.dragged() || response.clicked() || response.has_focus())
            {
                let mut values = jog_base(pose, &joints);
                values.insert(id, value);
                new_jog = Some(values);
                state.message = None;
            }
            ui.end_row();
        }
    });

    // TCP を少しずつ動かす (逆運動学)
    ui.monospace(format!(
        "TCP  X {:7.3}  Y {:7.3}  Z {:7.3}",
        tcp.position.x, tcp.position.y, tcp.position.z
    ));
    ui.horizontal(|ui| {
        ui.label("刻み");
        ui.add(
            egui::DragValue::new(&mut state.jog_step)
                .speed(0.01)
                .range(0.001..=10.0),
        );
        ui.checkbox(&mut state.keep_orientation, "工具の向きを保つ");
    });
    ui.horizontal(|ui| {
        for (label, delta) in [
            ("X-", DVec3::NEG_X),
            ("X+", DVec3::X),
            ("Y-", DVec3::NEG_Y),
            ("Y+", DVec3::Y),
            ("Z-", DVec3::NEG_Z),
            ("Z+", DVec3::Z),
        ] {
            if ui.button(label).clicked() {
                let approach = state.keep_orientation.then_some(tcp.tool_axis);
                let target = tcp.position + delta * state.jog_step;
                new_jog = Some(jog_to(
                    chart, &robot, pose, &joints, target, approach, state,
                ));
            }
        }
    });
    if let Some(message) = &state.message {
        ui.colored_label(ERROR_COLOR, message);
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.strong("教示点");
        if ui.button("+ 今の TCP を登録").clicked() {
            let name = teach::next_point_name(&robot);
            let approach = Some(teach::round_direction(tcp.tool_axis, 1.0).to_array());
            if let Some(r) = chart.robot_mut(robot.id) {
                r.add_point(name, tcp.position.to_array(), approach);
            }
        }
        let placing = state.placing == Some(robot.id);
        if ui
            .selectable_label(placing, "+ 3D ビューで置く")
            .on_hover_text("部品の面をクリックすると、そこに教示点を置きます")
            .clicked()
        {
            state.placing = if placing { None } else { Some(robot.id) };
            state.hover = None;
        }
    });
    if state.placing == Some(robot.id) {
        ui.horizontal(|ui| {
            ui.label("面から離す");
            ui.add(
                egui::DragValue::new(&mut state.standoff)
                    .speed(reach * 0.002)
                    .range(0.0..=f64::MAX),
            );
        });
        ui.weak("部品の面をクリックすると、そこから離した位置に、面に向かう工具の向きで教示点を置きます。Esc でやめます。");
    } else if robot.points.is_empty() {
        ui.weak("ジョグで TCP を動かしてから「今の TCP を登録」を押すか、「3D ビューで置く」で部品の面をクリックします。");
    }
    let step = chart_state.selected_step.filter(|&s| s < chart.steps.len());
    for point in &robot.points {
        let action = teach_point_row(ui, chart, &robot, point, step);
        match action {
            PointAction::None => {}
            PointAction::GoTo => {
                let target = DVec3::from_array(point.position);
                let approach = point.approach.map(DVec3::from_array);
                new_jog = Some(jog_to(
                    chart, &robot, pose, &joints, target, approach, state,
                ));
            }
            PointAction::AddMove(step) => {
                let s = &mut chart.steps[step];
                s.moves.retain(|m| m.robot != robot.id);
                s.moves.push(RobotMove {
                    robot: robot.id,
                    point: point.id,
                    duration: 1.0,
                    kind: Default::default(),
                });
            }
            PointAction::Delete => chart.remove_point(robot.id, point.id),
        }
    }
    super::ur_ui::ur_section(ui, chart, &robot, &mut state.ur);
    cycle_time(ui, chart, schedule, &robot, state);
    path_check(ui, schedule, &robot, chart_state, state);
    new_jog
}

/// サイクルタイムの見積もりと、ロボット言語への書き出し
fn cycle_time(
    ui: &mut egui::Ui,
    chart: &mut TimingChart,
    schedule: &ChartSchedule,
    robot: &Robot,
    state: &mut RobotUiState,
) {
    if chart.steps.is_empty() {
        return;
    }
    ui.separator();
    ui.strong("サイクルタイム");
    let estimate = robot_time::estimate_cycle(chart, schedule);
    ui.label(format!(
        "チャート {:.2} s / ロボットを最短で動かすと {:.2} s",
        estimate.current(),
        estimate.fastest()
    ))
    .on_hover_text("速度の上限 (「設定」) で各移動の最短時間を求めた見積もりです。条件待ちの時間は含みません。");
    let slow: Vec<String> = schedule
        .moves
        .iter()
        .filter(|m| m.robot == robot.id)
        .filter(|m| robot_time::min_move_time(chart, robot, m) > m.duration + 1e-6)
        .map(|m| format!("S{}", m.step + 1))
        .collect();
    if !slow.is_empty() {
        ui.colored_label(
            WARN_COLOR,
            format!(
                "⚠ 速度の上限を超える移動: {} (動作時間を延ばすか「すべて最短にする」)",
                slow.join(", ")
            ),
        );
    }
    ui.horizontal(|ui| {
        if ui
            .button("すべて最短にする")
            .on_hover_text("ロボットの移動の動作時間を、速度の上限で動いたときの最短時間にします")
            .clicked()
        {
            robot_time::set_fastest_durations(chart, schedule);
        }
        if ui
            .button("MELFA-BASIC VI で書き出す…")
            .on_hover_text("三菱電機のロボット言語のプログラム (.prg) に書き出します")
            .clicked()
        {
            state.export_request = Some(robot.id);
        }
    });
    if let Some(message) = &state.export_message {
        ui.weak(message);
    }
}

/// 経路の確認: チャートに置いた移動のうち、届かない・特異点の近くを通るものと、移動中の干渉
fn path_check(
    ui: &mut egui::Ui,
    schedule: &ChartSchedule,
    robot: &Robot,
    chart_state: &mut ChartUiState,
    state: &mut RobotUiState,
) {
    let moves: Vec<_> = schedule
        .moves
        .iter()
        .filter(|m| m.robot == robot.id)
        .collect();
    if moves.is_empty() {
        return;
    }
    ui.separator();
    ui.strong("経路の確認");
    let point_name = |id: u32| robot.point(id).map_or("?", |p| p.name.as_str());
    let mut problems = 0;
    for m in &moves {
        let issue = if !m.reached {
            Some((ERROR_COLOR, "届きません"))
        } else if !m.path_reached {
            Some((ERROR_COLOR, "直線の途中に届かないところがあります"))
        } else if m.singular {
            Some((WARN_COLOR, "特異点の近くを通ります"))
        } else {
            None
        };
        if let Some((color, text)) = issue {
            problems += 1;
            let label = format!("⚠ S{} → {}: {text}", m.step + 1, point_name(m.point));
            if ui
                .add(
                    egui::Label::new(egui::RichText::new(label).color(color))
                        .sense(egui::Sense::click()),
                )
                .on_hover_text("クリックでそのステップを選択")
                .clicked()
            {
                chart_state.selected_step = Some(m.step);
            }
        }
    }
    if problems == 0 {
        ui.weak("すべての移動が届き、特異点の近くも通りません。");
    }

    ui.horizontal(|ui| {
        if ui
            .button("移動中の干渉を調べる")
            .on_hover_text("ロボットに載った部品が、ほかの部品に食い込まないかを移動ごとに調べます")
            .clicked()
        {
            state.interference_request = Some(robot.id);
        }
    });
    let Some((checked, hits)) = &state.interference else {
        return;
    };
    if *checked != robot.id {
        return;
    }
    if hits.is_empty() {
        ui.weak("移動中の干渉はありません。");
    }
    for hit in hits {
        let label = format!(
            "⚠ S{} → {}: {} と {} が干渉 ({:.2}〜{:.2} s)",
            hit.step + 1,
            point_name(hit.point),
            hit.robot_part,
            hit.other_part,
            hit.start,
            hit.end
        );
        if ui
            .add(
                egui::Label::new(egui::RichText::new(label).color(ERROR_COLOR))
                    .sense(egui::Sense::click()),
            )
            .on_hover_text("クリックでそのステップを選択")
            .clicked()
        {
            chart_state.selected_step = Some(hit.step);
        }
    }
}

/// 「移動中の干渉を調べる」が押されていたら調べる。チャートが変わったら古い結果を消す。
pub fn check_path_interference(
    mut state: ResMut<RobotUiState>,
    chart: Res<TimingChart>,
    schedule: Res<ChartSchedule>,
    check: Res<InterferenceCheck>,
    model: Res<Model>,
    bodies: Query<(&SimBody, Option<&Name>)>,
) {
    if chart.is_changed() && state.interference.is_some() {
        state.interference = None;
    }
    let Some(robot) = state.interference_request.take() else {
        return;
    };
    let bodies: Vec<PathBody> = bodies
        .iter()
        .filter(|(body, _)| model.get(body.id).is_some())
        .map(|(body, name)| PathBody {
            id: body.id,
            name: name.map_or_else(|| format!("#{}", body.id), |n| n.to_string()),
            mesh: body.mesh.clone(),
            placement: model.world_matrix(body.id),
            ancestors: model.ancestors(body.id),
        })
        .collect();
    let hits = robot_path_interference(
        &bodies,
        &chart,
        &schedule,
        &check.settings,
        robot,
        INTERFERENCE_INTERVAL,
    );
    state.interference = Some((robot, hits));
}

fn robot_settings(
    ui: &mut egui::Ui,
    chart: &mut TimingChart,
    robot: &Robot,
    joint_candidates: &[(u32, String)],
    chart_state: &mut ChartUiState,
) {
    let chain: Vec<String> = kinematics::chain(chart, robot.tip)
        .iter()
        .filter_map(|id| chart.actuator(*id).map(|a| a.name.clone()))
        .collect();
    let linear_default = 2.0 * kinematics::reach(chart, robot);
    let Some(r) = chart.robot_mut(robot.id) else {
        return;
    };
    egui::Grid::new("robot_settings")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("名前");
            ui.text_edit_singleline(&mut r.name);
            ui.end_row();
            ui.label("先端の関節");
            let tip_name = joint_candidates
                .iter()
                .find(|(id, _)| *id == r.tip)
                .map_or("?", |(_, name)| name.as_str());
            egui::ComboBox::from_id_salt("robot_tip")
                .selected_text(tip_name)
                .show_ui(ui, |ui| {
                    for (id, name) in joint_candidates {
                        ui.selectable_value(&mut r.tip, *id, name);
                    }
                });
            ui.end_row();
            ui.label("TCP (設計位置)");
            ui.horizontal(|ui| {
                for v in &mut r.tcp {
                    ui.add(egui::DragValue::new(v).speed(0.01));
                }
            });
            ui.end_row();
            ui.label("工具軸 (設計位置)");
            ui.horizontal(|ui| {
                for v in &mut r.tool_axis {
                    ui.add(egui::DragValue::new(v).speed(0.01).range(-1.0..=1.0));
                }
            });
            ui.end_row();
            ui.label("関節の最高速度");
            ui.add(
                egui::DragValue::new(&mut r.speed.joint)
                    .speed(1.0)
                    .range(1.0..=3600.0)
                    .suffix(" °/s"),
            );
            ui.end_row();
            ui.label("直線の最高速度");
            ui.horizontal(|ui| {
                let mut custom = r.speed.linear.is_some();
                if ui.checkbox(&mut custom, "").changed() {
                    r.speed.linear = custom.then_some(linear_default);
                }
                match &mut r.speed.linear {
                    Some(v) => {
                        ui.add(
                            egui::DragValue::new(v)
                                .speed(linear_default * 0.01)
                                .range(0.0..=f64::MAX)
                                .suffix(" /s"),
                        );
                    }
                    None => {
                        ui.weak(format!("自動 ({linear_default:.0} /s)"));
                    }
                }
            });
            ui.end_row();
            ui.label("加速時間");
            ui.add(
                egui::DragValue::new(&mut r.speed.accel_time)
                    .speed(0.01)
                    .range(0.0..=5.0)
                    .suffix(" s"),
            );
            ui.end_row();
        });
    ui.weak(format!("関節: {}", chain.join(" → ")));
    ui.weak("可動範囲は各関節 (アクチュエータ) の設定で指定します。");
    if ui.button("ロボットを削除").clicked() {
        chart.remove_robot(robot.id);
        chart_state.selected_robot = None;
    }
}

enum PointAction {
    None,
    GoTo,
    AddMove(usize),
    Delete,
}

fn teach_point_row(
    ui: &mut egui::Ui,
    chart: &mut TimingChart,
    robot: &Robot,
    point: &TeachPoint,
    step: Option<usize>,
) -> PointAction {
    let mut action = PointAction::None;
    let Some(p) = chart
        .robot_mut(robot.id)
        .and_then(|r| r.points.iter_mut().find(|p| p.id == point.id))
    else {
        return action;
    };
    ui.push_id(point.id, |ui| {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(60.0));
            for v in &mut p.position {
                ui.add(egui::DragValue::new(v).speed(0.01).fixed_decimals(3));
            }
        });
        ui.horizontal(|ui| {
            let label = APPROACHES
                .iter()
                .find(|(_, a)| *a == p.approach)
                .map_or("その他の向き", |(label, _)| label);
            egui::ComboBox::from_id_salt("approach")
                .width(120.0)
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for (label, approach) in APPROACHES {
                        ui.selectable_value(&mut p.approach, approach, label);
                    }
                });
            if ui
                .button("移動")
                .on_hover_text("ジョグでこの点へ動かす")
                .clicked()
            {
                action = PointAction::GoTo;
            }
            if let Some(step) = step
                && ui
                    .button(format!("S{}へ", step + 1))
                    .on_hover_text("選択中のステップに、この点への移動を置く")
                    .clicked()
            {
                action = PointAction::AddMove(step);
            }
            if ui.button("削除").clicked() {
                action = PointAction::Delete;
            }
        });
    });
    action
}

/// ジョグの元にする関節の値 (今表示している値)
fn jog_base(pose: &SimulationPose, joints: &[u32]) -> std::collections::HashMap<u32, f64> {
    joints
        .iter()
        .map(|id| (*id, pose.values.get(id).copied().unwrap_or(0.0)))
        .collect()
}

/// 逆運動学で TCP を target へ動かすジョグの値。届かなければメッセージを出し、近づけた姿勢にする。
fn jog_to(
    chart: &TimingChart,
    robot: &Robot,
    pose: &SimulationPose,
    joints: &[u32],
    target: DVec3,
    approach: Option<DVec3>,
    state: &mut RobotUiState,
) -> std::collections::HashMap<u32, f64> {
    let solution = kinematics::solve_ik_to(chart, robot, target, approach, &pose.values);
    state.message = (!solution.reached).then(|| {
        format!(
            "⚠ 届きません (位置 {:.3} / 向き {:.1}° ずれ)",
            solution.position_error,
            solution.angle_error.to_degrees()
        )
    });
    let mut values = jog_base(pose, joints);
    values.extend(solution.values);
    values
}

// ---------------------------------------------------------------------------
// 3D ビューへの表示
// ---------------------------------------------------------------------------

const JOINT_COLOR: Color = Color::srgb(1.0, 0.85, 0.2);
const LINK_COLOR: Color = Color::srgb(0.3, 0.85, 0.95);
const TCP_COLOR: Color = Color::srgb(1.0, 0.45, 0.1);
const POINT_COLOR: Color = Color::srgb(0.3, 0.9, 0.4);
const POINT_ERROR_COLOR: Color = Color::srgb(0.95, 0.25, 0.25);
const PATH_COLOR: Color = Color::srgba(0.3, 0.9, 0.4, 0.7);
const PATH_WARN_COLOR: Color = Color::srgb(1.0, 0.75, 0.15);

/// 関節の軸、リンクの骨格、TCP と工具軸、教示点を描く
pub fn draw_robots(
    mut gizmos: Gizmos,
    chart: Res<TimingChart>,
    schedule: Res<ChartSchedule>,
    pose: Res<SimulationPose>,
) {
    let v = |p: DVec3| p.as_vec3();
    for robot in &chart.robots {
        // 印の大きさはロボットの大きさに合わせる (mm でも m でも同じ見た目にする)
        let k = kinematics::reach(&chart, robot);
        // 骨格: 根元の関節から順に、関節の中心を結ぶ
        let mut skeleton = Vec::new();
        for id in kinematics::chain(&chart, robot.tip) {
            let Some(actuator) = chart.actuator(id) else {
                continue;
            };
            let parent = actuator
                .mounted_on
                .and_then(|p| pose.actuators.get(&p).copied())
                .unwrap_or_default();
            let axis = parent.rotation * super::pose::direction(actuator);
            if actuator.kind == super::chart::ActuatorKind::RotaryTable {
                let pivot = parent.transform_point(DVec3::from_array(actuator.pivot));
                gizmos.line(
                    v(pivot - axis * 0.08 * k),
                    v(pivot + axis * 0.08 * k),
                    JOINT_COLOR,
                );
                gizmos.circle(
                    Isometry3d::new(
                        v(pivot),
                        Quat::from_rotation_arc(Vec3::Z, axis.as_vec3().normalize()),
                    ),
                    (0.03 * k) as f32,
                    JOINT_COLOR,
                );
                skeleton.push(pivot);
            } else if actuator.is_joint() {
                // 直動関節は、動いた後の位置に移動方向の矢印を描く
                let at = pose
                    .actuators
                    .get(&id)
                    .copied()
                    .unwrap_or_default()
                    .transform_point(DVec3::from_array(robot.tcp));
                let base = skeleton.last().copied().unwrap_or(at);
                gizmos.arrow(v(base), v(base + axis * 0.1 * k), JOINT_COLOR);
            }
        }
        let Tcp {
            position,
            tool_axis,
        } = kinematics::tcp(robot, &pose.actuators);
        skeleton.push(position);
        gizmos.linestrip(skeleton.iter().map(|p| v(*p)), LINK_COLOR);
        gizmos.sphere(v(position), (0.013 * k) as f32, TCP_COLOR);
        gizmos.arrow(v(position), v(position + tool_axis * 0.09 * k), TCP_COLOR);

        // TCP の通り道。特異点の近くは黄色、届かないところは赤
        for solved in schedule.moves.iter().filter(|m| m.robot == robot.id) {
            for pair in solved.path.windows(2) {
                let color = if !pair[1].reached {
                    POINT_ERROR_COLOR
                } else if solved.kind == MoveKind::Linear && pair[1].dexterity < SINGULAR_DEXTERITY
                {
                    PATH_WARN_COLOR
                } else {
                    PATH_COLOR
                };
                gizmos.line(
                    v(DVec3::from_array(pair[0].position)),
                    v(DVec3::from_array(pair[1].position)),
                    color,
                );
            }
        }

        for point in &robot.points {
            let reached = schedule
                .moves
                .iter()
                .filter(|m| m.robot == robot.id && m.point == point.id)
                .all(|m| m.reached && m.path_reached);
            let color = if reached {
                POINT_COLOR
            } else {
                POINT_ERROR_COLOR
            };
            let p = DVec3::from_array(point.position);
            gizmos.sphere(v(p), (0.016 * k) as f32, color);
            if let Some(approach) = point
                .approach
                .and_then(|a| DVec3::from_array(a).try_normalize())
            {
                // 工具が向かってくる向き: 点の手前から点へ
                gizmos.line(v(p - approach * 0.065 * k), v(p), color);
            }
        }
    }
}

/// 「3D ビューで置く」の間、カーソルの下の部品の面を調べ、クリックでそこに教示点を置く
#[allow(clippy::too_many_arguments)]
pub fn place_teach_point(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<bevy::window::PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    picker: PartPicker,
    bodies: Query<&SimBody>,
    mut state: ResMut<RobotUiState>,
    mut chart: ResMut<TimingChart>,
    mut gizmos: Gizmos,
) {
    let Some(robot) = state.placing else {
        return;
    };
    if keys.just_pressed(KeyCode::Escape) || chart.robot(robot).is_none() {
        state.placing = None;
        state.hover = None;
        return;
    }
    let (camera, camera_transform) = *camera;
    let standoff = state.standoff;
    state.hover = window.cursor_position().and_then(|cursor| {
        let ray = camera.viewport_to_world(camera_transform, cursor).ok()?;
        let hit = picker.cast(ray)?;
        let body = bodies.get(hit.entity).ok()?;
        // 当たった点を f64 の形状に載せ直し、その三角形の法線を使う
        let local = body.pose.inverse().apply(hit.point.as_dvec3());
        let (q, normal) = body.mesh.closest_point(local)?;
        let world = body.pose.apply(q);
        let normal = body.pose.rotation * normal;
        // 法線がカメラと反対を向いていたら裏返す (面の表裏が逆の形状でも外へ離す)
        let normal = if normal.dot(ray.direction.as_dvec3()) > 0.0 {
            -normal
        } else {
            normal
        };
        Some(point_on_surface(world, normal, standoff))
    });

    let Some(hover) = state.hover else {
        return;
    };
    let k = kinematics::reach(&chart, chart.robot(robot).expect("上で確かめた"));
    let p = hover.position.as_vec3();
    gizmos.sphere(p, (0.016 * k) as f32, POINT_COLOR);
    gizmos.line(
        (hover.position - hover.approach * 0.065 * k).as_vec3(),
        p,
        POINT_COLOR,
    );

    // Alt + 左ドラッグはカメラのオービットなので置かない
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    if !buttons.just_pressed(MouseButton::Left) || alt {
        return;
    }
    if let Some(r) = chart.robot_mut(robot) {
        let name = teach::next_point_name(r);
        r.add_point(
            name,
            hover.position.to_array(),
            Some(hover.approach.to_array()),
        );
    }
    state.placing = None;
    state.hover = None;
}

/// 「3D ビューで置く」の間は、クリックで部品を選ばない
pub fn teach_tool_off(state: Res<RobotUiState>) -> bool {
    state.placing.is_none()
}

/// 「MELFA-BASIC VI で書き出す」が押されていたら、保存先を選ばせて書き出す
pub fn export_melfa(
    mut state: ResMut<RobotUiState>,
    chart: Res<TimingChart>,
    schedule: Res<ChartSchedule>,
    model: Res<Model>,
    _main_thread: bevy::ecs::system::NonSendMarker,
) {
    let Some(id) = state.export_request.take() else {
        return;
    };
    let Some(robot) = chart.robot(id) else {
        return;
    };
    let Some(mut path) = rfd::FileDialog::new()
        .set_title("ロボットのプログラムを書き出す (MELFA-BASIC VI)")
        .add_filter("MELFA-BASIC VI", &["prg"])
        .set_file_name(format!("{}.prg", robot.name))
        .save_file()
    else {
        return;
    };
    if path.extension().is_none() {
        path.set_extension("prg");
    }
    let program = melfa::melfa_program(
        &chart,
        &schedule,
        robot,
        melfa::MelfaOptions {
            unit_to_mm: melfa::unit_to_mm(&model.unit),
        },
    );
    state.export_message = Some(match std::fs::write(&path, program) {
        Ok(()) => format!("{} に書き出しました", path.display()),
        Err(e) => format!("{} に書き込めませんでした: {e}", path.display()),
    });
}
