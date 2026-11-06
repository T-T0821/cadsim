//! 「モデリング」ウィンドウ: 円柱・押し出し・穴あけ・ブーリアン
//!
//! 作成メニューかプロパティの「形を編集」から開く。形状の計算は `modeling` が受け持ち、ここは入力を集めて呼ぶだけ。
//! 穴あけのタブは、3D ビューで面をクリックして置く道具 (`HoleTool`) の寸法を数値でも直せる欄。
//!
//! 末尾はプロパティに出す「調べる」欄 (断面の道具・質量と重心・形状の修復の説明)。計算は
//! `plantwright_core::modeling::{section_tool, mass, repair}`。

use bevy::math::DVec3;
use bevy_egui::egui;

use crate::features::modeling::{
    add_solid, apply_boolean, confirm_extrude, confirm_hole, node_solid,
};
use crate::ui::UiState;
use crate::ui::widgets::{round_for_display, row};
use plantwright_core::model::NodeId;
use plantwright_core::modeling::BooleanOp;
use plantwright_core::modeling::cylinder;
use plantwright_core::modeling::extrude;
use plantwright_core::modeling::mass::{MATERIALS, format_mass, material_name};
use plantwright_core::modeling::parse_outline;
use plantwright_core::modeling::record::JsonSolids;
use plantwright_core::modeling::repair::{describe, part_repair};
use plantwright_core::modeling::section_tool::SectionAxis;
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
    state.section_tool.stop();
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
    state.section_tool.stop();
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

// ---------------------------------------------------------------------------
// プロパティに出す「調べる」欄: 断面・質量と重心・形状の修復
// ---------------------------------------------------------------------------

/// これより部品の多いアセンブリは、ボタンを押すまで質量を計算しない (選んだだけで固まらないよう)
const MASS_AUTO_PARTS: usize = 100;

/// 「調べる: 断面」。`target` は選んだ部品かアセンブリ、None ならモデル全体
pub fn inspect_actions(ui: &mut egui::Ui, state: &mut UiState, target: Option<NodeId>) {
    let bounds = match target {
        Some(id) => state.model.world_bounds(id),
        None => state.model.scene_bounds(),
    };
    let Some(bounds) = bounds else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.label("調べる:");
        let active = state.section_tool.active;
        let hover = match target {
            Some(_) => "選んだ部品を平面で切って切り口を見る。矢印の先をドラッグで動かし、面をクリックでその面に平行にする",
            None => "モデル全体を平面で切って切り口を見る。矢印の先をドラッグで動かし、面をクリックでその面に平行にする",
        };
        if ui
            .selectable_label(active, "断面")
            .on_hover_text(hover)
            .clicked()
        {
            if active {
                state.section_tool.stop();
            } else {
                state.hole_tool.stop();
                state.extrude_tool.stop();
                state.gizmo.focused = false;
                state.section_tool.start(target, bounds);
            }
        }
    });
    if state.section_tool.active {
        section_controls(ui, state);
    }
}

/// 断面の向きと位置を数値でも直せる欄
fn section_controls(ui: &mut egui::Ui, state: &mut UiState) {
    let unit = state.model.unit.clone();
    let tool = &mut state.section_tool;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.weak("向き");
            for axis in SectionAxis::ALL {
                if ui
                    .selectable_label(tool.is_axis(axis), format!("{}軸", axis.label()))
                    .on_hover_text(format!("{} 軸に垂直な平面で切る", axis.label()))
                    .clicked()
                {
                    tool.set_axis(axis);
                }
            }
            if ui
                .button("⇄ 反転")
                .on_hover_text("平面の矢印の向きを逆にする")
                .clicked()
            {
                tool.flip();
            }
        });
        ui.horizontal(|ui| {
            ui.weak("位置");
            let (lo, hi) = tool.range;
            let mut offset = tool.offset;
            if ui
                .add(egui::Slider::new(&mut offset, lo..=hi).suffix(format!(" {unit}")))
                .on_hover_text("対象の中心からの距離")
                .changed()
            {
                tool.offset = offset.clamp(lo, hi);
            }
        });
        if tool.lines.is_empty() {
            ui.weak("この位置では切り口がありません");
        }
        if ui.button("断面を閉じる (Esc)").clicked() {
            tool.stop();
        }
    });
}

/// 「質量・重心」: 材質を選ぶと質量が変わる。重心は 3D に球で出る
pub fn mass_section(ui: &mut egui::Ui, state: &mut UiState, id: NodeId) {
    let unit = state.model.unit.clone();
    let parts: Vec<NodeId> = state
        .model
        .subtree(id)
        .into_iter()
        .filter(|&p| state.model.get(p).is_some_and(|n| n.kind.is_part()))
        .collect();
    if parts.is_empty() {
        state.mass.centroid = None;
        ui.weak("形を持つ部品がありません");
        return;
    }

    material_picker(ui, state, id, &parts);

    let requested_id = ui.make_persistent_id(("mass_requested", id));
    let requested = parts.len() <= MASS_AUTO_PARTS
        || ui.data(|d| d.get_temp::<bool>(requested_id).unwrap_or(false));
    if !requested {
        state.mass.centroid = None;
        ui.weak(format!(
            "部品が {} 個あるので、押したときに計算します",
            parts.len()
        ));
        if ui.button("質量を計算").clicked() {
            ui.data_mut(|d| d.insert_temp(requested_id, true));
        }
        return;
    }
    let Some(s) = state.mass.summary(&state.model, &JsonSolids, id) else {
        state.mass.centroid = None;
        ui.weak("形を持つ部品がありません");
        return;
    };
    state.mass.centroid = (s.parts > 0).then_some((id, s.centroid));
    if s.parts == 0 {
        ui.weak("閉じた形の部品が無いので計算できません");
    } else {
        egui::Grid::new("mass")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                row(ui, "質量", format_mass(s.mass_kg));
                row(
                    ui,
                    "体積",
                    format!("{} {unit}³", round_for_display(s.volume)),
                );
                row(
                    ui,
                    "表面積",
                    format!("{} {unit}²", round_for_display(s.area)),
                );
                if parts.len() > 1 {
                    row(ui, "部品", format!("{} 個の合計", s.parts));
                }
            });
        // 3 つ並ぶ値は長いので、パネルを広げないよう表の外に 2 行で出す
        ui.weak("重心 (X, Y, Z)");
        ui.label(format!(
            "{}, {}, {}",
            round_for_display(s.centroid.x),
            round_for_display(s.centroid.y),
            round_for_display(s.centroid.z)
        ));
        ui.weak("慣性モーメント (kg·m²)")
            .on_hover_text("重心を通り X / Y / Z 軸に平行な軸まわり");
        ui.label(format!(
            "{}, {}, {}",
            inertia_text(s.inertia.x),
            inertia_text(s.inertia.y),
            inertia_text(s.inertia.z)
        ));
        ui.weak("形状カーネルで正確に計算。重心は 3D に紫の球で出ます");
    }
    if !s.skipped.is_empty() {
        let names = s
            .skipped
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("、");
        let more = if s.skipped.len() > 3 {
            format!(" ほか {} 個", s.skipped.len() - 3)
        } else {
            String::new()
        };
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("閉じた形でないので計算に入れていません: {names}{more}"),
        );
    }
}

/// 慣性モーメント (kg·m²)。小さな部品でも 0 に見えないよう、小さい値は指数で書く
fn inertia_text(v: f64) -> String {
    if v == 0.0 || v.abs() >= 0.01 {
        format!("{v:.4}")
    } else {
        format!("{v:.2e}")
    }
}

/// 材質のドロップダウン。部品の密度 (物理演算と同じ値) を書きかえる。アセンブリなら中の部品をまとめて
fn material_picker(ui: &mut egui::Ui, state: &mut UiState, id: NodeId, parts: &[NodeId]) {
    let densities: Vec<f64> = parts
        .iter()
        .filter_map(|&p| state.model.get(p).map(|n| n.physics.density))
        .collect();
    let same = densities.windows(2).all(|w| w[0] == w[1]);
    let current = match (same, densities.first()) {
        (true, Some(&d)) => material_name(d)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("その他 ({d} g/cm³)")),
        _ => "部品ごとに違う".to_owned(),
    };
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.weak("材質");
        egui::ComboBox::from_id_salt(("material", id))
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (name, density) in MATERIALS {
                    let selected = same && densities.first() == Some(&density);
                    if ui
                        .selectable_label(selected, format!("{name}  {density} g/cm³"))
                        .clicked()
                    {
                        chosen = Some(density);
                    }
                }
            })
            .response
            .on_hover_text(if parts.len() > 1 {
                "中の部品すべての材質をまとめて変えます。物理演算の重さにも使われます"
            } else {
                "物理演算の重さにも使われます"
            });
    });
    if let Some(density) = chosen {
        state.history.begin_edit(&state.model, id);
        for &p in parts {
            if let Some(node) = state.model.get_mut(p) {
                node.physics.density = density;
            }
        }
        state.history.finish_edit(&state.model);
    }
}

/// 「形状の修復」: STEP の読み込みで直した部品・直せなかった部品の説明
pub fn repair_section(ui: &mut egui::Ui, state: &UiState, id: NodeId) {
    let Some(repair) = part_repair(&state.model, id) else {
        return;
    };
    let text = describe(repair);
    if repair.open > 0 {
        ui.colored_label(ui.visuals().warn_fg_color, format!("⚠ {text}"));
    } else {
        ui.label(format!("✔ {text}"));
    }
}
