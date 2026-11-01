//! 注視点を回るカメラ (Unity / Unreal のシーンビューと同じ操作)
//!
//! - Alt + 左ドラッグ: オービット / 中ドラッグ: パン / ホイール, Alt + 右ドラッグ: ズーム
//! - 右ドラッグ: その場で見回す。押している間 WASD で移動、Q / E で下降 / 上昇
//! - 透視投影と平行投影を切り替えられる。どちらでも上の操作はそのまま使える。
//! - 標準ビュー (正面・上面・等角など) と全体表示は `CameraRequest` で頼む。
//!   切り替えは少しだけアニメーションさせて、どこから来たか分かるようにする。

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

use bevy::{
    camera::{ScalingMode, Viewport},
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll},
    prelude::*,
    window::PrimaryWindow,
};

use crate::math::Aabb;
use crate::model::{Model, Selection};
use crate::view::gizmo::gizmo_not_focused;
use crate::view::recording::Recorder;
use cadsim_core::camera_path::CameraPose;

/// 透視投影の縦の画角 (Bevy の既定値と同じ 45°)
const FOV_Y: f32 = FRAC_PI_4;

/// 平行投影で描く奥行 (カメラの前後それぞれ、注視点までの距離の何倍まで描くか)。
/// 広すぎると深度の精度が落ちて、面の上の稜線が面に埋もれやすくなる。
const ORTHO_DEPTH_RATIO: f32 = 20.0;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OrbitCamera>()
            .init_resource::<ViewportRect>()
            .add_message::<CameraRequest>()
            .add_systems(Startup, spawn_camera)
            .add_systems(
                Update,
                (
                    mouse_camera_input
                        .run_if(pointer_in_3d_view)
                        .run_if(gizmo_not_focused),
                    fly_camera_input.run_if(keyboard_free),
                    handle_camera_requests,
                    animate_and_apply_camera,
                    fit_camera_viewport,
                )
                    .chain()
                    .in_set(CameraSystems),
            );
    }
}

/// カメラの入力と Transform への反映。動画の書き出しやプレビューはこれより前に視点を決める
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CameraSystems;

/// 6方向の標準ビューと等角ビュー
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StandardView {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Isometric,
}

impl StandardView {
    pub const ALL: [StandardView; 7] = [
        StandardView::Front,
        StandardView::Back,
        StandardView::Left,
        StandardView::Right,
        StandardView::Top,
        StandardView::Bottom,
        StandardView::Isometric,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StandardView::Front => "正面",
            StandardView::Back => "背面",
            StandardView::Left => "左側面",
            StandardView::Right => "右側面",
            StandardView::Top => "上面",
            StandardView::Bottom => "下面",
            StandardView::Isometric => "等角",
        }
    }

    /// テンキー風のショートカット
    pub fn shortcut(self) -> &'static str {
        match self {
            StandardView::Front => "1",
            StandardView::Back => "Ctrl+1",
            StandardView::Right => "3",
            StandardView::Left => "Ctrl+3",
            StandardView::Top => "7",
            StandardView::Bottom => "Ctrl+7",
            StandardView::Isometric => "0",
        }
    }

    /// (yaw, pitch)。正面ビューはカメラが +Z 側から -Z 方向を見る。
    fn angles(self) -> (f32, f32) {
        match self {
            StandardView::Front => (0.0, 0.0),
            StandardView::Back => (std::f32::consts::PI, 0.0),
            StandardView::Right => (FRAC_PI_2, 0.0),
            StandardView::Left => (-FRAC_PI_2, 0.0),
            StandardView::Top => (0.0, FRAC_PI_2),
            StandardView::Bottom => (0.0, -FRAC_PI_2),
            // 3軸が同じ長さに見える角度
            StandardView::Isometric => (FRAC_PI_4, (1.0f32 / 2f32.sqrt()).atan()),
        }
    }
}

/// UI やキー操作からカメラへの依頼
#[derive(Message, Clone, Copy)]
pub enum CameraRequest {
    /// 見る方向を変える (注視点と距離はそのまま)
    View(StandardView),
    /// 表示中のもの全体が収まるようにする
    FitAll,
    /// 選択中のものが収まるようにする
    FitSelection,
    /// true で平行投影、false で透視投影にする
    SetOrthographic(bool),
    /// 透視投影と平行投影を入れ替える
    ToggleProjection,
}

#[derive(Clone, Copy)]
struct Pose {
    focus: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
}

#[derive(Resource)]
pub struct OrbitCamera {
    pub focus: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    /// true なら平行投影、false なら透視投影
    pub orthographic: bool,
    /// アニメーションの行き先
    target: Option<Pose>,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            yaw: 0.8,
            pitch: 0.5,
            distance: 8.0,
            orthographic: false,
            target: None,
        }
    }
}

impl OrbitCamera {
    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, -self.pitch, 0.0)
    }

    pub fn eye(&self) -> Vec3 {
        self.focus + self.rotation() * Vec3::new(0.0, 0.0, self.distance)
    }

    fn pose(&self) -> Pose {
        let target = self.target.as_ref();
        Pose {
            focus: target.map_or(self.focus, |t| t.focus),
            yaw: target.map_or(self.yaw, |t| t.yaw),
            pitch: target.map_or(self.pitch, |t| t.pitch),
            distance: target.map_or(self.distance, |t| t.distance),
        }
    }

    /// 平行投影の画面の高さ (ワールド単位)。
    /// 透視投影で注視点の位置に見える高さと同じにするので、切り替えても注視点付近の大きさは
    /// 変わらず、距離を変えるズームがそのまま平行投影のズームになる。
    fn ortho_height(&self) -> f32 {
        2.0 * self.distance * (FOV_Y / 2.0).tan()
    }

    /// 手で操作したらアニメーションは止める
    fn cancel_animation(&mut self) {
        self.target = None;
    }

    /// 今の視点 (カメラワークの視点として覚える。アニメーション中なら行き先)
    pub fn camera_pose(&self) -> CameraPose {
        let p = self.pose();
        CameraPose {
            focus: p.focus,
            yaw: p.yaw,
            pitch: p.pitch,
            distance: p.distance,
        }
    }

    /// すぐにこの視点にする (動画の書き出しとプレビューで毎フレーム使う)
    pub fn set_camera_pose(&mut self, pose: CameraPose) {
        self.target = None;
        self.focus = pose.focus;
        self.yaw = pose.yaw;
        self.pitch = pose.pitch;
        self.distance = pose.distance;
    }

    /// 少しアニメーションしてこの視点に移る
    pub fn fly_to(&mut self, pose: CameraPose) {
        let turn = (pose.yaw - self.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        self.target = Some(Pose {
            focus: pose.focus,
            yaw: self.yaw + turn,
            pitch: pose.pitch,
            distance: pose.distance,
        });
    }
}

/// 3D ビューに使える画面上の領域 (パネルを除いた部分, 論理ピクセル)。UI が毎フレーム書く。
#[derive(Resource, Default, Clone, Copy)]
pub struct ViewportRect {
    pub rect: Option<Rect>,
    /// カーソルがパネルやビューキューブの上にある、または UI を操作中
    pub pointer_on_ui: bool,
    /// UI の入力欄に文字を打っている
    pub typing_in_ui: bool,
}

impl ViewportRect {
    /// ウィンドウ座標 (論理ピクセル) が 3D ビューの中にあるか
    pub fn contains(&self, window_pos: Vec2) -> bool {
        self.rect.is_none_or(|rect| rect.contains(window_pos))
    }
}

/// マウスで 3D ビューを操作してよいとき (クリック選択やカメラ操作の実行条件)
/// (動画の書き出し中はカメラを動かさない)
pub fn pointer_in_3d_view(viewport: Res<ViewportRect>, recorder: Res<Recorder>) -> bool {
    !viewport.pointer_on_ui && !recorder.is_capturing()
}

/// キーボードショートカットを受け付けてよいとき (入力欄に文字を打っている間は止める)
/// (動画の書き出し中も止める)
pub fn keyboard_free(viewport: Res<ViewportRect>, recorder: Res<Recorder>) -> bool {
    !viewport.typing_in_ui && !recorder.is_capturing()
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((Camera3d::default(), Transform::default()));
    commands.spawn((
        DirectionalLight {
            illuminance: 8_000.0,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 2_500.0,
            ..default()
        },
        Transform::from_xyz(-6.0, 3.0, -4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

pub fn alt_pressed(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight])
}

fn mouse_camera_input(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    mut orbit: ResMut<OrbitCamera>,
) {
    let delta = motion.delta;
    let alt = alt_pressed(&keys);
    let dragging = delta != Vec2::ZERO
        && (buttons.pressed(MouseButton::Middle)
            || buttons.pressed(MouseButton::Right)
            || (alt && buttons.pressed(MouseButton::Left)));
    if dragging || scroll.delta.y != 0.0 {
        orbit.cancel_animation();
    }

    if alt && buttons.pressed(MouseButton::Left) {
        // オービット: 注視点を中心に回る
        orbit.yaw -= delta.x * 0.005;
        orbit.pitch = (orbit.pitch + delta.y * 0.005).clamp(-FRAC_PI_2, FRAC_PI_2);
    } else if alt && buttons.pressed(MouseButton::Right) {
        // ドリー: 右ドラッグの上下でズーム
        orbit.distance *= 1.0 + (delta.x - delta.y) * 0.005;
    } else if buttons.pressed(MouseButton::Right) {
        // 見回し: カメラ位置を固定したまま向きだけ変える
        let eye = orbit.eye();
        orbit.yaw -= delta.x * 0.004;
        orbit.pitch = (orbit.pitch + delta.y * 0.004).clamp(-FRAC_PI_2, FRAC_PI_2);
        orbit.focus = eye - orbit.rotation() * Vec3::new(0.0, 0.0, orbit.distance);
    }
    if buttons.pressed(MouseButton::Middle) {
        let scale = orbit.distance * 0.0015;
        let pan = orbit.rotation() * Vec3::new(-delta.x * scale, delta.y * scale, 0.0);
        orbit.focus += pan;
    }
    if scroll.delta.y != 0.0 {
        orbit.distance *= 1.0 - scroll.delta.y.clamp(-3.0, 3.0) * 0.1;
    }
    orbit.distance = orbit.distance.clamp(1e-3, 1e6);
}

/// 右ボタンを押している間だけ WASD / QE でカメラを飛ばす
fn fly_camera_input(
    time: Res<Time>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut orbit: ResMut<OrbitCamera>,
) {
    if !buttons.pressed(MouseButton::Right) {
        return;
    }
    let mut dir = Vec3::ZERO;
    for (key, d) in [
        (KeyCode::KeyW, Vec3::NEG_Z),
        (KeyCode::KeyS, Vec3::Z),
        (KeyCode::KeyA, Vec3::NEG_X),
        (KeyCode::KeyD, Vec3::X),
    ] {
        if keys.pressed(key) {
            dir += orbit.rotation() * d;
        }
    }
    if keys.pressed(KeyCode::KeyE) {
        dir += Vec3::Y;
    }
    if keys.pressed(KeyCode::KeyQ) {
        dir -= Vec3::Y;
    }
    if dir == Vec3::ZERO {
        return;
    }
    let boost = if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
        3.0
    } else {
        1.0
    };
    // 速さは注視点までの距離に比例させる (mm の部品でも m の建物でも同じ感覚になる)
    let speed = orbit.distance * 0.5 * boost;
    orbit.cancel_animation();
    orbit.focus += dir.normalize() * speed * time.delta_secs();
}

fn handle_camera_requests(
    mut requests: MessageReader<CameraRequest>,
    mut orbit: ResMut<OrbitCamera>,
    model: Res<Model>,
    selection: Res<Selection>,
    camera: Single<&Projection, With<Camera3d>>,
) {
    for request in requests.read() {
        // 投影の切り替えは見る位置を変えないので、アニメーションさせずにすぐ反映する
        match *request {
            CameraRequest::SetOrthographic(orthographic) => {
                orbit.orthographic = orthographic;
                continue;
            }
            CameraRequest::ToggleProjection => {
                orbit.orthographic = !orbit.orthographic;
                continue;
            }
            _ => {}
        }
        let mut pose = orbit.pose();
        match request {
            CameraRequest::View(view) => {
                (pose.yaw, pose.pitch) = view.angles();
            }
            CameraRequest::FitAll => {
                let Some(bounds) = model.scene_bounds() else {
                    continue;
                };
                fit_pose(&mut pose, bounds, &camera);
            }
            CameraRequest::FitSelection => {
                let bounds = selection
                    .node
                    .and_then(|id| model.world_bounds(id))
                    .or_else(|| model.scene_bounds());
                let Some(bounds) = bounds else {
                    continue;
                };
                fit_pose(&mut pose, bounds, &camera);
            }
            CameraRequest::SetOrthographic(_) | CameraRequest::ToggleProjection => {
                unreachable!("上で処理済み")
            }
        }
        // ヨーは近い向きから回るようにする (359° → 1° で一周しない)
        let turn = (pose.yaw - orbit.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        pose.yaw = orbit.yaw + turn;
        orbit.target = Some(pose);
    }
}

/// 外接球がちょうど画面に収まる距離にする
fn fit_pose(pose: &mut Pose, bounds: Aabb, projection: &Projection) {
    // 平行投影は画面の高さを透視投影の画角から決めているので、同じ画角で計算すればよい
    let fov = match projection {
        Projection::Perspective(p) => p.fov.min(p.fov * p.aspect_ratio),
        _ => FOV_Y,
    };
    let radius = (bounds.size().length() / 2.0).max(1e-6) as f32;
    pose.focus = bounds.center().as_vec3();
    pose.distance = radius / (fov / 2.0).sin() * 1.3;
}

fn animate_and_apply_camera(
    time: Res<Time>,
    mut orbit: ResMut<OrbitCamera>,
    camera: Single<(&mut Transform, &mut Projection), With<Camera3d>>,
) {
    if let Some(target) = orbit.target {
        // 指数的に近づける (約 0.25 秒でほぼ到着)
        let t = 1.0 - (-time.delta_secs() * 18.0).exp();
        orbit.focus = orbit.focus.lerp(target.focus, t);
        orbit.yaw += (target.yaw - orbit.yaw) * t;
        orbit.pitch += (target.pitch - orbit.pitch) * t;
        orbit.distance += (target.distance - orbit.distance) * t;
        let close = orbit.focus.distance(target.focus) < target.distance * 1e-4
            && (target.yaw - orbit.yaw).abs() < 1e-4
            && (target.pitch - orbit.pitch).abs() < 1e-4
            && (target.distance - orbit.distance).abs() < target.distance * 1e-4;
        if close {
            orbit.focus = target.focus;
            orbit.yaw = target.yaw;
            orbit.pitch = target.pitch;
            orbit.distance = target.distance;
            orbit.target = None;
        }
    }

    let (mut transform, mut projection) = camera.into_inner();
    transform.translation = orbit.eye();
    transform.rotation = orbit.rotation();
    apply_projection(&orbit, &mut projection);
}

/// OrbitCamera の投影モードと距離を Bevy の Projection に写す
fn apply_projection(orbit: &OrbitCamera, projection: &mut Projection) {
    match (orbit.orthographic, &mut *projection) {
        (false, Projection::Perspective(p)) => {
            // mm 単位の大きな部品でも小さな部品でも欠けないよう、近クリップ面を距離に合わせる
            p.near = (orbit.distance * 1e-3).max(1e-5);
        }
        (true, Projection::Orthographic(p)) => {
            p.scaling_mode = ScalingMode::FixedVertical {
                viewport_height: orbit.ortho_height(),
            };
            // 平行投影ではカメラの後ろも描く。ズームやフライ移動でカメラが物体を
            // 通り越しても、物体が消えないようにするため。
            p.far = orbit.distance * ORTHO_DEPTH_RATIO;
            p.near = -p.far;
        }
        // モードが変わったときだけ作り直す
        (false, _) => {
            *projection = Projection::Perspective(PerspectiveProjection {
                fov: FOV_Y,
                ..default()
            });
            apply_projection(orbit, projection);
        }
        (true, _) => {
            *projection = Projection::Orthographic(OrthographicProjection::default_3d());
            apply_projection(orbit, projection);
        }
    }
}

/// カメラの描画範囲を左右のパネルを除いた領域に合わせる
fn fit_camera_viewport(
    rect: Res<ViewportRect>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut camera: Single<&mut Camera, With<Camera3d>>,
) {
    let Some(rect) = rect.rect else {
        return;
    };
    let scale = window.scale_factor();
    let physical = window.physical_size();
    let min = (rect.min * scale).as_uvec2().min(physical);
    let max = (rect.max * scale).as_uvec2().min(physical);
    if max.x <= min.x || max.y <= min.y {
        return;
    }
    let viewport = Viewport {
        physical_position: min,
        physical_size: max - min,
        ..default()
    };
    if camera
        .viewport
        .as_ref()
        .map(|v| (v.physical_position, v.physical_size))
        != Some((viewport.physical_position, viewport.physical_size))
    {
        camera.viewport = Some(viewport);
    }
}
