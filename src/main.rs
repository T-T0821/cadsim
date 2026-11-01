//! plantwright: STEP の取り込み・アセンブリツリー・箱の作成と編集
//!
//! 形状の「正」は truck の B-rep (f64) が持ち、Bevy は表示用の f32 メッシュと操作だけを担う。
//!
//! モジュールの分担:
//! - `model`       設計データ (アセンブリの木, f64) と選択状態
//! - `step_import` STEP ファイル → アセンブリの木と B-rep (Bevy に依存しない)
//! - `interchange` glTF / OBJ / STL などの読み書き (形式に依存しない中間表現を経由)
//! - `file_io`     読み込み・書き出し、形状の差し替え (URDF などは別スレッドで読む)
//! - `scene_asset` STEP / glTF / OBJ / STL / USD を Bevy のアセットとして読み、ファイルの更新で読み直す
//! - `project`     プロジェクトファイル (.plantwright) の保存と読み込み
//! - `animation`   ファイルから取り込んだアニメーションの再生
//! - `view`        3D 表示: `viewport` (表示・選択の強調・クリック選択)、`camera`、`gizmo`、`recording` (動画書き出しの画面とキャプチャ)、`walk` (一人称で歩く)
//! - `input`       入力: `shortcuts` (キー)、`file_dialog` (ダイアログ・ドラッグ&ドロップ)、`command_line` (起動時の引数)
//! - `part_mesh`   部品1つ分の表示用メッシュ (面をまとめたもの・簡略形状)
//! - `bvh`         クリック選択を速くする外接箱の木
//! - `history`     元に戻す・やり直し
//! - `motion`      タイミングチャート (ステップ) の編集と動作シミュレーション
//! - `ui`          メニュー・ツリー・プロパティ・ステータスバー・ビューキューブ
//! - `japanese_font` egui に埋め込みの日本語フォントを登録する
//! - `math`        回転や行列の変換
//! - `simulation`  干渉チェックと距離計測 (`SimBody` を付けた部品が対象)
//! - `library`     標準部品 (シリンダ・コンベア・センサー・シーケンサーなど) のパレットと形状
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
//! - `line_compose` 言葉でラインを組む (文から設備の配置とタイミングチャートの下書きを作る。中身は `cadsim_core::line_draft`)
//! - `floor`       作業者と AGV (床の上を動くもの)、床の上の幾何計算
//! - `floor_record` 安全機器・作業者・AGV の .plantwright での形
//! - `safety`      安全機器 (ライトカーテン・エリアセンサ・安全柵・非常停止) と動作範囲
//! - `preferences_file` 見た目とミッションの設定の読み書き (中身は `cadsim_core::gamify`)
//! - `review`      複数人で同じ工場をレビューする (互いの視点・指さし・チャット。中身は `cadsim_core::review`)
//! - `physics`     物理演算 (重力・接触・摩擦、ベルトコンベア、ケーブル・ホース)
//! - `video`       動画のエンコード (MP4 / GIF / PNG。Bevy に依存しない)
//! - `effects`     状態のエフェクト (詰まり・故障・安全停止の光と煙、溶接と干渉の火花。判断は `cadsim_core::effects`)
//! - `sound`       設備の稼働音・エアの排気音・停止の警報 (波形と鳴らし方の判断は `cadsim_core::sound`)
//! - `ui_check`    画面での動作確認の自動化 (`--ui-check`。手順書どおりに操作して画面を撮り、基準画像と比べる)

// Windows のリリースビルドでは、起動時に黒いコンソール窓を出さない
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod animation;
mod annotation;
mod assistant;
mod bench;
mod bom;
mod bvh;
mod conveyor;
mod custom_part;
mod defeature;
mod documents;
mod drawing;
mod effects;
mod file_io;
mod floor;
mod floor_record;
mod flow;
mod formats;
mod history;
mod input;
mod interchange;
mod japanese_font;
mod library;
mod line;
mod line_compose;
mod math;
mod model;
mod modeling;
mod motion;
mod part_mesh;
mod physics;
mod plc;
mod preferences_file;
mod project;
mod result_export;
mod review;
mod safety;
mod scene_asset;
mod scripting;
mod sequence_gen;
mod simulation;
mod site;
mod sound;
mod step_import;
mod stress;
mod ui;
mod ui_check;
mod video;
mod view;

use bevy::{diagnostic::FrameTimeDiagnosticsPlugin, prelude::*};
use bevy_egui::EguiPlugin;
use view::{camera, camera_path, gizmo, recording, render_quality, viewport, walk};

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
        .init_resource::<model::Model>()
        .init_resource::<model::Selection>()
        .init_resource::<gizmo::GizmoTool>()
        .add_plugins((
            camera::CameraPlugin,
            animation::AnimationPlugin,
            (viewport::ViewportPlugin, view::picking::PartPickingPlugin),
            (file_io::FileIoPlugin, scene_asset::SceneAssetPlugin),
            project::ProjectPlugin,
            input::InputPlugin,
            history::HistoryPlugin,
            ui::UiPlugin,
            simulation::SimulationPlugin,
            annotation::AnnotationPlugin,
            (
                motion::MotionPlugin,
                plc::PlcPlugin,
                scripting::ScriptPlugin,
                recording::RecordingPlugin,
                camera_path::CameraPathPlugin,
                walk::WalkPlugin,
                render_quality::RenderQualityPlugin,
            ),
            (
                flow::FlowPlugin,
                conveyor::ConveyorPlugin,
                library::LibraryPlugin,
                custom_part::CustomPartPlugin,
                bench::BenchPlugin,
            ),
            (
                safety::SafetyPlugin,
                floor::FloorPlugin,
                line::LinePlugin,
                line_compose::LineComposePlugin,
            ),
            (
                site::SitePlugin,
                documents::DocumentsPlugin,
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
