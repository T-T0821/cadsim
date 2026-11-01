//! ファイルを選ぶ・落とす (UI 層の入力): ダイアログ、ドラッグ&ドロップ、起動時の引数
//!
//! ユーザーが選んだファイルを集めて、読み込み (`file_io`) やプロジェクト (`project`) に渡すだけにする。
//! 読み込み・書き出しの処理そのものは `file_io` と `cadsim_io` にある。

use std::path::Path;

use bevy::{ecs::system::NonSendMarker, prelude::*};

use super::command_line::CommandLine;
use crate::animation::Animations;
use crate::file_io::{ExportScope, ImportJobs, export_model, file_name, open_any};
use crate::formats::PLAN_EXTENSIONS;
use crate::formats::pointcloud_read::CLOUD_EXTENSIONS;
use crate::interchange::FORMATS;
use crate::model::{Model, NodeId, Selection};
use crate::motion::{ChartSchedule, TimingChart};
use crate::project::{OPEN_EXTENSIONS, ProjectAction, ProjectState};

pub struct FileDialogPlugin;

impl Plugin for FileDialogPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, open_command_line_file)
            .add_systems(
                Update,
                (
                    open_dialog_if_requested,
                    export_if_requested,
                    open_in_browser_if_requested,
                    open_dropped_files,
                ),
            );
    }
}

/// `cargo run -- 部品.step` (または `line.plantwright`) のように渡されたファイルを開く。
/// 無ければデモラインを開いて動かす。`--stress 5000` なら負荷テスト用のシーンを開く
/// (`--bench` は bench.rs、`--physics` は physics が読む)。
fn open_command_line_file(mut jobs: ResMut<ImportJobs>, mut project: ResMut<ProjectState>) {
    let args = CommandLine::parse();
    if args.physics {
        // 物理演算のサンプルラインは physics が作る
    } else if let Some(count) = args.stress {
        jobs.open_stress(count);
    } else if let Some(path) = args.file {
        open_any(path, &mut jobs, &mut project);
    } else if args.bench_secs.is_some() {
        // 測る FPS を前と比べられるよう、ベンチマークは前と同じサンプルの STEP で測る
        jobs.open_sample();
    } else {
        // はじめて開いた人がすぐ動きを見られるよう、デモラインを開いて再生する
        project.request(ProjectAction::OpenDemoLine { play: true });
    }
}

/// ファイル選択ダイアログ。macOS ではメインスレッドで出す必要があるので NonSendMarker を付ける。
fn open_dialog_if_requested(
    mut jobs: ResMut<ImportJobs>,
    mut project: ResMut<ProjectState>,
    _main_thread: NonSendMarker,
) {
    if let Some(target) = jobs.take_replace_request() {
        let mut all: Vec<&str> = vec!["step", "stp"];
        all.extend(
            FORMATS
                .iter()
                .filter(|f| f.import.is_some())
                .flat_map(|f| f.extensions.iter().copied()),
        );
        let picked = rfd::FileDialog::new()
            .set_title("差し替える形状を選ぶ")
            .add_filter("対応しているすべての形式", &with_upper_case(&all))
            .pick_file();
        if let Some(path) = picked {
            jobs.replace_from(path, target);
        }
    }
    if jobs.take_urdf_dialog_request() {
        let picked = rfd::FileDialog::new()
            .set_title("ロボット機種 (URDF) を読み込む")
            .add_filter("URDF", &with_upper_case(&["urdf"]))
            .add_filter("すべてのファイル", &["*"])
            .pick_file();
        if let Some(path) = picked {
            jobs.open_path(path);
        }
    }
    if !jobs.take_dialog_request() {
        return;
    }
    let mut all: Vec<&str> = OPEN_EXTENSIONS.to_vec();
    all.extend(["step", "stp", "urdf"]);
    all.extend(
        FORMATS
            .iter()
            .filter(|f| f.import.is_some())
            .flat_map(|f| f.extensions.iter().copied()),
    );
    all.extend(PLAN_EXTENSIONS);
    all.extend(CLOUD_EXTENSIONS);
    let mut dialog = rfd::FileDialog::new()
        .set_title("開く")
        .add_filter("対応しているすべての形式", &with_upper_case(&all))
        .add_filter("STEP", &with_upper_case(&["step", "stp"]))
        .add_filter("ロボット機種 (URDF)", &with_upper_case(&["urdf"]))
        .add_filter("プロジェクト", OPEN_EXTENSIONS);
    for format in FORMATS.iter().filter(|f| f.import.is_some()) {
        dialog = dialog.add_filter(format.name, &with_upper_case(format.extensions));
    }
    dialog = dialog.add_filter(
        "平面図 (DXF / PDF / 画像)",
        &with_upper_case(PLAN_EXTENSIONS),
    );
    dialog = dialog.add_filter(
        "点群 (PLY / LAS / LAZ / E57)",
        &with_upper_case(CLOUD_EXTENSIONS),
    );
    let picked = dialog.add_filter("すべてのファイル", &["*"]).pick_file();
    if let Some(path) = picked {
        open_any(path, &mut jobs, &mut project);
    }
}

/// 拡張子の大文字小文字を区別するシステムのために、大文字版も足す
fn with_upper_case(extensions: &[&str]) -> Vec<String> {
    extensions
        .iter()
        .flat_map(|e| [e.to_string(), e.to_ascii_uppercase()])
        .collect()
}

/// 書き出し先を選ばせ、モデル (と取り込んだアニメーション) を書き出す
fn export_if_requested(
    mut jobs: ResMut<ImportJobs>,
    model: Res<Model>,
    selection: Res<Selection>,
    animations: Res<Animations>,
    chart: Res<TimingChart>,
    schedule: Res<ChartSchedule>,
    _main_thread: NonSendMarker,
) {
    let Some(scope) = jobs.take_export_request() else {
        return;
    };
    let roots: Vec<NodeId> = match (scope, selection.node) {
        (ExportScope::Selection, Some(node)) => vec![node],
        _ => Vec::new(),
    };
    let default_name = roots
        .first()
        .and_then(|&r| model.get(r))
        .map_or("model".to_owned(), |n| n.name.clone());
    let mut dialog = rfd::FileDialog::new()
        .set_title("書き出し")
        .set_file_name(format!("{default_name}.glb"));
    for format in FORMATS.iter().filter(|f| f.export.is_some()) {
        dialog = dialog.add_filter(format.name, format.extensions);
    }
    let Some(path) = dialog.save_file() else {
        return;
    };
    let chart = Some((&*chart, &**schedule));
    let message = match export_model(&model, &roots, &animations, chart, &path, &jobs.units) {
        Ok(()) => format!("{} に書き出しました", file_name(&path)),
        Err(err) => {
            error!("{err}");
            format!("書き出しに失敗しました: {err}")
        }
    };
    jobs.messages.push(message);
}

/// モデル全体を Web ビューアにして一時フォルダに書き、既定のブラウザで開く。
/// WebXR に対応したブラウザなら、そこから VR ヘッドセットで実寸で見られる。
fn open_in_browser_if_requested(
    mut jobs: ResMut<ImportJobs>,
    model: Res<Model>,
    animations: Res<Animations>,
    chart: Res<TimingChart>,
    schedule: Res<ChartSchedule>,
) {
    if !jobs.take_browser_request() {
        return;
    }
    let dir = std::env::temp_dir().join("plantwright_viewer");
    let chart = Some((&*chart, &**schedule));
    let result = std::fs::create_dir_all(&dir)
        .map_err(|e| e.to_string())
        .map(|()| dir.join("plantwright.html"))
        .and_then(|path| {
            export_model(&model, &[], &animations, chart, &path, &jobs.units)?;
            open_with_default_app(&path)?;
            Ok(path)
        });
    let message = match result {
        Ok(path) => format!("ブラウザで開きました ({})", path.display()),
        Err(err) => {
            error!("{err}");
            format!("ブラウザで開けませんでした: {err}")
        }
    };
    jobs.messages.push(message);
}

/// OS の既定のアプリでファイルを開く
fn open_with_default_app(path: &Path) -> Result<(), String> {
    let mut command = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else {
        std::process::Command::new("xdg-open")
    };
    command
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn open_dropped_files(
    mut drops: MessageReader<FileDragAndDrop>,
    mut jobs: ResMut<ImportJobs>,
    mut project: ResMut<ProjectState>,
) {
    for drop in drops.read() {
        if let FileDragAndDrop::DroppedFile { path_buf, .. } = drop {
            open_any(path_buf.clone(), &mut jobs, &mut project);
        }
    }
}
