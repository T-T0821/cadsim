//! ファイルを開く: ダイアログ・ドラッグ&ドロップ・コマンドライン引数・サンプル
//!
//! STEP の読み込みとテッセレーションは重いので別スレッドで行い、
//! 終わったものを毎フレーム受け取ってモデルに加える。
//! プロジェクトファイル (.cadsim) が選ばれたときは project.rs に渡す。

use std::path::{Path, PathBuf};
use std::sync::{
    Mutex,
    mpsc::{Receiver, Sender, channel},
};

use bevy::{ecs::system::NonSendMarker, prelude::*};

use crate::camera::CameraRequest;
use crate::history::History;
use crate::model::{Model, Selection, StepSource};
use crate::project::{ProjectAction, ProjectState, is_project_file};
use crate::step_import::{ImportedFile, import_step};

/// 起動時に開くファイルを指定しなかったときに使うサンプル
const SAMPLE_STEP: &str = include_str!("../assets/sample_assembly.step");
const SAMPLE_NAME: &str = "sample_assembly.step";

pub struct FileIoPlugin;

impl Plugin for FileIoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ImportJobs>()
            .add_systems(Startup, open_command_line_file)
            .add_systems(
                Update,
                (
                    open_dialog_if_requested,
                    open_dropped_files,
                    receive_imports,
                ),
            );
    }
}

/// 読み込みの結果 (ファイル名, 読み込み元, 中身 or エラー文)
type ImportResult = (String, StepSource, Result<ImportedFile, String>);

/// 読み込み中の仕事と、ユーザーへのお知らせ
#[derive(Resource)]
pub struct ImportJobs {
    sender: Sender<ImportResult>,
    receiver: Mutex<Receiver<ImportResult>>,
    /// 読み込み中のファイル名
    pub loading: Vec<String>,
    /// 直近の読み込み結果やエラー (ステータスバーとダイアログに出す)
    pub messages: Vec<String>,
    /// メニューで「開く」が押された (ダイアログはメインスレッドで出す)
    dialog_requested: bool,
}

impl Default for ImportJobs {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver: Mutex::new(receiver),
            loading: Vec::new(),
            messages: Vec::new(),
            dialog_requested: false,
        }
    }
}

impl ImportJobs {
    pub fn request_dialog(&mut self) {
        self.dialog_requested = true;
    }

    pub fn open_sample(&mut self) {
        self.spawn(StepSource::Sample);
    }

    pub fn open_path(&mut self, path: PathBuf) {
        self.spawn(StepSource::File(path));
    }

    /// 別スレッドで読み込む
    fn spawn(&mut self, source: StepSource) {
        let name = source_name(&source);
        self.loading.push(name.clone());
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let result = read_step(&source).and_then(|text| import_step(&text, &name));
            // 受け取り側がいなくなっていたら (アプリ終了時) 何もしない
            let _ = sender.send((name, source, result));
        });
    }
}

/// STEP の中身を文字列で読む (サンプルは埋め込みのものを返す)
pub fn read_step(source: &StepSource) -> Result<String, String> {
    match source {
        StepSource::Sample => Ok(SAMPLE_STEP.to_owned()),
        StepSource::File(path) => std::fs::read(path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .map_err(|e| format!("{} を開けませんでした: {e}", path.display())),
    }
}

/// 読み込み元の表示名。STEP のルートノードの名前にもなる。
pub fn source_name(source: &StepSource) -> String {
    match source {
        StepSource::Sample => SAMPLE_NAME.into(),
        StepSource::File(path) => file_name(path),
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// `cargo run -- 部品.step` (または `line.cadsim`) のように渡されたファイルを開く。
/// 無ければサンプルを開く。
fn open_command_line_file(mut jobs: ResMut<ImportJobs>, mut project: ResMut<ProjectState>) {
    match std::env::args_os().nth(1) {
        Some(path) => open_any(path.into(), &mut jobs, &mut project),
        None => jobs.open_sample(),
    }
}

/// 拡張子を見て、プロジェクトなら開き直し、STEP なら今のモデルに取り込む
fn open_any(path: PathBuf, jobs: &mut ImportJobs, project: &mut ProjectState) {
    if is_project_file(&path) {
        project.request(ProjectAction::OpenPath(path));
    } else {
        jobs.open_path(path);
    }
}

/// ファイル選択ダイアログ。macOS ではメインスレッドで出す必要があるので NonSendMarker を付ける。
fn open_dialog_if_requested(
    mut jobs: ResMut<ImportJobs>,
    mut project: ResMut<ProjectState>,
    _main_thread: NonSendMarker,
) {
    if !std::mem::take(&mut jobs.dialog_requested) {
        return;
    }
    let picked = rfd::FileDialog::new()
        .set_title("開く")
        .add_filter(
            "プロジェクト / STEP",
            &["cadsim", "step", "stp", "STEP", "STP"],
        )
        .add_filter("STEP", &["step", "stp", "STEP", "STP"])
        .add_filter("プロジェクト", &["cadsim"])
        .add_filter("すべてのファイル", &["*"])
        .pick_file();
    if let Some(path) = picked {
        open_any(path, &mut jobs, &mut project);
    }
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

/// 読み込みが終わったファイルをモデルに加え、選択して全体が見えるようにする
pub fn receive_imports(
    mut jobs: ResMut<ImportJobs>,
    mut model: ResMut<Model>,
    mut selection: ResMut<Selection>,
    mut camera: MessageWriter<CameraRequest>,
    mut history: ResMut<History>,
) {
    let finished: Vec<ImportResult> = match jobs.receiver.lock() {
        Ok(receiver) => receiver.try_iter().collect(),
        Err(_) => return,
    };
    for (name, source, result) in finished {
        if let Some(i) = jobs.loading.iter().position(|n| *n == name) {
            jobs.loading.remove(i);
        }
        match result {
            Ok(file) => {
                let warnings = file.warnings.len();
                let root = model.add_imported(file.root, file.unit, source);
                history.record_added(&model, root, format!("{name} を読み込み"));
                let parts = model.measure(root).part_count;
                // 部品の色が見えるよう、読み込み直後は何も選ばない
                selection.clear();
                camera.write(CameraRequest::FitAll);
                let mut message = format!("{name} を読み込みました (部品 {parts} 個)");
                if warnings > 0 {
                    message += &format!("。読めなかった形状が {warnings} 件あります");
                }
                for warning in &file.warnings {
                    warn!("{warning}");
                }
                jobs.messages.push(message);
            }
            Err(err) => {
                error!("{name}: {err}");
                jobs.messages.push(format!("{name}: {err}"));
            }
        }
    }
}
