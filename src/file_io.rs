//! ファイルを開く: ダイアログ・ドラッグ&ドロップ・コマンドライン引数・サンプル
//!
//! STEP の読み込みとテッセレーションは重いので別スレッドで行い、
//! 終わったものを毎フレーム受け取ってモデルに加える。

use std::path::{Path, PathBuf};
use std::sync::{
    Mutex,
    mpsc::{Receiver, Sender, channel},
};

use bevy::{ecs::system::NonSendMarker, prelude::*};

use crate::camera::CameraRequest;
use crate::history::History;
use crate::model::{Model, Selection};
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

/// 読み込みの結果 (ファイル名, 中身 or エラー文)
type ImportResult = (String, Result<ImportedFile, String>);

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
        self.spawn(SAMPLE_NAME.into(), || Ok(SAMPLE_STEP.to_owned()));
    }

    pub fn open_path(&mut self, path: PathBuf) {
        let name = file_name(&path);
        self.spawn(name, move || {
            std::fs::read(&path)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .map_err(|e| format!("{} を開けませんでした: {e}", path.display()))
        });
    }

    /// 別スレッドで読み込む
    fn spawn(
        &mut self,
        name: String,
        read: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) {
        self.loading.push(name.clone());
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let result = read().and_then(|text| import_step(&text, &name));
            // 受け取り側がいなくなっていたら (アプリ終了時) 何もしない
            let _ = sender.send((name, result));
        });
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// `cargo run -- 部品.step` のように渡されたファイルを開く。無ければサンプルを開く。
fn open_command_line_file(mut jobs: ResMut<ImportJobs>) {
    match std::env::args_os().nth(1) {
        Some(path) => jobs.open_path(path.into()),
        None => jobs.open_sample(),
    }
}

/// ファイル選択ダイアログ。macOS ではメインスレッドで出す必要があるので NonSendMarker を付ける。
fn open_dialog_if_requested(mut jobs: ResMut<ImportJobs>, _main_thread: NonSendMarker) {
    if !std::mem::take(&mut jobs.dialog_requested) {
        return;
    }
    let picked = rfd::FileDialog::new()
        .set_title("STEP ファイルを開く")
        .add_filter("STEP", &["step", "stp", "STEP", "STP"])
        .add_filter("すべてのファイル", &["*"])
        .pick_file();
    if let Some(path) = picked {
        jobs.open_path(path);
    }
}

fn open_dropped_files(mut drops: MessageReader<FileDragAndDrop>, mut jobs: ResMut<ImportJobs>) {
    for drop in drops.read() {
        if let FileDragAndDrop::DroppedFile { path_buf, .. } = drop {
            jobs.open_path(path_buf.clone());
        }
    }
}

/// 読み込みが終わったファイルをモデルに加え、選択して全体が見えるようにする
fn receive_imports(
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
    for (name, result) in finished {
        if let Some(i) = jobs.loading.iter().position(|n| *n == name) {
            jobs.loading.remove(i);
        }
        match result {
            Ok(file) => {
                let warnings = file.warnings.len();
                let root = model.add_imported(file.root, file.unit);
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
