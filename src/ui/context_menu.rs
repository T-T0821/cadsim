//! 3D ビューで右クリックしたときのメニュー
//!
//! 部品の上なら、プロパティの「拡大・これだけ表示・隠す・親を選択・削除」と「穴をあける」を
//! その場で選べる。何もないところなら「全体を表示・等角・箱を追加」。
//! 開く・閉じるの判定 (右ボタンを動かさずに離す) は `viewport/picking.rs` と
//! `plantwright_core::pointer`。ここは描いて、押されたらプロパティと同じことをするだけ。

use bevy_egui::egui;

use super::UiState;
use crate::viewport::camera::{CameraRequest, StandardView};
use plantwright_core::model::NodeId;

pub fn show(ctx: &egui::Context, state: &mut UiState) {
    let Some(menu) = state.overlay.menu else {
        return;
    };
    // 部品が消えていたら閉じる
    if menu.node.is_some_and(|id| state.model.get(id).is_none()) {
        state.overlay.menu = None;
        return;
    }
    let at = egui::pos2(menu.at[0], menu.at[1]);
    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let response = egui::Area::new(egui::Id::new("part_context_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_min_width(150.0);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                match menu.node {
                    Some(id) => close |= part_items(ui, state, id),
                    None => close |= view_items(ui, state),
                }
            });
        });
    // メニューの外をクリックしたら閉じる (押した場所を先に取り出す。ctx.input の中で ctx を使わない)
    let pressed_outside = ctx.input(|i| {
        i.pointer
            .any_pressed()
            .then(|| i.pointer.interact_pos())
            .flatten()
    });
    if pressed_outside.is_some_and(|p| !response.response.rect.contains(p)) {
        close = true;
    }
    if close {
        state.overlay.menu = None;
    }
}

/// 部品のメニュー。何か押したら true (閉じる)
fn part_items(ui: &mut egui::Ui, state: &mut UiState, id: NodeId) -> bool {
    let Some(node) = state.model.get(id) else {
        return true;
    };
    let name = node.name.clone();
    let visible = node.visible;
    let parent = node.parent;
    ui.weak(name);
    ui.separator();
    let mut done = false;
    if ui
        .button("🔍 拡大")
        .on_hover_text("ダブルクリックでも同じ (F)")
        .clicked()
    {
        state.selection.select(id);
        state.camera.write(CameraRequest::FitSelection);
        done = true;
    }
    if ui.button("これだけ表示").clicked() {
        state.model.isolate(id);
        done = true;
    }
    if ui
        .button(if visible { "隠す" } else { "表示する" })
        .on_hover_text("H で選択を隠す・Alt + H ですべて表示")
        .clicked()
    {
        if let Some(node) = state.model.get_mut(id) {
            node.visible = !visible;
        }
        done = true;
    }
    if ui
        .add_enabled(parent.is_some(), egui::Button::new("⬆ 親を選択"))
        .clicked()
        && let Some(parent) = parent
    {
        state.selection.select(parent);
        state.selection.reveal_in_tree = true;
        done = true;
    }
    if super::menu::part_can_be_modeled(state) {
        ui.separator();
        if ui
            .button("穴をあける")
            .on_hover_text("面をクリックして穴を置く")
            .clicked()
        {
            crate::features::modeling::panel::start_hole_tool(state);
            done = true;
        }
    }
    ui.separator();
    if ui.button("🗑 削除").clicked() {
        state.history.remove(&mut state.model, id);
        state.selection.clear();
        done = true;
    }
    done
}

/// 何もないところのメニュー
fn view_items(ui: &mut egui::Ui, state: &mut UiState) -> bool {
    let mut done = false;
    if ui.button("全体を表示").on_hover_text("Home").clicked() {
        state.camera.write(CameraRequest::FitAll);
        done = true;
    }
    if ui.button("等角ビュー").on_hover_text("0").clicked() {
        state
            .camera
            .write(CameraRequest::View(StandardView::Isometric));
        done = true;
    }
    if ui.button("すべて表示").clicked() {
        state.model.show_all();
        done = true;
    }
    ui.separator();
    if ui.button("箱を追加").clicked() {
        super::menu::add_box(state);
        done = true;
    }
    if ui
        .button("床に描いて押し出す")
        .on_hover_text("床をクリックして形を描き、上に押し出す")
        .clicked()
    {
        crate::features::modeling::panel::start_extrude_tool(state);
        done = true;
    }
    done
}
