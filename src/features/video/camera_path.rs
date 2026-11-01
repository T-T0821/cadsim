//! 紹介動画のカメラワーク
//!
//! 通り道の計算は `plantwright_core::camera_path` が受け持つ。ここでは
//! - 通り道を Resource に包み、元に戻す履歴に載せる
//! - 動画の書き出し窓の「カメラワーク」欄 (視点を足す・時刻を変える・見る・消す、自動で作る)
//! - 3D ビューに通り道を線で描く
//! - プレビュー (通り道どおりにカメラを動かし、チャートも同じ速さで再生する)
//!
//! だけを行う。書き出し中にカメラを動かすのは `recording` が受け持つ。

use bevy::prelude::*;
use bevy_egui::egui;

use crate::document::history::{TrackDocExt, TrackedDoc};
use crate::features::motion::{ChartSchedule, MotionSet, Playback};
use crate::features::video::recording::{RecordSettings, Recorder};
use crate::viewport::camera::{CameraSystems, OrbitCamera};
use plantwright_core::camera_path::{CameraPath, CameraPose, Interpolation};

pub struct CameraPathPlugin;

impl Plugin for CameraPathPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraPathRes>()
            .track_doc::<CameraPathDoc>()
            .init_resource::<CameraPathUi>()
            .add_systems(
                Update,
                (
                    run_preview.before(CameraSystems).before(MotionSet::Pose),
                    draw_path,
                ),
            );
    }
}

/// カメラの通り道 (プロジェクトに保存する設計データ)
#[derive(Resource, Default, Deref, DerefMut)]
pub struct CameraPathRes(pub CameraPath);

/// カメラワークの編集を元に戻す履歴に載せる
struct CameraPathDoc;

impl TrackedDoc for CameraPathDoc {
    type Value = CameraPath;
    const LABEL: &'static str = "カメラワークを編集";

    fn read(world: &World) -> CameraPath {
        world.resource::<CameraPathRes>().0.clone()
    }

    fn write(world: &mut World, value: CameraPath) {
        world.resource_mut::<CameraPathRes>().0 = value;
    }

    fn same(a: &CameraPath, b: &CameraPath) -> bool {
        a == b
    }
}

/// 画面の状態 (保存しない)
#[derive(Resource)]
pub struct CameraPathUi {
    /// 3D ビューで強調する視点
    selected: Option<usize>,
    /// 自動で作るときの長さ (秒)
    auto_seconds: f64,
    preview: Option<Preview>,
}

impl Default for CameraPathUi {
    fn default() -> Self {
        Self {
            selected: None,
            auto_seconds: 8.0,
            preview: None,
        }
    }
}

impl CameraPathUi {
    pub fn is_previewing(&self) -> bool {
        self.preview.is_some()
    }
}

struct Preview {
    /// 通り道の頭からの秒数
    time: f64,
    /// チャートの再生速度の倍率 (書き出しの設定と同じ)
    speed: f64,
    /// 終わったら戻す視点と再生状態
    saved_pose: CameraPose,
    saved_playback: Playback,
}

/// プレビュー中は通り道どおりにカメラを動かす。終わるか Esc で元の視点に戻す
#[allow(clippy::too_many_arguments)]
fn run_preview(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    path: Res<CameraPathRes>,
    schedule: Res<ChartSchedule>,
    mut ui: ResMut<CameraPathUi>,
    mut orbit: ResMut<OrbitCamera>,
    mut playback: ResMut<Playback>,
    recorder: Res<Recorder>,
) {
    let Some(preview) = ui.preview.as_mut() else {
        return;
    };
    let stop = keys.just_pressed(KeyCode::Escape)
        || recorder.is_busy()
        || !path.is_playable()
        || preview.time > path.duration();
    if stop {
        stop_preview(&mut ui, &mut orbit, &mut playback);
        return;
    }
    if let Some(pose) = path.sample(preview.time) {
        orbit.set_camera_pose(pose);
    }
    if schedule.cycle_time > 0.0 {
        playback.playing = false;
        playback.time = (preview.time * preview.speed) % schedule.cycle_time;
    }
    preview.time += f64::from(time.delta_secs());
}

fn stop_preview(ui: &mut CameraPathUi, orbit: &mut OrbitCamera, playback: &mut Playback) {
    if let Some(preview) = ui.preview.take() {
        orbit.set_camera_pose(preview.saved_pose);
        *playback = preview.saved_playback;
    }
}

const PATH_COLOR: Color = Color::srgb(1.0, 0.75, 0.1);
const KEY_COLOR: Color = Color::srgb(1.0, 0.45, 0.1);
const SELECTED_COLOR: Color = Color::srgb(0.2, 0.8, 1.0);

/// 書き出しの窓を開いている間、カメラの通り道 (カメラの位置の軌跡) と視点の位置を描く
fn draw_path(
    mut gizmos: Gizmos,
    path: Res<CameraPathRes>,
    ui: Res<CameraPathUi>,
    settings: Res<RecordSettings>,
    recorder: Res<Recorder>,
    orbit: Res<OrbitCamera>,
) {
    // 撮影中とプレビュー中は映り込むので描かない
    if !settings.open || recorder.is_capturing() || ui.is_previewing() || path.is_empty() {
        return;
    }
    let line = path.eye_polyline(200);
    if line.len() >= 2 {
        gizmos.linestrip(line, PATH_COLOR);
    }
    for (i, key) in path.keys().iter().enumerate() {
        let eye = key.pose.eye();
        // 見ている距離に合わせた大きさにして、遠くの視点も近くの視点も同じくらいに見せる
        let size = eye.distance(orbit.eye()).max(1e-3) * 0.012;
        let color = if ui.selected == Some(i) {
            SELECTED_COLOR
        } else {
            KEY_COLOR
        };
        gizmos.sphere(Isometry3d::from_translation(eye), size, color);
        // 見ている向き (注視点へ向かう短い線)
        let toward = (key.pose.focus - eye).normalize_or_zero();
        gizmos.line(eye, eye + toward * size * 6.0, color);
    }
}

// ---------------------------------------------------------------------------
// 書き出し窓の「カメラワーク」欄
// ---------------------------------------------------------------------------

/// 書き出し窓に出すカメラワークの欄。`use_path` は「通り道に沿って撮る」のチェック
#[allow(clippy::too_many_arguments)]
pub fn camera_path_ui(
    ui: &mut egui::Ui,
    path: &mut CameraPathRes,
    state: &mut CameraPathUi,
    orbit: &mut OrbitCamera,
    playback: &mut Playback,
    use_path: &mut bool,
    speed: f64,
    busy: bool,
) {
    let playable = path.is_playable();
    ui.add_enabled_ui(playable, |ui| {
        ui.checkbox(use_path, "カメラの通り道に沿って撮る")
            .on_hover_text("置いた視点の間をカメラがなめらかに動きます");
    });
    if !playable {
        ui.weak("視点を 2 つ以上置くと、その間をカメラが動きます");
    }

    ui.horizontal_wrapped(|ui| {
        if ui
            .button("＋ 今の視点を足す")
            .on_hover_text("3D ビューの今の見え方を、最後の視点の 3 秒あとに置きます")
            .clicked()
        {
            let index = path.push(orbit.camera_pose());
            state.selected = Some(index);
            if path.is_playable() {
                *use_path = true;
            }
        }
        if ui
            .button("ぐるっと一周")
            .on_hover_text("今の注視点のまわりを一周する通り道を作ります (今の視点は置き換えます)")
            .clicked()
        {
            path.0 = CameraPath::turntable(orbit.camera_pose(), state.auto_seconds);
            state.selected = None;
            *use_path = true;
        }
        if ui
            .button("遠くから寄る")
            .on_hover_text("遠くの見下ろしから、今の視点まで寄ってくる通り道を作ります")
            .clicked()
        {
            let target = orbit.camera_pose();
            let overview = CameraPose {
                distance: target.distance * 3.0,
                pitch: (target.pitch + 0.5).min(1.2),
                yaw: target.yaw - 0.6,
                ..target
            };
            path.0 = CameraPath::fly_in(overview, target, state.auto_seconds);
            state.selected = None;
            *use_path = true;
        }
        ui.add(
            egui::DragValue::new(&mut state.auto_seconds)
                .range(2.0..=120.0)
                .speed(0.5)
                .suffix(" 秒"),
        )
        .on_hover_text("自動で作る通り道の長さ");
    });

    if path.is_empty() {
        return;
    }

    let mut edit = None;
    let mut retime = None;
    egui::ScrollArea::vertical()
        .max_height(160.0)
        .id_salt("camera_keys")
        .show(ui, |ui| {
            egui::Grid::new("camera_keys_grid")
                .num_columns(4)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    for (i, key) in path.keys().iter().enumerate() {
                        let selected = state.selected == Some(i);
                        if ui
                            .selectable_label(selected, format!("視点 {}", i + 1))
                            .on_hover_text("この視点を 3D ビューで強調します")
                            .clicked()
                        {
                            state.selected = (!selected).then_some(i);
                        }
                        let mut time = key.time;
                        if ui
                            .add(
                                egui::DragValue::new(&mut time)
                                    .range(0.0..=600.0)
                                    .speed(0.1)
                                    .max_decimals(1)
                                    .suffix(" 秒"),
                            )
                            .changed()
                        {
                            retime = Some((i, time));
                        }
                        ui.horizontal(|ui| {
                            if ui
                                .small_button("見る")
                                .on_hover_text("3D ビューをこの視点にします")
                                .clicked()
                            {
                                orbit.fly_to(key.pose);
                                state.selected = Some(i);
                            }
                            if ui
                                .small_button("今の視点に")
                                .on_hover_text("この視点を 3D ビューの今の見え方で置き換えます")
                                .clicked()
                            {
                                state.selected = Some(i);
                                edit = Some(Edit::SetPose(i));
                            }
                        });
                        if ui
                            .small_button("削除")
                            .on_hover_text("この視点を消します")
                            .clicked()
                        {
                            edit = Some(Edit::Remove(i));
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some((i, time)) = retime {
        let moved = path.set_time(i, time);
        state.selected = Some(moved);
    }
    match edit {
        Some(Edit::Remove(i)) => {
            path.remove(i);
            state.selected = None;
        }
        Some(Edit::SetPose(i)) => path.set_pose(i, orbit.camera_pose()),
        None => {}
    }

    ui.horizontal(|ui| {
        ui.label("つなぎ方");
        for interpolation in Interpolation::ALL {
            ui.selectable_value(
                &mut path.interpolation,
                interpolation,
                interpolation.label(),
            );
        }
    });
    ui.horizontal(|ui| {
        ui.label("通り道の長さ");
        let mut seconds = path.duration();
        if ui
            .add_enabled(
                playable,
                egui::DragValue::new(&mut seconds)
                    .range(1.0..=600.0)
                    .speed(0.2)
                    .max_decimals(1)
                    .suffix(" 秒"),
            )
            .on_hover_text("視点の間隔の比はそのままで、全体の長さを変えます")
            .changed()
        {
            path.scale_to(seconds);
        }
    });
    ui.horizontal(|ui| {
        if state.is_previewing() {
            if ui.button("■ プレビューを止める").clicked() {
                stop_preview(state, orbit, playback);
            }
        } else if ui
            .add_enabled(playable && !busy, egui::Button::new("▶ プレビュー"))
            .on_hover_text("通り道どおりにカメラを動かして見せます (Esc で止める)")
            .clicked()
        {
            state.preview = Some(Preview {
                time: 0.0,
                speed,
                saved_pose: orbit.camera_pose(),
                saved_playback: *playback,
            });
        }
        if ui.button("すべて消す").clicked() {
            path.clear();
            state.selected = None;
            *use_path = false;
        }
    });
}

enum Edit {
    Remove(usize),
    SetPose(usize),
}
