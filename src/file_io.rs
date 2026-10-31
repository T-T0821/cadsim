//! ファイルを開く・書き出す: ダイアログ・ドラッグ&ドロップ・コマンドライン引数・サンプル
//!
//! 読み込み (STEP のテッセレーションなど) は重いので別スレッドで行い、
//! 終わったものを毎フレーム受け取ってモデルに加える。
//! STEP 以外 (glTF / OBJ / STL など) は `interchange` の中間表現を経由する。
//! プロジェクトファイル (.cadsim) が選ばれたときは project.rs に渡す。

use std::path::{Path, PathBuf};
use std::sync::{
    Mutex,
    mpsc::{Receiver, Sender, channel},
};

use bevy::{ecs::system::NonSendMarker, prelude::*};

use crate::animation::Animations;
use crate::camera::CameraRequest;
use crate::formats::pointcloud_read::{CLOUD_EXTENSIONS, is_cloud_file};
use crate::formats::{PLAN_EXTENSIONS, is_plan_file};
use crate::history::History;
use crate::interchange::{
    self, FORMATS, Scene, UnitOptions,
    model_io::{model_to_scene, scene_to_import},
};
use crate::model::{Model, NodeId, Selection, StepSource};
use crate::motion::{ChartSchedule, Schedule, TimingChart, chart_animation};
use crate::project::{ProjectAction, ProjectState, is_project_file};
use crate::step_import::{ImportedFile, import_step};
use crate::stress::stress_scene;

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
                    export_if_requested,
                    open_in_browser_if_requested,
                    open_dropped_files,
                    receive_imports,
                ),
            );
    }
}

/// 別スレッドで読んだもの。シーンはモデルの単位に合わせる必要があるので、受け取ってから変換する
pub enum Loaded {
    Step(Box<ImportedFile>),
    Scene(Scene),
}

impl Loaded {
    /// モデルに加えられる形にする。シーンは `unit` (モデルの単位) に合わせる
    pub fn into_imported(self, name: &str, unit: &str) -> ImportedFile {
        match self {
            Loaded::Step(file) => *file,
            Loaded::Scene(scene) => scene_to_import(&scene, name, unit),
        }
    }
}

/// 読み込みの結果 (ファイル名, 読み込み元, 中身 or エラー文)
type ImportResult = (String, StepSource, Result<Loaded, String>);

/// 何を書き出すか
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExportScope {
    All,
    Selection,
}

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
    /// メニューで「書き出し」が押された
    export_requested: Option<ExportScope>,
    /// メニューで「ブラウザ / VR で見る」が押された
    browser_requested: bool,
    /// OBJ / STL の単位と上方向 (読み込みと書き出しの両方で使う)
    pub units: UnitOptions,
    /// 平面図・点群として開くファイル (site が受け取って読む)
    pub site_files: Vec<PathBuf>,
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
            export_requested: None,
            browser_requested: false,
            units: UnitOptions::default(),
            site_files: Vec::new(),
        }
    }
}

impl ImportJobs {
    pub fn request_dialog(&mut self) {
        self.dialog_requested = true;
    }

    pub fn request_export(&mut self, scope: ExportScope) {
        self.export_requested = Some(scope);
    }

    pub fn request_browser_view(&mut self) {
        self.browser_requested = true;
    }

    pub fn open_sample(&mut self) {
        self.spawn(StepSource::Sample);
    }

    pub fn open_path(&mut self, path: PathBuf) {
        self.spawn(StepSource::File(path));
    }

    /// 負荷テスト用に、部品が `part_count` 個ほどのシーンを作る
    pub fn open_stress(&mut self, part_count: usize) {
        self.spawn(StepSource::Stress(part_count));
    }

    /// 別スレッドで読み込む
    fn spawn(&mut self, source: StepSource) {
        let name = source_name(&source);
        self.loading.push(name.clone());
        let sender = self.sender.clone();
        let units = self.units;
        std::thread::spawn(move || {
            let result = load_source(&source, &units);
            // 受け取り側がいなくなっていたら (アプリ終了時) 何もしない
            let _ = sender.send((name, source, result));
        });
    }
}

/// 読み込み元を読む。STEP 以外は拡張子で形式を選び、`interchange` で読む
pub fn load_source(source: &StepSource, units: &UnitOptions) -> Result<Loaded, String> {
    match source {
        StepSource::File(path) if !is_step(path) => {
            interchange::import_file(path, units).map(Loaded::Scene)
        }
        StepSource::Modeled(modeled) => modeled
            .to_imported()
            .map(|file| Loaded::Step(Box::new(file))),
        &StepSource::Stress(parts) => Ok(Loaded::Step(Box::new(stress_scene(parts)))),
        _ => read_step(source)
            .and_then(|text| import_step(&text, &source_name(source)))
            .map(|file| Loaded::Step(Box::new(file))),
    }
}

fn is_step(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("step") || e.eq_ignore_ascii_case("stp"))
}

/// STEP の中身を文字列で読む (サンプルは埋め込みのものを返す)
pub fn read_step(source: &StepSource) -> Result<String, String> {
    match source {
        StepSource::Sample => Ok(SAMPLE_STEP.to_owned()),
        StepSource::Stress(_) => Err("負荷テスト用シーンは STEP ではありません".into()),
        StepSource::File(path) => std::fs::read(path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .map_err(|e| format!("{} を開けませんでした: {e}", path.display())),
        StepSource::Modeled(modeled) => Err(format!("{} は STEP ではありません", modeled.name)),
    }
}

/// 読み込み元の表示名。STEP のルートノードの名前にもなる。
pub fn source_name(source: &StepSource) -> String {
    match source {
        StepSource::Sample => SAMPLE_NAME.into(),
        StepSource::Stress(parts) => format!("負荷テスト ({parts} 部品)"),
        StepSource::File(path) => file_name(path),
        StepSource::Modeled(modeled) => modeled.name.clone(),
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// `cargo run -- 部品.step` (または `line.cadsim`) のように渡されたファイルを開く。
/// 無ければサンプルを開く。`--stress 5000` なら負荷テスト用のシーンを開く
/// (`--bench` は bench.rs が読む)。
fn open_command_line_file(mut jobs: ResMut<ImportJobs>, mut project: ResMut<ProjectState>) {
    let args = CommandLine::parse();
    if let Some(count) = args.stress {
        jobs.open_stress(count);
    } else if let Some(path) = args.file {
        open_any(path, &mut jobs, &mut project);
    } else {
        jobs.open_sample();
    }
}

/// コマンドライン引数
#[derive(Default, Debug, PartialEq)]
pub struct CommandLine {
    /// 開くファイル
    pub file: Option<PathBuf>,
    /// `--stress N`: 部品 N 個の負荷テスト用シーンを開く
    pub stress: Option<usize>,
    /// `--bench 秒`: カメラを回しながら FPS を測り、結果を出して終了する
    pub bench_secs: Option<f32>,
}

impl CommandLine {
    pub fn parse() -> Self {
        Self::from_args(std::env::args_os().skip(1).map(PathBuf::from))
    }

    fn from_args(args: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut out = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--stress") => out.stress = Some(parse_next(args.next()).unwrap_or(5000)),
                Some("--bench") => out.bench_secs = Some(parse_next(args.next()).unwrap_or(10.0)),
                _ => out.file = Some(arg),
            }
        }
        out
    }
}

fn parse_next<T: std::str::FromStr>(next: Option<PathBuf>) -> Option<T> {
    next.and_then(|n| n.to_str()?.parse().ok())
}

/// 拡張子を見て、プロジェクトなら開き直し、STEP なら今のモデルに取り込む
fn open_any(path: PathBuf, jobs: &mut ImportJobs, project: &mut ProjectState) {
    if is_project_file(&path) {
        project.request(ProjectAction::OpenPath(path));
    } else if is_plan_file(&path) || is_cloud_file(&path) {
        jobs.site_files.push(path);
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
    let mut all: Vec<&str> = vec!["cadsim", "step", "stp"];
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
        .add_filter("プロジェクト", &["cadsim"]);
    for format in FORMATS.iter().filter(|f| f.import.is_some()) {
        dialog = dialog.add_filter(format.name, &with_upper_case(format.extensions));
    }
    dialog = dialog.add_filter(
        "平面図 (DXF / PDF / 画像)",
        &with_upper_case(PLAN_EXTENSIONS),
    );
    dialog = dialog.add_filter("点群 (PLY / LAS / E57)", &with_upper_case(CLOUD_EXTENSIONS));
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
    let Some(scope) = jobs.export_requested.take() else {
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
    if !std::mem::take(&mut jobs.browser_requested) {
        return;
    }
    let dir = std::env::temp_dir().join("cadsim_viewer");
    let chart = Some((&*chart, &**schedule));
    let result = std::fs::create_dir_all(&dir)
        .map_err(|e| e.to_string())
        .map(|()| dir.join("cadsim.html"))
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

/// モデルをファイルに書き出す。`roots` が空ならモデル全体。拡張子が無ければ glb にする。
/// タイミングチャートに動きがあれば、その1サイクルもアニメーションとして書き出す
pub fn export_model(
    model: &Model,
    roots: &[NodeId],
    animations: &Animations,
    chart: Option<(&TimingChart, &Schedule)>,
    path: &Path,
    units: &UnitOptions,
) -> Result<(), String> {
    let path = if path.extension().is_none() {
        path.with_extension("glb")
    } else {
        path.to_owned()
    };
    let (mut scene, index_of) = model_to_scene(model, roots);
    scene.animations = animations.to_scene(&index_of, &model.unit);
    if let Some((chart, schedule)) = chart {
        let parents: Vec<_> = scene.nodes.iter().map(|n| n.parent).collect();
        // USD のタイムラインは1本なので、チャートの動きを先頭に置いて優先する
        if let Some(motion) = chart_animation(chart, schedule, model, &index_of, &parents) {
            scene.animations.insert(0, motion);
        }
    }
    interchange::export_file(&scene, &path, units)
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
    mut animations: ResMut<Animations>,
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
        // 単位の情報を持つシーンは、いまのモデルの単位に合わせて取り込む
        let result = result.map(|loaded| loaded.into_imported(&name, &model.unit));
        match result {
            Ok(file) => {
                let warnings = file.warnings.len();
                let clips = file.animations;
                let root = model.add_imported(file.root, file.unit, source);
                if !clips.is_empty() {
                    // トラックの番号は取り込んだ木を深さ優先でたどった順 = subtree の順
                    let ids = model.subtree(root);
                    animations.add_clips(clips, &ids);
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_line() {
        let parse = |args: &[&str]| CommandLine::from_args(args.iter().map(PathBuf::from));
        assert_eq!(parse(&[]), CommandLine::default());
        assert_eq!(parse(&["a.step"]).file, Some(PathBuf::from("a.step")));
        let bench = parse(&["--stress", "2000", "--bench", "5"]);
        assert_eq!(bench.stress, Some(2000));
        assert_eq!(bench.bench_secs, Some(5.0));
    }

    #[test]
    fn non_step_files_load_through_interchange() {
        // プロジェクトを開き直すときもこの経路で glTF から形状を読み直す
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/sample_robot.glb");
        let loaded = load_source(&StepSource::File(path), &UnitOptions::default()).unwrap();
        assert!(matches!(loaded, Loaded::Scene(_)));
        let file = loaded.into_imported("sample_robot.glb", "mm");
        assert_eq!(file.root.name, "ロボット");
        assert_eq!(file.animations.len(), 1);

        let step = load_source(&StepSource::Sample, &UnitOptions::default()).unwrap();
        assert!(matches!(step, Loaded::Step(_)));
    }
}
