//! 3D ビューに重ねる案内: ビューの下の案内バーと、カーソルの下の部品の名前
//!
//! 案内バーはどの道具を使っていても同じ場所・同じ見た目で出す。道具ごとの文は
//! [`gather_hints`] が毎フレーム集めて `plantwright_core::pointer::ViewHint` に書き、
//! [`show`] はそれを描くだけ。部品の名前は、カーソルが部品の上で少し止まったら出す
//! (`ViewOverlay::hovering`)。

use bevy::prelude::*;
use bevy_egui::egui;

use super::UiState;
use crate::features::annotation::AnnotateTool;
use crate::features::motion::panel::robot::RobotUiState;
use crate::features::simulation::measure::MeasureTool;
use crate::features::site::PlanTool;
use plantwright_core::modeling::extrude_tool::ExtrudeTool;
use plantwright_core::modeling::hole_tool::HoleTool;
use plantwright_core::modeling::section_tool::SectionTool;
use plantwright_core::part_role::RoleTool;
use plantwright_core::pointer::ViewHint;

/// 使っている道具の案内を、優先順 (穴あけ > 押し出し > 断面 > 部品の役割 > 寸法 > 計測 > 平面図 > 教示) に1つだけ出す
#[allow(clippy::too_many_arguments)]
pub fn gather_hints(
    hole: Res<HoleTool>,
    extrude: Res<ExtrudeTool>,
    section: Res<SectionTool>,
    role: Res<RoleTool>,
    annotate: Res<AnnotateTool>,
    measure: Res<MeasureTool>,
    plan: Res<PlanTool>,
    robot: Res<RobotUiState>,
    mut hint: ResMut<ViewHint>,
) {
    let next = if hole.active {
        match &hole.error {
            Some(error) => Some((error.clone(), true)),
            None => Some((hole.hint().to_owned(), false)),
        }
    } else if extrude.active {
        match &extrude.error {
            Some(error) => Some((error.clone(), true)),
            None => Some((extrude.hint().to_owned(), false)),
        }
    } else if section.active {
        Some((section.full_hint(), false))
    } else if role.active() {
        Some((role.hint(), false))
    } else {
        annotate
            .hint()
            .or_else(|| measure.hint())
            .or_else(|| plan.hint())
            .or_else(|| robot.hint())
            .map(|text| (text, false))
    };
    let wanted = match next {
        Some((text, error)) => ViewHint {
            text: Some(text),
            error,
        },
        None => ViewHint::default(),
    };
    // 変わったときだけ書く (Changed を無駄に立てない)
    if *hint != wanted {
        *hint = wanted;
    }
}

/// 案内バーと部品名を描く。`view` は 3D ビューの領域
pub fn show(ctx: &egui::Context, view: egui::Rect, state: &mut UiState) {
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("view_hints"),
        ))
        .with_clip_rect(view);

    if let Some(text) = state.hint.text.clone() {
        let background = if state.hint.error {
            egui::Color32::from_rgba_unmultiplied(120, 30, 30, 230)
        } else {
            egui::Color32::from_rgba_unmultiplied(20, 50, 90, 220)
        };
        // ビューが狭ければ折り返す
        let galley = painter.layout(
            text,
            egui::FontId::proportional(14.0),
            egui::Color32::WHITE,
            (view.width() - 48.0).max(80.0),
        );
        let size = galley.size() + egui::vec2(16.0, 10.0);
        let rect = egui::Rect::from_center_size(
            egui::pos2(view.center().x, view.max.y - 18.0 - size.y / 2.0),
            size,
        );
        painter.rect_filled(rect, 6.0, background);
        painter.galley(
            rect.min + egui::vec2(8.0, 5.0),
            galley,
            egui::Color32::WHITE,
        );
    }

    // 部品の名前: ビューの中でカーソルが部品の上に止まり、ボタンを押していないとき
    let pointer = ctx.input(|i| i.pointer.hover_pos());
    let pressing = ctx.input(|i| i.pointer.any_down());
    let on_view = pointer.is_some_and(|p| view.contains(p)) && !state.viewport.pointer_on_ui;
    let hit = (on_view && !pressing && state.overlay.menu.is_none())
        .then(|| {
            state
                .under_pointer
                .hit()
                .map(|hit| state.model.pick_target(hit.node))
        })
        .flatten();
    let at = pointer.map(|p| [p.x, p.y]).unwrap_or_default();
    let time = ctx.input(|i| i.time);
    let Some(node) = state.overlay.hovering(hit, at, time) else {
        return;
    };
    let Some(part) = state.model.get(node) else {
        return;
    };
    let text = if part.children.is_empty() {
        part.name.clone()
    } else {
        format!("{} ({} 個)", part.name, part.children.len())
    };
    let galley =
        painter.layout_no_wrap(text, egui::FontId::proportional(13.0), egui::Color32::WHITE);
    let mut rect = egui::Rect::from_min_size(
        egui::pos2(at[0] + 14.0, at[1] + 18.0),
        galley.size() + egui::vec2(12.0, 6.0),
    );
    // ビューからはみ出すなら内側へ
    let shift = (rect.max - view.max).max(egui::Vec2::ZERO);
    rect = rect.translate(-shift);
    painter.rect_filled(
        rect,
        4.0,
        egui::Color32::from_rgba_unmultiplied(30, 30, 40, 220),
    );
    painter.galley(
        rect.min + egui::vec2(6.0, 3.0),
        galley,
        egui::Color32::WHITE,
    );
}
