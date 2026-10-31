//! CAD プロトタイプ: STEP の取り込み・アセンブリツリー・箱の作成と編集
//!
//! 形状の「正」は truck の B-rep (f64) が持ち、Bevy は表示用の f32 メッシュと操作だけを担う。
//!
//! モジュールの分担:
//! - `model`       設計データ (アセンブリの木, f64) と選択状態
//! - `step_import` STEP ファイル → アセンブリの木と B-rep (Bevy に依存しない)
//! - `interchange` glTF / OBJ / STL などの読み書き (形式に依存しない中間表現を経由)
//! - `file_io`     ダイアログ・ドラッグ&ドロップ・別スレッドでの読み込み・書き出し
//! - `project`     プロジェクトファイル (.cadsim) の保存と読み込み
//! - `animation`   ファイルから取り込んだアニメーションの再生
//! - `viewport`    モデルを 3D 表示し、選択を強調する。クリックで選択
//! - `camera`      注視点を回るカメラ、標準ビュー、全体表示
//! - `gizmo`       移動・回転・拡大縮小のギズモ
//! - `history`     元に戻す・やり直し
//! - `motion`      タイミングチャート (ステップ) の編集と動作シミュレーション
//! - `shortcuts`   キーボードショートカット
//! - `ui`          メニュー・ツリー・プロパティ・ステータスバー・ビューキューブ
//! - `japanese_font` egui に埋め込みの日本語フォントを登録する
//! - `math`        回転や行列の変換
//! - `simulation`  干渉チェックと距離計測 (`SimBody` を付けた部品が対象)
//! - `annotation`  3D 上の寸法 (点間距離・辺の長さ・面間距離) と注記
//! - `drawing`     2D 図面 (上面図・正面図、寸法付き) を作る
//! - `bom`         部品表を作る
//! - `documents`   図面と部品表の書き出し (ダイアログ)
//! - `site`        床に敷く平面図 (DXF / PDF / 画像) と縮尺合わせ
//! - `formats`     平面図や図面などのファイル形式の読み書き (Bevy に依存しない)

// Windows のリリースビルドでは、起動時に黒いコンソール窓を出さない
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod animation;
mod bom;
mod annotation;
mod camera;
mod documents;
mod drawing;
mod file_io;
mod formats;
mod gizmo;
mod history;
mod interchange;
mod japanese_font;
mod math;
mod model;
mod motion;
mod project;
mod shortcuts;
mod simulation;
mod site;
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
            animation::AnimationPlugin,
            viewport::ViewportPlugin,
            file_io::FileIoPlugin,
            project::ProjectPlugin,
            shortcuts::ShortcutsPlugin,
            history::HistoryPlugin,
            ui::UiPlugin,
            simulation::SimulationPlugin,
            annotation::AnnotationPlugin,
            motion::MotionPlugin,
            site::SitePlugin,
            documents::DocumentsPlugin,
        ))
        .run();
}
