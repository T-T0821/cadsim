//! 画面の UI (egui)
//!
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │ メニュー / ツールバー                (menu.rs) │
//! ├──────────┬────────────────────────┬──────────┤
//! │ アセンブリ │                        │ プロパティ │
//! │ ツリー     │       3D ビュー         │          │
//! │ (tree.rs) │      ビューキューブ ┐    │(properties│
//! │           │      (view_cube.rs) ┘   │   .rs)   │
//! ├──────────┴────────────────────────┴──────────┤
//! │ ステータスバー                     (status.rs) │
//! └──────────────────────────────────────────────┘
//! ```
//!
//! egui のパネルは「外側から順に場所を取る」ので、1つのシステムの中で
//! 上 → 下 → 左 → 右 の順に描き、残った領域を 3D ビューにする。

mod animation;
mod menu;
mod properties;
mod status;
mod tree;
mod view_cube;
mod widgets;

use bevy::{
    camera::{CameraOutputMode, visibility::RenderLayers},
    ecs::system::SystemParam,
    prelude::*,
    render::render_resource::BlendState,
};
use bevy_egui::{
    EguiContexts, EguiGlobalSettings, EguiPrimaryContextPass, PrimaryEguiContext, egui,
};

use crate::animation::Animations;
use crate::camera::{CameraRequest, OrbitCamera, ViewportRect};
use crate::file_io::ImportJobs;
use crate::gizmo::{GizmoTool, transform_gizmo_system};
use crate::history::History;
use crate::japanese_font::install_japanese_font;
use crate::model::{Model, Selection};
use crate::project::{ProjectState, show_confirm_dialog};
use crate::viewport::ViewSettings;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_ui_camera).add_systems(
            EguiPrimaryContextPass,
            (install_japanese_font, draw_ui, transform_gizmo_system).chain(),
        );
    }
}

/// UI の各部品が読み書きするもの一式
#[derive(SystemParam)]
pub struct UiState<'w> {
    pub model: ResMut<'w, Model>,
    pub selection: ResMut<'w, Selection>,
    pub jobs: ResMut<'w, ImportJobs>,
    pub animations: ResMut<'w, Animations>,
    pub project: ResMut<'w, ProjectState>,
    pub settings: ResMut<'w, ViewSettings>,
    pub gizmo: ResMut<'w, GizmoTool>,
    pub history: ResMut<'w, History>,
    pub orbit: Res<'w, OrbitCamera>,
    pub camera: MessageWriter<'w, CameraRequest>,
    pub viewport: ResMut<'w, ViewportRect>,
}

/// UI 専用のカメラ。3D カメラはパネルを除いた領域だけに描くので、
/// egui はウィンドウ全体を覆うこちらのカメラに描く。
fn spawn_ui_camera(mut commands: Commands, mut egui_settings: ResMut<EguiGlobalSettings>) {
    egui_settings.auto_create_primary_context = false;
    commands.spawn((
        PrimaryEguiContext,
        Camera2d,
        // UI 以外は何も映さない
        RenderLayers::none(),
        Camera {
            order: 1,
            output_mode: CameraOutputMode::Write {
                blend_state: Some(BlendState::ALPHA_BLENDING),
                clear_color: ClearColorConfig::None,
            },
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
    ));
}

fn draw_ui(mut contexts: EguiContexts, mut state: UiState) -> Result {
    let ctx = contexts.ctx_mut()?.clone();

    let mut root = egui::Ui::new(
        ctx.clone(),
        "root".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    menu::show(&mut root, &mut state);
    status::show(&mut root, &mut state);
    tree::show(&mut root, &mut state);
    properties::show(&mut root, &mut state);

    // 残った領域が 3D ビュー
    let view = root.available_rect_before_wrap();
    state.viewport.rect = Some(Rect::new(view.min.x, view.min.y, view.max.x, view.max.y));
    view_cube::show(&ctx, view, &mut state);
    status::show_notifications(&ctx, view, &mut state);
    animation::show(&ctx, view, &mut state);
    show_confirm_dialog(&ctx, &mut state.project);

    // パネルの上やビューキューブの上では 3D ビューのマウス操作をしない
    let pointer = ctx.input(|i| i.pointer.hover_pos());
    let on_overlay = pointer
        .and_then(|p| ctx.layer_id_at(p))
        .is_some_and(|layer| layer.order != egui::Order::Background);
    state.viewport.pointer_on_ui = match pointer {
        Some(p) => !view.contains(p) || on_overlay || ctx.egui_is_using_pointer(),
        None => true,
    };
    // ツリーの行などをクリックしてもフォーカスは移るので、入力欄 (TextEdit) のときだけ止める
    state.viewport.typing_in_ui = ctx
        .memory(|m| m.focused())
        .is_some_and(|id| egui::TextEdit::load_state(&ctx, id).is_some());
    Ok(())
}
