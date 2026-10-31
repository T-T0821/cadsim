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
//! │ タイミングチャート          (motion/ui.rs)      │
//! ├──────────────────────────────────────────────┤
//! │ ステータスバー                     (status.rs) │
//! └──────────────────────────────────────────────┘
//! ```
//!
//! 3D ビューの左下にはミッションカード、達成時は上にお祝いと紙吹雪を重ねる (playful.rs)。
//! 見た目 (ゲーム風 / シンプル) は theme.rs。
//!
//! egui のパネルは「外側から順に場所を取る」ので、1つのシステムの中で
//! 上 → 下 → 左 → 右 の順に描き、残った領域を 3D ビューにする。

mod animation;
mod menu;
pub mod modeling;
pub mod playful;
mod properties;
mod replace;
mod status;
pub mod theme;
mod tree;
mod view_cube;
pub mod widgets;

use bevy::{
    camera::{CameraOutputMode, visibility::RenderLayers},
    diagnostic::DiagnosticsStore,
    ecs::{change_detection::DetectChangesMut, system::SystemParam},
    prelude::*,
    render::render_resource::BlendState,
};
use bevy_egui::{
    EguiContexts, EguiGlobalSettings, EguiPrimaryContextPass, PrimaryEguiContext, egui,
};

use crate::animation::Animations;
use crate::camera::{CameraRequest, OrbitCamera, ViewportRect};
use crate::file_io::ImportJobs;
use crate::flow::{self, FlowUi};
use crate::gizmo::{GizmoTool, transform_gizmo_system};
use crate::history::History;
use crate::japanese_font::install_japanese_font;
use crate::model::{Model, Selection};
use crate::motion::{self, MotionUi, at_design_pose};
use crate::physics::physics_stopped;
use crate::plc::plc_active;
use crate::project::{ProjectState, show_confirm_dialog};
use crate::recording::RecordSettings;
use crate::result_export::ResultExport;
use crate::viewport::{Defeaturing, RenderStats, ViewSettings};

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<modeling::ModelingUi>()
            .add_plugins(playful::PlayfulPlugin)
            .add_systems(Startup, spawn_ui_camera)
            .add_systems(
                EguiPrimaryContextPass,
                (
                    install_japanese_font,
                    playful::apply_theme,
                    draw_ui,
                    crate::library::palette_window,
                    crate::custom_part::save_window,
                    crate::library::io_list_window,
                    crate::library::drop_on_viewport,
                    playful::draw_overlay,
                    // 動作シミュレーション・物理演算・PLC 連携で部品が動いている間はギズモで編集しない
                    transform_gizmo_system
                        .run_if(at_design_pose)
                        .run_if(physics_stopped)
                        .run_if(not(plc_active)),
                )
                    .chain(),
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
    pub motion: MotionUi<'w>,
    pub documents: ResMut<'w, crate::documents::DocumentExport>,
    pub modeling: ResMut<'w, modeling::ModelingUi>,
    pub flow: FlowUi<'w>,
    pub line: ResMut<'w, crate::line::LineState>,
    pub diagnostics: Res<'w, DiagnosticsStore>,
    pub render_stats: Res<'w, RenderStats>,
    pub playful: ResMut<'w, playful::PlayfulUi>,
    pub results: ResMut<'w, ResultExport>,
    pub defeaturing: Res<'w, Defeaturing>,
    pub recording: ResMut<'w, RecordSettings>,
    /// 標準部品の動作テストの値
    pub jog: ResMut<'w, crate::library::Jog>,
    /// 「自作部品として保存」のウィンドウ
    pub custom_dialog: ResMut<'w, crate::custom_part::SaveDialog>,
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
    // チェックボックスは値が同じでも書き込むので、終わってから本当に変わったかを見る
    // (変わった扱いのままだと、3D ビューが数千部品の表示を毎フレーム計算し直す)
    let settings_before = (*state.settings).clone();
    let settings_tick = state.settings.last_changed();

    menu::show(&mut root, &mut state);
    status::show(&mut root, &mut state);
    motion::show_panel(
        &mut root,
        &mut state.motion,
        &mut state.model,
        &state.selection,
        &mut state.camera,
        &mut state.history,
    );
    tree::show(&mut root, &mut state);
    properties::show(&mut root, &mut state);

    // 残った領域が 3D ビュー
    let view = root.available_rect_before_wrap();
    state.viewport.rect = Some(Rect::new(view.min.x, view.min.y, view.max.x, view.max.y));
    view_cube::show(&ctx, view, &mut state);
    status::show_notifications(&ctx, view, &mut state);
    animation::show(&ctx, view, &mut state);
    replace::show(&ctx, &mut state);
    modeling::show(&ctx, &mut state);
    flow::show_window(&ctx, &mut state.flow, &state.model, &state.selection);
    show_confirm_dialog(&ctx, &mut state.project);
    if *state.settings == settings_before {
        state.settings.set_last_changed(settings_tick);
    }

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
