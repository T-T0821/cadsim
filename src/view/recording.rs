//! 動画書き出しの UI 層: 設定ウィンドウと画面のキャプチャ
//!
//! 撮る時刻と枚数は `cadsim_core::video` が決める。ここでは再生位置をその時刻に合わせて描画し、
//! キャプチャが届くまで次の時刻に進まない (PC の速さや描画の重さに関係なく同じ動画になる)。
//!
//! 書き出し中は設定ウィンドウを隠し、進み具合はウィンドウのタイトルに出す。
//! カメラ操作・編集・ショートカットは止め、Esc で中止できる。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::render::view::window::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::window::PrimaryWindow;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::model::Selection;
use crate::motion::{ChartSchedule, MotionSet, Playback};
use crate::video::encode::{EncodeSettings, EncoderThread};
use crate::view::camera::{CameraSystems, OrbitCamera, ViewportRect};
use crate::view::camera_path::{CameraPathRes, CameraPathUi, camera_path_ui};
use cadsim_core::camera_path::{CameraPath, CameraPose};
use cadsim_core::video::{CropRect, Format, Frame, FramePlan, RecordingSession};

pub struct RecordingPlugin;

impl Plugin for RecordingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RecordSettings>()
            .init_resource::<Recorder>()
            .add_systems(EguiPrimaryContextPass, recording_window)
            // 再生位置を決めてから部品を動かすので、MotionSet より前に動かす
            .add_systems(
                Update,
                (start_if_requested, drive_recording)
                    .chain()
                    .before(MotionSet::Pose)
                    .before(CameraSystems),
            );
    }
}

/// 書き出し中 (キャプチャ中) なら true。カメラ操作や編集を止めるのに使う
pub fn is_recording(recorder: Res<Recorder>) -> bool {
    recorder.is_capturing()
}

/// 動画に映す範囲
#[derive(Clone, Copy, PartialEq, Eq)]
enum Area {
    /// パネルを除いた 3D ビューだけ
    View3d,
    Window,
}

#[derive(Resource)]
pub struct RecordSettings {
    /// 設定ウィンドウを開いているか (メニューの「動画を書き出し…」で開く)
    pub open: bool,
    format: Format,
    fps: u32,
    /// 何サイクル分を書き出すか
    cycles: u32,
    /// 再生速度の倍率 (0.5 ならスローモーション)
    speed: f64,
    /// 1: 等倍、2: 縦横 1/2
    downscale: u32,
    area: Area,
    /// 書き出し前に選択を解除して、部品の色を元に戻す
    clear_selection: bool,
    /// カメラの通り道に沿って撮る (通り道が動かせるときだけ効く)
    use_camera_path: bool,
    /// 「書き出し開始」が押された。保存先のダイアログは Update で出す
    start_requested: bool,
}

impl RecordSettings {
    /// 書き出しに使うカメラの通り道 (使わないなら None)
    fn camera_path<'a>(&self, path: &'a CameraPath) -> Option<&'a CameraPath> {
        (self.use_camera_path && path.is_playable()).then_some(path)
    }

    /// 撮るフレームの計画。カメラの通り道を使うなら、通り道の終わりまで撮る
    fn plan(&self, cycle_time: f64, path: &CameraPath) -> FramePlan {
        let plan = FramePlan::new(cycle_time, self.cycles, self.fps, self.speed);
        match self.camera_path(path) {
            Some(path) => plan.extend_to(path.duration()),
            None => plan,
        }
    }
}

impl Default for RecordSettings {
    fn default() -> Self {
        Self {
            open: false,
            format: Format::Mp4,
            fps: Format::Mp4.default_fps(),
            cycles: 1,
            speed: 1.0,
            downscale: 1,
            area: Area::View3d,
            clear_selection: true,
            use_camera_path: false,
            start_requested: false,
        }
    }
}

#[derive(Resource, Default)]
pub struct Recorder {
    state: State,
    /// 最後の書き出しの結果 (成功ならファイルの場所)
    result: Option<Result<PathBuf, String>>,
}

impl Recorder {
    pub fn is_capturing(&self) -> bool {
        matches!(self.state, State::Capturing(_))
    }

    /// 撮影かエンコードの途中
    pub fn is_busy(&self) -> bool {
        !matches!(self.state, State::Idle)
    }
}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Capturing(Box<Capture>),
    /// キャプチャは終わり、エンコードの完了を待っている
    Encoding {
        encoder: EncoderThread,
        cancelled: bool,
    },
}

struct Capture {
    session: RecordingSession<EncoderThread>,
    /// キャプチャ待ちのとき、画像が届く場所
    pending: Option<Arc<Mutex<Option<Image>>>>,
    /// 書き出し前の再生状態とウィンドウタイトル (終わったら戻す)
    saved_playback: Playback,
    saved_title: String,
    /// カメラの通り道 (使うときだけ)。撮り終えたら `saved_pose` の視点に戻す
    camera_path: Option<CameraPath>,
    saved_pose: CameraPose,
}

/// 保存先を選ばせ、キャプチャを始める。ダイアログはメインスレッドで出す必要がある
#[allow(clippy::too_many_arguments)]
fn start_if_requested(
    mut settings: ResMut<RecordSettings>,
    mut recorder: ResMut<Recorder>,
    schedule: Res<ChartSchedule>,
    mut playback: ResMut<Playback>,
    mut selection: ResMut<Selection>,
    viewport: Res<ViewportRect>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera_path: Res<CameraPathRes>,
    orbit: Res<OrbitCamera>,
    _main_thread: NonSendMarker,
) {
    if !std::mem::take(&mut settings.start_requested) || !matches!(recorder.state, State::Idle) {
        return;
    }
    let Some(path) = pick_output(settings.format) else {
        return;
    };
    let plan = settings.plan(schedule.cycle_time, &camera_path);
    let crop = match settings.area {
        Area::Window => None,
        Area::View3d => viewport.rect.map(|r| {
            let scale = window.scale_factor();
            let size = window.physical_size();
            let x = ((r.min.x * scale).round() as u32).min(size.x.saturating_sub(16));
            let y = ((r.min.y * scale).round() as u32).min(size.y.saturating_sub(16));
            let w = ((r.width() * scale).round() as u32).clamp(16, size.x - x);
            let h = ((r.height() * scale).round() as u32).clamp(16, size.y - y);
            CropRect {
                x,
                y,
                width: w,
                height: h,
            }
        }),
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
        && let Err(err) = std::fs::create_dir_all(dir)
    {
        recorder.result = Some(Err(err.to_string()));
        return;
    }
    let encoder = EncoderThread::spawn(EncodeSettings {
        format: settings.format,
        fps: settings.fps,
        downscale: settings.downscale,
        path,
    });
    recorder.state = State::Capturing(Box::new(Capture {
        session: RecordingSession::new(plan, crop, encoder),
        pending: None,
        saved_playback: *playback,
        saved_title: window.title.clone(),
        camera_path: settings.camera_path(&camera_path).cloned(),
        saved_pose: orbit.camera_pose(),
    }));
    recorder.result = None;
    playback.playing = false;
    if settings.clear_selection {
        selection.clear();
    }
}

/// 保存先のダイアログ。PNG 連番はフォルダを作ってその中に並べる
fn pick_output(format: Format) -> Option<PathBuf> {
    // 画面の自動確認では OS のダイアログを操作できないので、決まった場所に書く
    let default_name = match format.extension() {
        Some(ext) => format!("simulation.{ext}"),
        None => "simulation_frames".into(),
    };
    if let Some(path) = crate::ui_check::save_path(&default_name) {
        return Some(path);
    }
    let dialog = rfd::FileDialog::new().set_title("動画の書き出し");
    match format.extension() {
        Some(ext) => {
            let path = dialog
                .set_file_name(format!("simulation.{ext}"))
                .add_filter(format.label(), &[ext])
                .save_file()?;
            // 拡張子を消して保存されたときは付け直す
            let has_ext = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(ext));
            Some(if has_ext {
                path
            } else {
                path.with_extension(ext)
            })
        }
        None => dialog.set_file_name("simulation_frames").save_file(),
    }
}

fn drive_recording(
    mut commands: Commands,
    mut recorder: ResMut<Recorder>,
    mut playback: ResMut<Playback>,
    mut orbit: ResMut<OrbitCamera>,
    keys: Res<ButtonInput<KeyCode>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    let recorder = &mut *recorder;
    match &mut recorder.state {
        State::Idle => {}
        State::Encoding { encoder, .. } => {
            if !encoder.is_finished() {
                return;
            }
            let State::Encoding { encoder, cancelled } = std::mem::take(&mut recorder.state) else {
                unreachable!()
            };
            recorder.result = Some(match encoder.join() {
                Ok(path) if cancelled => {
                    remove_output(&path);
                    Err("中止しました".into())
                }
                Ok(path) => Ok(path),
                Err(err) => Err(err.to_string()),
            });
        }
        State::Capturing(capture) => {
            if keys.just_pressed(KeyCode::Escape) {
                let capture = take_capture(&mut recorder.state);
                finish_capture(
                    recorder,
                    *capture,
                    &mut playback,
                    &mut orbit,
                    &mut window,
                    true,
                );
                return;
            }

            // 前に頼んだキャプチャが届いていればエンコーダに渡す
            if let Some(slot) = &capture.pending {
                let Some(image) = slot.lock().ok().and_then(|mut slot| slot.take()) else {
                    return;
                };
                capture.pending = None;
                let submitted = image_to_frame(image)
                    .ok_or_else(|| "画面を読み取れませんでした".to_string())
                    .and_then(|frame| capture.session.submit(frame));
                if submitted.is_err() {
                    // エンコード側が止まった (書き込み失敗など)。原因は join で受け取る
                    let capture = take_capture(&mut recorder.state);
                    finish_capture(
                        recorder,
                        *capture,
                        &mut playback,
                        &mut orbit,
                        &mut window,
                        false,
                    );
                    return;
                }
            }

            let Some(time) = capture.session.current_time() else {
                let capture = take_capture(&mut recorder.state);
                finish_capture(
                    recorder,
                    *capture,
                    &mut playback,
                    &mut orbit,
                    &mut window,
                    false,
                );
                return;
            };

            // 次のフレームの時刻に合わせて、このフレームの描画をキャプチャする
            playback.playing = false;
            playback.time = time;
            if let Some(path) = &capture.camera_path
                && let Some(pose) = capture
                    .session
                    .current_video_time()
                    .and_then(|t| path.sample(t))
            {
                orbit.set_camera_pose(pose);
            }
            let slot = Arc::new(Mutex::new(None));
            let sender = slot.clone();
            commands.spawn(Screenshot::primary_window()).observe(
                move |captured: On<ScreenshotCaptured>| {
                    if let Ok(mut slot) = sender.lock() {
                        *slot = Some(captured.image.clone());
                    }
                },
            );
            capture.pending = Some(slot);
            let (done, total) = capture.session.progress();
            window.title = format!(
                "{} - 動画を書き出し中 {}/{total} (Esc で中止)",
                capture.saved_title,
                done + 1,
            );
        }
    }
}

fn take_capture(state: &mut State) -> Box<Capture> {
    match std::mem::take(state) {
        State::Capturing(capture) => capture,
        _ => unreachable!(),
    }
}

/// キャプチャを終えて、再生状態とタイトルを戻し、エンコードの完了待ちに移る
fn finish_capture(
    recorder: &mut Recorder,
    capture: Capture,
    playback: &mut Playback,
    orbit: &mut OrbitCamera,
    window: &mut Window,
    cancelled: bool,
) {
    let mut encoder = capture.session.into_sink();
    encoder.close();
    *playback = capture.saved_playback;
    if capture.camera_path.is_some() {
        orbit.set_camera_pose(capture.saved_pose);
    }
    window.title = capture.saved_title;
    recorder.state = State::Encoding { encoder, cancelled };
}

fn image_to_frame(image: Image) -> Option<Frame> {
    let rgba = image.try_into_dynamic().ok()?.to_rgba8();
    Some(Frame {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

fn remove_output(path: &Path) {
    let _ = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
}

// ---------------------------------------------------------------------------
// 設定ウィンドウ
// ---------------------------------------------------------------------------

/// カメラワークの欄が書き換えるもの
#[derive(bevy::ecs::system::SystemParam)]
struct CameraWork<'w> {
    path: ResMut<'w, CameraPathRes>,
    ui: ResMut<'w, CameraPathUi>,
    orbit: ResMut<'w, OrbitCamera>,
    playback: ResMut<'w, Playback>,
}

fn recording_window(
    mut contexts: EguiContexts,
    mut settings: ResMut<RecordSettings>,
    recorder: Res<Recorder>,
    schedule: Res<ChartSchedule>,
    viewport: Res<ViewportRect>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut camera: CameraWork,
) -> Result {
    // 書き出し中とプレビュー中のウィンドウは動画に映るので出さない
    if !settings.open || recorder.is_capturing() {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let center = viewport.rect.map_or(egui::pos2(400.0, 200.0), |r| {
        egui::pos2(r.center().x, r.min.y + 40.0)
    });
    let mut open = true;
    egui::Window::new("動画の書き出し")
        .open(&mut open)
        .default_pos(center)
        .pivot(egui::Align2::CENTER_TOP)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            settings_ui(
                ui,
                &mut settings,
                &recorder,
                &schedule,
                &window,
                &mut camera,
            );
        });
    if !open {
        settings.open = false;
    }
    Ok(())
}

fn settings_ui(
    ui: &mut egui::Ui,
    settings: &mut RecordSettings,
    recorder: &Recorder,
    schedule: &ChartSchedule,
    window: &Window,
    camera: &mut CameraWork,
) {
    egui::Grid::new("record_settings")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("形式");
            egui::ComboBox::from_id_salt("record_format")
                .selected_text(settings.format.label())
                .show_ui(ui, |ui| {
                    for format in Format::ALL {
                        ui.selectable_value(&mut settings.format, format, format.label());
                    }
                });
            ui.end_row();
            if !settings.format.fps_choices().contains(&settings.fps) {
                settings.fps = settings.format.default_fps();
            }

            ui.label("フレームレート");
            ui.horizontal(|ui| {
                for &fps in settings.format.fps_choices() {
                    ui.selectable_value(&mut settings.fps, fps, format!("{fps}"));
                }
                ui.label("fps");
            });
            ui.end_row();

            ui.label("サイクル数");
            ui.add(egui::DragValue::new(&mut settings.cycles).range(1..=20));
            ui.end_row();

            ui.label("再生速度");
            ui.horizontal(|ui| {
                for speed in [0.25, 0.5, 1.0, 2.0] {
                    ui.selectable_value(&mut settings.speed, speed, format!("x{speed}"));
                }
            });
            ui.end_row();

            ui.label("映す範囲");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut settings.area, Area::View3d, "3D ビュー");
                ui.selectable_value(&mut settings.area, Area::Window, "ウィンドウ全体");
            });
            ui.end_row();

            ui.label("大きさ");
            ui.horizontal(|ui| {
                for (factor, label) in [(1, "等倍"), (2, "1/2")] {
                    ui.selectable_value(&mut settings.downscale, factor, label);
                }
                if settings.area == Area::Window {
                    let size = window.physical_size();
                    ui.weak(format!(
                        "{}×{}",
                        size.x / settings.downscale,
                        size.y / settings.downscale
                    ));
                }
            });
            ui.end_row();
        });
    ui.checkbox(&mut settings.clear_selection, "選択を解除してから撮る");

    let busy = !matches!(recorder.state, State::Idle);
    ui.separator();
    ui.strong("カメラワーク");
    let speed = settings.speed;
    camera_path_ui(
        ui,
        &mut camera.path,
        &mut camera.ui,
        &mut camera.orbit,
        &mut camera.playback,
        &mut settings.use_camera_path,
        speed,
        busy,
    );

    let cycle = schedule.cycle_time;
    let plan = settings.plan(cycle, &camera.path);
    let with_camera = settings.camera_path(&camera.path).is_some();
    let can_record = cycle > 0.0 || with_camera;
    ui.separator();
    if !can_record {
        ui.weak("タイミングチャートに動作がありません (カメラワークだけでも撮れます)");
    } else {
        ui.label(format!(
            "動画の長さ {:.1} 秒 ({} フレーム)",
            plan.video_seconds(),
            plan.frames
        ));
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                can_record && !busy && !camera.ui.is_previewing(),
                egui::Button::new("● 書き出し開始…"),
            )
            .on_hover_text("保存先を選ぶと書き出しを始めます")
            .clicked()
        {
            settings.start_requested = true;
        }
        if busy {
            ui.spinner();
            ui.label("エンコード中…");
        }
    });
    ui.weak("書き出し中は操作しないでください。Esc で中止");

    match &recorder.result {
        Some(Ok(path)) => {
            ui.separator();
            ui.label("書き出しました:");
            ui.monospace(path.display().to_string());
            if ui.button("フォルダを開く").clicked() {
                open_folder(path);
            }
        }
        Some(Err(message)) => {
            ui.separator();
            ui.colored_label(
                egui::Color32::from_rgb(230, 90, 80),
                format!("書き出せませんでした: {message}"),
            );
        }
        None => {}
    }
}

fn open_folder(path: &Path) {
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(dir).spawn();
}
