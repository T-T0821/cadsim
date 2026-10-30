//! 上部のメニューバーとツールバー

use bevy_egui::egui;

use super::UiState;
use crate::camera::{CameraRequest, StandardView};
use crate::gizmo::Tool;

pub fn show(root: &mut egui::Ui, state: &mut UiState) {
    egui::Panel::top("menu_bar").show(root, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            file_menu(ui, state);
            view_menu(ui, state);
            create_menu(ui, state);
            help_menu(ui);
        });
        ui.separator();
        toolbar(ui, state);
        ui.add_space(2.0);
    });
}

fn file_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("ファイル", |ui| {
        if ui.button("STEP を開く…  (Ctrl+O)").clicked() {
            state.jobs.request_dialog();
        }
        if ui.button("サンプルのアセンブリを開く").clicked() {
            state.jobs.open_sample();
        }
        ui.separator();
        if ui.button("すべて閉じる").clicked() {
            state.model.clear();
            state.selection.clear();
        }
        ui.separator();
        ui.weak("ウィンドウへのドラッグ&ドロップでも開けます");
    });
}

fn view_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("表示", |ui| {
        if ui.button("全体を表示  (Home)").clicked() {
            state.camera.write(CameraRequest::FitAll);
        }
        if ui.button("選択を拡大  (F)").clicked() {
            state.camera.write(CameraRequest::FitSelection);
        }
        ui.menu_button("標準ビュー", |ui| {
            for view in StandardView::ALL {
                let label = format!("{}  ({})", view.label(), view.shortcut());
                if ui.button(label).clicked() {
                    state.camera.write(CameraRequest::View(view));
                }
            }
        });
        ui.separator();
        ui.checkbox(&mut state.settings.show_edges, "稜線を表示");
        ui.checkbox(&mut state.settings.show_grid, "グリッドを表示");
        ui.checkbox(&mut state.settings.show_bounds, "選択の外接箱を表示");
        ui.checkbox(
            &mut state.settings.ghost_unselected,
            "選択以外を半透明にする",
        );
        ui.separator();
        if ui.button("すべて表示").clicked() {
            state.model.show_all();
        }
    });
}

fn create_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("作成", |ui| {
        if ui.button("箱を追加").clicked() {
            add_box(state);
        }
    });
}

fn help_menu(ui: &mut egui::Ui) {
    ui.menu_button("ヘルプ", |ui| {
        egui::Grid::new("shortcuts").striped(true).show(ui, |ui| {
            for (key, action) in [
                ("左クリック", "部品を選択"),
                ("Alt + 左ドラッグ", "回転 (オービット)"),
                ("中ドラッグ", "平行移動 (パン)"),
                ("ホイール", "ズーム"),
                ("右ドラッグ + WASD/QE", "フライ移動"),
                ("W / E / R", "移動 / 回転 / 拡大縮小ギズモ"),
                ("F / Home", "選択を拡大 / 全体を表示"),
                (
                    "1 / 3 / 7 / 0",
                    "正面 / 右側面 / 上面 / 等角 (Ctrl で反対側)",
                ),
                ("H / Alt+H", "選択を隠す / すべて表示"),
                ("Esc", "選択を解除"),
                ("Delete", "選択を削除"),
            ] {
                ui.label(key);
                ui.label(action);
                ui.end_row();
            }
        });
    });
}

fn toolbar(ui: &mut egui::Ui, state: &mut UiState) {
    ui.horizontal(|ui| {
        if ui
            .button("📂 開く")
            .on_hover_text("STEP ファイルを開く (Ctrl+O)")
            .clicked()
        {
            state.jobs.request_dialog();
        }
        ui.separator();

        for tool in [Tool::Translate, Tool::Rotate, Tool::Scale] {
            let selected = state.gizmo.tool == tool;
            if ui.selectable_label(selected, tool.label()).clicked() {
                state.gizmo.tool = tool;
            }
        }
        ui.separator();

        for view in StandardView::ALL {
            let button = ui.button(view.label()).on_hover_text(format!(
                "{}ビュー ({})",
                view.label(),
                view.shortcut()
            ));
            if button.clicked() {
                state.camera.write(CameraRequest::View(view));
            }
        }
        if ui
            .button("⛶ 全体")
            .on_hover_text("全体を表示 (Home)")
            .clicked()
        {
            state.camera.write(CameraRequest::FitAll);
        }
        ui.separator();

        ui.toggle_value(&mut state.settings.show_edges, "稜線");
        ui.toggle_value(&mut state.settings.ghost_unselected, "半透明")
            .on_hover_text("選択以外を半透明にする");
    });
}

pub fn add_box(state: &mut UiState) {
    let size = state.model.new_box_size;
    let position = state.model.free_spot(size);
    let id = state.model.add_box(size, position);
    state.selection.select(id);
    state.selection.reveal_in_tree = true;
}
