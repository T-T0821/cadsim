//! 画面の UI (egui)
//!
//! ```text
//! ┌────────────────────────────────────────────────────┐
//! │ メニュー / ツールバー                      (menu.rs) │
//! ├──┬──────────┬────────────────────────┬──────────┤
//! │ツ│ アセンブリ │  道具のパネル           │ プロパティ │
//! │ー│ ツリー     │       3D ビュー         │          │
//! │ル│ (tree.rs) │      ビューキューブ ┐    │(properties│
//! │レ│           │      (view_cube.rs) ┘   │   .rs)   │
//! │ー├──────────┴────────────────────────┴──────────┤
//! │ル│ タイミングチャート          (motion/ui.rs)      │
//! ├──┴─────────────────────────────────────────────────┤
//! │ ステータスバー                           (status.rs) │
//! └────────────────────────────────────────────────────┘
//! ```
//!
//! 左端のツールレール (dock.rs) で、干渉チェック・安全・PLC 連携などの道具のパネルを開け閉めする。
//! 3D ビューの左下にはミッションカード、達成時は上にお祝いと紙吹雪を重ねる (playful.rs)。
//! 配色と文字の大きさは theme.rs。
//!
//! egui のパネルは「外側から順に場所を取る」ので、1つのシステムの中で
//! 上 → 下 → 左 → 右 の順に描き、残った領域を 3D ビューにする。

pub mod dock;
pub mod font;
mod guide;
mod licenses;
pub mod logo;
mod menu;
pub mod playful;
mod properties;
mod replace;
mod status;
pub mod theme;
mod tree;
mod tutorial;
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

use crate::document::history::History;
use crate::document::import::ImportJobs;
use crate::document::project::{ProjectState, show_confirm_dialog};
use crate::document::selection::Selection;
use crate::features::animation::Animations;
use crate::features::flow::{self, FlowUi};
use crate::features::motion::{self, MotionUi, at_design_pose};
use crate::features::physics::physics_stopped;
use crate::features::plc::plc_active;
use crate::features::result_export::ResultExport;
use crate::features::video::recording::RecordSettings;
use crate::ui::font::install_japanese_font;
use crate::viewport::camera::{CameraRequest, OrbitCamera, ViewportRect};
use crate::viewport::gizmo::{GizmoTool, transform_gizmo_system};
use crate::viewport::parts::{Defeaturing, RenderStats, ViewSettings};
use plantwright_core::model::Model;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<crate::features::modeling::panel::ModelingUi>()
            .init_resource::<dock::Dock>()
            .add_plugins((playful::PlayfulPlugin, logo::LogoPlugin))
            .add_systems(Startup, spawn_ui_camera)
            .add_systems(
                EguiPrimaryContextPass,
                (
                    install_japanese_font,
                    playful::apply_theme,
                    draw_ui,
                    crate::features::library::palette_window,
                    crate::features::custom_part::save_window,
                    crate::features::library::io_list_window,
                    crate::features::library::drop_on_viewport,
                    playful::draw_overlay,
                    tutorial::draw,
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
    pub documents: ResMut<'w, crate::features::drawing::DocumentExport>,
    pub modeling: ResMut<'w, crate::features::modeling::panel::ModelingUi>,
    pub flow: FlowUi<'w>,
    pub diagnostics: Res<'w, DiagnosticsStore>,
    pub render_stats: Res<'w, RenderStats>,
    pub playful: ResMut<'w, playful::PlayfulUi>,
    pub tutorial: ResMut<'w, tutorial::TutorialUi>,
    pub results: ResMut<'w, ResultExport>,
    pub defeaturing: Res<'w, Defeaturing>,
    pub recording: ResMut<'w, RecordSettings>,
    /// 標準部品の動作テストの値
    pub jog: ResMut<'w, crate::features::library::Jog>,
    /// 「自作部品として保存」のウィンドウ
    pub custom_dialog: ResMut<'w, crate::features::custom_part::SaveDialog>,
    pub assistant: ResMut<'w, crate::features::assistant::AiAssistant>,
    /// 開いている道具
    pub dock: ResMut<'w, dock::Dock>,
    /// 「AI に聞く」
    pub help: ResMut<'w, crate::features::assistant::help::HelpChat>,
    /// ツールレールの印 (干渉の数・安全停止)
    pub interference: Res<'w, crate::features::simulation::InterferenceCheck>,
    pub safety: Res<'w, crate::features::safety::SafetyState>,
    /// 一人称で歩く
    pub walk: ResMut<'w, crate::viewport::walk::WalkMode>,
    /// 音の設定 (ミュート・音量)
    pub sound: ResMut<'w, crate::features::sound::SoundUi>,
    /// 3D ビューの画質 (影・陰影・光のにじみ)
    pub render_quality: ResMut<'w, crate::viewport::render_quality::RenderQualityUi>,
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
    tool_rail(&mut root, &mut state);
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
    // 開いた道具のパネルは 3D ビューの左隣の欄に、タブで1つずつ出す
    dock::dock_area(&mut root, &mut state.dock);

    // 残った領域が 3D ビュー
    let view = root.available_rect_before_wrap();
    state.viewport.rect = Some(Rect::new(view.min.x, view.min.y, view.max.x, view.max.y));
    view_cube::show(&ctx, view, &mut state);
    status::show_notifications(&ctx, view, &mut state);
    crate::features::animation::panel::show(&ctx, view, &mut state);
    replace::show(&ctx, &mut state);
    licenses::show(&ctx);
    crate::features::modeling::panel::show(&ctx, &mut state);
    flow::show_window(
        &ctx,
        &mut state.flow,
        &state.model,
        &state.selection,
        &mut state.dock,
    );
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
    // 3D ビューで押してドラッグしている (視点を回す・部品を動かす) 間は、浮かぶウィンドウを薄くする
    // (ctx.input の中で ctx の別の関数を呼ぶと止まるので、押した場所を先に取り出す)
    let drag_origin = ctx.input(|i| {
        i.pointer
            .is_decidedly_dragging()
            .then(|| i.pointer.press_origin())
            .flatten()
    });
    let view_dragging = !ctx.egui_is_using_pointer()
        && drag_origin.is_some_and(|origin| {
            view.contains(origin)
                && ctx
                    .layer_id_at(origin)
                    .is_none_or(|layer| layer.order == egui::Order::Background)
        });
    dock::set_sight(
        &ctx,
        plantwright_core::workspace::PanelSight {
            see_through: state.playful.prefs.see_through_panels,
            typing: state.viewport.typing_in_ui,
            view_dragging,
            ..Default::default()
        },
    );
    Ok(())
}

/// 左端のツールレール。押した道具のパネルを開け閉めする。
fn tool_rail(root: &mut egui::Ui, state: &mut UiState) {
    use dock::{Badge, RailItem};
    use plantwright_core::workspace::Tool;

    let interfering = state
        .interference
        .report
        .pairs
        .iter()
        .filter(|p| {
            matches!(
                p.status,
                plantwright_core::simulation::interference::PairStatus::Interfering { .. }
            )
        })
        .count();
    let has_site = !state.model.site.is_empty();
    let items: Vec<RailItem> = Tool::ALL
        .into_iter()
        .map(|tool| RailItem {
            tool,
            open: state.dock.is_open(tool),
            active: state.dock.is_active(tool),
            enabled: tool != Tool::Site || has_site,
            badge: match tool {
                Tool::Interference if interfering > 0 => Some(Badge::Count(interfering)),
                Tool::Safety if state.safety.is_stopped() => Some(Badge::Alert),
                _ => None,
            },
            disabled_reason: "平面図 (DXF / PDF / 画像) か点群を開くと使えます",
        })
        .collect();
    if let Some(tool) = dock::rail(root, &items) {
        state.dock.toggle(tool);
    }
}
