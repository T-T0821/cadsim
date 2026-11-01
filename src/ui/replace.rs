//! 形状の差し替えの結果ウィンドウ。対応がつかなかった部品を一覧で出す。

use bevy_egui::egui;

use super::UiState;
use plantwright_core::model::NodeId;

pub fn show(ctx: &egui::Context, state: &mut UiState) {
    let Some(summary) = &state.jobs.replace_summary else {
        return;
    };
    let mut open = true;
    let mut picked: Option<NodeId> = None;
    egui::Window::new("形状の差し替え")
        .open(&mut open)
        .collapsible(false)
        .default_width(360.0)
        .show(ctx, |ui| {
            ui.label(&summary.title);
            ui.label(format!(
                "対応がついた部品・アセンブリ: {} 件",
                summary.matched
            ));
            ui.separator();
            let list = |ui: &mut egui::Ui, title: &str, note: &str, items: &[(NodeId, String)]| {
                ui.strong(format!("{title} ({} 件)", items.len()));
                if items.is_empty() {
                    ui.weak("なし");
                    return None;
                }
                ui.weak(note);
                let mut clicked = None;
                egui::ScrollArea::vertical()
                    .id_salt(title)
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for (id, path) in items {
                            if ui.link(path).on_hover_text("クリックで選択").clicked() {
                                clicked = Some(*id);
                            }
                        }
                    });
                clicked
            };
            picked = picked.or(list(
                ui,
                "新しい形状に見つからなかったもの",
                "古い形状のまま残しています。不要ならツリーから削除してください。",
                &summary.unmatched,
            ));
            ui.add_space(6.0);
            picked = picked.or(list(
                ui,
                "新しく加わったもの",
                "動きの設定はまだ付いていません。",
                &summary.added,
            ));
        });
    if let Some(id) = picked
        && state.model.get(id).is_some()
    {
        state.selection.select(id);
        state.selection.reveal_in_tree = true;
    }
    if !open {
        state.jobs.replace_summary = None;
    }
}
