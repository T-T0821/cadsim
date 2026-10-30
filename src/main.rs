//! 最小CADプロトタイプ: 箱を作る・選ぶ・回す
//!
//! 形状の「正」は truck の B-rep (f64) が持ち、Bevy は表示用の f32 メッシュと操作だけを担う。
//! 部品を編集するたびに truck で形状を作り直し、面ごとにテッセレーションして Bevy に渡す。
//!
//! 操作 (Unity / Unreal のシーンビューに合わせている):
//! - 左クリック: 面を選択 (その面を含む箱が選択される)
//! - Alt + 左ドラッグ: 注視点のまわりを回転 (オービット)
//! - 中ドラッグ: 平行移動 (パン)
//! - ホイール / Alt + 右ドラッグ: ズーム
//! - 右ドラッグ: その場で見回す。押している間 WASD で移動、Q / E で下降 / 上昇、Shift で加速
//! - F: 選択中の箱にフォーカス / Delete: 選択中の箱を削除
//! - W / E / R: 移動 / 回転 / 拡大縮小のギズモに切り替え (ギズモの軸を左ドラッグ)
//! - Ctrl + ギズモをドラッグ: 一定量ずつ動かす (刻み幅はパネルで設定)
//! - 左パネル: 箱の追加、寸法・位置・回転の編集、削除

// Windows のリリースビルドでは、起動時に黒いコンソール窓を出さない
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::collections::HashSet;

use bevy::{
    asset::RenderAssetUsages,
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll},
    mesh::PrimitiveTopology,
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
    window::PrimaryWindow,
};
// truck の prelude にも Transform トレイトがあるため Bevy 側を明示する
use bevy::prelude::Transform;
use bevy_egui::{
    EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui,
    input::{egui_wants_any_keyboard_input, egui_wants_any_pointer_input},
};
use truck_meshalgo::prelude::*;
use truck_modeling::{Solid, builder};

mod gizmo;
mod japanese_font;
use gizmo::{GizmoTool, Tool, gizmo_not_focused, snap_settings_ui, transform_gizmo_system};

/// テッセレーションの許容誤差 (モデル単位)
const TESSELLATION_TOL: f64 = 0.005;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "CADプロトタイプ".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .init_resource::<Model>()
        .init_resource::<Selection>()
        .init_resource::<OrbitCamera>()
        .init_resource::<GizmoTool>()
        .add_systems(Startup, setup)
        .add_systems(
            EguiPrimaryContextPass,
            (
                japanese_font::install_japanese_font,
                ui_panel,
                transform_gizmo_system,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (
                pick_face
                    .run_if(not(egui_wants_any_pointer_input))
                    .run_if(gizmo_not_focused),
                mouse_camera_input.run_if(not(egui_wants_any_pointer_input)),
                keyboard_input.run_if(not(egui_wants_any_keyboard_input)),
                rebuild_dirty_parts,
                apply_camera,
                update_materials,
                draw_edges_and_grid,
            )
                .chain(),
        )
        .run();
}

// ---------------------------------------------------------------------------
// モデル (f64 の設計データ)
// ---------------------------------------------------------------------------

/// 箱1つ分の設計パラメータ。値はすべて f64 で持つ。
#[derive(Clone)]
struct BoxPart {
    id: u32,
    name: String,
    /// 幅 (X), 高さ (Y), 奥行 (Z)
    size: [f64; 3],
    position: [f64; 3],
    /// X → Y → Z の順に回す角度 (度)
    rotation_deg: [f64; 3],
}

impl BoxPart {
    /// パラメータから truck の B-rep ソリッドを作る。
    fn build_solid(&self) -> Solid {
        let [w, h, d] = self.size;
        let v = builder::vertex(Point3::new(-w / 2.0, -h / 2.0, -d / 2.0));
        let e = builder::tsweep(&v, Vector3::new(w, 0.0, 0.0));
        let f = builder::tsweep(&e, Vector3::new(0.0, h, 0.0));
        let mut solid = builder::tsweep(&f, Vector3::new(0.0, 0.0, d));
        let axes = [Vector3::unit_x(), Vector3::unit_y(), Vector3::unit_z()];
        for (axis, deg) in axes.into_iter().zip(self.rotation_deg) {
            if deg != 0.0 {
                solid = builder::rotated(&solid, Point3::origin(), axis, Rad(deg.to_radians()));
            }
        }
        let [x, y, z] = self.position;
        builder::translated(&solid, Vector3::new(x, y, z))
    }
}

#[derive(Resource)]
struct Model {
    parts: Vec<BoxPart>,
    next_id: u32,
    /// 形状を作り直す必要がある部品
    dirty: HashSet<u32>,
    /// 新規作成フォームの寸法
    new_size: [f64; 3],
}

impl Default for Model {
    fn default() -> Self {
        let mut model = Self {
            parts: Vec::new(),
            next_id: 1,
            dirty: HashSet::new(),
            new_size: [1.0, 1.0, 1.0],
        };
        model.add_box([2.0, 1.0, 1.0], [0.0, 0.5, 0.0]);
        model
    }
}

impl Model {
    fn add_box(&mut self, size: [f64; 3], position: [f64; 3]) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.parts.push(BoxPart {
            id,
            name: format!("箱 {id}"),
            size,
            position,
            rotation_deg: [0.0; 3],
        });
        self.dirty.insert(id);
        id
    }

    fn part_mut(&mut self, id: u32) -> Option<&mut BoxPart> {
        self.parts.iter_mut().find(|p| p.id == id)
    }
}

#[derive(Resource, Default)]
struct Selection {
    part: Option<u32>,
    face: Option<usize>,
}

// ---------------------------------------------------------------------------
// 表示側 (Bevy のエンティティ)
// ---------------------------------------------------------------------------

/// 1つの部品を表すルートエンティティ。子に面ごとのメッシュを持つ。
#[derive(Component)]
struct PartRoot {
    id: u32,
    /// truck の稜線をポリライン化したもの (Gizmos で描く)
    edges: Vec<Vec<Vec3>>,
}

/// 面メッシュ。ピッキングでどの部品のどの面かを引くための対応表。
#[derive(Component)]
struct FaceTag {
    part: u32,
    face: usize,
}

#[derive(Resource)]
struct Palette {
    normal: Handle<StandardMaterial>,
    selected_part: Handle<StandardMaterial>,
    selected_face: Handle<StandardMaterial>,
}

#[derive(Resource)]
struct OrbitCamera {
    focus: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            yaw: 0.8,
            pitch: 0.5,
            distance: 8.0,
        }
    }
}

fn setup(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    let material = |color: Color| StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.6,
        double_sided: true,
        cull_mode: None,
        ..default()
    };
    commands.insert_resource(Palette {
        normal: materials.add(material(Color::srgb(0.70, 0.74, 0.80))),
        selected_part: materials.add(material(Color::srgb(0.45, 0.65, 0.95))),
        selected_face: materials.add(material(Color::srgb(1.0, 0.60, 0.20))),
    });

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

/// 変更のあった部品だけ truck で作り直し、面ごとのメッシュに変換して差し替える。
fn rebuild_dirty_parts(
    mut commands: Commands,
    mut model: ResMut<Model>,
    roots: Query<(Entity, &PartRoot)>,
    mut meshes: ResMut<Assets<Mesh>>,
    palette: Res<Palette>,
) {
    if model.dirty.is_empty() {
        return;
    }
    let dirty: Vec<u32> = model.dirty.drain().collect();
    for (entity, root) in &roots {
        if dirty.contains(&root.id) {
            commands.entity(entity).despawn();
        }
    }
    for part in model.parts.iter().filter(|p| dirty.contains(&p.id)) {
        let meshed = part.build_solid().triangulation(TESSELLATION_TOL);
        let shell = &meshed.boundaries()[0];

        let edges = shell
            .edge_iter()
            .map(|edge| edge.curve().iter().map(to_vec3).collect())
            .collect();

        let face_meshes: Vec<Mesh> = shell
            .face_iter()
            .map(|face| {
                let mut poly = face.surface().unwrap_or_default();
                if !face.orientation() {
                    poly.invert();
                }
                polygon_to_bevy_mesh(&poly)
            })
            .collect();

        commands
            .spawn((
                PartRoot { id: part.id, edges },
                Transform::default(),
                Visibility::default(),
            ))
            .with_children(|parent| {
                for (index, mesh) in face_meshes.into_iter().enumerate() {
                    parent.spawn((
                        Mesh3d(meshes.add(mesh)),
                        MeshMaterial3d(palette.normal.clone()),
                        FaceTag {
                            part: part.id,
                            face: index,
                        },
                    ));
                }
            });
    }
}

fn to_vec3(p: &Point3) -> Vec3 {
    Vec3::new(p.x as f32, p.y as f32, p.z as f32)
}

/// truck の PolygonMesh (f64) を Bevy の三角形メッシュ (f32) に変換する。
/// 平面の箱なので法線は三角形ごとに計算したフラット法線を使う。
fn polygon_to_bevy_mesh(poly: &PolygonMesh) -> Mesh {
    let positions = poly.positions();
    let mut out_pos = Vec::new();
    let mut out_nor = Vec::new();
    for tri in poly.faces().triangle_iter() {
        let [a, b, c] = tri.map(|v| to_vec3(&positions[v.pos]));
        let n = (b - a).cross(c - a).normalize_or_zero();
        for p in [a, b, c] {
            out_pos.push(p.to_array());
            out_nor.push(n.to_array());
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, out_pos)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, out_nor)
}

// ---------------------------------------------------------------------------
// 入力
// ---------------------------------------------------------------------------

fn pick_face(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform)>,
    mut ray_cast: MeshRayCast,
    faces: Query<&FaceTag>,
    mut selection: ResMut<Selection>,
) {
    // Alt + 左ドラッグはオービットなので選択しない
    if !buttons.just_pressed(MouseButton::Left) || alt_pressed(&keys) {
        return;
    }
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let (camera, camera_transform) = *camera;
    let Ok(ray) = camera.viewport_to_world(camera_transform, cursor) else {
        return;
    };
    let hit = ray_cast
        .cast_ray(ray, &MeshRayCastSettings::default())
        .iter()
        .find_map(|(entity, _)| faces.get(*entity).ok());
    match hit {
        Some(tag) => {
            selection.part = Some(tag.part);
            selection.face = Some(tag.face);
        }
        None => *selection = Selection::default(),
    }
}

fn alt_pressed(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight])
}

impl OrbitCamera {
    fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, -self.pitch, 0.0)
    }

    fn eye(&self) -> Vec3 {
        self.focus + self.rotation() * Vec3::new(0.0, 0.0, self.distance)
    }
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
    if alt && buttons.pressed(MouseButton::Left) {
        // オービット: 注視点を中心に回る
        orbit.yaw -= delta.x * 0.005;
        orbit.pitch = (orbit.pitch + delta.y * 0.005).clamp(-1.5, 1.5);
    } else if alt && buttons.pressed(MouseButton::Right) {
        // ドリー: 右ドラッグの上下でズーム
        orbit.distance = (orbit.distance * (1.0 + (delta.x - delta.y) * 0.005)).clamp(0.5, 200.0);
    } else if buttons.pressed(MouseButton::Right) {
        // 見回し: カメラ位置を固定したまま向きだけ変える
        let eye = orbit.eye();
        orbit.yaw -= delta.x * 0.004;
        orbit.pitch = (orbit.pitch + delta.y * 0.004).clamp(-1.5, 1.5);
        orbit.focus = eye - orbit.rotation() * Vec3::new(0.0, 0.0, orbit.distance);
    }
    if buttons.pressed(MouseButton::Middle) {
        let scale = orbit.distance * 0.0015;
        let pan = orbit.rotation() * Vec3::new(-delta.x * scale, delta.y * scale, 0.0);
        orbit.focus += pan;
    }
    if scroll.delta.y != 0.0 {
        orbit.distance = (orbit.distance * (1.0 - scroll.delta.y * 0.1)).clamp(0.5, 200.0);
    }
}

fn keyboard_input(
    time: Res<Time>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut orbit: ResMut<OrbitCamera>,
    mut model: ResMut<Model>,
    mut selection: ResMut<Selection>,
    mut gizmo_tool: ResMut<GizmoTool>,
) {
    if buttons.pressed(MouseButton::Right) {
        // フライ移動: 右ボタンを押している間だけ WASD / QE が効く
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
        let boost = if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
            3.0
        } else {
            1.0
        };
        orbit.focus += dir.normalize_or_zero() * 4.0 * boost * time.delta_secs();
        return;
    }

    // ギズモの切り替え (Unity / Unreal と同じ W / E / R)
    for (key, tool) in [
        (KeyCode::KeyW, Tool::Translate),
        (KeyCode::KeyE, Tool::Rotate),
        (KeyCode::KeyR, Tool::Scale),
    ] {
        if keys.just_pressed(key) {
            gizmo_tool.tool = tool;
        }
    }

    let Some(id) = selection.part else {
        return;
    };
    if keys.just_pressed(KeyCode::KeyF)
        && let Some(part) = model.parts.iter().find(|p| p.id == id)
    {
        let [x, y, z] = part.position;
        let extent = part.size.iter().copied().fold(0.0, f64::max) as f32;
        orbit.focus = Vec3::new(x as f32, y as f32, z as f32);
        orbit.distance = (extent * 2.5).max(1.0);
    }
    if keys.just_pressed(KeyCode::Delete) {
        model.parts.retain(|p| p.id != id);
        model.dirty.insert(id);
        *selection = Selection::default();
    }
}

fn apply_camera(orbit: Res<OrbitCamera>, mut camera: Single<&mut Transform, With<Camera3d>>) {
    camera.translation = orbit.eye();
    camera.rotation = orbit.rotation();
}

// ---------------------------------------------------------------------------
// 表示の更新
// ---------------------------------------------------------------------------

fn update_materials(
    selection: Res<Selection>,
    palette: Res<Palette>,
    mut faces: Query<(&FaceTag, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    for (tag, mut material) in &mut faces {
        let wanted = if selection.part != Some(tag.part) {
            &palette.normal
        } else if selection.face == Some(tag.face) {
            &palette.selected_face
        } else {
            &palette.selected_part
        };
        if material.0 != *wanted {
            material.0 = wanted.clone();
        }
    }
}

fn draw_edges_and_grid(mut gizmos: Gizmos, roots: Query<&PartRoot>) {
    let grid = Color::srgba(0.5, 0.5, 0.5, 0.35);
    for i in -10..=10 {
        let t = i as f32;
        gizmos.line(Vec3::new(t, 0.0, -10.0), Vec3::new(t, 0.0, 10.0), grid);
        gizmos.line(Vec3::new(-10.0, 0.0, t), Vec3::new(10.0, 0.0, t), grid);
    }
    gizmos.line(Vec3::ZERO, Vec3::X * 2.0, Color::srgb(0.9, 0.2, 0.2));
    gizmos.line(Vec3::ZERO, Vec3::Y * 2.0, Color::srgb(0.2, 0.8, 0.2));
    gizmos.line(Vec3::ZERO, Vec3::Z * 2.0, Color::srgb(0.2, 0.4, 0.9));

    let edge_color = Color::srgb(0.1, 0.1, 0.12);
    for root in &roots {
        for edge in &root.edges {
            gizmos.linestrip(edge.iter().copied(), edge_color);
        }
    }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

fn ui_panel(
    mut contexts: EguiContexts,
    mut model: ResMut<Model>,
    mut selection: ResMut<Selection>,
    mut gizmo_tool: ResMut<GizmoTool>,
) -> Result {
    let window = egui::Window::new("部品")
        .anchor(egui::Align2::LEFT_TOP, [8.0, 8.0])
        .resizable(false)
        .collapsible(true);
    window.show(contexts.ctx_mut()?, |ui| {
        ui.horizontal(|ui| {
            for tool in [Tool::Translate, Tool::Rotate, Tool::Scale] {
                if ui
                    .selectable_label(gizmo_tool.tool == tool, tool.label())
                    .clicked()
                {
                    gizmo_tool.tool = tool;
                }
            }
        });
        snap_settings_ui(ui, &mut gizmo_tool);
        ui.separator();
        ui.heading("新しい箱");
        let mut new_size = model.new_size;
        xyz_editor(ui, "寸法 幅/高さ/奥行", &mut new_size, 0.1, Some(0.01));
        model.new_size = new_size;
        if ui.button("箱を追加").clicked() {
            // 既存の箱と重ならないよう X 方向に並べる
            let x = model.parts.len() as f64 * 2.5;
            let id = model.add_box(new_size, [x, new_size[1] / 2.0, 0.0]);
            selection.part = Some(id);
            selection.face = None;
        }

        ui.separator();
        ui.heading("一覧");
        for part in &model.parts {
            let selected = selection.part == Some(part.id);
            if ui.selectable_label(selected, &part.name).clicked() {
                selection.part = Some(part.id);
                selection.face = None;
            }
        }

        let Some(id) = selection.part else {
            ui.separator();
            ui.label(
                "クリック: 選択   Alt+左ドラッグ: 回転
中ドラッグ: 移動   ホイール: ズーム
右ドラッグ+WASD/QE: 飛行   F: フォーカス
W / E / R: 移動 / 回転 / 拡大縮小
Ctrl + ギズモをドラッグ: スナップ",
            );
            return;
        };
        let face = selection.face;
        let Some(part) = model.part_mut(id) else {
            return;
        };

        ui.separator();
        ui.heading(part.name.clone());
        if let Some(face) = face {
            ui.label(format!("選択中の面: #{face}"));
        }
        let mut edited = part.clone();
        let mut changed = false;
        changed |= xyz_editor(ui, "寸法 幅/高さ/奥行", &mut edited.size, 0.05, Some(0.01));
        changed |= xyz_editor(ui, "位置", &mut edited.position, 0.05, None);
        changed |= xyz_editor(ui, "回転 (度)", &mut edited.rotation_deg, 1.0, None);
        ui.horizontal(|ui| {
            for (label, axis) in [("+15° X", 0), ("+15° Y", 1), ("+15° Z", 2)] {
                if ui.button(label).clicked() {
                    edited.rotation_deg[axis] = (edited.rotation_deg[axis] + 15.0) % 360.0;
                    changed = true;
                }
            }
        });
        let delete = ui.button("削除").clicked();

        if changed {
            *part = edited;
            model.dirty.insert(id);
        }
        if delete {
            model.parts.retain(|p| p.id != id);
            model.dirty.insert(id);
            *selection = Selection::default();
        }
    });
    Ok(())
}

/// X/Y/Z の3つの数値を編集する。値が変わったら true を返す。
fn xyz_editor(
    ui: &mut egui::Ui,
    label: &str,
    values: &mut [f64; 3],
    speed: f64,
    min: Option<f64>,
) -> bool {
    let mut changed = false;
    ui.label(label);
    ui.horizontal(|ui| {
        for value in values.iter_mut() {
            let mut drag = egui::DragValue::new(value).speed(speed).max_decimals(4);
            if let Some(min) = min {
                drag = drag.range(min..=f64::MAX);
            }
            changed |= ui.add(drag).changed();
        }
    });
    changed
}
