//! 「AI の設定」ウィンドウ: Claude API のキーとモデルを選ぶ

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use super::AiAssistant;
use plantwright_core::assistant::claude::{DEFAULT_MODEL, MODELS};

pub fn settings_window(mut contexts: EguiContexts, mut ai: ResMut<AiAssistant>) -> Result {
    if !ai.settings_open {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let mut open = true;
    egui::Window::new("AI の設定")
        .open(&mut open)
        .default_width(420.0)
        .collapsible(false)
        .show(ctx, |ui| {
            let ai = &mut *ai;
            ui.label("タイミングチャートやラダーの自動生成などに、Anthropic の Claude API を使います。");
            ui.hyperlink_to(
                "API キーの発行 (Claude Console)",
                "https://platform.claude.com/settings/keys",
            );
            ui.add_space(4.0);
            egui::Grid::new("ai_settings").num_columns(2).show(ui, |ui| {
                ui.label("API キー");
                ui.label(ai.settings.key_status());
                ui.end_row();

                ui.label("");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut ai.draft_key)
                            .password(true)
                            .hint_text("sk-ant-…")
                            .desired_width(220.0),
                    );
                    if ui
                        .add_enabled(!ai.draft_key.trim().is_empty(), egui::Button::new("保存"))
                        .clicked()
                    {
                        ai.settings.api_key = std::mem::take(&mut ai.draft_key).trim().into();
                        ai.save();
                    }
                    if ui
                        .add_enabled(!ai.settings.api_key.is_empty(), egui::Button::new("消す"))
                        .on_hover_text("保存したキーを消す (環境変数 ANTHROPIC_API_KEY があればそれを使う)")
                        .clicked()
                    {
                        ai.settings.api_key.clear();
                        ai.save();
                    }
                });
                ui.end_row();

                ui.label("モデル");
                let current = if ai.settings.model.is_empty() {
                    DEFAULT_MODEL
                } else {
                    ai.settings.model.as_str()
                };
                let label = MODELS
                    .iter()
                    .find(|(id, _)| *id == current)
                    .map_or(current, |(_, name)| *name)
                    .to_string();
                let mut chosen = None;
                egui::ComboBox::from_id_salt("ai_model")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        for (id, name) in MODELS {
                            if ui.selectable_label(current == id, name).clicked() {
                                chosen = Some(id);
                            }
                        }
                    });
                if let Some(id) = chosen {
                    ai.settings.model = if id == DEFAULT_MODEL {
                        String::new()
                    } else {
                        id.into()
                    };
                    ai.save();
                }
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.small("キーはこのパソコンの設定フォルダ (ai.json) にだけ保存し、プロジェクトファイルには書きません。");
            ui.small("送った説明文やチャートの内容は Anthropic に送信されます。料金は API キーの持ち主に請求されます。");
            if let Some(notice) = &ai.notice {
                ui.label(notice);
            }
        });
    if !open {
        ai.settings_open = false;
    }
    Ok(())
}
