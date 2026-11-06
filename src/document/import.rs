//! ファイルを開く・書き出す: 別スレッドでの読み込み、モデルへの追加、形状の差し替え、書き出し
//!
//! どのファイルを開くか (ダイアログ・ドラッグ&ドロップ・起動時の引数) は `input::file_dialog` が集めて、
//! [`ImportJobs`] に頼む。
//!
//! 読み込み (STEP のテッセレーションなど) は重いので画面を止めずに行い、
//! 終わったものを毎フレーム受け取ってモデルに加える。ファイル 1つで読める形式 (STEP / glTF / OBJ /
//! STL / USD) は Bevy のアセットとして読み (`scene_asset`。ファイルを直すと開いたまま読み直す)、
//! それ以外 (URDF・サンプル・負荷テスト・作成メニューの形状) は別スレッドで読む。
//! STEP 以外 (glTF / OBJ / STL など) は `interchange` の中間表現を経由する。
//! プロジェクトファイル (.plantwright) が選ばれたときは project.rs に渡す。

use std::path::{Path, PathBuf};
use std::sync::{
    Mutex,
    mpsc::{Receiver, Sender, channel},
};

use bevy::prelude::*;

use crate::dev::stress::stress_scene;
use crate::document::history::History;
use crate::document::project::{ProjectAction, ProjectState, is_project_file};
use crate::document::selection::Selection;
use crate::features::animation::Animations;
use crate::features::motion::chart_animation;
use crate::viewport::camera::CameraRequest;
use plantwright_core::formats;
use plantwright_core::formats::Scene;
use plantwright_core::formats::UnitOptions;
use plantwright_core::formats::model_io::{model_to_scene, scene_to_import};
use plantwright_core::formats::step::ImportedFile;
use plantwright_core::formats::step::import_step;
use plantwright_core::history::ModelChange;
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::model::StepSource;
use plantwright_core::motion::chart::Schedule;
use plantwright_core::motion::chart::TimingChart;
use plantwright_core::motion::robot_import::RobotDescription;
use plantwright_core::motion::urdf::{self, UrdfImport, is_urdf_file};
use plantwright_core::site::plan_file::is_plan_file;
use plantwright_core::site::pointcloud_read::is_cloud_file;

/// 同梱の UR5e の URDF (Universal Robots の ROS の記述、BSD 3-Clause) と見た目のメッシュ
const URDF_SAMPLE_FILES: [(&str, &[u8]); 9] = [
    (
        URDF_SAMPLE,
        include_bytes!("../../samples/real_robot/ur5e_urdf/ur5e.urdf"),
    ),
    (
        "base.glb",
        include_bytes!("../../samples/real_robot/ur5e_urdf/base.glb"),
    ),
    (
        "shoulder.glb",
        include_bytes!("../../samples/real_robot/ur5e_urdf/shoulder.glb"),
    ),
    (
        "upperarm.glb",
        include_bytes!("../../samples/real_robot/ur5e_urdf/upperarm.glb"),
    ),
    (
        "forearm.glb",
        include_bytes!("../../samples/real_robot/ur5e_urdf/forearm.glb"),
    ),
    (
        "wrist1.glb",
        include_bytes!("../../samples/real_robot/ur5e_urdf/wrist1.glb"),
    ),
    (
        "wrist2.glb",
        include_bytes!("../../samples/real_robot/ur5e_urdf/wrist2.glb"),
    ),
    (
        "wrist3.glb",
        include_bytes!("../../samples/real_robot/ur5e_urdf/wrist3.glb"),
    ),
    (
        "LICENSE-UR5e.txt",
        include_bytes!("../../samples/real_robot/LICENSE-UR5e.txt"),
    ),
];
const URDF_SAMPLE: &str = "ur5e.urdf";

/// 同梱の URDF を書き出すフォルダ。設定ファイルと同じ場所の `samples/ur5e_urdf`
fn urdf_sample_dir() -> PathBuf {
    plantwright_core::preferences::default_path()
        .and_then(|p| p.parent().map(|dir| dir.join("samples").join("ur5e_urdf")))
        .unwrap_or_else(|| std::env::temp_dir().join("plantwright").join("ur5e_urdf"))
}

/// 同梱の URDF を `dir` に書き出し、URDF のパスを返す。中身が同じファイルは書き直さない
fn write_urdf_sample(dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    for (name, bytes) in URDF_SAMPLE_FILES {
        let path = dir.join(name);
        if std::fs::read(&path).ok().as_deref() != Some(bytes) {
            std::fs::write(&path, bytes)?;
        }
    }
    Ok(dir.join(URDF_SAMPLE))
}

/// 起動時に開くファイルを指定しなかったときに使うサンプル
const SAMPLE_STEP: &str = include_str!("../../assets/sample_assembly.step");

pub struct FileIoPlugin;

impl Plugin for FileIoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ImportJobs>()
            .add_systems(Update, receive_imports);
    }
}

/// 別スレッドで読んだもの。シーンはモデルの単位に合わせる必要があるので、受け取ってから変換する
pub enum Loaded {
    Step(Box<ImportedFile>),
    Scene(Scene),
    /// URDF のロボット (形状と関節の木)
    Robot(Box<UrdfImport>),
}

impl Loaded {
    /// モデルに加えられる形にする。シーンは `unit` (モデルの単位) に合わせる
    pub fn into_imported(self, name: &str, unit: &str) -> ImportedFile {
        match self {
            Loaded::Step(file) => *file,
            Loaded::Scene(scene) => scene_to_import(&scene, name, unit),
            Loaded::Robot(robot) => {
                let mut file = scene_to_import(&robot.scene, name, unit);
                file.warnings = robot.warnings;
                file
            }
        }
    }

    /// URDF なら、ロボットの機種 (関節の木)
    pub fn robot(&self) -> Option<&RobotDescription> {
        match self {
            Loaded::Robot(robot) => Some(&robot.robot),
            _ => None,
        }
    }
}

/// 読み込んだ URDF のロボット。モデルに加えたあと、motion がチャートに関節を足す
pub struct ImportedRobot {
    /// モデルに加えたロボットのいちばん上のノード
    pub root: NodeId,
    pub description: RobotDescription,
}

/// 読んだものをどうするか
#[derive(Clone, Debug, PartialEq)]
pub enum ImportTarget {
    /// モデルに加える
    Add,
    /// このノードの形状を差し替える (メニューの「形状を差し替え」)
    Replace(NodeId),
    /// ファイルが更新されたので、このファイルから取り込んだ木を読み直す
    Refresh(PathBuf),
}

/// 読み込みの結果 (ファイル名, 読み込み元, どうするか, 中身 or エラー文)
pub type ImportResult = (String, StepSource, ImportTarget, Result<Loaded, String>);

/// Bevy のアセットとして読むファイル (`scene_asset` が受け取って読み始める)
pub struct AssetRequest {
    pub name: String,
    pub path: PathBuf,
    pub target: ImportTarget,
    /// 頼んだときの OBJ / STL の単位と上方向
    pub units: UnitOptions,
}

/// 形状の差し替えの結果。対応がつかなかったものを画面に一覧で出す。
pub struct ReplaceSummary {
    pub title: String,
    pub matched: usize,
    /// 新しい形状に見つからず、古い形状のまま残した部品 (ツリーでのパス)
    pub unmatched: Vec<(NodeId, String)>,
    /// 新しく加わった部品
    pub added: Vec<(NodeId, String)>,
}

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
    /// メニューで「形状を差し替え」が押された (差し替えるノード)
    replace_requested: Option<NodeId>,
    /// 直近の形状の差し替えの結果 (閉じるまで表示する)
    pub replace_summary: Option<ReplaceSummary>,
    /// メニューで「ブラウザ / VR で見る」が押された
    browser_requested: bool,
    /// OBJ / STL の単位と上方向 (読み込みと書き出しの両方で使う)
    pub units: UnitOptions,
    /// 平面図・点群として開くファイル (site が受け取って読む)
    pub site_files: Vec<PathBuf>,
    /// 読み込んだばかりの URDF のロボット (motion が関節をチャートに足す)
    pub robots: Vec<ImportedRobot>,
    /// 読み込んだばかりの木のルート (modeling が形状の修復の案内を出す)
    pub imported: Vec<NodeId>,
    /// 「URDF を読み込む」が押された (URDF だけを選ぶダイアログを出す)
    urdf_dialog_requested: bool,
    /// Bevy のアセットとして読むファイル (`scene_asset` が取り出す)
    pub asset_requests: Vec<AssetRequest>,
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
            replace_requested: None,
            replace_summary: None,
            browser_requested: false,
            units: UnitOptions::default(),
            site_files: Vec::new(),
            imported: Vec::new(),
            robots: Vec::new(),
            urdf_dialog_requested: false,
            asset_requests: Vec::new(),
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

    /// `target` の形状を、選んだファイルの形状に差し替える (動きの設定は残す)
    pub fn request_replace(&mut self, target: NodeId) {
        self.replace_requested = Some(target);
    }

    pub fn request_browser_view(&mut self) {
        self.browser_requested = true;
    }

    /// 「開く」が押されていたら true を返して取り消す (ダイアログは input::file_dialog が出す)
    pub fn request_urdf_dialog(&mut self) {
        self.urdf_dialog_requested = true;
    }

    pub fn take_urdf_dialog_request(&mut self) -> bool {
        std::mem::take(&mut self.urdf_dialog_requested)
    }

    /// 同梱の UR5e の URDF を設定ファイルと同じ場所に書き出して読み込む
    pub fn open_urdf_sample(&mut self) {
        let dir = urdf_sample_dir();
        match write_urdf_sample(&dir) {
            Ok(path) => self.open_path(path),
            Err(e) => self
                .messages
                .push(format!("UR5e の URDF を書き出せませんでした: {e}")),
        }
    }

    pub fn take_dialog_request(&mut self) -> bool {
        std::mem::take(&mut self.dialog_requested)
    }

    pub fn take_export_request(&mut self) -> Option<ExportScope> {
        self.export_requested.take()
    }

    pub fn take_replace_request(&mut self) -> Option<NodeId> {
        self.replace_requested.take()
    }

    pub fn take_browser_request(&mut self) -> bool {
        std::mem::take(&mut self.browser_requested)
    }

    /// 選んだファイルの形状で `target` を差し替える (読み込みは画面を止めずに行う)
    pub fn replace_from(&mut self, path: PathBuf, target: NodeId) {
        self.spawn(StepSource::File(path), Some(target));
    }

    pub fn open_sample(&mut self) {
        self.spawn(StepSource::Sample, None);
    }

    pub fn open_path(&mut self, path: PathBuf) {
        self.spawn(StepSource::File(path), None);
    }

    /// 負荷テスト用に、部品が `part_count` 個ほどのシーンを作る
    pub fn open_stress(&mut self, part_count: usize) {
        self.spawn(StepSource::Stress(part_count), None);
    }

    /// 読み込みを始める。`target` があれば、読んだ形状でそのノードを差し替える。
    /// ファイル 1つで読める形式は Bevy のアセットとして、それ以外は別スレッドで読む
    fn spawn(&mut self, source: StepSource, target: Option<NodeId>) {
        let name = source_name(&source);
        self.loading.push(name.clone());
        let target = target.map_or(ImportTarget::Add, ImportTarget::Replace);
        if let StepSource::File(path) = &source
            && let Some(path) = asset_loadable(path)
        {
            self.asset_requests.push(AssetRequest {
                name,
                path,
                target,
                units: self.units,
            });
            return;
        }
        self.spawn_thread(name, source, target);
    }

    /// 別スレッドで読み込む (`name` は読み込み中の一覧に入れてあるもの)
    pub fn spawn_thread(&self, name: String, source: StepSource, target: ImportTarget) {
        let sender = self.sender.clone();
        let units = self.units;
        std::thread::spawn(move || {
            let result = load_source(&source, &units);
            // 受け取り側がいなくなっていたら (アプリ終了時) 何もしない
            let _ = sender.send((name, source, target, result));
        });
    }
}

impl ImportJobs {
    /// ほかの読み方 (Bevy のアセット) で読み終わったものを、別スレッドの結果と同じように受け取らせる
    pub fn deliver(&self, result: ImportResult) {
        let _ = self.sender.send(result);
    }
}

/// Bevy のアセットとして読めるファイルなら、その絶対パス。
/// URDF (メッシュを別のファイルから読む) と、読み込みに対応していない形式は None
pub fn asset_loadable(path: &Path) -> Option<PathBuf> {
    if is_urdf_file(path) {
        return None;
    }
    let readable =
        is_step(path) || formats::format_for_path(path).is_some_and(|f| f.import.is_some());
    if !readable {
        return None;
    }
    std::path::absolute(path).ok()
}

/// 読み込み元を読む。STEP 以外は拡張子で形式を選び、`interchange` で読む
pub fn load_source(source: &StepSource, units: &UnitOptions) -> Result<Loaded, String> {
    match source {
        StepSource::File(path) if is_urdf_file(path) => {
            urdf::import(path).map(|robot| Loaded::Robot(Box::new(robot)))
        }
        StepSource::File(path) if !is_step(path) => {
            formats::import_file(path, units).map(Loaded::Scene)
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
    source.display_name()
}

/// 拡張子を見て、プロジェクトなら開き直し、STEP なら今のモデルに取り込む
pub fn open_any(path: PathBuf, jobs: &mut ImportJobs, project: &mut ProjectState) {
    if is_project_file(&path) {
        project.request(ProjectAction::OpenPath(path));
    } else if is_plan_file(&path) || is_cloud_file(&path) {
        jobs.site_files.push(path);
    } else {
        jobs.open_path(path);
    }
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
    formats::export_file(&scene, &path, units)
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
    for (name, source, target, result) in finished {
        if let Some(i) = jobs.loading.iter().position(|n| *n == name) {
            jobs.loading.remove(i);
        }
        let robot = result
            .as_ref()
            .ok()
            .and_then(Loaded::robot)
            .filter(|_| target == ImportTarget::Add)
            .cloned();
        // 単位の情報を持つシーンは、いまのモデルの単位に合わせて取り込む
        let result = result.map(|loaded| loaded.into_imported(&name, &model.unit));
        match (&target, result) {
            (ImportTarget::Add, Ok(file)) => {
                add_imported(
                    &mut jobs,
                    &mut model,
                    &mut animations,
                    &mut history,
                    &name,
                    source,
                    file,
                    robot,
                );
                // 部品の色が見えるよう、読み込み直後は何も選ばない
                selection.clear();
                camera.write(CameraRequest::FitAll);
            }
            (ImportTarget::Replace(target), Ok(file)) => {
                replace_geometry(&mut jobs, &mut model, &mut history, *target, file, source)
            }
            (ImportTarget::Refresh(path), Ok(file)) => {
                refresh_geometry(&mut jobs, &mut model, &mut history, path, file, source)
            }
            (ImportTarget::Refresh(_), Err(err)) => {
                warn!("{name}: {err}");
                jobs.messages.push(format!(
                    "{name} が更新されましたが、読み直せませんでした (前の形状のままです): {err}"
                ));
            }
            (_, Err(err)) => {
                error!("{name}: {err}");
                jobs.messages.push(format!("{name}: {err}"));
            }
        }
    }
}

/// 読み込んだファイルをモデルに加える
#[allow(clippy::too_many_arguments)]
fn add_imported(
    jobs: &mut ImportJobs,
    model: &mut Model,
    animations: &mut Animations,
    history: &mut History,
    name: &str,
    source: StepSource,
    file: ImportedFile,
    robot: Option<RobotDescription>,
) {
    let warnings = file.warnings.len();
    let clips = file.animations;
    let root = model.add_imported(file.root, file.unit, source);
    if !clips.is_empty() {
        // トラックの番号は取り込んだ木を深さ優先でたどった順 = subtree の順
        let ids = model.subtree(root);
        animations.add_clips(clips, &ids);
    }
    history.record_added(model, root, format!("{name} を読み込み"));
    jobs.imported.push(root);
    if let Some(description) = robot {
        // メーカーのメッシュはリンクの継ぎ目で重なるので、ロボットの中どうしの干渉は調べない
        if let Some(node) = model.get_mut(root) {
            node.skip_inner_interference = true;
        }
        jobs.robots.push(ImportedRobot { root, description });
    }
    let parts = model.measure(root).part_count;
    let mut message = format!("{name} を読み込みました (部品 {parts} 個)");
    if warnings > 0 {
        message += &format!("。読めなかった形状が {warnings} 件あります");
    }
    for warning in &file.warnings {
        warn!("{warning}");
    }
    jobs.messages.push(message);
}

/// 読み込んだ形状で `target` 以下を差し替え、元に戻せるよう履歴に積む
fn replace_geometry(
    jobs: &mut ImportJobs,
    model: &mut Model,
    history: &mut History,
    target: NodeId,
    file: ImportedFile,
    source: StepSource,
) {
    let Some(before) = model.snapshot(target) else {
        jobs.messages
            .push("差し替える部品が見つかりません (削除されたかもしれません)".into());
        return;
    };
    let old_name = before.nodes[0].1.name.clone();
    let file_name = source_name(&source);
    let Some(report) = model.replace_geometry(target, file.root, source) else {
        return;
    };
    let label = format!("{old_name} の形状を {file_name} に差し替え");
    history.push(ModelChange {
        label: label.clone(),
        changes: vec![(Some(before), model.snapshot(target))],
    });
    let paths = |ids: &[NodeId]| -> Vec<(NodeId, String)> {
        ids.iter().map(|&id| (id, model.path_label(id))).collect()
    };
    let mut message = format!("{label}ました (対応 {} 件", report.matched);
    if !report.unmatched.is_empty() {
        message += &format!("、見つからない {} 件", report.unmatched.len());
    }
    if !report.added.is_empty() {
        message += &format!("、追加 {} 件", report.added.len());
    }
    message += ")";
    jobs.messages.push(message);
    jobs.replace_summary = Some(ReplaceSummary {
        title: label,
        matched: report.matched,
        unmatched: paths(&report.unmatched),
        added: paths(&report.added),
    });
}

/// 更新されたファイルから取り込んだ木を、読み直した形状に差し替える (1回の「元に戻す」で戻せる)
fn refresh_geometry(
    jobs: &mut ImportJobs,
    model: &mut Model,
    history: &mut History,
    path: &Path,
    file: ImportedFile,
    source: StepSource,
) {
    let roots = model.roots_from_file(path);
    if roots.is_empty() {
        return;
    }
    let file_name = source_name(&source);
    let mut changes = Vec::new();
    let (mut matched, mut unmatched, mut added) = (0, 0, 0);
    for root in roots {
        let Some(before) = model.snapshot(root) else {
            continue;
        };
        let Some(report) = model.replace_geometry(root, file.root.clone(), source.clone()) else {
            continue;
        };
        matched += report.matched;
        unmatched += report.unmatched.len();
        added += report.added.len();
        changes.push((Some(before), model.snapshot(root)));
    }
    if changes.is_empty() {
        return;
    }
    let label = format!("更新された {file_name} を読み直し");
    history.push(ModelChange {
        label: label.clone(),
        changes,
    });
    let mut message = format!("{file_name} が更新されたので読み直しました (対応 {matched} 件");
    if unmatched > 0 {
        message += &format!("、見つからない {unmatched} 件");
    }
    if added > 0 {
        message += &format!("、追加 {added} 件");
    }
    message += ")";
    info!("{message}");
    jobs.messages.push(message);
}

#[cfg(test)]
mod tests {
    use super::*;

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
