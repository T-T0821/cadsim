//! 実機の UR (Universal Robots) とつなぎ、受け取った関節角度でモデルのロボットを動かす画面
//!
//! 通信は `cadsim_io::rtde`、角度からモデルの関節の値への直し方は `live_joints` に置き、
//! ここは「設定を集める → つなぐ → 受け取った値をジョグの値に写す」だけにする。

use bevy::prelude::*;
use bevy_egui::egui;
use cadsim_io::rtde::{RtdeClient, RtdeSettings, RtdeStatus};

use super::JogPose;
use super::chart::{Robot, TimingChart};
use super::live_joints::{self, JOINT_COUNT, JointMap};
use super::ui::{ERROR_COLOR, WARN_COLOR};

/// 実機とのつながり (ロボットの画面の状態の一部)
#[derive(Default)]
pub struct UrLinkState {
    settings: RtdeSettings,
    map: JointMap,
    /// つないでいる間だけ Some (動かすロボット, クライアント)
    link: Option<(u32, RtdeClient)>,
    /// 直前に受け取った角度の範囲外の関節 (表示用)
    out_of_limits: Vec<u32>,
    /// 切断した直後に、ジョグの値を空に戻す
    clear_jog: bool,
}

impl UrLinkState {
    /// このロボットを実機の角度で動かしているか
    pub fn following(&self, robot: u32) -> bool {
        self.link.as_ref().is_some_and(|(id, _)| *id == robot)
    }
}

/// ロボットの画面の中の「実機 (UR) とつなぐ」欄
pub fn ur_section(ui: &mut egui::Ui, chart: &TimingChart, robot: &Robot, state: &mut UrLinkState) {
    egui::CollapsingHeader::new("実機 (UR) とつなぐ")
        .id_salt("ur_link")
        .default_open(false)
        .show(ui, |ui| ur_panel(ui, chart, robot, state));
}

fn ur_panel(ui: &mut egui::Ui, chart: &TimingChart, robot: &Robot, state: &mut UrLinkState) {
    let joints = live_joints::rotary_joints(chart, robot);
    let connected = state.link.is_some();
    ui.add_enabled_ui(!connected, |ui| {
        egui::Grid::new("ur_settings")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("IP アドレス");
                ui.add(egui::TextEdit::singleline(&mut state.settings.host).desired_width(130.0));
                ui.end_row();
                ui.label("ポート");
                ui.add(egui::DragValue::new(&mut state.settings.port));
                ui.end_row();
                ui.label("周期");
                ui.add(
                    egui::DragValue::new(&mut state.settings.frequency)
                        .range(1.0..=500.0)
                        .suffix(" Hz"),
                );
                ui.end_row();
            });
    });

    ui.horizontal(|ui| {
        if let Some((id, client)) = &state.link {
            if ui.button("切断").clicked() {
                state.link = None;
                state.clear_jog = true;
                state.out_of_limits.clear();
                return;
            }
            if *id != robot.id {
                let name = chart.robot(*id).map_or("?", |r| r.name.as_str());
                ui.colored_label(WARN_COLOR, format!("「{name}」を動かしています"));
                return;
            }
            match client.status() {
                RtdeStatus::Connecting => {
                    ui.label("接続中…");
                }
                RtdeStatus::Receiving => {
                    ui.colored_label(egui::Color32::from_rgb(120, 220, 120), "● 受信中");
                }
                RtdeStatus::Failed(reason) => {
                    ui.colored_label(ERROR_COLOR, format!("⚠ {reason} (つなぎ直します)"));
                }
            }
        } else {
            let can_connect = !joints.is_empty();
            if ui
                .add_enabled(can_connect, egui::Button::new("接続"))
                .on_disabled_hover_text("ロボットに回転関節がありません")
                .clicked()
            {
                state.link = Some((robot.id, RtdeClient::start(state.settings.clone())));
            }
        }
    });

    if joints.len() != JOINT_COUNT {
        ui.colored_label(
            WARN_COLOR,
            format!(
                "回転関節が {} 個あります。実機の 6 軸の角度を根元から順に {} 個まで割り当てます。",
                joints.len(),
                joints.len().min(JOINT_COUNT)
            ),
        );
    }

    let latest = state
        .link
        .as_ref()
        .filter(|(id, _)| *id == robot.id)
        .and_then(|(_, client)| client.latest());
    egui::Grid::new("ur_joint_map")
        .num_columns(4)
        .striped(true)
        .show(ui, |ui| {
            ui.strong("実機");
            ui.strong("関節");
            ui.strong("反転");
            ui.strong("0° のとき");
            ui.end_row();
            for (i, id) in joints.iter().take(JOINT_COUNT).enumerate() {
                let received = latest.map_or("—".to_string(), |s| {
                    format!("J{} {:7.2}°", i + 1, s.q[i].to_degrees())
                });
                ui.monospace(received);
                let name = chart.actuator(*id).map_or("?", |a| a.name.as_str());
                if state.out_of_limits.contains(id) {
                    ui.colored_label(ERROR_COLOR, format!("{name} (範囲外)"));
                } else {
                    ui.label(name);
                }
                ui.checkbox(&mut state.map.invert[i], "");
                ui.add(
                    egui::DragValue::new(&mut state.map.offset_deg[i])
                        .speed(0.5)
                        .suffix("°"),
                );
                ui.end_row();
            }
        });
    ui.weak(
        "つないでいる間は、受け取った角度でロボットを動かします (ジョグと同じく、チャートは変えません)。\n\
         向きが逆の関節は「反転」、実機の 0° とモデルの設計位置がずれている関節は「0° のとき」の角度を入れます。",
    );
}

/// 受け取った角度を、ジョグの値としてモデルの関節に写す
pub fn follow_ur(
    mut state: ResMut<super::robot_ui::RobotUiState>,
    chart: Res<TimingChart>,
    mut jog: ResMut<JogPose>,
) {
    let ur = &mut state.bypass_change_detection().ur;
    if std::mem::take(&mut ur.clear_jog) {
        jog.set_if_neq(JogPose::default());
    }
    let Some((robot_id, client)) = &ur.link else {
        return;
    };
    let (Some(robot), Some(sample)) = (chart.robot(*robot_id), client.latest()) else {
        return;
    };
    let live = live_joints::live_joint_values(&chart, robot, &sample.q, &ur.map);
    if ur.out_of_limits != live.out_of_limits {
        ur.out_of_limits = live.out_of_limits;
    }
    jog.set_if_neq(JogPose {
        values: live.values,
    });
}
