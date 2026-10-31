//! 画面での動作確認の自動化 (UI 層)
//!
//! `cargo run -- --ui-check tests/ui` で、手順書 (`*.scenario`) ごとにアプリを起動し直し、
//! 書いてある操作を流して画面を撮り、基準画像と比べる (suite.rs)。
//!
//! 起動されたアプリの側 (このファイル) は、毎フレーム
//! 「画面の様子を集める → `cadsim_core::ui_check::runner` に渡す → 返ってきた入力を流す・写真を撮る」
//! だけをする。手順の進め方・部品の探し方・画像の比べ方は `cadsim_core::ui_check`、
//! PNG・報告書の読み書きは `cadsim_io::ui_check`。
//!
//! 入力は本物のマウス・キーボードと同じ Bevy のメッセージとして流すので、
//! egui の画面もショートカットも 3D ビューのクリックも、手で触ったときと同じ経路を通る。
//! ボタンは egui のアクセシビリティ情報 (AccessKit) の表示名と位置で探す。

pub mod suite;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key as LogicalKey, KeyboardInput};
use bevy::input::mouse::{MouseButtonInput, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::render::view::window::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::time::TimeUpdateStrategy;
use bevy::window::{PrimaryWindow, WindowEvent};
use bevy_egui::input::EguiInputEvent;
use bevy_egui::{
    EguiContexts, EguiInput, EguiOutput, EguiPreUpdateSet, EguiPrimaryContextPass,
    PrimaryEguiContext, egui,
};
use cadsim_core::ui_check::Rect as CheckRect;
use cadsim_core::ui_check::compare::{Image as RgbaImage, Tolerance};
use cadsim_core::ui_check::report::{ScenarioResult, ShotResult, judge};
use cadsim_core::ui_check::runner::{Command, Failure, Input, Observation, Runner};
use cadsim_core::ui_check::scenario::{self, Key, MouseButton as CheckButton};
use cadsim_core::ui_check::widget::{self, Widget};
use cadsim_io::ui_check as files;

use crate::file_io::{CommandLine, ImportJobs, UiCheckRun, open_any};
use crate::project::ProjectState;
use crate::recording::Recorder;
use crate::simulation::InterferenceCheck;
use crate::viewport::Defeaturing;

/// 自動確認中は 1 フレーム = 1/60 秒として時計を進める。
/// 描画の速さに関係なく、アニメーションや通知が毎回同じ場面で撮れるようにする
const FRAME: Duration = Duration::from_nanos(16_666_667);

pub struct UiCheckPlugin;

/// 自動確認の途中か。計測した時間や FPS のような毎回変わる表示を止めるのに使う
static RUNNING: AtomicBool = AtomicBool::new(false);

pub fn running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

/// 自動確認の途中に保存ダイアログの代わりに使うフォルダ
static SAVE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 自動確認の途中なら、保存ダイアログを出さずに使う保存先 (`<出力>/<手順書>/saved/<名前>`)。
/// OS のダイアログは操作できないので、書き出しの手順はここに書かせる。ふだんは None
pub fn save_path(file_name: &str) -> Option<PathBuf> {
    let dir = SAVE_DIR.get()?;
    std::fs::create_dir_all(dir).ok()?;
    Some(dir.join(file_name))
}

/// 計測した数 (時間・FPS) を小数 `digits` 桁の文字にする。自動確認の途中は毎回同じ写真になるよう `--` にする
pub fn measured(value: f64, digits: usize) -> String {
    if running() {
        "--".into()
    } else {
        format!("{value:.digits$}")
    }
}

impl Plugin for UiCheckPlugin {
    fn build(&self, app: &mut App) {
        let Some(run) = CommandLine::parse().ui_check else {
            return;
        };
        RUNNING.store(true, Ordering::Relaxed);
        let _ = SAVE_DIR.set(scenario_dir(&run).join("saved"));
        let name = scenario_name(&run.scenario);
        let (runner, failure) = match std::fs::read_to_string(&run.scenario)
            .map_err(|e| format!("{} を読めません: {e}", run.scenario.display()))
            .and_then(|text| scenario::parse(&text).map_err(|e| e.to_string()))
        {
            Ok(lines) => (Runner::new(lines), None),
            Err(message) => (Runner::new(Vec::new()), Some(Failure { line: 0, message })),
        };
        app.insert_resource(TimeUpdateStrategy::ManualDuration(FRAME))
            .insert_resource(UiCheck {
                runner,
                run,
                result: ScenarioResult {
                    name,
                    failure,
                    shots: Vec::new(),
                },
                widgets: Vec::new(),
                pending: None,
                failure_shot: false,
            })
            .add_systems(EguiPrimaryContextPass, enable_accesskit)
            // 時計を止める・動かすのが、同じフレームの時計の更新より前に決まるようにする
            .add_systems(First, drive.before(bevy::time::TimeSystems))
            .add_systems(
                PreUpdate,
                egui_clock
                    .after(EguiPreUpdateSet::ProcessInput)
                    .before(EguiPreUpdateSet::BeginPass),
            )
            .add_systems(Last, (collect_widgets, receive_capture));
    }
}

/// 手順書のファイル名 (拡張子なし) を結果の名前にする
pub fn scenario_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "scenario".into())
}

#[derive(Resource)]
pub struct UiCheck {
    runner: Runner,
    run: UiCheckRun,
    result: ScenarioResult,
    /// 前のフレームに画面に出ていた部品
    widgets: Vec<Widget>,
    /// 撮影を頼んで、まだ届いていない写真
    pending: Option<PendingShot>,
    /// 失敗したときの画面を撮った
    failure_shot: bool,
}

/// 失敗したときの画面の写真の名前 (手順書の写真の名前と重ならないよう記号で始める)
const FAILURE_SHOT: &str = "(失敗)";

struct PendingShot {
    name: String,
    masks: Vec<CheckRect>,
    slot: Arc<Mutex<Option<bevy::image::Image>>>,
}

/// egui の時計 (ツールチップの遅れ・たたむ動き・通知・紙吹雪) も、実時間ではなく
/// 1 フレーム 1/60 秒で進む時計にする
fn egui_clock(time: Res<Time<Virtual>>, mut inputs: Query<&mut EguiInput>) {
    for mut input in &mut inputs {
        input.time = Some(time.elapsed_secs_f64());
    }
}

fn enable_accesskit(mut contexts: EguiContexts) -> Result {
    contexts.ctx_mut()?.enable_accesskit();
    Ok(())
}

/// egui が出したアクセシビリティ情報から、表示名と位置のある部品を集める
fn collect_widgets(
    mut check: ResMut<UiCheck>,
    outputs: Query<&EguiOutput, With<PrimaryEguiContext>>,
) {
    let Ok(output) = outputs.single() else {
        return;
    };
    let Some(update) = &output.platform_output.accesskit_update else {
        return;
    };
    let raw = update
        .nodes
        .iter()
        .filter_map(|(_, node)| {
            let label = node.label().or(node.value())?.trim().to_string();
            let b = node.bounds()?;
            Some(Widget {
                label,
                role: format!("{:?}", node.role()),
                rect: CheckRect::new(
                    b.x0 as f32,
                    b.y0 as f32,
                    (b.x1 - b.x0) as f32,
                    (b.y1 - b.y0) as f32,
                ),
            })
        })
        .collect();
    check.widgets = widget::normalize(raw);
}

/// 別スレッドで進む計算や書き出し (読み込みと開く・保存は drive が直接見る)
#[derive(bevy::ecs::system::SystemParam)]
struct Busy<'w> {
    recorder: Res<'w, Recorder>,
    interference: Res<'w, InterferenceCheck>,
    defeaturing: Res<'w, Defeaturing>,
}

impl Busy<'_> {
    fn any(&self) -> bool {
        self.recorder.is_busy()
            || self.interference.report.overlap_pending
            || self.defeaturing.pending() > 0
    }
}

/// 毎フレームの始めに手順を1つ進め、入力を流す
#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    mut check: ResMut<UiCheck>,
    busy: Busy,
    mut virtual_time: ResMut<Time<Virtual>>,
    mut jobs: ResMut<ImportJobs>,
    mut project: ResMut<ProjectState>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    egui_context: Query<Entity, With<PrimaryEguiContext>>,
    mut writers: InputWriters,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok((window_entity, mut window)) = windows.single_mut() else {
        return;
    };
    let check = &mut *check;
    let is_busy = !jobs.loading.is_empty() || project.is_busy() || busy.any();
    // 計算中と、撮った写真が届くまでは時計を止める (runner も手順を進めない)。
    // かかるフレーム数が毎回違っても、アニメーションや通知が同じ場面で撮れるようにする
    if is_busy || check.pending.is_some() {
        virtual_time.pause();
    } else {
        virtual_time.unpause();
    }
    let observation = Observation {
        busy: is_busy,
        capturing: check.pending.is_some(),
        widgets: &check.widgets,
    };
    // 開く前に手順書の読み取りで失敗していたら、すぐ終える
    let command = if check.result.failure.is_some() {
        Command::Finished
    } else {
        check.runner.tick(&observation)
    };
    match command {
        Command::Idle => {}
        Command::Inputs(inputs) => {
            for input in inputs {
                writers.send(
                    window_entity,
                    &mut window,
                    egui_context.single().ok(),
                    input,
                );
            }
        }
        Command::Open(path) => open_any(resolve_open_path(&path), &mut jobs, &mut project),
        Command::Capture { name, masks } => {
            check.pending = Some(capture(&mut commands, name, masks));
        }
        Command::DumpWidgets { name, text } => {
            let path = scenario_dir(&check.run).join(format!("{name}.txt"));
            if let Err(e) = std::fs::create_dir_all(scenario_dir(&check.run))
                .and_then(|_| std::fs::write(&path, text))
            {
                warn!("{} に書けません: {e}", path.display());
            }
        }
        Command::Finished => {
            if check.pending.is_some() {
                return;
            }
            if check.result.failure.is_none() {
                check.result.failure = check.runner.failure().cloned();
            }
            // 失敗したら、そのときの画面を報告書に載せる (比べはしない)
            if check.result.failure.is_some() && !check.failure_shot {
                check.failure_shot = true;
                check.pending = Some(capture(&mut commands, FAILURE_SHOT.into(), Vec::new()));
                return;
            }
            let passed = finish(&check.run, &check.result);
            exit.write(if passed {
                AppExit::Success
            } else {
                AppExit::error()
            });
        }
    }
}

/// 画面の写真を頼む。届くと receive_capture が受け取る
fn capture(commands: &mut Commands, name: String, masks: Vec<CheckRect>) -> PendingShot {
    let slot = Arc::new(Mutex::new(None));
    let sender = slot.clone();
    commands.spawn(Screenshot::primary_window()).observe(
        move |captured: On<ScreenshotCaptured>| {
            if let Ok(mut slot) = sender.lock() {
                *slot = Some(captured.image.clone());
            }
        },
    );
    PendingShot { name, masks, slot }
}

/// 写真が届いたら基準画像と比べて保存する
fn receive_capture(mut check: ResMut<UiCheck>) {
    let Some(pending) = &check.pending else {
        return;
    };
    let Some(image) = pending.slot.lock().ok().and_then(|mut s| s.take()) else {
        return;
    };
    let pending = check.pending.take().expect("確認済み");
    if pending.name == FAILURE_SHOT {
        if let Some(img) = to_rgba(image) {
            let path = scenario_dir(&check.run).join(files::FAILURE_PNG);
            if let Err(e) = files::write_png(&path, &img) {
                warn!("{e}");
            }
        }
        return;
    }
    let Some(actual) = to_rgba(image) else {
        check.result.failure = Some(Failure {
            line: check.runner.current_line().unwrap_or(0),
            message: format!("写真「{}」を画像にできません", pending.name),
        });
        return;
    };
    let status = save_shot(&check.run, &pending.name, &actual, &pending.masks);
    match status {
        Ok(status) => check.result.shots.push(ShotResult {
            name: pending.name,
            status,
        }),
        Err(message) => {
            check.result.failure = Some(Failure {
                line: check.runner.current_line().unwrap_or(0),
                message,
            })
        }
    }
}

/// `open saved/名前` は、この手順書で保存したファイル (保存ダイアログの代わりの場所) を開く
fn resolve_open_path(path: &str) -> PathBuf {
    match path.strip_prefix("saved/").and_then(save_path) {
        Some(saved) => saved,
        None => PathBuf::from(path),
    }
}

fn to_rgba(image: bevy::image::Image) -> Option<RgbaImage> {
    let rgba = image.try_into_dynamic().ok()?.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(RgbaImage::new(w, h, rgba.into_raw()))
}

fn scenario_dir(run: &UiCheckRun) -> PathBuf {
    run.out.join(scenario_name(&run.scenario))
}

fn baseline_path(run: &UiCheckRun, shot: &str) -> PathBuf {
    run.baseline
        .join(scenario_name(&run.scenario))
        .join(format!("{shot}.png"))
}

/// 今回の写真・差分・基準画像の写しを書き、判定を返す
fn save_shot(
    run: &UiCheckRun,
    name: &str,
    actual: &RgbaImage,
    masks: &[CheckRect],
) -> Result<cadsim_core::ui_check::report::ShotStatus, String> {
    let dir = scenario_dir(run);
    let baseline_file = baseline_path(run, name);
    let baseline = baseline_file
        .exists()
        .then(|| files::read_png(&baseline_file))
        .transpose()?;
    let judgement = judge(
        baseline.as_ref(),
        actual,
        Tolerance::default(),
        masks,
        run.update,
    );
    files::write_png(&dir.join("actual").join(format!("{name}.png")), actual)?;
    if let Some(diff) = &judgement.diff {
        files::write_png(&dir.join("diff").join(format!("{name}.png")), diff)?;
    }
    if let (false, Some(baseline)) = (judgement.status.passed(), &baseline) {
        files::write_png(&dir.join("expected").join(format!("{name}.png")), baseline)?;
    }
    if judgement.save_as_baseline {
        files::write_png(&baseline_file, actual)?;
    }
    Ok(judgement.status)
}

/// 結果と報告書を書き、要約を出す。合格なら true
fn finish(run: &UiCheckRun, result: &ScenarioResult) -> bool {
    let dir = scenario_dir(run);
    let written = std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(dir.join("result.json"), files::result_to_json(result)))
        .and_then(|_| std::fs::write(dir.join("report.html"), files::scenario_html(result)));
    if let Err(e) = written {
        eprintln!("{} に結果を書けません: {e}", dir.display());
    }
    print!("{}", result.summary());
    result.passed()
}

/// 本物の入力と同じメッセージを書くもの一式
#[derive(bevy::ecs::system::SystemParam)]
struct InputWriters<'w> {
    window_events: MessageWriter<'w, WindowEvent>,
    cursor: MessageWriter<'w, CursorMoved>,
    buttons: MessageWriter<'w, MouseButtonInput>,
    wheel: MessageWriter<'w, MouseWheel>,
    keys: MessageWriter<'w, KeyboardInput>,
    egui: MessageWriter<'w, EguiInputEvent>,
}

impl InputWriters<'_> {
    fn send(
        &mut self,
        window_entity: Entity,
        window: &mut Window,
        egui_context: Option<Entity>,
        input: Input,
    ) {
        match input {
            Input::MoveTo(x, y) => {
                let position = Vec2::new(x, y);
                let delta = window.cursor_position().map(|old| position - old);
                window.set_cursor_position(Some(position));
                let message = CursorMoved {
                    window: window_entity,
                    position,
                    delta,
                };
                self.cursor.write(message.clone());
                // egui はウィンドウのイベントからマウスの位置とボタンを読む
                self.window_events.write(WindowEvent::CursorMoved(message));
            }
            Input::Button { button, pressed } => {
                let message = MouseButtonInput {
                    button: match button {
                        CheckButton::Left => MouseButton::Left,
                        CheckButton::Right => MouseButton::Right,
                        CheckButton::Middle => MouseButton::Middle,
                    },
                    state: if pressed {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Released
                    },
                    window: window_entity,
                };
                self.buttons.write(message);
                self.window_events
                    .write(WindowEvent::MouseButtonInput(message));
            }
            Input::Scroll(lines) => {
                self.wheel.write(MouseWheel {
                    unit: MouseScrollUnit::Line,
                    x: 0.0,
                    y: lines,
                    window: window_entity,
                    phase: bevy::input::touch::TouchPhase::Moved,
                });
            }
            Input::Key { key, pressed } => {
                let (key_code, logical_key) = key_codes(key);
                self.keys.write(KeyboardInput {
                    key_code,
                    logical_key,
                    state: if pressed {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Released
                    },
                    text: None,
                    repeat: false,
                    window: window_entity,
                });
            }
            Input::Text(text) => {
                if let Some(context) = egui_context {
                    self.egui.write(EguiInputEvent {
                        context,
                        event: egui::Event::Text(text),
                    });
                }
            }
        }
    }
}

/// 手順書のキーを、キーボードの位置 (KeyCode) と意味 (Key) に直す
fn key_codes(key: Key) -> (KeyCode, LogicalKey) {
    use LogicalKey as L;
    let named = |code, logical| (code, logical);
    match key {
        Key::Letter(c) => {
            const LETTERS: [KeyCode; 26] = [
                KeyCode::KeyA,
                KeyCode::KeyB,
                KeyCode::KeyC,
                KeyCode::KeyD,
                KeyCode::KeyE,
                KeyCode::KeyF,
                KeyCode::KeyG,
                KeyCode::KeyH,
                KeyCode::KeyI,
                KeyCode::KeyJ,
                KeyCode::KeyK,
                KeyCode::KeyL,
                KeyCode::KeyM,
                KeyCode::KeyN,
                KeyCode::KeyO,
                KeyCode::KeyP,
                KeyCode::KeyQ,
                KeyCode::KeyR,
                KeyCode::KeyS,
                KeyCode::KeyT,
                KeyCode::KeyU,
                KeyCode::KeyV,
                KeyCode::KeyW,
                KeyCode::KeyX,
                KeyCode::KeyY,
                KeyCode::KeyZ,
            ];
            let code = LETTERS[(c as u8 - b'A') as usize];
            (
                code,
                L::Character(c.to_ascii_lowercase().to_string().into()),
            )
        }
        Key::Digit(d) => {
            const DIGITS: [KeyCode; 10] = [
                KeyCode::Digit0,
                KeyCode::Digit1,
                KeyCode::Digit2,
                KeyCode::Digit3,
                KeyCode::Digit4,
                KeyCode::Digit5,
                KeyCode::Digit6,
                KeyCode::Digit7,
                KeyCode::Digit8,
                KeyCode::Digit9,
            ];
            (DIGITS[d as usize], L::Character(d.to_string().into()))
        }
        Key::F(n) => {
            const FS: [(KeyCode, LogicalKey); 12] = [
                (KeyCode::F1, L::F1),
                (KeyCode::F2, L::F2),
                (KeyCode::F3, L::F3),
                (KeyCode::F4, L::F4),
                (KeyCode::F5, L::F5),
                (KeyCode::F6, L::F6),
                (KeyCode::F7, L::F7),
                (KeyCode::F8, L::F8),
                (KeyCode::F9, L::F9),
                (KeyCode::F10, L::F10),
                (KeyCode::F11, L::F11),
                (KeyCode::F12, L::F12),
            ];
            FS[(n - 1) as usize].clone()
        }
        Key::Enter => named(KeyCode::Enter, L::Enter),
        Key::Escape => named(KeyCode::Escape, L::Escape),
        Key::Space => named(KeyCode::Space, L::Space),
        Key::Tab => named(KeyCode::Tab, L::Tab),
        Key::Backspace => named(KeyCode::Backspace, L::Backspace),
        Key::Delete => named(KeyCode::Delete, L::Delete),
        Key::Home => named(KeyCode::Home, L::Home),
        Key::End => named(KeyCode::End, L::End),
        Key::Left => named(KeyCode::ArrowLeft, L::ArrowLeft),
        Key::Right => named(KeyCode::ArrowRight, L::ArrowRight),
        Key::Up => named(KeyCode::ArrowUp, L::ArrowUp),
        Key::Down => named(KeyCode::ArrowDown, L::ArrowDown),
        Key::Ctrl => named(KeyCode::ControlLeft, L::Control),
        Key::Shift => named(KeyCode::ShiftLeft, L::Shift),
        Key::Alt => named(KeyCode::AltLeft, L::Alt),
    }
}
