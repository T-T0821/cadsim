//! 「複数人でレビュー」パネル: ホストになる・参加する、参加者の一覧、視点を追う・呼ぶ、チャット

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use cadsim_core::review::{DEFAULT_PORT, peer_color};
use cadsim_core::workspace::Tool;

use super::Review;
use crate::project::ProjectState;
use crate::ui::dock::{self, Dock};
use crate::view::camera::{OrbitCamera, ViewportRect};

const OK_COLOR: egui::Color32 = egui::Color32::from_rgb(60, 200, 90);
const WAIT_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 180, 50);
const WARN_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 90, 70);

pub fn review_panel(
    mut contexts: EguiContexts,
    mut review: ResMut<Review>,
    orbit: Res<OrbitCamera>,
    project: Res<ProjectState>,
    viewport: Res<ViewportRect>,
    mut dock: ResMut<Dock>,
) -> Result {
    let Some(window) = dock::panel(Tool::Review, &dock, viewport.rect) else {
        return Ok(());
    };
    let review = review.bypass_change_detection();
    let project_name = project.file_name();
    let mut open = true;
    window
        .open(&mut open)
        .default_width(320.0)
        .show(contexts.ctx_mut()?, |ui| {
            if review.connected() {
                connected(ui, review, &orbit, &project_name);
            } else {
                setup(ui, review, &project_name);
            }
        });
    if !open {
        dock.set_open(Tool::Review, false);
    }
    Ok(())
}

/// つなぐ前: 名前・合言葉と、ホストになる / 参加する
fn setup(ui: &mut egui::Ui, review: &mut Review, project_name: &str) {
    ui.colored_label(egui::Color32::GRAY, "● 未接続");
    ui.label(
        "同じプロジェクトを開いた人どうしで、互いの視点と指した場所を見ながらレビューします。",
    );
    ui.add_space(4.0);
    egui::Grid::new("review_setup")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("名前");
            ui.add(
                egui::TextEdit::singleline(&mut review.form.name)
                    .hint_text("ゲスト")
                    .desired_width(180.0),
            );
            ui.end_row();
            ui.label("合言葉");
            ui.add(
                egui::TextEdit::singleline(&mut review.form.passphrase)
                    .hint_text("空なら誰でも入れます")
                    .password(true)
                    .desired_width(180.0),
            );
            ui.end_row();
        });
    ui.separator();
    ui.strong("ホストになる");
    ui.horizontal(|ui| {
        ui.label("ポート");
        ui.add(egui::TextEdit::singleline(&mut review.form.port).desired_width(60.0));
        if ui
            .button("ホストになる")
            .on_hover_text("このパソコンで待ち受けて、ほかの人に参加してもらう")
            .clicked()
        {
            review.start_host(project_name);
        }
    });
    ui.separator();
    ui.strong("参加する");
    ui.horizontal(|ui| {
        ui.label("アドレス");
        ui.add(
            egui::TextEdit::singleline(&mut review.form.address)
                .hint_text(format!("192.168.1.10:{DEFAULT_PORT}"))
                .desired_width(150.0),
        );
        if ui
            .button("参加する")
            .on_hover_text("ホストの人に教えてもらったアドレスにつなぐ")
            .clicked()
        {
            review.join(project_name);
        }
    });
    if let Some(status) = &review.status {
        ui.add_space(4.0);
        ui.colored_label(WARN_COLOR, status);
    }
}

/// つないだ後: 参加者の一覧・視点・チャット
fn connected(ui: &mut egui::Ui, review: &mut Review, orbit: &OrbitCamera, project_name: &str) {
    let leave_label = if review.is_host() {
        "ホストをやめる"
    } else {
        "抜ける"
    };
    ui.horizontal(|ui| {
        if !review.session.joined() {
            ui.colored_label(WAIT_COLOR, "● 接続中…");
        } else if review.is_host() {
            ui.colored_label(OK_COLOR, "● ホスト中");
        } else {
            ui.colored_label(OK_COLOR, "● 参加中");
        }
        if ui.button(leave_label).clicked() {
            review.leave();
        }
    });
    if let Some(port) = review.host_port() {
        let addresses = review.host_addresses.clone();
        ui.horizontal_wrapped(|ui| {
            ui.label("参加する人に伝えるアドレス:");
            if addresses.is_empty() {
                ui.monospace(format!("このパソコンのアドレス:{port}"));
            }
            for address in &addresses {
                if ui
                    .add(egui::Button::new(egui::RichText::new(address).monospace()))
                    .on_hover_text("押すとコピーします")
                    .clicked()
                {
                    ui.ctx().copy_text(address.clone());
                }
            }
        });
        ui.small("社外の人とつなぐときは、ルーターのポート開放か VPN が要ります。");
    }
    if !review.session.joined() {
        return;
    }
    if let Some(host_project) = &review.session.host_project
        && !review.is_host()
        && host_project != project_name
    {
        ui.colored_label(
            WARN_COLOR,
            format!("ホストは「{host_project}」を開いています。同じプロジェクトを開いてください。"),
        );
    }
    ui.separator();
    ui.strong("参加者");
    let me = review.session.me.unwrap_or_default();
    ui.horizontal(|ui| {
        ui.colored_label(color32(peer_color(me)), "●");
        ui.label(format!(
            "{} (自分)",
            cadsim_core::review::clean_name(&review.form.name)
        ));
    });
    let mut follow = None;
    for peer in review.session.peers() {
        ui.horizontal(|ui| {
            ui.colored_label(color32(peer.info.color), "●");
            ui.label(&peer.info.name);
            if peer.info.id == cadsim_core::review::HOST_ID {
                ui.weak("ホスト");
            }
            let following = review.session.following == Some(peer.info.id);
            let label = if following {
                "追うのをやめる"
            } else {
                "視点を追う"
            };
            if ui
                .add_enabled(peer.view.is_some(), egui::Button::new(label).small())
                .on_hover_text(
                    "この人の視点に自分のカメラを合わせ続ける。自分でカメラを動かすとやめる",
                )
                .clicked()
            {
                follow = Some((!following).then_some(peer.info.id));
            }
        });
    }
    if let Some(follow) = follow {
        review.session.following = follow;
    }
    if review.session.peers().next().is_none() {
        ui.weak("まだ誰もいません");
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                review.session.peers().next().is_some(),
                egui::Button::new("みんなを自分の視点へ呼ぶ"),
            )
            .on_hover_text("参加している全員のカメラを、自分の今の視点に合わせる")
            .clicked()
        {
            review.summon(orbit);
        }
    });
    if ui
        .checkbox(&mut review.form.share_placement, "配置の変更も共有する")
        .on_hover_text(
            "部品を動かすと、ほかの人の画面でも同じ場所へ動く。全員がオンにしたときだけ届く",
        )
        .changed()
    {
        review.placement_sharing_changed();
    }
    ui.small("P キー: マウスの場所に「ここを見て」の印を置く");
    ui.separator();
    chat(ui, review);
}

fn chat(ui: &mut egui::Ui, review: &mut Review) {
    ui.strong("チャット");
    egui::ScrollArea::vertical()
        .max_height(90.0)
        .stick_to_bottom(true)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for line in review.session.chat() {
                match line.from {
                    None => {
                        ui.weak(&line.text);
                    }
                    Some(id) => {
                        let rgb = review
                            .session
                            .peer(id)
                            .map_or_else(|| peer_color(id), |p| p.info.color);
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(color32(rgb), format!("{}:", line.name));
                            ui.label(&line.text);
                        });
                    }
                }
            }
        });
    let mut send = false;
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut review.chat_input)
                .hint_text("メッセージ")
                .desired_width(220.0),
        );
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            send = true;
            response.request_focus();
        }
        if ui.button("送信").clicked() {
            send = true;
        }
    });
    if send {
        review.send_chat();
    }
}

fn color32(rgb: [u8; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}
