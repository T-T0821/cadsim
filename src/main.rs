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
//! - `part_mesh`   部品1つ分の表示用メッシュ (面をまとめたもの・簡略形状)
//! - `bvh`         クリック選択を速くする外接箱の木
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
//! - `modeling`    円柱・押し出し・穴あけ・ブーリアン (truck の B-rep)
//! - `bom`         部品表を作る
//! - `documents`   図面と部品表の書き出し (ダイアログ)
//! - `site`        床に敷く平面図 (DXF / PDF / 画像) と縮尺合わせ
//! - `formats`     平面図や図面などのファイル形式の読み書き (Bevy に依存しない)
//! - `conveyor`    コンベアのワーク搬送とグリッパの把持・解放
//! - `flow`        工程モデル (生成・工程・バッファ・排出) の早回しとスループット統計
//! - `stress`      負荷テスト用の数千部品のシーン
//! - `bench`       `--bench` で FPS を測って終了する
//! - `line`        ライン連携 (工程・AGV・作業者・チャート・安全停止を1つの時計で動かす、PLC の信号表)
//! - `floor`       作業者と AGV (床の上を動くもの)、床の上の幾何計算
//! - `floor_record` 安全機器・作業者・AGV の .cadsim での形
//! - `safety`      安全機器 (ライトカーテン・エリアセンサ・安全柵・非常停止) と動作範囲
//! - `preferences_file` 見た目とミッションの設定の読み書き (中身は `cadsim_core::gamify`)
//! - `physics`     物理演算 (重力・接触・摩擦、ベルトコンベア、ケーブル・ホース)
//! - `recording`   動画書き出しの設定画面と画面のキャプチャ
//! - `video`       動画のエンコード (MP4 / GIF / PNG。Bevy に依存しない)

// Windows のリリースビルドでは、起動時に黒いコンソール窓を出さない
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod animation;
mod annotation;
mod bench;
mod bom;
mod bvh;
mod camera;
mod conveyor;
mod defeature;
mod documents;
mod drawing;
mod file_io;
mod floor;
mod floor_record;
mod flow;
mod formats;
mod gizmo;
mod history;
mod interchange;
mod japanese_font;
mod line;
mod math;
mod model;
mod modeling;
mod motion;
mod part_mesh;
mod physics;
mod plc;
mod preferences_file;
mod project;
mod recording;
mod result_export;
mod safety;
mod scripting;
mod shortcuts;
mod simulation;
mod site;
mod step_import;
mod stress;
mod ui;
mod video;
mod viewport;

use bevy::{diagnostic::FrameTimeDiagnosticsPlugin, prelude::*};
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
        .add_plugins((EguiPlugin::default(), FrameTimeDiagnosticsPlugin::default()))
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
            (
                motion::MotionPlugin,
                plc::PlcPlugin,
                scripting::ScriptPlugin,
                recording::RecordingPlugin,
            ),
            (
                flow::FlowPlugin,
                conveyor::ConveyorPlugin,
                bench::BenchPlugin,
            ),
            (safety::SafetyPlugin, floor::FloorPlugin, line::LinePlugin),
            (
                site::SitePlugin,
                documents::DocumentsPlugin,
                result_export::ResultExportPlugin,
            ),
            physics::PhysicsPlugin,
        ))
        .run();
}
