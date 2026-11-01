//! plantwright: 工場の設備とラインを 3D で組んで、動かして確かめる
//!
//! 形状の「正」は truck の B-rep (f64) が持ち、Bevy は表示用の f32 メッシュと操作だけを担う。
//! コードの構成は docs/architecture.md。
//!
//! - `document`  土台: 設計データの Resource・選択・元に戻す・開く/保存・取り込み/書き出し・ダイアログ
//! - `viewport`  土台: 3D 表示・カメラ・ギズモ・クリックでの選択・画質・工場の建屋・歩行・ショートカット
//! - `ui`        土台: 画面の枠 (メニュー・ツールレール・部品の木・プロパティ・状態表示・テーマ・フォント)
//! - `features`  機能ごとのプラグイン。Bevy を使わない中身は `plantwright_core` の同じ名前のフォルダ
//! - `dev`       開発用: 負荷テストのシーン、FPS の計測、サンプルファイルの作り直し
//!
//! ここでは App を作り、プラグインを足すだけにする。プラグインの順番は画面の並び (egui のパネル) に
//! 効くので、変えるときは画面を確かめる。

// Windows のリリースビルドでは、起動時に黒いコンソール窓を出さない
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod dev;
mod document;
mod features;
mod ui;
mod viewport;

use bevy::{diagnostic::FrameTimeDiagnosticsPlugin, prelude::*};
use bevy_egui::EguiPlugin;

use document::{dialog, history, import, project, scene_asset, selection};
use features::{
    animation, annotation, assistant, conveyor, custom_part, drawing, effects, floor, flow,
    library, line, line_compose, motion, physics, plc, result_export, review, safety, scripting,
    sequence_gen, simulation, site, sound, ui_check, video,
};
use viewport::{camera, gizmo, picking, render_quality, shortcuts, walk};

fn main() {
    // `--ui-check` なら手順書ごとにアプリを起動し直して流し、結果を返して終わる
    if let Some(code) = ui_check::suite::run_if_requested() {
        std::process::exit(code);
    }
    App::new()
        // 取り込むファイルのアセットの読み元は AssetPlugin (DefaultPlugins) より前に足す
        .add_plugins(scene_asset::SceneSourcePlugin)
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "plantwright".into(),
                resolution: (1440, 900).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins((EguiPlugin::default(), FrameTimeDiagnosticsPlugin::default()))
        .insert_resource(ClearColor(Color::srgb(0.16, 0.17, 0.19)))
        .init_resource::<plantwright_core::model::Model>()
        .init_resource::<selection::Selection>()
        .init_resource::<gizmo::GizmoTool>()
        .add_plugins((
            camera::CameraPlugin,
            animation::AnimationPlugin,
            (viewport::parts::ViewportPlugin, picking::PartPickingPlugin),
            (import::FileIoPlugin, scene_asset::SceneAssetPlugin),
            project::ProjectPlugin,
            (shortcuts::ShortcutsPlugin, dialog::FileDialogPlugin),
            history::HistoryPlugin,
            ui::UiPlugin,
            simulation::SimulationPlugin,
            annotation::AnnotationPlugin,
            (
                motion::MotionPlugin,
                plc::PlcPlugin,
                scripting::ScriptPlugin,
                video::recording::RecordingPlugin,
                video::camera_path::CameraPathPlugin,
                walk::WalkPlugin,
                render_quality::RenderQualityPlugin,
            ),
            (
                flow::FlowPlugin,
                conveyor::ConveyorPlugin,
                library::LibraryPlugin,
                custom_part::CustomPartPlugin,
                dev::bench::BenchPlugin,
            ),
            (
                safety::SafetyPlugin,
                floor::FloorPlugin,
                line::LinePlugin,
                line_compose::LineComposePlugin,
            ),
            (
                site::SitePlugin,
                drawing::DocumentsPlugin,
                result_export::ResultExportPlugin,
                sound::SoundPlugin,
            ),
            (
                physics::PhysicsPlugin,
                assistant::AssistantPlugin,
                sequence_gen::SequenceGenPlugin,
                ui_check::UiCheckPlugin,
                effects::EffectsPlugin,
                review::ReviewPlugin,
            ),
        ))
        .run();
}
