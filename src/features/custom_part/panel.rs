//! 自作部品の画面: 保存のウィンドウ、部品ライブラリのパレットの「自作部品」、プロパティ欄

use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use super::CustomLibrary;
use super::file::{self, LibraryEntry, SaveRequest};
use crate::document::import::ImportJobs;
use plantwright_core::custom_part::ParamTarget;
use plantwright_core::custom_part::param::{TargetOnChart, candidates};
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::motion::chart::TimingChart;

/// パレットからビューへドラッグしている自作部品
#[derive(Clone)]
pub struct CustomDrag(pub LibraryEntry);

/// 公開するかどうかを選ぶパラメータ1つ
struct ParamChoice {
    on: bool,
    label: String,
    target: ParamTarget,
}

/// 「自作部品として保存」のウィンドウの入力
#[derive(Resource, Default)]
pub struct SaveDialog {
    /// 保存する部品。None ならウィンドウを閉じている
    target: Option<NodeId>,
    name: String,
    category: String,
    description: String,
    params: Vec<ParamChoice>,
    /// 一緒に保存するアクチュエータ・センサーの数
    attached: (usize, usize),
    error: Option<String>,
}

impl SaveDialog {
    /// `id` 以下を保存するウィンドウを開く
    pub fn open(&mut self, model: &Model, chart: &TimingChart, id: NodeId) {
        let Some(node) = model.get(id) else {
            return;
        };
        let (actuators, sensors) = file::attached(model, chart, id);
        let (name, category, description) = match &node.custom_part {
            // 置いた自作部品を保存し直すときは、元の部品名を出す
            Some(c) => (c.part.clone(), String::new(), String::new()),
            None => (node.name.clone(), String::new(), String::new()),
        };
        *self = Self {
            target: Some(id),
            name,
            category,
            description,
            params: candidates(chart, &actuators, &sensors)
                .into_iter()
                .map(|target| ParamChoice {
                    on: false,
                    label: target.default_label(chart),
                    target,
                })
                .collect(),
            attached: (actuators.len(), sensors.len()),
            error: None,
        };
    }
}

/// 部品ライブラリのパレットの「自作部品」。+ は `focus` に置く。
pub fn palette_section(
    ui: &mut egui::Ui,
    library: &mut CustomLibrary,
    jobs: &mut ImportJobs,
    focus: DVec3,
) {
    egui::CollapsingHeader::new("自作部品")
        .default_open(false)
        .show(ui, |ui| {
            if library.dir.is_none() {
                ui.weak("保存先のフォルダが分かりません (ホームフォルダが見つかりません)");
                return;
            }
            if library.entries.is_empty() {
                ui.weak(
                    "まだありません。部品を選んで、プロパティの\n「自作部品」から保存できます。",
                );
            }
            let mut category: Option<&str> = None;
            let mut place = None;
            for entry in &library.entries {
                if category != Some(entry.category.as_str()) {
                    category = Some(entry.category.as_str());
                    if !entry.category.is_empty() {
                        ui.strong(&entry.category);
                    }
                }
                ui.horizontal(|ui| {
                    if ui.small_button("+").clicked() {
                        place = Some(entry.clone());
                    }
                    let id = egui::Id::new(("custom_palette", &entry.path));
                    ui.dnd_drag_source(id, CustomDrag(entry.clone()), |ui| {
                        let button = ui.add(
                            egui::Button::new(&entry.name)
                                .min_size(egui::vec2(150.0, 0.0))
                                .sense(egui::Sense::hover()),
                        );
                        if !entry.description.is_empty() {
                            button.on_hover_text(&entry.description);
                        }
                    });
                });
            }
            if let Some(entry) = place {
                library.request_place(&entry, jobs, DVec3::new(focus.x, 0.0, focus.z));
            }
            if ui.small_button("一覧を読み直す").clicked() {
                library.needs_scan = true;
            }
        });
}

/// プロパティ欄の「自作部品」。置いた自作部品ならパラメータを編集でき、
/// どの部品も自作部品として保存できる。変えたパラメータを (番号, 値) で返す。
pub fn properties_section(
    ui: &mut egui::Ui,
    model: &Model,
    chart: &TimingChart,
    dialog: &mut SaveDialog,
    id: NodeId,
) -> Option<(usize, f64)> {
    let node = model.get(id)?;
    let mut changed = None;
    if let Some(instance) = &node.custom_part {
        ui.label(format!("自作部品「{}」を置いたものです。", instance.part));
        egui::Grid::new(("custom_params", id))
            .num_columns(2)
            .show(ui, |ui| {
                for (i, param) in instance.params.iter().enumerate() {
                    ui.label(&param.label);
                    let mut value = param.target.read(chart).unwrap_or(param.value);
                    let speed = (value.abs() * 0.01).max(0.01);
                    let unit = param.target.unit(chart, &model.unit);
                    let drag = egui::DragValue::new(&mut value)
                        .speed(speed)
                        .suffix(format!(" {unit}"));
                    if ui.add(drag).changed() {
                        changed = Some((i, value));
                    }
                    ui.end_row();
                }
            });
        if instance.params.is_empty() {
            ui.weak("変えられるパラメータはありません。");
        }
        ui.weak(format!(
            "この部品のスクリプトでは、関節・信号の名前の頭「{}」を省けます。",
            instance.prefix
        ));
    } else {
        ui.weak("この部品と、付けたアクチュエータ・センサー・スクリプトを、部品ライブラリから何個でも置けるように保存します。");
    }
    if ui.button("自作部品として保存…").clicked() {
        dialog.open(model, chart, id);
    }
    changed
}

/// 「自作部品として保存」のウィンドウ
pub fn save_window(
    mut contexts: EguiContexts,
    mut dialog: ResMut<SaveDialog>,
    mut library: ResMut<CustomLibrary>,
    model: Res<Model>,
    chart: Res<TimingChart>,
    mut jobs: ResMut<ImportJobs>,
) -> Result {
    let Some(id) = dialog.target else {
        return Ok(());
    };
    if model.get(id).is_none() {
        dialog.target = None;
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let mut open = true;
    let mut save = false;
    let mut cancel = false;
    egui::Window::new("自作部品として保存")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            let dialog = &mut *dialog;
            egui::Grid::new("custom_save").num_columns(2).show(ui, |ui| {
                ui.label("部品名");
                ui.text_edit_singleline(&mut dialog.name);
                ui.end_row();
                ui.label("分類");
                ui.add(egui::TextEdit::singleline(&mut dialog.category).hint_text("例: 治具"));
                ui.end_row();
                ui.label("説明");
                ui.add(egui::TextEdit::multiline(&mut dialog.description).desired_rows(2));
                ui.end_row();
            });
            let (actuators, sensors) = dialog.attached;
            ui.label(format!(
                "アクチュエータ {actuators} 個・センサー {sensors} 個と、付けたスクリプトも一緒に保存します。"
            ));
            if !dialog.params.is_empty() {
                ui.separator();
                ui.label("置いたあとで変えられるようにする値");
                for choice in &mut dialog.params {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut choice.on, "");
                        ui.add_enabled(
                            choice.on,
                            egui::TextEdit::singleline(&mut choice.label).desired_width(220.0),
                        );
                    });
                }
            }
            let name = dialog.name.trim();
            if library.entries.iter().any(|e| e.name == name) {
                ui.colored_label(
                    egui::Color32::from_rgb(230, 170, 60),
                    "同じ名前の自作部品を置き換えます。",
                );
            }
            if let Some(e) = &dialog.error {
                ui.colored_label(egui::Color32::from_rgb(235, 100, 90), e);
            }
            ui.horizontal(|ui| {
                save = ui
                    .add_enabled(!name.is_empty(), egui::Button::new("保存"))
                    .clicked();
                cancel = ui.button("やめる").clicked();
            });
        });
    if !open || cancel {
        dialog.target = None;
        return Ok(());
    }
    if !save {
        return Ok(());
    }
    let Some(dir) = library.dir.clone() else {
        dialog.error = Some("保存先のフォルダが分かりません".into());
        return Ok(());
    };
    let request = SaveRequest {
        name: dialog.name.clone(),
        category: dialog.category.clone(),
        description: dialog.description.clone(),
        params: dialog
            .params
            .iter()
            .filter(|c| c.on)
            .map(|c| (c.label.trim().to_owned(), c.target))
            .collect(),
    };
    let result = file::build(&model, &chart, id, &request)
        .ok_or_else(|| "部品を書き出せませんでした".to_owned())
        .and_then(|part| file::save(&part, &dir));
    match result {
        Ok(path) => {
            jobs.messages.push(format!(
                "自作部品「{}」を保存しました ({})",
                request.name.trim(),
                path.display()
            ));
            library.needs_scan = true;
            dialog.target = None;
        }
        Err(e) => dialog.error = Some(e),
    }
    Ok(())
}
