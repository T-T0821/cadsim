//! CAD プロトタイプ: STEP の取り込み・アセンブリツリー・箱の作成と編集
//!
//! 形状の「正」は truck の B-rep (f64) が持ち、Bevy は表示用の f32 メッシュと操作だけを担う。
//!
//! モジュールの分担:
//! - `model`       設計データ (アセンブリの木, f64) と選択状態
//! - `step_import` STEP ファイル → アセンブリの木と B-rep (Bevy に依存しない)
//! - `file_io`     ダイアログ・ドラッグ&ドロップ・別スレッドでの読み込み
//! - `viewport`    モデルを 3D 表示し、選択を強調する。クリックで選択
//! - `camera`      注視点を回るカメラ、標準ビュー、全体表示
//! - `gizmo`       移動・回転・拡大縮小のギズモ
//! - `history`     元に戻す・やり直し
//! - `shortcuts`   キーボードショートカット
//! - `ui`          メニュー・ツリー・プロパティ・ステータスバー・ビューキューブ
//! - `japanese_font` egui に埋め込みの日本語フォントを登録する
//! - `math`        回転や行列の変換
//! - `simulation`  干渉チェックと距離計測 (`SimBody` を付けた部品が対象)

mod camera;
mod file_io;
mod gizmo;
mod history;
mod japanese_font;
mod math;
mod model;
mod shortcuts;
mod simulation;
mod step_import;
mod ui;
mod viewport;

use bevy::prelude::*;
use bevy_egui::EguiPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "CADプロトタイプ".into(),
                resolution: (1440, 900).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .insert_resource(ClearColor(Color::srgb(0.16, 0.17, 0.19)))
        .init_resource::<model::Model>()
        .init_resource::<model::Selection>()
        .init_resource::<gizmo::GizmoTool>()
        .add_plugins((
            camera::CameraPlugin,
            viewport::ViewportPlugin,
            file_io::FileIoPlugin,
            shortcuts::ShortcutsPlugin,
            history::HistoryPlugin,
            ui::UiPlugin,
            simulation::SimulationPlugin,
        ))
        .run();
}
