//! プロパティの「シミュレーションの役割」: 部品を選ぶと「この部品を…にする」のボタンが並び、
//! 付いている役割はその場で見て、3D で調整したり外したりできる

use bevy_egui::egui;

use crate::ui::UiState;
use plantwright_core::library::catalog::units_per_mm;
use plantwright_core::model::NodeId;
use plantwright_core::part_role::assign;
use plantwright_core::part_role::{RoleKind, RoleRef};
use plantwright_core::workspace::Tool;

const WARN_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 170, 30);

pub fn properties_section(ui: &mut egui::Ui, state: &mut UiState, id: NodeId) {
    let roles = assign::roles_of(id, state.motion.chart(), state.motion.conveyors());
    egui::CollapsingHeader::new("シミュレーションの役割")
        .default_open(true)
        .show(ui, |ui| {
            for role in &roles {
                role_row(ui, state, id, *role);
            }
            if state.role_tool.active() {
                ui.colored_label(
                    egui::Color32::from_rgb(90, 160, 255),
                    "3D ビューで矢印や点をドラッグ → Enter で確定 / Esc で取り消し",
                );
            }
            ui.label(if roles.is_empty() {
                "この部品を:"
            } else {
                "役割を足す:"
            });
            ui.horizontal_wrapped(|ui| {
                for kind in RoleKind::ALL {
                    role_button(ui, state, id, kind);
                }
            });
        });
}

/// 「この部品を…にする」ボタン。押すと寸法から決めた既定値で下書きを作り、3D の道具を始める
fn role_button(ui: &mut egui::Ui, state: &mut UiState, id: NodeId, kind: RoleKind) {
    let editing = state
        .role_tool
        .draft
        .as_ref()
        .is_some_and(|d| d.existing.is_none() && d.body.kind() == kind);
    let gripper_ready =
        kind != RoleKind::Gripper || assign::moving_actuator(id, state.motion.chart()).is_some();
    let help = if gripper_ready {
        kind.help()
    } else {
        "先にこの部品をシリンダにしてください (グリッパはそのシリンダで開閉します)"
    };
    let response = ui
        .add_enabled(
            gripper_ready,
            egui::Button::new(kind.label()).selected(editing),
        )
        .on_hover_text(help)
        .on_disabled_hover_text(help);
    if !response.clicked() {
        return;
    }
    if editing {
        state.role_tool.stop();
        return;
    }
    start_new_role(state, id, kind);
}

/// 部品 `id` に役割 `kind` の下書きを置いて 3D の道具を始める (右クリックメニューからも)。
/// グリッパで、部品を動かすアクチュエータがなければ始めずに false
pub fn start_new_role(state: &mut UiState, id: NodeId, kind: RoleKind) -> bool {
    let Some(bounds) = state.model.world_bounds(id) else {
        return false;
    };
    let name = state.model.get(id).map_or("", |n| n.name.as_str());
    let Some(draft) = assign::new_draft(
        kind,
        id,
        name,
        bounds,
        units_per_mm(&state.model.unit),
        state.motion.chart(),
    ) else {
        return false;
    };
    state.hole_tool.stop();
    state.extrude_tool.stop();
    state.selection.select(id);
    state.role_tool.start(draft);
    true
}

/// 付いている役割の1行: 名前と値、足りない設定、「3D で調整」「開く」「外す」
fn role_row(ui: &mut egui::Ui, state: &mut UiState, id: NodeId, role: RoleRef) {
    let Some(body) = assign::body_of(role, state.motion.chart(), state.motion.conveyors()) else {
        return;
    };
    let issues = assign::issues(role, state.motion.chart(), state.motion.conveyors());
    let adjusting = state
        .role_tool
        .draft
        .as_ref()
        .is_some_and(|d| d.existing == Some(role));
    // 調整中は下書きの値を出す
    let shown = match (&state.role_tool.draft, adjusting) {
        (Some(d), true) => d.body.clone(),
        _ => body.clone(),
    };
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.strong(body.kind().label());
            ui.label(body.name());
        });
        ui.weak(assign::summary(&shown, &state.model.unit));
        for issue in &issues {
            ui.colored_label(WARN_COLOR, format!("! {issue}"));
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_label(adjusting, "3D で調整")
                .on_hover_text("矢印や点をドラッグして決め直す")
                .clicked()
            {
                if adjusting {
                    state.role_tool.stop();
                } else if let Some(bounds) = state.model.world_bounds(id) {
                    state.hole_tool.stop();
                    state.extrude_tool.stop();
                    state
                        .role_tool
                        .start(plantwright_core::part_role::RoleDraft {
                            part: id,
                            bounds,
                            units_per_mm: units_per_mm(&state.model.unit),
                            body,
                            existing: Some(role),
                        });
                }
            }
            if ui
                .button("詳しく")
                .on_hover_text("この設定の行をパネルで開く")
                .clicked()
            {
                open_in_panel(state, role);
            }
            if ui.button("外す").clicked() {
                if adjusting {
                    state.role_tool.stop();
                }
                let (chart, conveyors) = state.motion.chart_and_conveyors_mut();
                assign::remove(role, chart, conveyors);
            }
        });
    });
}

/// 役割の行をパネルで開く (アクチュエータ・センサーは下のタイミングチャート、ほかはワーク搬送)
fn open_in_panel(state: &mut UiState, role: RoleRef) {
    match role {
        RoleRef::Actuator(id) => state.motion.focus_actuator(id),
        RoleRef::Sensor(id) => state.motion.focus_sensor(id),
        RoleRef::Conveyor(_) | RoleRef::Source(_) | RoleRef::Sink(_) | RoleRef::Gripper(_) => {
            state.dock.set_open(Tool::Conveyor, true);
        }
    }
}
