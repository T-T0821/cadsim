//! 3D ビューの表示: モデルの部品を Bevy のメッシュにし、選択を強調表示する
//!
//! - 部品1つにつき `PartView` エンティティを1つ作り、子に面ごとのメッシュ (`FaceTag`) を持たせる。
//! - 配置と表示/非表示は毎フレームモデルから写す。形状が変わったときだけメッシュを作り直す。
//! - 取り込んだ部品のメッシュはインスタンス間で共有する (同じボルトが100本でも1回だけ作る)。
//! - 選択中のノード (アセンブリなら子孫すべて) を青、クリックした面をオレンジで塗る。
//! - 部品には干渉チェック用の `SimBody` (f64 メッシュ + 姿勢) も付け、干渉中の部品は半透明にする。

use std::collections::HashMap;
use std::sync::Arc;

use bevy::{
    asset::RenderAssetUsages,
    mesh::PrimitiveTopology,
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
    window::PrimaryWindow,
};
use truck_meshalgo::prelude::*;
// truck の prelude にも Transform トレイトがあるため Bevy 側を明示する
use bevy::prelude::Transform;

use crate::annotation::annotate_tool_off;
use crate::camera::{ViewportRect, alt_pressed, pointer_in_3d_view};
use crate::gizmo::gizmo_not_focused;
use crate::math::to_transform;
use crate::model::{Model, NodeId, NodeKind, Selection};
use crate::motion::{MotionSet, SimulationPose};
use crate::simulation::{self, InterferenceCheck, Pose, SimBody, TriMesh, measure::not_measuring};

/// 箱のテッセレーションの許容誤差 (モデル単位)
const BOX_TESSELLATION_TOL: f64 = 0.005;

pub struct ViewportPlugin;

impl Plugin for ViewportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewSettings>()
            .init_resource::<SharedMeshes>()
            .add_systems(Startup, create_palette)
            .add_systems(
                Update,
                (
                    pick_part
                        .run_if(pointer_in_3d_view)
                        .run_if(gizmo_not_focused)
                        .run_if(not_measuring)
                        .run_if(annotate_tool_off)
                        .run_if(crate::site::plan_tool_off),
                    rebuild_dirty_parts,
                    // 動作シミュレーションの動きが決まってから配置を写す
                    sync_placement_and_visibility.after(MotionSet::Pose),
                    update_materials,
                    draw_edges_grid_and_bounds,
                )
                    .chain(),
            );
    }
}

/// 表示メニューの設定
#[derive(Resource)]
pub struct ViewSettings {
    pub show_edges: bool,
    pub show_grid: bool,
    pub show_bounds: bool,
    /// 選択中以外の部品を半透明にする
    pub ghost_unselected: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            show_edges: true,
            show_grid: true,
            show_bounds: true,
            ghost_unselected: false,
        }
    }
}

/// 部品1つ分の表示。子に面のメッシュを持つ。
#[derive(Component)]
struct PartView {
    node: NodeId,
    /// 稜線 (部品の座標系)。Gizmos で毎フレーム描く。
    edges: Arc<Vec<Vec<Vec3>>>,
}

/// 面メッシュ。クリックしたときにどの部品のどの面かを引くための札。
#[derive(Component)]
pub struct FaceTag {
    pub node: NodeId,
    pub face: usize,
}

/// 取り込んだ形状のメッシュ (形状の id ごと)
#[derive(Resource, Default)]
struct SharedMeshes(HashMap<usize, PartMeshes>);

/// 部品1つ分の表示データ (面ごとのメッシュ, 稜線, 干渉チェック用メッシュ)
type PartMeshes = (Vec<Handle<Mesh>>, Arc<Vec<Vec<Vec3>>>, Arc<TriMesh>);

#[derive(Resource)]
struct Palette {
    selected: Handle<StandardMaterial>,
    selected_face: Handle<StandardMaterial>,
    hovered: Handle<StandardMaterial>,
    /// 干渉している部品 (半透明にして、中の干渉部分を見せる)
    interfering: Handle<StandardMaterial>,
    /// 干渉している部品を選択したとき (選択色のまま半透明)
    selected_interfering: Handle<StandardMaterial>,
    /// 部品の色ごとのマテリアル (色, 半透明か)
    by_color: HashMap<([u8; 3], bool), Handle<StandardMaterial>>,
}

pub const SELECTED_COLOR: Color = Color::srgb(0.25, 0.55, 1.0);
const SELECTED_FACE_COLOR: Color = Color::srgb(1.0, 0.60, 0.20);
const HOVER_COLOR: Color = Color::srgb(0.55, 0.85, 1.0);

fn material(color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.6,
        double_sided: true,
        cull_mode: None,
        ..default()
    }
}

fn translucent(color: Color) -> StandardMaterial {
    StandardMaterial {
        alpha_mode: AlphaMode::Blend,
        ..material(color)
    }
}

fn create_palette(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    commands.insert_resource(Palette {
        selected: materials.add(material(SELECTED_COLOR)),
        selected_face: materials.add(material(SELECTED_FACE_COLOR)),
        hovered: materials.add(material(HOVER_COLOR)),
        interfering: materials.add(translucent(Color::srgba(0.95, 0.55, 0.55, 0.35))),
        selected_interfering: materials.add(translucent(SELECTED_COLOR.with_alpha(0.4))),
        by_color: HashMap::new(),
    });
}

// ---------------------------------------------------------------------------
// メッシュの作成
// ---------------------------------------------------------------------------

/// 変更のあった部品だけ表示を作り直す
fn rebuild_dirty_parts(
    mut commands: Commands,
    mut model: ResMut<Model>,
    views: Query<(Entity, &PartView)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut shared: ResMut<SharedMeshes>,
    palette: Res<Palette>,
) {
    if model.dirty.is_empty() {
        return;
    }
    let dirty: Vec<NodeId> = model.dirty.drain().collect();
    for (entity, view) in &views {
        if dirty.contains(&view.node) {
            commands.entity(entity).despawn();
        }
    }
    for id in dirty {
        let Some(node) = model.get(id) else {
            continue; // 削除されたノード
        };
        let (face_meshes, edges, sim_mesh) = match &node.kind {
            NodeKind::Assembly { .. } => continue,
            NodeKind::Box(part) => {
                let meshed = part.build_solid().triangulation(BOX_TESSELLATION_TOL);
                let shell = &meshed.boundaries()[0];
                let edges = shell
                    .edge_iter()
                    .map(|edge| edge.curve().iter().map(to_vec3).collect())
                    .collect();
                let polygons: Vec<PolygonMesh> = shell
                    .face_iter()
                    .map(|face| {
                        let mut poly = face.surface().unwrap_or_default();
                        if !face.orientation() {
                            poly.invert();
                        }
                        poly
                    })
                    .collect();
                let faces = polygons
                    .iter()
                    .map(|poly| meshes.add(polygon_to_bevy_mesh(poly)))
                    .collect();
                let sim_mesh = simulation::trimesh_from_polygons(&polygons);
                (faces, Arc::new(edges), Arc::new(sim_mesh))
            }
            NodeKind::Imported { shape, .. } => shared
                .0
                .entry(shape.id)
                .or_insert_with(|| {
                    let faces = shape
                        .faces
                        .iter()
                        .map(|poly| meshes.add(polygon_to_bevy_mesh(poly)))
                        .collect();
                    let edges = shape
                        .edges
                        .iter()
                        .map(|line| line.iter().map(to_vec3).collect())
                        .collect();
                    let sim_mesh = simulation::trimesh_from_polygons(&shape.faces);
                    (faces, Arc::new(edges), Arc::new(sim_mesh))
                })
                .clone(),
        };

        commands
            .spawn((
                PartView { node: id, edges },
                // 干渉チェックの対象。姿勢は sync_placement_and_visibility が毎フレーム写す。
                SimBody {
                    id,
                    mesh: sim_mesh,
                    pose: Pose::from_matrix(model.world_matrix(id)),
                },
                Name::new(short_path(&model, id)),
                to_transform(model.world_matrix(id)),
                Visibility::default(),
            ))
            .with_children(|parent| {
                for (face, mesh) in face_meshes.into_iter().enumerate() {
                    parent.spawn((
                        Mesh3d(mesh),
                        MeshMaterial3d(palette.selected.clone()),
                        FaceTag { node: id, face },
                    ));
                }
            });
    }
}

/// 干渉チェックの一覧で見分けられるよう、親の名前を前に付けた名前 ("Bracket_L / Upright_1")
fn short_path(model: &Model, id: NodeId) -> String {
    let path = model.path_label(id);
    let parts: Vec<&str> = path.split(" / ").collect();
    parts[parts.len().saturating_sub(2)..].join(" / ")
}

fn to_vec3(p: &Point3) -> Vec3 {
    Vec3::new(p.x as f32, p.y as f32, p.z as f32)
}

/// truck の PolygonMesh (f64) を Bevy の三角形メッシュ (f32) に変換する。
/// 曲面は truck が付けた頂点法線を使い、無ければ三角形の向きから計算する。
fn polygon_to_bevy_mesh(poly: &PolygonMesh) -> Mesh {
    let positions = poly.positions();
    let normals = poly.normals();
    let mut out_pos = Vec::new();
    let mut out_nor = Vec::new();
    for tri in poly.faces().triangle_iter() {
        let [a, b, c] = tri.map(|v| to_vec3(&positions[v.pos]));
        let flat = (b - a).cross(c - a).normalize_or_zero();
        for (v, p) in tri.iter().zip([a, b, c]) {
            let n = v
                .nor
                .and_then(|i| normals.get(i))
                .map(|n| Vec3::new(n.x as f32, n.y as f32, n.z as f32).normalize_or_zero())
                // 法線が三角形の裏を向いていたら (面の向きを反転したとき) 平らな法線にする
                .filter(|n| n.dot(flat) > 0.0)
                .unwrap_or(flat);
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
// 毎フレームの同期
// ---------------------------------------------------------------------------

/// 配置 (ギズモで動かしたとき) と表示/非表示をモデルから写す。
/// 動作シミュレーションの再生中は、設計上の配置にアクチュエータの動きを重ねる。
fn sync_placement_and_visibility(
    model: Res<Model>,
    motion: Res<SimulationPose>,
    mut views: Query<(&PartView, &mut Transform, &mut Visibility, &mut SimBody)>,
) {
    for (view, mut transform, mut visibility, mut body) in &mut views {
        let mut matrix = model.world_matrix(view.node);
        if let Some(moved) = motion.pose_of(&model, view.node) {
            matrix = moved.to_dmat4() * matrix;
        }
        let wanted = to_transform(matrix);
        if *transform != wanted {
            *transform = wanted;
        }
        // 変わったときだけ書く (書くと干渉チェックが走る)。再生中は動いた位置で判定される。
        let pose = Pose::from_matrix(matrix);
        if body.pose != pose {
            body.pose = pose;
        }
        let shown = if model.is_shown(view.node) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        visibility.set_if_neq(shown);
    }
}

/// ノードの強調の種類
#[derive(PartialEq, Eq, Clone, Copy)]
enum Highlight {
    None,
    /// 選択中 (アセンブリを選んだらその子孫すべて)
    Selected,
    Hovered,
    /// 半透明表示モードで選択の外にあるもの
    Ghost,
}

fn highlight_of(
    model: &Model,
    selection: &Selection,
    settings: &ViewSettings,
    id: NodeId,
) -> Highlight {
    let in_selection = selection.node.is_some_and(|s| model.is_in_subtree(id, s));
    if in_selection {
        Highlight::Selected
    } else if selection
        .hovered
        .is_some_and(|h| model.is_in_subtree(id, h))
    {
        Highlight::Hovered
    } else if settings.ghost_unselected && selection.node.is_some() {
        Highlight::Ghost
    } else {
        Highlight::None
    }
}

fn update_materials(
    model: Res<Model>,
    selection: Res<Selection>,
    settings: Res<ViewSettings>,
    check: Res<InterferenceCheck>,
    mut palette: ResMut<Palette>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut faces: Query<(&FaceTag, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    for (tag, mut current) in &mut faces {
        let wanted = match highlight_of(&model, &selection, &settings, tag.node) {
            Highlight::Selected
                if selection.node == Some(tag.node) && selection.face == Some(tag.face) =>
            {
                palette.selected_face.clone()
            }
            Highlight::Selected if check.is_interfering(tag.node) => {
                palette.selected_interfering.clone()
            }
            Highlight::Selected => palette.selected.clone(),
            Highlight::None if check.is_interfering(tag.node) => palette.interfering.clone(),
            Highlight::Hovered => palette.hovered.clone(),
            highlight => {
                let ghost = highlight == Highlight::Ghost;
                let rgb = model.display_color(tag.node);
                let key = (
                    rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8),
                    ghost,
                );
                palette
                    .by_color
                    .entry(key)
                    .or_insert_with(|| {
                        let mut m = material(Color::srgb(rgb[0], rgb[1], rgb[2]));
                        if ghost {
                            m.base_color.set_alpha(0.15);
                            m.alpha_mode = AlphaMode::Blend;
                        }
                        materials.add(m)
                    })
                    .clone()
            }
        };
        if current.0 != wanted {
            current.0 = wanted;
        }
    }
}

// ---------------------------------------------------------------------------
// 線の描画 (稜線・グリッド・選択の外接箱)
// ---------------------------------------------------------------------------

fn draw_edges_grid_and_bounds(
    mut gizmos: Gizmos,
    model: Res<Model>,
    selection: Res<Selection>,
    settings: Res<ViewSettings>,
    // GlobalTransform は PostUpdate で更新されるので、同じフレームで書いた Transform を使う
    // (部品のエンティティは親を持たないので Transform がそのままワールドの配置)
    views: Query<(&PartView, &Transform, &InheritedVisibility)>,
) {
    if settings.show_grid {
        draw_grid(&mut gizmos, &model);
    }

    if settings.show_edges {
        for (view, transform, visible) in &views {
            if !visible.get() {
                continue;
            }
            let color = match highlight_of(&model, &selection, &settings, view.node) {
                Highlight::Selected => Color::srgb(0.05, 0.15, 0.45),
                Highlight::Ghost => Color::srgba(0.1, 0.1, 0.12, 0.15),
                _ => Color::srgb(0.1, 0.1, 0.12),
            };
            for edge in view.edges.iter() {
                gizmos.linestrip(edge.iter().map(|&p| transform.transform_point(p)), color);
            }
        }
    }

    if settings.show_bounds
        && let Some(bounds) = selection.node.and_then(|id| model.world_bounds(id))
    {
        let c = bounds.corners().map(|p| p.as_vec3());
        let color = Color::srgb(1.0, 0.85, 0.2);
        for (a, b) in [
            (0, 1),
            (1, 2),
            (2, 3),
            (3, 0),
            (4, 5),
            (5, 6),
            (6, 7),
            (7, 4),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ] {
            gizmos.line(c[a], c[b], color);
        }
    }
}

/// 床のグリッド。間隔はモデルの大きさに合わせて 1, 10, 100 ... と切り替える。
fn draw_grid(gizmos: &mut Gizmos, model: &Model) {
    let extent = model
        .scene_bounds()
        .map(|b| b.size().x.max(b.size().z) as f32)
        .unwrap_or(10.0)
        .max(1e-3);
    let step = 10f32.powf((extent / 4.0).log10().floor());
    let count = 20;
    let half = step * count as f32 / 2.0;
    // 原点の近くに揃える
    let center = model
        .scene_bounds()
        .map(|b| b.center().as_vec3())
        .unwrap_or(Vec3::ZERO);
    let cx = (center.x / step).round() * step;
    let cz = (center.z / step).round() * step;
    let floor = model
        .scene_bounds()
        .map_or(0.0, |b| b.min.y.min(0.0) as f32);

    let grid = Color::srgba(0.5, 0.5, 0.5, 0.35);
    for i in 0..=count {
        let t = -half + step * i as f32;
        gizmos.line(
            Vec3::new(cx + t, floor, cz - half),
            Vec3::new(cx + t, floor, cz + half),
            grid,
        );
        gizmos.line(
            Vec3::new(cx - half, floor, cz + t),
            Vec3::new(cx + half, floor, cz + t),
            grid,
        );
    }
    // 原点の座標軸 (X 赤, Y 緑, Z 青)
    let axis = step * 2.0;
    gizmos.line(Vec3::ZERO, Vec3::X * axis, Color::srgb(0.9, 0.2, 0.2));
    gizmos.line(Vec3::ZERO, Vec3::Y * axis, Color::srgb(0.2, 0.8, 0.2));
    gizmos.line(Vec3::ZERO, Vec3::Z * axis, Color::srgb(0.2, 0.4, 0.9));
}

// ---------------------------------------------------------------------------
// クリックで選択
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn pick_part(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    viewport: Res<ViewportRect>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut ray_cast: MeshRayCast,
    faces: Query<&FaceTag>,
    mut selection: ResMut<Selection>,
) {
    // Alt + 左ドラッグはオービットなので選択しない
    if !buttons.just_pressed(MouseButton::Left) || alt_pressed(&keys) {
        return;
    }
    // viewport_to_world はウィンドウ座標を受け取る (ビューの位置は Bevy が差し引く)
    let Some(cursor) = window.cursor_position().filter(|&c| viewport.contains(c)) else {
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
            selection.node = Some(tag.node);
            selection.face = Some(tag.face);
            selection.reveal_in_tree = true;
        }
        None => selection.clear(),
    }
}
