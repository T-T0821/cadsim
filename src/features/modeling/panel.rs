//! 「モデリング」ウィンドウ: 円柱・押し出し・穴あけ・ブーリアン
//!
//! 作成メニューかプロパティの「形を編集」から開く。形状の計算は `modeling` が受け持ち、ここは入力を集めて呼ぶだけ。
//! 穴あけのタブは、3D ビューで面をクリックして置く道具 (`HoleTool`) の寸法を数値でも直せる欄。

use bevy::math::DVec3;
use bevy_egui::egui;

use crate::features::modeling::{
    add_solid, apply_boolean, confirm_extrude, confirm_hole, node_solid,
};
use crate::ui::UiState;
use plantwright_core::model::NodeId;
use plantwright_core::modeling::BooleanOp;
use plantwright_core::modeling::cylinder;
use plantwright_core::modeling::extrude;
use plantwright_core::modeling::parse_outline;
use plantwright_core::workspace::Tool;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Cylinder,
    Extrude,
    Hole,
    Boolean,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Cylinder, Tab::Extrude, Tab::Hole, Tab::Boolean];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Cylinder => "円柱",
            Tab::Extrude => "押し出し",
            Tab::Hole => "穴あけ",
            Tab::Boolean => "ブーリアン",
        }
    }
}

#[derive(bevy::prelude::Resource)]
pub struct ModelingUi {
    pub tab: Tab,
    radius: f64,
    height: f64,
    outline: String,
    extrude_height: f64,
    boolean_op: BooleanOp,
    boolean_other: Option<NodeId>,
    error: Option<String>,
}

impl Default for ModelingUi {
    fn default() -> Self {
        Self {
            tab: Tab::Cylinder,
            radius: 50.0,
            height: 200.0,
            outline: "0,0; 400,0; 400,200; 200,300; 0,200".into(),
            extrude_height: 100.0,
            boolean_op: BooleanOp::Subtract,
            boolean_other: None,
            error: None,
        }
    }
}

impl ModelingUi {
    /// タブを選ぶ (ウィンドウを開くのは呼ぶ側が `Dock` で行う)
    pub fn open_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.error = None;
    }
}

pub fn show(ctx: &egui::Context, state: &mut UiState) {
    let Some(window) = crate::ui::dock::panel(ctx, Tool::Modeling, &state.dock) else {
        return;
    };
    let mut open = true;
    window
        .open(&mut open)
        .resizable(false)
        .default_width(320.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for tab in Tab::ALL {
                    let selected = state.modeling.tab == tab;
                    if ui.selectable_label(selected, tab.label()).clicked() {
                        state.modeling.tab = tab;
                        state.modeling.error = None;
                    }
                }
            });
            ui.separator();
            match state.modeling.tab {
                Tab::Cylinder => cylinder_tab(ui, state),
                Tab::Extrude => extrude_tab(ui, state),
                Tab::Hole => hole_tab(ui, state),
                Tab::Boolean => boolean_tab(ui, state),
            }
            if let Some(error) = &state.modeling.error {
                ui.separator();
                ui.colored_label(egui::Color32::from_rgb(240, 90, 90), error);
            }
        });
    if !open {
        state.dock.set_open(Tool::Modeling, false);
    }
}

fn number(ui: &mut egui::Ui, label: &str, value: &mut f64, unit: &str) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(value).speed(1.0).max_decimals(3));
        ui.label(unit);
    });
}

/// 作った部品を加えて選び、元に戻せるよう履歴に積む
fn finish_add(state: &mut UiState, result: Result<NodeId, String>, what: &str) {
    match result {
        Ok(id) => {
            let label = format!("{} を追加", state.model.path_label(id));
            state.history.record_added(&state.model, id, label);
            state.selection.select(id);
            state.selection.reveal_in_tree = true;
            state.modeling.error = None;
            state.jobs.messages.push(format!("{what}を作りました"));
        }
        Err(e) => state.modeling.error = Some(e),
    }
}

fn next_name(state: &UiState, base: &str) -> String {
    format!("{base} {}", state.model.roots.len() + 1)
}

fn cylinder_tab(ui: &mut egui::Ui, state: &mut UiState) {
    let unit = state.model.unit.clone();
    ui.label("床に立つ円柱を作ります (下の面の中心が置き場所)");
    number(ui, "半径", &mut state.modeling.radius, &unit);
    number(ui, "高さ", &mut state.modeling.height, &unit);
    if ui.button("円柱を追加").clicked() {
        let m = &state.modeling;
        let (r, h) = (m.radius, m.height);
        let position = state.model.free_spot([2.0 * r, 0.0, 2.0 * r]);
        let name = next_name(state, "円柱");
        let result = cylinder(r, h).and_then(|solid| {
            add_solid(
                &mut state.model,
                name,
                &solid,
                DVec3::new(position[0], 0.0, position[2]),
            )
        });
        finish_add(state, result, "円柱");
    }
}

fn extrude_tab(ui: &mut egui::Ui, state: &mut UiState) {
    let unit = state.model.unit.clone();
    if state.extrude_tool.active {
        extrude_tool_section(ui, state, &unit);
        return;
    }
    ui.label("3D ビューの床をクリックして形を描き、上に押し出します");
    if ui
        .button("床に描いて押し出す")
        .on_hover_text(
            "床をクリックして頂点を置き、最初の点をクリックで閉じ、上の点をドラッグで高さ",
        )
        .clicked()
    {
        start_extrude_tool(state);
        return;
    }
    ui.separator();
    ui.label(
        "数値で入れる: 上から見た形の頂点 (x, y) を「;」か改行で区切って入れ、上に押し出します",
    );
    ui.weak("y は上面図で上向き (奥, -Z) です");
    ui.add(
        egui::TextEdit::multiline(&mut state.modeling.outline)
            .desired_rows(3)
            .desired_width(f32::INFINITY),
    );
    number(ui, "高さ", &mut state.modeling.extrude_height, &unit);
    if ui.button("押し出して追加").clicked() {
        let height = state.modeling.extrude_height;
        let outline = parse_outline(&state.modeling.outline);
        let name = next_name(state, "押し出し");
        let position = state.model.free_spot([0.0; 3]);
        let result = outline.and_then(|o| extrude(&o, height)).and_then(|solid| {
            add_solid(
                &mut state.model,
                name,
                &solid,
                DVec3::new(position[0], 0.0, position[2]),
            )
        });
        finish_add(state, result, "押し出しの形");
    }
}

/// 押し出しの道具を使っている間のパネル: 案内、高さの数値、確定・やめる
fn extrude_tool_section(ui: &mut egui::Ui, state: &mut UiState, unit: &str) {
    ui.weak(state.extrude_tool.hint());
    ui.label(state.extrude_tool.summary());
    if state.extrude_tool.closed {
        ui.horizontal(|ui| {
            ui.label("高さ");
            let mut height = state.extrude_tool.height;
            if ui
                .add(
                    egui::DragValue::new(&mut height)
                        .speed(0.5)
                        .range(0.001..=f64::MAX)
                        .max_decimals(3),
                )
                .changed()
            {
                state.extrude_tool.set_height(height);
            }
            ui.label(unit);
        });
    }
    ui.horizontal(|ui| {
        if state.extrude_tool.closed {
            if ui.button("✔ 押し出す").on_hover_text("Enter").clicked() {
                confirm_extrude(
                    &mut state.extrude_tool,
                    &mut state.model,
                    &mut state.history,
                    &mut state.selection,
                    &mut state.jobs,
                );
            }
        } else if ui
            .add_enabled(
                state.extrude_tool.points.len() >= 3,
                egui::Button::new("形を閉じる"),
            )
            .on_hover_text("Enter か、最初の点をクリック")
            .clicked()
        {
            state.extrude_tool.close();
        }
        if ui.button("やめる").on_hover_text("Esc").clicked() {
            state.extrude_tool.stop();
        }
    });
    if let Some(e) = &state.extrude_tool.error {
        ui.colored_label(egui::Color32::from_rgb(240, 90, 90), e);
    }
}

/// 押し出しの道具を始める (床の高さは 0)
pub fn start_extrude_tool(state: &mut UiState) {
    state.extrude_tool.start(0.0);
    state.hole_tool.stop();
    state.selection.clear();
    state.gizmo.focused = false;
    state.modeling.open_tab(Tab::Extrude);
    state.dock.set_open(Tool::Modeling, true);
}

fn selected_part(state: &UiState) -> Option<NodeId> {
    state
        .selection
        .node
        .filter(|&id| state.model.get(id).is_some_and(|n| n.kind.is_part()))
}

/// 穴あけ: 3D ビューで面をクリックして置く。ここでは道具の開始と、直径・深さの数値での修正
fn hole_tab(ui: &mut egui::Ui, state: &mut UiState) {
    let unit = state.model.unit.clone();
    let Some(id) = selected_part(state) else {
        state.hole_tool.stop();
        ui.label("穴をあける部品 (箱か、ここで作った部品) をクリックして選んでください");
        return;
    };
    if let Err(e) = node_solid(&state.model, id) {
        state.hole_tool.stop();
        ui.label(e);
        return;
    }
    ui.label(format!("対象: {}", state.model.path_label(id)));
    if !state.hole_tool.active {
        ui.label("3D ビューで面をクリックすると、そこが穴の入口になります");
        if ui.button("面をクリックして穴をあける").clicked() {
            start_hole_tool(state);
        }
        return;
    }
    ui.weak(state.hole_tool.hint());
    if let Some(draft) = state.hole_tool.draft.as_mut() {
        ui.horizontal(|ui| {
            ui.label("直径");
            ui.add(
                egui::DragValue::new(&mut draft.diameter)
                    .speed(0.5)
                    .range(0.001..=f64::MAX)
                    .max_decimals(3),
            );
            ui.label(&unit);
        });
        ui.horizontal(|ui| {
            ui.label("深さ");
            let mut depth = draft.depth.unwrap_or(0.0);
            if ui
                .add(
                    egui::DragValue::new(&mut depth)
                        .speed(0.5)
                        .range(0.0..=f64::MAX)
                        .max_decimals(3),
                )
                .changed()
            {
                draft.set_depth(depth);
            }
            ui.label(&unit);
            ui.weak(if draft.depth.is_none() {
                "(貫通)"
            } else {
                "(0 で貫通)"
            });
        });
    }
    ui.horizontal(|ui| {
        let placed = state.hole_tool.draft.is_some();
        if ui
            .add_enabled(placed, egui::Button::new("✔ 穴をあける"))
            .on_hover_text("Enter")
            .clicked()
        {
            confirm_hole(
                &mut state.hole_tool,
                &mut state.model,
                &mut state.history,
                &mut state.jobs,
            );
        }
        if ui.button("やめる").on_hover_text("Esc").clicked() {
            state.hole_tool.stop();
        }
    });
    if let Some(e) = &state.hole_tool.error {
        ui.colored_label(egui::Color32::from_rgb(240, 90, 90), e);
    }
}

/// 穴あけの道具を始める (ギズモがクリックを横取りしないよう、ギズモの操作中の印も消す)
pub fn start_hole_tool(state: &mut UiState) {
    state.hole_tool.start();
    state.selection.face = None;
    state.gizmo.focused = false;
    state.modeling.open_tab(Tab::Hole);
    state.dock.set_open(Tool::Modeling, true);
}

fn boolean_tab(ui: &mut egui::Ui, state: &mut UiState) {
    let Some(a) = selected_part(state) else {
        ui.label("1つ目の部品 (箱か、ここで作った部品) を選んでください");
        return;
    };
    if let Err(e) = node_solid(&state.model, a) {
        ui.label(e);
        return;
    }
    ui.label(format!("1つ目 (結果が入る): {}", state.model.path_label(a)));
    // 演算できる部品の一覧 (選択中のもの以外)
    let candidates: Vec<(NodeId, String)> = state
        .model
        .roots
        .iter()
        .flat_map(|&r| state.model.subtree(r))
        .filter(|&id| id != a && node_solid(&state.model, id).is_ok())
        .map(|id| (id, state.model.path_label(id)))
        .collect();
    if candidates.is_empty() {
        ui.label("2つ目にできる部品 (箱か、ここで作った部品) がありません");
        return;
    }
    let m = &mut state.modeling;
    if m.boolean_other
        .is_none_or(|b| !candidates.iter().any(|(id, _)| *id == b))
    {
        m.boolean_other = Some(candidates[0].0);
    }
    let selected_label = candidates
        .iter()
        .find(|(id, _)| Some(*id) == m.boolean_other)
        .map(|(_, l)| l.clone())
        .unwrap_or_default();
    egui::ComboBox::from_label("2つ目 (消える)")
        .selected_text(selected_label)
        .width(220.0)
        .show_ui(ui, |ui| {
            for (id, label) in &candidates {
                ui.selectable_value(&mut m.boolean_other, Some(*id), label);
            }
        });
    for op in [BooleanOp::Union, BooleanOp::Subtract, BooleanOp::Intersect] {
        ui.radio_value(&mut m.boolean_op, op, op.label());
    }
    if ui.button("演算する").clicked()
        && let Some(b) = m.boolean_other
    {
        let op = m.boolean_op;
        match apply_boolean(&mut state.model, &mut state.history, a, b, op) {
            Ok(()) => {
                state.modeling.error = None;
                state.modeling.boolean_other = None;
                state
                    .jobs
                    .messages
                    .push(format!("{}をしました", op.label()));
            }
            Err(e) => state.modeling.error = Some(e),
        }
    }
}
