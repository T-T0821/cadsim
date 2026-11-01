//! プロジェクトファイル (.plantwright) の保存と読み込み
//!
//! ファイルの中身は人が読める JSON。アセンブリの木をそのまま入れ子で書く。
//! 書くのは設計データ (名前・配置・表示/非表示・色・箱の寸法) と、取り込んだ STEP の場所だけ。
//! STEP の形状 (B-rep やメッシュ) は書かず、開くときに STEP から読み直す。
//! 3D 上の寸法と注記 (annotation.rs) も一緒に書く。部品は木の中の位置で指す。
//! そのため取り込んだ部品は「どの STEP の、どの製品か」で形状を引く。
//!
//! ファイル形式の構造体 (`ProjectFile` など) はアプリ内部の `Model` とは別に持つ。
//! 内部の型を変えてもファイルの互換を保ちやすくするため。
//!
//! 操作:
//! - ファイルメニュー / ツールバー: 新規・開く・保存・名前を付けて保存
//! - Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Shift+S
//! - `cargo run -- line.plantwright`、またはウィンドウへのドラッグ&ドロップでも開ける
//!
//! ここ (UI 層) は、頼まれた操作を受けて `document` の手順を呼び、結果を Resource と画面に写すだけにする。
//! ファイルの形は `file`、開く・保存の手順は `document` にある。

mod document;
mod file;
#[cfg(test)]
mod tests;

pub use document::*;
pub use file::*;

use std::path::{Path, PathBuf};

use bevy::{
    ecs::system::NonSendMarker,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, block_on, poll_once},
    window::PrimaryWindow,
};
use bevy_egui::egui;

use crate::document::history::History;
use crate::document::import::{ImportJobs, receive_imports};
use crate::document::selection::Selection;
use crate::features::animation::Animations;
use crate::features::annotation::Annotations;
use crate::features::conveyor::ConveyorConfigRes;
use crate::features::floor::{FloorDoc, FloorView};
use crate::features::flow::{FlowRes, FlowRun};
use crate::features::motion::Playback;
use crate::features::plc::Plc;
use crate::features::video::camera_path::CameraPathRes;
use crate::features::video::recording::is_recording;
use crate::viewport::camera::CameraRequest;
use plantwright_core::conveyor::ConveyorConfig;
use plantwright_core::model::Model;
use plantwright_core::model::file_name;
use plantwright_core::motion::chart::TimingChart;
use plantwright_core::plc::record::PlcRecord;
use plantwright_core::site::model::Site;

/// ウィンドウタイトルの先頭 (後ろにファイル名を付ける)
const APP_TITLE: &str = "plantwright";

pub struct ProjectPlugin;

impl Plugin for ProjectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ProjectState>().add_systems(
            Update,
            (
                // STEP の読み込み結果がモデルに入った後で、未保存かどうかを判断する
                run_project_actions.after(receive_imports),
                // 動画の書き出し中は進み具合をタイトルに出すので書き換えない
                update_window_title.run_if(not(is_recording)),
            )
                .chain(),
        );
    }
}

// ---------------------------------------------------------------------------
// アプリ側の状態と操作
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum ProjectAction {
    New,
    OpenPath(PathBuf),
    /// アプリに埋め込みのデモラインを無題のプロジェクトとして開く。`play` なら開いたら再生する。
    OpenDemoLine {
        play: bool,
    },
    /// アプリに埋め込みの本物の装置のサンプル (UR5e と 2F-85) を開いて再生する
    OpenRealRobot,
    Save,
    SaveAs,
}

impl ProjectAction {
    /// 今のモデルを捨てる操作か (未保存の変更があれば確認する)
    fn discards_model(&self) -> bool {
        matches!(
            self,
            ProjectAction::New
                | ProjectAction::OpenPath(_)
                | ProjectAction::OpenDemoLine { .. }
                | ProjectAction::OpenRealRobot
        )
    }
}

/// 読み込み元。デモラインはファイルに結びつけない (保存すると名前を聞く)。
enum LoadOrigin {
    File(PathBuf),
    DemoLine {
        play: bool,
    },
    /// 本物の装置のサンプル。書き出したフォルダから読むが、ファイルには結びつけない
    RealRobot(PathBuf),
}

impl LoadOrigin {
    fn name(&self) -> String {
        match self {
            LoadOrigin::File(path) => file_name(path),
            LoadOrigin::DemoLine { .. } => DEMO_LINE_NAME.into(),
            LoadOrigin::RealRobot(_) => REAL_ROBOT_NAME.into(),
        }
    }
}

type LoadTask = Task<(LoadOrigin, Result<LoadedProject, ProjectError>)>;

#[derive(Resource)]
pub struct ProjectState {
    /// 今開いているファイル。まだ保存していなければ None
    path: Option<PathBuf>,
    /// 最後に保存または読み込んだときの内容。今のモデルと比べて未保存の変更を見つける。
    saved: ProjectFile,
    /// 起動直後のサンプル読み込みが終わるまでは、それも「保存済み」の状態に含める
    startup: bool,
    /// メニューやショートカットから頼まれた操作。(操作, 破棄の確認が済んでいるか)
    requested: Option<(ProjectAction, bool)>,
    /// 未保存の変更を捨ててよいか確認している操作
    confirm_discard: Option<ProjectAction>,
    loading: Option<LoadTask>,
    /// プロジェクトを開いた・新規にした回数。ミッションの判定が、開いた中身を「操作した」と数えないように使う
    opened: u64,
    /// 直前のフレームで未保存の変更があったか (`update_window_title` が毎フレーム更新する)
    unsaved: bool,
}

impl ProjectState {
    /// 開く・保存などの操作が進行中か (確認ダイアログの返事待ちは含まない)
    pub fn is_busy(&self) -> bool {
        self.loading.is_some() || self.requested.is_some()
    }
}

impl Default for ProjectState {
    fn default() -> Self {
        Self {
            path: None,
            saved: ProjectFile::from_model(&Model::default(), None),
            startup: true,
            requested: None,
            confirm_discard: None,
            loading: None,
            opened: 0,
            unsaved: false,
        }
    }
}

impl ProjectState {
    pub fn request(&mut self, action: ProjectAction) {
        self.requested = Some((action, false));
    }

    pub fn has_unsaved_changes(&self, docs: &ProjectDocs) -> bool {
        self.saved != ProjectFile::capture(docs, self.base_dir())
    }

    /// ファイルに結びついているか (保存したか、ファイルから開いたか)
    pub fn has_file(&self) -> bool {
        self.path.is_some()
    }

    /// 直前のフレームで調べた未保存の変更の有無 (比べ直さないので軽い)
    pub fn unsaved(&self) -> bool {
        self.unsaved
    }

    pub fn file_name(&self) -> String {
        match &self.path {
            Some(path) => file_name(path),
            None => "無題".into(),
        }
    }

    fn base_dir(&self) -> Option<&Path> {
        self.path.as_deref().and_then(Path::parent)
    }

    /// 読み込み中か
    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    /// プロジェクトを開いた・新規にした回数
    pub fn opened_count(&self) -> u64 {
        self.opened
    }

    fn mark_saved(&mut self, docs: &ProjectDocs) {
        self.saved = ProjectFile::capture(docs, self.base_dir());
    }

    fn save(&mut self, path: PathBuf, docs: &ProjectDocs) -> Result<(), ProjectError> {
        self.saved = save_project(&path, docs)?;
        self.path = Some(path);
        Ok(())
    }

    fn start_loading(&mut self, origin: LoadOrigin) {
        self.loading = Some(AsyncComputeTaskPool::get().spawn(async move {
            let result = match &origin {
                LoadOrigin::File(path) => LoadedProject::load(path),
                LoadOrigin::DemoLine { .. } => load_demo_line(),
                LoadOrigin::RealRobot(dir) => load_real_robot_sample(dir),
            };
            (origin, result)
        }));
    }
}

/// タイミングチャート・取り込んだアニメーション・PLC 連携の設定の読み書き
#[derive(bevy::ecs::system::SystemParam)]
pub struct MotionDoc<'w> {
    chart: ResMut<'w, TimingChart>,
    playback: ResMut<'w, Playback>,
    animations: ResMut<'w, Animations>,
    plc: ResMut<'w, Plc>,
    conveyors: ResMut<'w, ConveyorConfigRes>,
    camera_path: ResMut<'w, CameraPathRes>,
}

impl MotionDoc<'_> {
    /// 開いた (または新規の) プロジェクトのものに置き換え、再生位置を最初に戻す
    fn replace(
        &mut self,
        chart: TimingChart,
        animations: Animations,
        plc: Option<&PlcRecord>,
        conveyors: ConveyorConfig,
    ) {
        *self.chart = chart;
        self.conveyors.0 = conveyors;
        *self.animations = animations;
        self.plc.restore(plc);
        let playback = &mut *self.playback;
        playback.time = 0.0;
        playback.playing = false;
    }
}

/// 頼まれた操作を実行し、読み込みが終わっていればモデルに反映する。
/// 新規作成・開くと元に戻す履歴は消す (前のプロジェクトに戻せてしまわないように)。
/// 「名前を付けて保存」のダイアログは macOS ではメインスレッドで出す必要があるので NonSendMarker を付ける。
#[allow(clippy::too_many_arguments)]
fn run_project_actions(
    mut state: ResMut<ProjectState>,
    mut model: ResMut<Model>,
    mut annotations: ResMut<Annotations>,
    mut flow: ResMut<FlowRes>,
    mut flow_run: ResMut<FlowRun>,
    mut floor: FloorDoc,
    mut selection: ResMut<Selection>,
    mut jobs: ResMut<ImportJobs>,
    mut camera: MessageWriter<CameraRequest>,
    mut history: ResMut<History>,
    mut motion: MotionDoc,
    _main_thread: NonSendMarker,
) {
    // 保存・比較する設計データ一式 (呼ぶたびに今の値から作る)
    macro_rules! docs {
        () => {
            ProjectDocs {
                model: &model,
                annotations: &annotations,
                flow: &flow,
                floor: floor.record(),
                chart: &motion.chart,
                animations: &motion.animations,
                plc: motion.plc.record(),
                conveyors: &motion.conveyors,
                camera_path: &motion.camera_path,
            }
        };
    }
    let state = &mut *state;
    if state.startup {
        state.mark_saved(&docs!());
        state.startup = !jobs.loading.is_empty();
    }

    if let Some((origin, result)) = state.loading.as_mut().and_then(|t| block_on(poll_once(t))) {
        state.loading = None;
        let name = origin.name();
        jobs.loading.retain(|n| *n != name);
        match result {
            Ok(loaded) => {
                let opened = loaded.open(&mut model);
                let warnings = opened.warnings;
                *annotations = opened.annotations;
                flow.0 = opened.flow;
                flow_run.stop();
                motion.replace(
                    opened.chart,
                    opened.animations,
                    loaded.file.plc.as_ref(),
                    opened.conveyors,
                );
                motion.camera_path.0 = opened.camera_path;
                floor.apply(opened.floor);
                history.clear();
                selection.clear();
                camera.write(CameraRequest::FitAll);
                state.opened += 1;
                let mut message = format!("{name} を開きました");
                match origin {
                    LoadOrigin::File(path) => state.path = Some(path),
                    LoadOrigin::DemoLine { play } => {
                        state.path = None;
                        if play {
                            motion.playback.playing = true;
                            motion.playback.looping = true;
                            message = format!("{name} を動かしています");
                        }
                    }
                    LoadOrigin::RealRobot(dir) => {
                        state.path = None;
                        motion.playback.playing = true;
                        motion.playback.looping = true;
                        info!("{name}: {}", dir.display());
                        message = format!("{name} を動かしています");
                    }
                }
                state.mark_saved(&docs!());
                if !warnings.is_empty() {
                    message += &format!("。注意が {} 件あります", warnings.len());
                }
                for warning in warnings {
                    warn!("{warning}");
                    jobs.messages.push(warning);
                }
                jobs.messages.push(message);
            }
            Err(e) => {
                error!("{name}: {e}");
                jobs.messages
                    .push(format!("{name} を開けませんでした: {e}"));
            }
        }
    }

    let Some((action, confirmed)) = state.requested.take() else {
        return;
    };
    if state.loading.is_some() {
        jobs.messages.push("プロジェクトを読み込み中です".into());
        return;
    }
    if action.discards_model() && !confirmed && state.has_unsaved_changes(&docs!()) {
        state.confirm_discard = Some(action);
        return;
    }
    match action {
        ProjectAction::New => {
            model.clear();
            model.site = Site::default();
            let opened = OpenedDocs::default();
            *annotations = opened.annotations;
            flow.0 = opened.flow;
            flow_run.stop();
            motion.replace(opened.chart, opened.animations, None, opened.conveyors);
            motion.camera_path.0 = opened.camera_path;
            floor.apply(opened.floor);
            history.clear();
            selection.clear();
            state.path = None;
            state.opened += 1;
            state.mark_saved(&docs!());
            jobs.messages.push("新しいプロジェクトを作りました".into());
        }
        ProjectAction::OpenPath(path) => {
            jobs.loading.push(file_name(&path));
            state.start_loading(LoadOrigin::File(path));
        }
        ProjectAction::OpenDemoLine { play } => {
            jobs.loading.push(DEMO_LINE_NAME.into());
            state.start_loading(LoadOrigin::DemoLine { play });
        }
        ProjectAction::OpenRealRobot => {
            jobs.loading.push(REAL_ROBOT_NAME.into());
            state.start_loading(LoadOrigin::RealRobot(real_robot_sample_dir()));
        }
        ProjectAction::Save | ProjectAction::SaveAs => {
            let path = match (&action, &state.path) {
                (ProjectAction::Save, Some(path)) => Some(path.clone()),
                // 画面の自動確認では OS のダイアログを操作できないので、決まった場所に書く
                _ if crate::features::ui_check::running() => {
                    crate::features::ui_check::save_path(&format!("project.{FILE_EXTENSION}"))
                }
                _ => rfd::FileDialog::new()
                    .set_title("名前を付けて保存")
                    .add_filter("プロジェクト", &[FILE_EXTENSION])
                    .set_file_name(match &state.path {
                        Some(_) => state.file_name(),
                        None => format!("無題.{FILE_EXTENSION}"),
                    })
                    .save_file()
                    .map(with_extension),
            };
            // ダイアログをキャンセルしたら何もしない
            let Some(path) = path else {
                return;
            };
            match state.save(path.clone(), &docs!()) {
                Ok(()) => jobs
                    .messages
                    .push(format!("{} を保存しました", file_name(&path))),
                Err(e) => {
                    error!("{}: {e}", path.display());
                    jobs.messages
                        .push(format!("{} を保存できませんでした: {e}", file_name(&path)));
                }
            }
        }
    }
}

/// 未保存の変更を捨ててよいかの確認。ui/mod.rs から毎フレーム呼ぶ。
pub fn show_confirm_dialog(ctx: &egui::Context, state: &mut ProjectState) {
    let Some(action) = state.confirm_discard.clone() else {
        return;
    };
    let file_name = state.file_name();
    let modal = egui::Modal::new(egui::Id::new("confirm_discard")).show(ctx, |ui| {
        ui.heading("保存していない変更を破棄しますか?");
        ui.label(format!("{file_name} には保存していない変更があります。"));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("破棄する").clicked() {
                Some(Choice::Discard)
            } else if ui.button("キャンセル").clicked() {
                Some(Choice::Cancel)
            } else {
                None
            }
        })
        .inner
    });
    let choice = match modal.inner {
        None if modal.should_close() => Some(Choice::Cancel),
        choice => choice,
    };
    match choice {
        Some(Choice::Discard) => {
            state.confirm_discard = None;
            state.requested = Some((action, true));
        }
        Some(Choice::Cancel) => state.confirm_discard = None,
        None => {}
    }
}

enum Choice {
    Discard,
    Cancel,
}

/// ウィンドウのタイトルにファイル名と未保存の印 (*) を出す。
#[allow(clippy::too_many_arguments)]
fn update_window_title(
    mut state: ResMut<ProjectState>,
    model: Res<Model>,
    annotations: Res<Annotations>,
    flow: Res<FlowRes>,
    floor: FloorView,
    chart: Res<TimingChart>,
    animations: Res<Animations>,
    plc: Res<Plc>,
    conveyors: Res<ConveyorConfigRes>,
    camera_path: Res<CameraPathRes>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    let docs = ProjectDocs {
        model: &model,
        annotations: &annotations,
        flow: &flow,
        floor: floor.record(),
        chart: &chart,
        animations: &animations,
        plc: plc.record(),
        conveyors: &conveyors,
        camera_path: &camera_path,
    };
    let unsaved = state.has_unsaved_changes(&docs);
    // 毎フレーム書くと変更検知が走り続けるので、変わったときだけ書く
    if state.unsaved != unsaved {
        state.unsaved = unsaved;
    }
    let marker = if unsaved { " *" } else { "" };
    let title = format!("{APP_TITLE} - {}{marker}", state.file_name());
    // 毎フレーム書き換えると変更検知が走り続けるので、変わったときだけ書く
    if window.title != title {
        window.title = title;
    }
}
