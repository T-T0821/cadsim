//! 一人称で工場を歩く (歩行モード) の画面
//!
//! 入力 (WASD・矢印キー・右ドラッグ) を集めて `plantwright_core::walk` に渡し、
//! 返ってきた位置と向きを 3D カメラに写す。通路幅・前方の距離・手の届く部品は
//! 3D ビューの左上の表示と、床の線 (ギズモ) に出す。
//! 歩いている間はカメラのオービット操作とキーのショートカットを止める。
//! 障害物は表示中の部品の外接箱と、安全柵・非常停止の柱。歩いている人は安全機器の
//! 侵入物にも入れるので、エリアセンサやライトカーテンに歩いて入ると設備が止まる。

use bevy::{input::mouse::AccumulatedMouseMotion, prelude::*};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use glam::{DVec2, DVec3};
use plantwright_core::floor::geom::{Rect2, Zone};
use plantwright_core::math::Aabb;
use plantwright_core::safety::device::DeviceKind;
use plantwright_core::walk::{self, Survey, WalkBody, WalkInput, Walker};

use crate::features::safety::{Intruder, SafetyIntruders, SafetyLayoutRes, SafetySet};
use crate::viewport::camera::{
    CameraRequest, FOV_Y, OrbitCamera, ViewportRect, handle_camera_requests, keyboard_free,
    pointer_in_3d_view,
};
use plantwright_core::model::Model;
use plantwright_core::site::underlay::unit_in_mm;

/// 歩いているときの縦の画角 (人の視野に近づけて、普段の 45° より広くする)
const WALK_FOV_Y: f32 = 65f32.to_radians();
/// 通路幅などを測る最大の距離 (m)
const SURVEY_RANGE_M: f64 = 20.0;
/// 右ドラッグで見回す速さ (ラジアン/ピクセル)
const LOOK_SPEED: f64 = 0.004;
/// 矢印キーで向きを変える速さ (ラジアン/秒)
const TURN_SPEED: f64 = 1.8;

const AISLE_OK: Color = Color::srgb(0.35, 0.85, 0.45);
const AISLE_NARROW: Color = Color::srgb(0.95, 0.3, 0.25);
const REACH_COLOR: Color = Color::srgb(0.3, 0.75, 1.0);

pub struct WalkPlugin;

impl Plugin for WalkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WalkMode>()
            .add_systems(
                Update,
                (
                    collect_obstacles,
                    toggle_walk,
                    walk_input
                        .run_if(walking)
                        .run_if(keyboard_free)
                        .run_if(pointer_in_3d_view),
                    apply_walk_camera.run_if(walking),
                    draw_guides.run_if(walking),
                )
                    .chain()
                    // 標準ビューなどの依頼で歩行モードを抜けてから、普段のカメラが依頼を処理する
                    .before(handle_camera_requests),
            )
            .add_systems(
                Update,
                add_walker_intruder
                    .run_if(walking)
                    .in_set(SafetySet::Intruders),
            )
            .add_systems(EguiPrimaryContextPass, walk_hud.run_if(walking));
    }
}

/// 歩行モードの状態
#[derive(Resource, Default)]
pub struct WalkMode {
    /// 歩いている人。None なら歩行モードではない
    pub walker: Option<Walker>,
    /// 目の高さ (mm)。0 なら既定の 1600 mm
    pub eye_height_mm: f64,
    /// 歩行モードの入り・出を頼む (メニューやボタンから)
    pub toggle_requested: bool,
    /// 最後に測った結果
    pub survey: Survey,
    /// 障害物 (表示中の部品の外接箱・安全柵) と、その名前
    obstacles: Vec<Zone>,
    obstacle_names: Vec<String>,
    /// モデルの 1 単位が何 mm か
    unit_mm: f64,
}

impl WalkMode {
    pub fn active(&self) -> bool {
        self.walker.is_some()
    }

    pub fn toggle(&mut self) {
        self.toggle_requested = true;
    }

    fn body(&self) -> WalkBody {
        let mut body = WalkBody::for_unit(self.unit_mm.max(1e-9));
        if self.eye_height_mm > 0.0 {
            let scale = self.eye_height_mm / 1600.0;
            body.eye_height *= scale;
            body.head_height *= scale;
            body.shoulder_height *= scale;
        }
        body
    }

    /// 1 m がモデルの何単位か
    pub fn units_per_meter(&self) -> f64 {
        1000.0 / self.unit_mm.max(1e-9)
    }

    /// モデルの長さを m の文字にする
    fn meters(&self, length: f64) -> String {
        format!("{:.2} m", length * self.unit_mm / 1000.0)
    }
}

/// 歩行モードのとき (カメラ・ショートカットの実行条件)
pub fn walking(walk: Res<WalkMode>) -> bool {
    walk.active()
}

/// 歩行モードでないとき
pub fn not_walking(walk: Res<WalkMode>) -> bool {
    !walk.active()
}

/// 障害物を集める: 表示中の部品の外接箱と、安全柵・非常停止の柱 (安全機器の寸法は mm)
fn obstacles(model: &Model, safety: &SafetyLayoutRes) -> (Vec<Zone>, Vec<String>) {
    let mut zones = Vec::new();
    let mut names = Vec::new();
    model.visit_parts(|part| {
        if let (true, Some(local)) = (part.shown, part.local_bounds) {
            zones.push(walk::zone_from_aabb(&local.transformed(part.world)));
            names.push(
                model
                    .get(part.id)
                    .map_or_else(String::new, |n| n.name.clone()),
            );
        }
    });
    let mm = 1.0 / unit_in_mm(&model.unit).max(1e-9);
    for device in &safety.devices {
        if !matches!(device.kind, DeviceKind::Fence | DeviceKind::EmergencyStop) {
            continue;
        }
        let footprint = device.footprint();
        zones.push(Zone {
            footprint: Rect2 {
                center: footprint.center * mm,
                half: footprint.half * mm,
                yaw: footprint.yaw,
            },
            y_min: device.position[1] * mm,
            y_max: (device.position[1] + device.size[1]) * mm,
        });
        names.push(device.name.clone());
    }
    (zones, names)
}

/// 障害物を集め直す (モデルか安全機器が変わったときだけ)
fn collect_obstacles(model: Res<Model>, safety: Res<SafetyLayoutRes>, mut walk: ResMut<WalkMode>) {
    if !walk.active() || !(model.is_changed() || safety.is_changed() || walk.obstacles.is_empty()) {
        return;
    }
    let (zones, names) = obstacles(&model, &safety);
    let walk = walk.bypass_change_detection();
    walk.obstacles = zones;
    walk.obstacle_names = names;
    walk.unit_mm = unit_in_mm(&model.unit);
}

/// 歩いている人を安全機器の侵入物に入れる (エリアセンサなどに入ると止まる)
fn add_walker_intruder(walk: Res<WalkMode>, mut intruders: ResMut<SafetyIntruders>) {
    let Some(walker) = walk.walker else {
        return;
    };
    let body = walk.body();
    let r = DVec3::new(body.radius, 0.0, body.radius);
    intruders.0.push(Intruder {
        label: "歩いている人 (一人称)".into(),
        bounds: Aabb {
            min: walker.feet - r,
            max: walker.feet + r + DVec3::Y * body.head_height,
        },
    });
}

/// 歩行モードに入る・出る
fn toggle_walk(
    keys: Res<ButtonInput<KeyCode>>,
    mut requests: MessageReader<CameraRequest>,
    model: Res<Model>,
    safety: Res<SafetyLayoutRes>,
    mut walk: ResMut<WalkMode>,
    mut orbit: ResMut<OrbitCamera>,
    mut projection: Single<&mut Projection, With<Camera3d>>,
) {
    // 標準ビューや全体表示を頼まれたら、歩くのをやめてそちらへ動く
    let view_requested = requests.read().any(|r| {
        matches!(
            r,
            CameraRequest::View(_)
                | CameraRequest::FitAll
                | CameraRequest::FitSelection
                | CameraRequest::SetOrthographic(true)
                | CameraRequest::ToggleProjection
        )
    });
    let requested = std::mem::take(&mut walk.toggle_requested);
    let esc = keys.just_pressed(KeyCode::Escape);
    match walk.walker {
        Some(walker) if requested || esc || view_requested => {
            // 歩いていた目の位置から、同じ向きで普段のカメラに戻す
            let body = walk.body();
            let eye = walker.eye(&body).as_vec3();
            orbit.yaw = walker.yaw as f32;
            orbit.pitch = walker.pitch as f32;
            orbit.orthographic = false;
            orbit.focus = eye - orbit.rotation() * Vec3::new(0.0, 0.0, orbit.distance);
            if let Projection::Perspective(p) = &mut **projection {
                p.fov = FOV_Y;
            }
            walk.walker = None;
            walk.survey = Survey::default();
        }
        None if requested => {
            walk.unit_mm = unit_in_mm(&model.unit);
            let (zones, names) = obstacles(&model, &safety);
            // 普段のカメラが見ていた場所 (注視点の真下の床) に立ち、同じ向きを向く
            let body = walk.body();
            let start = orbit.focus.as_dvec3();
            walk.walker = Some(walk::place(start, orbit.yaw as f64, &body, &zones));
            walk.obstacles = zones;
            walk.obstacle_names = names;
        }
        _ => {}
    }
}

/// キーとマウスで歩く・見回す
fn walk_input(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut walk: ResMut<WalkMode>,
) {
    let pressed = |codes: &[KeyCode]| keys.any_pressed(codes.iter().copied());
    let axis = |plus: &[KeyCode], minus: &[KeyCode]| {
        f64::from(u8::from(pressed(plus))) - f64::from(u8::from(pressed(minus)))
    };
    let dt = time.delta_secs_f64();
    let mut input = WalkInput {
        forward: axis(
            &[KeyCode::KeyW, KeyCode::ArrowUp],
            &[KeyCode::KeyS, KeyCode::ArrowDown],
        ),
        right: axis(&[KeyCode::KeyD], &[KeyCode::KeyA]),
        run: pressed(&[KeyCode::ShiftLeft, KeyCode::ShiftRight]),
        turn: axis(&[KeyCode::ArrowLeft], &[KeyCode::ArrowRight]) * TURN_SPEED * dt,
        look_down: 0.0,
    };
    if buttons.pressed(MouseButton::Right) {
        input.turn -= f64::from(motion.delta.x) * LOOK_SPEED;
        input.look_down += f64::from(motion.delta.y) * LOOK_SPEED;
    }
    let body = walk.body();
    let Some(mut walker) = walk.walker else {
        return;
    };
    walk::step(&mut walker, &input, &body, &walk.obstacles, dt);
    walk.walker = Some(walker);
}

/// 歩いている人の目にカメラを置き、まわりを測る
fn apply_walk_camera(
    mut walk: ResMut<WalkMode>,
    camera: Single<(&mut Transform, &mut Projection), With<Camera3d>>,
) {
    let Some(walker) = walk.walker else {
        return;
    };
    let body = walk.body();
    let (mut transform, mut projection) = camera.into_inner();
    transform.translation = walker.eye(&body).as_vec3();
    transform.rotation = Quat::from_euler(
        EulerRot::YXZ,
        walker.yaw as f32,
        -(walker.pitch as f32),
        0.0,
    );
    let near = (body.radius * 0.05) as f32;
    match &mut *projection {
        Projection::Perspective(p) => {
            p.fov = WALK_FOV_Y;
            p.near = near;
        }
        _ => {
            *projection = Projection::Perspective(PerspectiveProjection {
                fov: WALK_FOV_Y,
                near,
                ..default()
            });
        }
    }
    let range = SURVEY_RANGE_M * 1000.0 / walk.unit_mm.max(1e-9);
    walk.survey = walk::survey(&walker, &body, &walk.obstacles, range);
}

/// 通路幅の線・手の届く範囲の円・手の届く部品の箱を描く
fn draw_guides(mut gizmos: Gizmos, walk: Res<WalkMode>) {
    let Some(walker) = walk.walker else {
        return;
    };
    let body = walk.body();
    let survey = &walk.survey;
    let lift = |p: DVec3, h: f64| (p + DVec3::Y * h).as_vec3();
    // 床から少し浮かせて、通路幅を左右の障害物の間の線で示す
    let line_y = body.radius * 0.1;
    if let (Some(left), Some(right)) = (survey.left, survey.right) {
        let color = if survey.aisle_narrow {
            AISLE_NARROW
        } else {
            AISLE_OK
        };
        let a = walker.feet - walker.right() * left;
        let b = walker.feet + walker.right() * right;
        gizmos.line(lift(a, line_y), lift(b, line_y), color);
        for end in [a, b] {
            gizmos.line(lift(end, line_y), lift(end, body.shoulder_height), color);
        }
    }
    // 手の届く範囲 (肩を中心にした腕の長さ) を床に写した円
    gizmos.circle(
        Isometry3d::new(
            lift(walker.feet, line_y),
            Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
        ),
        body.reach as f32,
        REACH_COLOR,
    );
    for &i in &survey.in_reach {
        if let Some(z) = walk.obstacles.get(i) {
            let f = &z.footprint;
            let center = DVec3::new(f.center.x, (z.y_min + z.y_max) / 2.0, f.center.y);
            let size = DVec2::new(f.half.x * 2.0, f.half.y * 2.0);
            gizmos.cube(
                Transform::from_translation(center.as_vec3())
                    .with_rotation(Quat::from_rotation_y(f.yaw as f32))
                    .with_scale(Vec3::new(
                        size.x as f32,
                        (z.y_max - z.y_min) as f32,
                        size.y as f32,
                    )),
                REACH_COLOR,
            );
        }
    }
}

/// 3D ビューの左上に、測った値と操作の案内を出す
fn walk_hud(
    mut contexts: EguiContexts,
    mut walk: ResMut<WalkMode>,
    viewport: Res<ViewportRect>,
) -> Result {
    let Some(rect) = viewport.rect else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?.clone();
    let survey = walk.survey.clone();
    let body = walk.body();
    let mut toggle = false;
    let mut eye_mm = if walk.eye_height_mm > 0.0 {
        walk.eye_height_mm
    } else {
        1600.0
    };
    egui::Area::new("walk_hud".into())
        .fixed_pos(egui::pos2(rect.min.x + 12.0, rect.min.y + 12.0))
        .order(egui::Order::Foreground)
        .show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(260.0);
                ui.strong("🚶 歩いて見る");
                egui::Grid::new("walk_grid").num_columns(2).show(ui, |ui| {
                    ui.label("通路幅");
                    match survey.aisle_width {
                        Some(w) => {
                            let text = walk.meters(w);
                            if survey.aisle_narrow {
                                ui.colored_label(
                                    egui::Color32::from_rgb(240, 80, 70),
                                    format!("{text} (狭い: {} 未満)", walk.meters(body.min_aisle)),
                                );
                            } else {
                                ui.colored_label(egui::Color32::from_rgb(90, 215, 115), text);
                            }
                        }
                        None => {
                            ui.label("広い (左右 20 m 以内に壁なし)");
                        }
                    }
                    ui.end_row();
                    ui.label("前の設備まで");
                    ui.label(survey.ahead.map_or("20 m 以上".into(), |d| walk.meters(d)));
                    ui.end_row();
                    ui.label("手が届く");
                    if survey.in_reach.is_empty() {
                        ui.label("なし");
                    } else {
                        let names: Vec<String> = survey
                            .in_reach
                            .iter()
                            .take(3)
                            .filter_map(|&i| walk.obstacle_names.get(i).cloned())
                            .collect();
                        let more = survey.in_reach.len().saturating_sub(names.len());
                        let mut text = names.join("、");
                        if more > 0 {
                            text.push_str(&format!(" ほか {more} 個"));
                        }
                        ui.label(text);
                    }
                    ui.end_row();
                    ui.label("目の高さ");
                    ui.add(
                        egui::DragValue::new(&mut eye_mm)
                            .range(1000.0..=2000.0)
                            .speed(10.0)
                            .suffix(" mm"),
                    )
                    .on_hover_text("作業者の背丈に合わせて変えると、見通しを確かめられます");
                    ui.end_row();
                });
                ui.separator();
                ui.small("W A S D / ↑↓: 歩く   Shift: 走る");
                ui.small("右ドラッグ / ←→: 見回す");
                if ui.button("歩くのをやめる (Esc)").clicked() {
                    toggle = true;
                }
            });
        });
    if (eye_mm - 1600.0).abs() > 1e-6 || walk.eye_height_mm > 0.0 {
        walk.eye_height_mm = eye_mm;
    }
    if toggle {
        walk.toggle();
    }
    Ok(())
}
