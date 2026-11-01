//! 「音の設定」ウィンドウと、警報が鳴っている間の帯

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use cadsim_core::sound::SoundCategory;

use super::SoundUi;
use crate::view::camera::ViewportRect;

/// 「表示」メニューに並べる項目
pub fn show_menu_items(ui: &mut egui::Ui, sound: &mut SoundUi) {
    let mut on = !sound.settings.muted;
    if ui
        .checkbox(&mut on, "音を出す")
        .on_hover_text("設備の稼働音・エアの排気音・停止の警報を鳴らす")
        .changed()
    {
        sound.settings.muted = !on;
    }
    if ui
        .button("音の設定…")
        .on_hover_text("全体と種類ごとの音量を変える")
        .clicked()
    {
        sound.show_window = true;
    }
}

pub fn sound_window(mut contexts: EguiContexts, mut sound: ResMut<SoundUi>) -> Result {
    if !sound.show_window {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    // 毎フレームの変更検知を避けるため、変えたときだけ変更を知らせる
    let sound_ref = sound.bypass_change_detection();
    let before = sound_ref.settings.clone();
    let mut open = true;
    egui::Window::new("音の設定")
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .default_width(300.0)
        .show(ctx, |ui| settings_body(ui, sound_ref));
    sound_ref.show_window &= open;
    if sound_ref.settings != before || !open {
        sound.set_changed();
    }
    Ok(())
}

fn settings_body(ui: &mut egui::Ui, sound: &mut SoundUi) {
    let mut on = !sound.settings.muted;
    if ui.checkbox(&mut on, "音を出す").changed() {
        sound.settings.muted = !on;
    }
    ui.add_enabled_ui(on, |ui| {
        egui::Grid::new("sound_volumes")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("全体の音量");
                percent_slider(ui, &mut sound.settings.master);
                ui.end_row();
                for category in SoundCategory::ALL {
                    ui.label(category.label());
                    percent_slider(ui, sound.settings.volume_mut(category));
                    ui.end_row();
                }
            });
    });
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "設備の音は 3D ビューで見ている位置から聞こえ、近くの設備ほど大きく鳴ります。\
             警報は安全停止とインターロック停止で鳴り、干渉すると「ピピッ」と鳴ります。",
        )
        .small()
        .weak(),
    );
}

fn percent_slider(ui: &mut egui::Ui, value: &mut f32) {
    let mut percent = (*value * 100.0).round();
    if ui
        .add(egui::Slider::new(&mut percent, 0.0..=100.0).suffix(" %"))
        .changed()
    {
        *value = percent / 100.0;
    }
}

/// 警報が起きている間、3D ビューの下端の真ん中に (道具のパネルに重ならないように)理由とブザー停止のボタンを出す
pub fn alarm_banner(
    mut contexts: EguiContexts,
    mut sound: ResMut<SoundUi>,
    viewport: Res<ViewportRect>,
) -> Result {
    let reasons: Vec<&str> = sound.alarms().map(|r| r.label()).collect();
    if reasons.is_empty() {
        return Ok(());
    }
    let Some(rect) = viewport.rect else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let sounding = sound.alarm_sounding();
    let mut acknowledge = false;
    egui::Area::new(egui::Id::new("alarm_banner"))
        .fixed_pos(egui::pos2(rect.center().x, rect.max.y - 12.0))
        .pivot(egui::Align2::CENTER_BOTTOM)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            // 鳴っている間は赤を点滅させて目立たせる
            let blink = sounding && (ui.input(|i| i.time) * 2.0).fract() < 0.5;
            let fill = if blink {
                egui::Color32::from_rgb(200, 40, 40)
            } else {
                egui::Color32::from_rgb(120, 30, 30)
            };
            egui::Frame::new()
                .fill(fill)
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let icon = if sounding { "🔔" } else { "🔕" };
                        ui.label(
                            egui::RichText::new(format!("{icon} 警報: {}", reasons.join("・")))
                                .color(egui::Color32::WHITE)
                                .strong(),
                        );
                        if sounding
                            && ui
                                .button("ブザー停止")
                                .on_hover_text("警報の音を止める。別の異常が起きるとまた鳴る")
                                .clicked()
                        {
                            acknowledge = true;
                        }
                    });
                });
            if sounding {
                ctx.request_repaint();
            }
        });
    if acknowledge {
        sound.acknowledge();
    }
    Ok(())
}
