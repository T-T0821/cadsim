//! 取り込んだアニメーションの再生パネル (3D ビューの左下)

use bevy_egui::egui;

use crate::ui::UiState;

pub fn show(ctx: &egui::Context, view: egui::Rect, state: &mut UiState) {
    let anim = &mut *state.animations;
    if anim.clips.is_empty() {
        return;
    }
    egui::Window::new("アニメーション")
        .pivot(egui::Align2::CENTER_BOTTOM)
        .default_pos(view.center_bottom() + egui::vec2(0.0, -12.0))
        .resizable(false)
        .collapsible(true)
        .show(ctx, |ui| {
            let current = anim.current;
            egui::ComboBox::from_id_salt("clip")
                .selected_text(anim.clips[current].name.clone())
                .show_ui(ui, |ui| {
                    for (i, clip) in anim.clips.iter().enumerate() {
                        if ui.selectable_label(i == current, &clip.name).clicked() {
                            anim.current = i;
                        }
                    }
                });
            if anim.current != current {
                anim.time = 0.0;
                anim.pose_changed = true;
            }
            let duration = anim.clips[anim.current].duration;

            ui.horizontal(|ui| {
                if ui.button("⏮").on_hover_text("最初に戻す").clicked() {
                    anim.time = 0.0;
                    anim.pose_changed = true;
                }
                let label = if anim.playing {
                    "⏸ 停止"
                } else {
                    "▶ 再生"
                };
                if ui.button(label).clicked() {
                    if !anim.playing && anim.time >= duration {
                        anim.time = 0.0;
                    }
                    anim.playing = !anim.playing;
                }
                ui.checkbox(&mut anim.looping, "繰り返す");
            });
            ui.spacing_mut().slider_width = 240.0;
            let slider = egui::Slider::new(&mut anim.time, 0.0..=duration.max(0.001))
                .suffix(" 秒")
                .max_decimals(2);
            if ui.add(slider).changed() {
                anim.pose_changed = true;
            }
        });
}
