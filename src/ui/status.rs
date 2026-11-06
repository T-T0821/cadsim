//! 下部のステータスバーと、読み込み中の表示

use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy_egui::egui;

use super::UiState;

pub fn show(root: &mut egui::Ui, state: &mut UiState) {
    egui::Panel::bottom("status_bar").show(root, |ui| {
        ui.horizontal(|ui| {
            match state.selection.node.filter(|&id| state.model.get(id).is_some()) {
                Some(id) => {
                    ui.label(format!("選択: {}", state.model.path_label(id)));
                    if let Some(face) = state.selection.face {
                        ui.weak(format!("(面 #{face})"));
                    }
                }
                None => {
                    ui.weak("クリックで選択 / Alt+左ドラッグで回転 / 中ドラッグで移動 / ホイールでズーム");
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("単位: {}", state.model.unit));
                ui.separator();
                let fps = state
                    .diagnostics
                    .get(&FrameTimeDiagnosticsPlugin::FPS)
                    .and_then(|d| d.smoothed());
                if let Some(fps) = fps {
                    let fps = crate::features::ui_check::measured(fps, 1);
                    ui.monospace(format!("{fps:>5} FPS"));
                }
                let stats = *state.render_stats;
                if stats.parts > 0 {
                    ui.separator();
                    ui.weak(format!("三角形 {}", stats.triangles))
                        .on_hover_text(format!(
                            "部品 {} 個のうち\n詳細に表示: {}\n簡略表示: {}\n小さいので省略: {}\n画面の外: {}",
                            stats.parts,
                            stats.full,
                            stats.coarse,
                            stats.hidden_small,
                            stats.outside_view
                        ));
                }
                ui.separator();
                if let Some(message) = state.jobs.messages.last() {
                    ui.weak(message);
                }
                repair_notice(ui, state);
            });
        });
    });
}

/// STEP を読み込んで形状を直したときの案内。押すたびに直した部品を順に選んで寄る
fn repair_notice(ui: &mut egui::Ui, state: &mut UiState) {
    let Some(report) = &state.repair.report else {
        return;
    };
    let message = report.message();
    let warn = !report.open.is_empty();
    if ui
        .small_button("消す")
        .on_hover_text("この案内を消す")
        .clicked()
    {
        state.repair.dismiss();
        return;
    }
    let text = egui::RichText::new(format!("🔧 {message}"));
    let text = if warn {
        text.color(ui.visuals().warn_fg_color)
    } else {
        text
    };
    if ui
        .link(text)
        .on_hover_text("押すと直した部品を選んで寄ります (押すたびに次の部品)")
        .clicked()
        && let Some(id) = state.repair.next_part(&state.model)
    {
        state.selection.select(id);
        state.selection.reveal_in_tree = true;
        state
            .camera
            .write(crate::viewport::camera::CameraRequest::FitSelection);
    }
    ui.separator();
}

/// 読み込み中のファイルを 3D ビューの上に出す
pub fn show_notifications(ctx: &egui::Context, view: egui::Rect, state: &mut UiState) {
    if state.jobs.loading.is_empty() {
        return;
    }
    egui::Area::new("loading".into())
        .fixed_pos(view.center_top() + egui::vec2(-120.0, 12.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("読み込み中: {}", state.jobs.loading.join(", ")));
                });
            });
        });
}
