//! 3D ビューの表示: モデルの部品を Bevy のメッシュにし、選択を強調表示する
//!
//! 数千部品のアセンブリでも軽く動かすため、次のようにしている。
//!
//! - 部品1つにつきエンティティは2つだけ (`PartView` の面メッシュと、子の稜線 `EdgeLines`)。
//!   面はすべて1つのメッシュにまとめ、どの三角形がどの面かは `PartMesh` が表で持つ。
//! - 同じ形状の部品 (同じボルト100本など) はメッシュとマテリアルを共有するので、
//!   Bevy がまとめて1回の描画命令 (インスタンシング) で描く。
//! - 稜線も線のメッシュにして GPU で描く (毎フレーム CPU で線を積まない)。
//! - 配置・色・強調の計算はモデルや選択が変わったフレームだけ、木を1回たどって行う。
//! - 画面の外の部品は Bevy の視錐台カリングで描かない。画面上で小さい部品は粗いメッシュに
//!   差し替え (LOD)、さらに小さいものは描かない。
//! - クリックでの選択は、部品の外接箱と部品内の三角形の2段の BVH で探す (`PartPicker`)。
//!   計測や寸法のツールも同じものを使う。
//! - 部品には干渉チェック用の `SimBody` (f64 メッシュ + 姿勢) も付け、干渉中の部品は半透明にする。
//! - 「小さな穴とフィレットを省略」をオンにすると、取り込んだ形状ごとに裏のスレッドで
//!   簡略化した形 (`defeature.rs`) を作り、できたものから表示を差し替える。干渉チェックは元の形のまま。

use std::collections::HashMap;
use std::sync::Arc;

use bevy::{
    asset::RenderAssetUsages,
    ecs::system::SystemParam,
    mesh::PrimitiveTopology,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, block_on, poll_once},
    window::PrimaryWindow,
};
use truck_meshalgo::prelude::*;
// truck の prelude にも Transform トレイトがあるため Bevy 側を明示する
use bevy::prelude::Transform;

use crate::annotation::annotate_tool_off;
use crate::bvh::{Bounds, Bvh};
use crate::defeature::{DefeatureOptions, DefeatureStats, defeature_shape};
use crate::flow::FlowColors;
use crate::library;
use crate::math::{Aabb, to_transform};
use crate::model::{Model, NodeId, NodeKind, Selection};
use crate::motion::{MotionSet, SimulationPose};
use crate::part_mesh::{PartMesh, shell_edges};
use crate::physics::{PhysicsPose, PhysicsSet};
use crate::simulation::{self, InterferenceCheck, Pose, SimBody, TriMesh, measure::not_measuring};
use crate::step_import::ImportedShape;
use crate::view::camera::{ViewportRect, alt_pressed, pointer_in_3d_view};
use crate::view::gizmo::gizmo_not_focused;

/// 箱のテッセレーションの許容誤差 (モデル単位)
const BOX_TESSELLATION_TOL: f64 = 0.005;

pub struct ViewportPlugin;

impl Plugin for ViewportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewSettings>()
            .init_resource::<SharedMeshes>()
            .init_resource::<SceneCache>()
            .init_resource::<RenderStats>()
            .init_resource::<ViewsRebuilt>()
            .init_resource::<Defeaturing>()
            .add_systems(Startup, create_palette)
            .add_systems(
                Update,
                (
                    pick_part
                        .run_if(pointer_in_3d_view)
                        .run_if(gizmo_not_focused)
                        .run_if(not_measuring)
                        .run_if(annotate_tool_off)
                        .run_if(crate::motion::teach_tool_off)
                        .run_if(crate::site::plan_tool_off),
                    rebuild_dirty_parts,
                    defeature_shapes,
                    // 動作シミュレーションと物理演算の動きが決まってから配置を写す
                    sync_views.after(MotionSet::Pose).after(PhysicsSet::Step),
                    update_level_of_detail,
                    update_face_overlay,
                    draw_grid_and_bounds,
                )
                    .chain(),
            );
    }
}

/// 表示メニューの設定
#[derive(Resource, Clone, PartialEq)]
pub struct ViewSettings {
    pub show_edges: bool,
    pub show_grid: bool,
    pub show_bounds: bool,
    /// 選択中以外の部品を半透明にする
    pub ghost_unselected: bool,
    /// 画面上で小さい部品を粗いメッシュにし、ごく小さい部品は描かない
    pub simplify_small_parts: bool,
    /// 部品の外接球の直径が画面上でこれ (ピクセル) より小さければ粗いメッシュにする
    pub coarse_below_px: f32,
    /// これより小さければ描かない
    pub hide_below_px: f32,
    /// 取り込んだ部品の小さな穴とフィレットを消した形で表示する
    pub defeature: bool,
    /// この直径以下の穴を消す (モデルの単位)
    pub defeature_hole: f32,
    /// この半径以下のフィレットを消す (モデルの単位)
    pub defeature_fillet: f32,
}

impl ViewSettings {
    /// 形状の簡略化がオンならその条件
    fn defeature_options(&self) -> Option<DefeatureOptions> {
        self.defeature.then_some(DefeatureOptions {
            max_hole_diameter: self.defeature_hole as f64,
            max_fillet_radius: self.defeature_fillet as f64,
        })
    }
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            show_edges: true,
            show_grid: true,
            show_bounds: true,
            ghost_unselected: false,
            simplify_small_parts: true,
            coarse_below_px: 60.0,
            hide_below_px: 2.0,
            defeature: false,
            defeature_hole: 20.0,
            defeature_fillet: 5.0,
        }
    }
}

/// ステータスバーに出す表示の内訳 (毎フレーム数える)
#[derive(Resource, Default, Clone, Copy)]
pub struct RenderStats {
    pub parts: usize,
    /// 詳細なメッシュで描いている部品
    pub full: usize,
    /// 粗いメッシュで描いている部品
    pub coarse: usize,
    /// 小さすぎて描いていない部品
    pub hidden_small: usize,
    /// 画面の外で描いていない部品 (Bevy の視錐台カリング、1フレーム前の結果)
    pub outside_view: usize,
    /// 描いている三角形の数
    pub triangles: usize,
}

/// モデルや選択が変わったときにだけ計算し直す値
#[derive(Resource, Default)]
pub struct SceneCache {
    /// 表示中の部品すべての外接箱
    pub scene_bounds: Option<Aabb>,
    /// 選択中のノードの外接箱
    pub selection_bounds: Option<Aabb>,
}

/// このフレームで部品の表示を作り直した (新しいエンティティにも色や配置を写す必要がある)
#[derive(Resource, Default)]
struct ViewsRebuilt(bool);

/// 部品1つ分の表示。面メッシュを持ち、子に稜線 (`EdgeLines`) を持つ。
#[derive(Component)]
struct PartView {
    node: NodeId,
    /// 取り込んだ形状の id (簡略化した形に差し替えるため)。箱では None。
    shape: Option<usize>,
    /// いま表示している形 (元の形か、簡略化した形)
    display: DisplayMesh,
    edges: Entity,
    /// 自分と祖先がすべて表示中
    shown: bool,
    /// 選択中またはカーソルが乗っている (小さくても詳細に描く)
    highlighted: bool,
}

#[derive(Component)]
struct EdgeLines;

/// 選択した面を上から塗る強調表示
#[derive(Component)]
struct FaceOverlay;

/// 1つの形の表示用メッシュ (選択用の `PartMesh` と GPU 用のメッシュ)
#[derive(Clone)]
struct DisplayMesh {
    mesh: Arc<PartMesh>,
    full: Handle<Mesh>,
    coarse: Option<Handle<Mesh>>,
    edges: Handle<Mesh>,
}

/// 部品1つ分の GPU 用メッシュ
#[derive(Clone)]
struct PartAssets {
    display: DisplayMesh,
    /// 干渉チェック用の f64 メッシュ
    sim: Arc<TriMesh>,
    /// 取り込んだ形状 (簡略化の元の B-rep を持つ)
    shape: Option<Arc<ImportedShape>>,
}

/// 取り込んだ形状のメッシュ (形状の id ごと)。同じ形状の部品はこれを共有する。
#[derive(Resource, Default)]
struct SharedMeshes(HashMap<usize, PartAssets>);

#[derive(Resource)]
struct Palette {
    selected: Handle<StandardMaterial>,
    selected_face: Handle<StandardMaterial>,
    hovered: Handle<StandardMaterial>,
    /// 干渉している部品 (半透明にして、中の干渉部分を見せる)
    interfering: Handle<StandardMaterial>,
    /// 干渉している部品を選択したとき (選択色のまま半透明)
    selected_interfering: Handle<StandardMaterial>,
    edge: Handle<StandardMaterial>,
    edge_selected: Handle<StandardMaterial>,
    edge_ghost: Handle<StandardMaterial>,
    /// 部品の色ごとのマテリアル (色, 半透明か)
    by_color: HashMap<([u8; 3], bool), Handle<StandardMaterial>>,
}

pub const SELECTED_COLOR: Color = Color::srgb(0.25, 0.55, 1.0);
const SELECTED_FACE_COLOR: Color = Color::srgb(1.0, 0.60, 0.20);
const HOVER_COLOR: Color = Color::srgb(0.55, 0.85, 1.0);

/// 面の深度を少し奥にずらす量。同じ位置にある稜線 (線には深度のずらしが使えない) が
/// 面に隠れてちらつかないようにする。
const FACE_DEPTH_BIAS: f32 = -FACE_BIAS_STEP;
const FACE_BIAS_STEP: f32 = 64.0;

fn material(color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.6,
        double_sided: true,
        cull_mode: None,
        depth_bias: FACE_DEPTH_BIAS,
        ..default()
    }
}

fn translucent(color: Color) -> StandardMaterial {
    StandardMaterial {
        alpha_mode: AlphaMode::Blend,
        ..material(color)
    }
}

/// 稜線のマテリアル (光を当てない単色)
fn line_material(color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color,
        unlit: true,
        alpha_mode: if color.alpha() < 1.0 {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        },
        ..default()
    }
}

fn create_palette(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    commands.insert_resource(Palette {
        selected: materials.add(material(SELECTED_COLOR)),
        selected_face: materials.add(StandardMaterial {
            // 部品の面よりは手前、稜線よりは奥に描く
            depth_bias: FACE_DEPTH_BIAS + FACE_BIAS_STEP / 2.0,
            ..material(SELECTED_FACE_COLOR)
        }),
        hovered: materials.add(material(HOVER_COLOR)),
        interfering: materials.add(translucent(Color::srgba(0.95, 0.55, 0.55, 0.35))),
        selected_interfering: materials.add(translucent(SELECTED_COLOR.with_alpha(0.4))),
        edge: materials.add(line_material(Color::srgb(0.1, 0.1, 0.12))),
        edge_selected: materials.add(line_material(Color::srgb(0.05, 0.15, 0.45))),
        edge_ghost: materials.add(line_material(Color::srgba(0.1, 0.1, 0.12, 0.15))),
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
    mut rebuilt: ResMut<ViewsRebuilt>,
) {
    rebuilt.0 = !model.dirty.is_empty();
    if model.dirty.is_empty() {
        return;
    }
    let dirty = std::mem::take(&mut model.dirty);
    for (entity, view) in &views {
        if dirty.contains(&view.node) {
            commands.entity(entity).despawn();
        }
    }
    for id in dirty {
        let Some(node) = model.get(id) else {
            continue; // 削除されたノード
        };
        let assets = match &node.kind {
            NodeKind::Assembly { .. } | NodeKind::Standard(_) => continue,
            NodeKind::Box(part) => {
                let meshed = part.build_solid().triangulation(BOX_TESSELLATION_TOL);
                let shell = &meshed.boundaries()[0];
                let edges = shell_edges(shell);
                let faces: Vec<PolygonMesh> = shell
                    .face_iter()
                    .map(|face| {
                        let mut poly = face.surface().unwrap_or_default();
                        if !face.orientation() {
                            poly.invert();
                        }
                        poly
                    })
                    .collect();
                PartAssets {
                    display: upload(Arc::new(PartMesh::new(&faces, &edges)), &mut meshes),
                    sim: Arc::new(simulation::trimesh_from_polygons(&faces)),
                    shape: None,
                }
            }
            NodeKind::Imported { shape, .. } | NodeKind::StandardBody { shape, .. } => shared
                .0
                .entry(shape.id)
                .or_insert_with(|| PartAssets {
                    display: upload(shape.mesh.clone(), &mut meshes),
                    sim: Arc::new(simulation::trimesh_from_polygons(&shape.faces)),
                    shape: Some(shape.clone()),
                })
                .clone(),
        };
        let shape = assets.shape.as_ref().map(|shape| shape.id);

        let edges = commands
            .spawn((
                Mesh3d(assets.display.edges.clone()),
                MeshMaterial3d(palette.edge.clone()),
                EdgeLines,
            ))
            .id();
        commands
            .spawn((
                Mesh3d(assets.display.full.clone()),
                MeshMaterial3d(palette.selected.clone()),
                // 干渉チェックの対象。姿勢と表示/非表示は sync_views がモデルから写す。
                SimBody {
                    id,
                    group: model.pick_target(id),
                    mesh: assets.sim,
                    pose: Pose::from_matrix(model.world_matrix(id)),
                    shown: true,
                },
                Name::new(short_path(&model, id)),
                to_transform(model.world_matrix(id)),
                Visibility::default(),
                PartView {
                    node: id,
                    shape,
                    display: assets.display,
                    edges,
                    shown: true,
                    highlighted: false,
                },
            ))
            .add_child(edges);
    }
}

/// 干渉チェックの一覧で見分けられるよう、親の名前を前に付けた名前 ("Bracket_L / Upright_1")
fn short_path(model: &Model, id: NodeId) -> String {
    let path = model.path_label(id);
    let parts: Vec<&str> = path.split(" / ").collect();
    parts[parts.len().saturating_sub(2)..].join(" / ")
}

/// 表示用メッシュを GPU に送る形にする
fn upload(mesh: Arc<PartMesh>, meshes: &mut Assets<Mesh>) -> DisplayMesh {
    DisplayMesh {
        full: meshes.add(triangle_mesh(&mesh.positions, &mesh.normals)),
        coarse: mesh
            .coarse
            .as_ref()
            .map(|(positions, normals)| meshes.add(triangle_mesh(positions, normals))),
        edges: meshes.add(line_mesh(&mesh.edges)),
        mesh,
    }
}

fn triangle_mesh(positions: &[Vec3], normals: &[Vec3]) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions.to_vec())
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals.to_vec())
}

/// ポリラインを線分の並び (LineList) にする
fn line_mesh(lines: &[Vec<Vec3>]) -> Mesh {
    let positions: Vec<Vec3> = lines
        .iter()
        .flat_map(|line| line.windows(2).flat_map(|w| [w[0], w[1]]))
        .collect();
    Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
}

// ---------------------------------------------------------------------------
// 形状の簡略化 (小さな穴とフィレットを消す)
// ---------------------------------------------------------------------------

type DefeatureTask = Task<Option<(PartMesh, DefeatureStats)>>;

/// 簡略化の計算結果 (形状の id ごと)。表示メニューの状態表示にも使う。
#[derive(Resource, Default)]
pub struct Defeaturing {
    /// 計算中 (条件, タスク)
    tasks: HashMap<usize, (DefeatureOptions, DefeatureTask)>,
    /// できた結果 (条件, 簡略化した形, 消したものの数)。消すものが無ければ形は None。
    done: HashMap<usize, (DefeatureOptions, Option<DisplayMesh>, DefeatureStats)>,
    /// 表示に反映した条件
    applied: Option<DefeatureOptions>,
}

impl Defeaturing {
    /// まだ計算中の形状の数
    pub fn pending(&self) -> usize {
        self.tasks.len()
    }

    /// いまの条件で消した穴とフィレットの数 (形状ごとに数える)
    pub fn removed(&self) -> DefeatureStats {
        let mut total = DefeatureStats::default();
        for (options, _, stats) in self.done.values() {
            if Some(*options) == self.applied {
                total += *stats;
            }
        }
        total
    }
}

/// 簡略化がオンなら形状ごとに裏のスレッドで簡略化し、できたものから表示を差し替える。
/// 条件を変えている間は、新しい結果ができるまで前の結果を表示しておく。
#[allow(clippy::too_many_arguments)]
fn defeature_shapes(
    settings: Res<ViewSettings>,
    shared: Res<SharedMeshes>,
    mut work: ResMut<Defeaturing>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut rebuilt: ResMut<ViewsRebuilt>,
    mut views: Query<&mut PartView>,
    mut edge_meshes: Query<&mut Mesh3d, (With<EdgeLines>, Without<PartView>)>,
) {
    let wanted = settings.defeature_options();
    let mut changed = rebuilt.0 || wanted != work.applied;
    work.applied = wanted;
    let Some(options) = wanted else {
        work.tasks.clear(); // 落とすと計算も止まる
        if changed {
            apply_display(&shared, &work, &mut views, &mut edge_meshes, &mut rebuilt);
        }
        return;
    };

    // 条件が変わった計算はやめ、まだの形状の計算を始める
    work.tasks.retain(|_, (o, _)| *o == options);
    for (&id, assets) in &shared.0 {
        let Some(shape) = assets.shape.as_ref().filter(|s| !s.breps.is_empty()) else {
            continue;
        };
        let up_to_date = work.done.get(&id).is_some_and(|(o, ..)| *o == options);
        if up_to_date || work.tasks.contains_key(&id) {
            continue;
        }
        let shape = shape.clone();
        let task = AsyncComputeTaskPool::get()
            .spawn(async move { defeature_shape(&shape.breps, &options) });
        work.tasks.insert(id, (options, task));
    }

    let mut finished = Vec::new();
    for (&id, (_, task)) in work.tasks.iter_mut() {
        if let Some(result) = block_on(poll_once(task)) {
            finished.push((id, result));
        }
    }
    for (id, result) in finished {
        work.tasks.remove(&id);
        let (display, stats) = match result {
            Some((mesh, stats)) => (Some(upload(Arc::new(mesh), &mut meshes)), stats),
            None => (None, DefeatureStats::default()),
        };
        work.done.insert(id, (options, display, stats));
        changed = true;
    }
    if changed {
        apply_display(&shared, &work, &mut views, &mut edge_meshes, &mut rebuilt);
    }
}

/// 部品ごとに、元の形か簡略化した形のどちらを表示するかを決めて差し替える
fn apply_display(
    shared: &SharedMeshes,
    work: &Defeaturing,
    views: &mut Query<&mut PartView>,
    edge_meshes: &mut Query<&mut Mesh3d, (With<EdgeLines>, Without<PartView>)>,
    rebuilt: &mut ViewsRebuilt,
) {
    for mut view in views.iter_mut() {
        let Some(id) = view.shape else {
            continue;
        };
        let simplified = work
            .applied
            .and_then(|_| work.done.get(&id))
            .and_then(|(_, display, _)| display.as_ref());
        let Some(wanted) = simplified.or_else(|| shared.0.get(&id).map(|a| &a.display)) else {
            continue;
        };
        if view.display.full == wanted.full {
            continue;
        }
        view.display = wanted.clone();
        if let Ok(mut edges) = edge_meshes.get_mut(view.edges) {
            edges.0 = wanted.edges.clone();
        }
        // 面の強調表示などを新しい形で作り直す (面の番号は変わらない)
        rebuilt.0 = true;
    }
}

// ---------------------------------------------------------------------------
// モデル・選択が変わったときの同期
// ---------------------------------------------------------------------------

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

/// 部品1つ分の、モデルから求めた表示の状態
struct PartState {
    world: bevy::math::DMat4,
    transform: Transform,
    shown: bool,
    highlight: Highlight,
    color: [f32; 3],
}

/// 配置・表示/非表示・色・強調をモデルから写す。
/// モデルか選択か表示設定が変わったとき、または部品の表示を作り直したときだけ動く。
#[allow(clippy::too_many_arguments)]
fn sync_views(
    model: Res<Model>,
    selection: Res<Selection>,
    settings: Res<ViewSettings>,
    check: Res<InterferenceCheck>,
    motion: Res<SimulationPose>,
    physics: Res<PhysicsPose>,
    jog: Res<library::Jog>,
    rebuilt: Res<ViewsRebuilt>,
    flow_colors: Res<FlowColors>,
    mut palette: ResMut<Palette>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cache: ResMut<SceneCache>,
    mut views: Query<(
        &mut PartView,
        &mut Transform,
        &mut MeshMaterial3d<StandardMaterial>,
        &mut SimBody,
    )>,
    mut edges: Query<&mut MeshMaterial3d<StandardMaterial>, (With<EdgeLines>, Without<PartView>)>,
) {
    let changed = model.is_changed()
        || selection.is_changed()
        || settings.is_changed()
        || check.is_changed()
        || motion.is_changed()
        || flow_colors.is_changed()
        || physics.is_changed()
        || jog.is_changed();
    if !changed && !rebuilt.0 {
        return;
    }

    let mut states = HashMap::new();
    let mut scene_bounds: Option<Aabb> = None;
    let mut selection_bounds: Option<Aabb> = None;
    model.visit_parts(|part| {
        let in_selection = selection.node.is_some_and(|s| part.path.contains(&s));
        let highlight = if in_selection {
            Highlight::Selected
        } else if selection.hovered.is_some_and(|h| part.path.contains(&h)) {
            Highlight::Hovered
        } else if settings.ghost_unselected && selection.node.is_some() {
            Highlight::Ghost
        } else {
            Highlight::None
        };
        // 標準部品の動作テスト (library::Jog) の動きを重ねる
        let base = if jog.0.is_empty() {
            part.world
        } else {
            library::display_matrix(&model, &jog, part.id)
        };
        // 物理演算で動いている部品は物理の位置に置く。動作シミュレーションの再生中は、
        // 設計上の配置にアクチュエータの動きを重ねる (自分に割り当てがなければ、いちばん近い祖先の動き)
        let moved = physics.parts.get(&part.id).copied().unwrap_or_else(|| {
            part.path
                .iter()
                .rev()
                .find_map(|id| motion.parts.get(id))
                .map_or(base, |pose| pose.to_dmat4() * base)
        });
        if let Some(bounds) = part.local_bounds.map(|b| b.transformed(part.world)) {
            if part.shown {
                scene_bounds = Some(scene_bounds.map_or(bounds, |s| s.union(bounds)));
            }
            if in_selection {
                selection_bounds = Some(selection_bounds.map_or(bounds, |s| s.union(bounds)));
            }
        }
        states.insert(
            part.id,
            PartState {
                world: moved,
                transform: to_transform(moved),
                shown: part.shown,
                highlight,
                // 工程シミュレーションの色分けがあれば、部品の色の代わりに使う
                color: flow_colors.color_on_path(part.path).unwrap_or(part.color),
            },
        );
    });
    cache.scene_bounds = scene_bounds;
    cache.selection_bounds = selection_bounds;

    for (mut view, mut transform, mut face_material, mut body) in &mut views {
        let Some(state) = states.get(&view.node) else {
            continue; // 削除済み (次の rebuild で消える)
        };
        if *transform != state.transform {
            *transform = state.transform;
        }
        // 変わったときだけ書く (書くと干渉チェックが走る)。再生中は動いた位置で判定される。
        let pose = Pose::from_matrix(state.world);
        if body.pose != pose || body.shown != state.shown {
            body.pose = pose;
            body.shown = state.shown;
        }
        let highlighted = matches!(state.highlight, Highlight::Selected | Highlight::Hovered);
        if view.shown != state.shown || view.highlighted != highlighted {
            view.shown = state.shown;
            view.highlighted = highlighted;
        }

        let interfering = check.is_interfering(view.node);
        let wanted = match state.highlight {
            Highlight::Selected if interfering => palette.selected_interfering.clone(),
            Highlight::Selected => palette.selected.clone(),
            Highlight::None if interfering => palette.interfering.clone(),
            Highlight::Hovered => palette.hovered.clone(),
            highlight => {
                let ghost = highlight == Highlight::Ghost;
                let rgb = state.color;
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
        if face_material.0 != wanted {
            face_material.0 = wanted;
        }

        let wanted_edge = match state.highlight {
            Highlight::Selected => &palette.edge_selected,
            Highlight::Ghost => &palette.edge_ghost,
            _ => &palette.edge,
        };
        if let Ok(mut edge_material) = edges.get_mut(view.edges)
            && edge_material.0 != *wanted_edge
        {
            edge_material.0 = wanted_edge.clone();
        }
    }
}

// ---------------------------------------------------------------------------
// 毎フレーム: 画面上の大きさで詳細度を選ぶ
// ---------------------------------------------------------------------------

/// 部品の外接球が画面上で何ピクセルになるかを見て、詳細 / 粗い / 描かないを選ぶ
fn update_level_of_detail(
    settings: Res<ViewSettings>,
    camera: Single<(&Camera, &GlobalTransform, &Projection), With<Camera3d>>,
    mut views: Query<(
        &PartView,
        &GlobalTransform,
        &ViewVisibility,
        &mut Visibility,
        &mut Mesh3d,
    )>,
    mut edges: Query<&mut Visibility, (With<EdgeLines>, Without<PartView>)>,
    mut stats: ResMut<RenderStats>,
) {
    let (camera, camera_transform, projection) = *camera;
    let eye = camera_transform.translation();
    let view_height = camera
        .physical_viewport_size()
        .map_or(1000.0, |s| s.y as f32);
    // 距離 1 の位置にある直径 1 のものが何ピクセルになるか
    let px_per_unit = match projection {
        Projection::Perspective(p) => view_height / (2.0 * (p.fov / 2.0).tan()),
        Projection::Orthographic(o) => view_height / o.area.height().max(1e-9),
        _ => f32::INFINITY,
    };
    let orthographic = matches!(projection, Projection::Orthographic(_));

    let mut new_stats = RenderStats::default();
    for (view, transform, in_view, mut visibility, mut mesh) in &mut views {
        new_stats.parts += 1;
        let bounds = view.display.mesh.bounds;
        let diameter = (bounds.max - bounds.min).length() * transform.scale().max_element();
        let center = transform.transform_point(bounds.center());
        let distance = if orthographic {
            1.0
        } else {
            center.distance(eye).max(1e-6)
        };
        let pixels = diameter * px_per_unit / distance;

        let simplify = settings.simplify_small_parts && !view.highlighted;
        let visible = view.shown && !(simplify && pixels < settings.hide_below_px);
        let coarse = simplify && pixels < settings.coarse_below_px && view.display.coarse.is_some();

        visibility.set_if_neq(if visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
        let wanted = match (&view.display.coarse, coarse) {
            (Some(handle), true) => handle,
            _ => &view.display.full,
        };
        if mesh.0 != *wanted {
            mesh.0 = wanted.clone();
        }
        // 粗い表示のときは稜線も省く (遠くでは線が重なって黒くつぶれるだけ)
        if let Ok(mut edge_visibility) = edges.get_mut(view.edges) {
            edge_visibility.set_if_neq(if settings.show_edges && !coarse {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            });
        }

        if !view.shown {
            continue;
        }
        if !visible {
            new_stats.hidden_small += 1;
        } else if !in_view.get() {
            new_stats.outside_view += 1;
        } else if coarse {
            new_stats.coarse += 1;
            new_stats.triangles += view
                .display
                .mesh
                .coarse
                .as_ref()
                .map_or(0, |c| c.0.len() / 3);
        } else {
            new_stats.full += 1;
            new_stats.triangles += view.display.mesh.triangle_count();
        }
    }
    *stats = new_stats;
}

// ---------------------------------------------------------------------------
// 選択した面の強調
// ---------------------------------------------------------------------------

/// クリックした面だけのメッシュを作り、部品の上に重ねてオレンジで塗る
fn update_face_overlay(
    mut commands: Commands,
    selection: Res<Selection>,
    views: Query<(Entity, &PartView)>,
    rebuilt: Res<ViewsRebuilt>,
    mut meshes: ResMut<Assets<Mesh>>,
    palette: Res<Palette>,
    // いま出している強調 (部品のエンティティ, 面, 強調のエンティティ)
    mut current: Local<Option<(Entity, usize, Entity)>>,
) {
    if !selection.is_changed() && !rebuilt.0 {
        return;
    }
    let wanted = selection.node.zip(selection.face).and_then(|(node, face)| {
        views
            .iter()
            .find(|(_, view)| view.node == node)
            .map(|(entity, view)| (entity, face, view))
    });
    if let (Some((part, face, _)), Some((cur_part, cur_face, _))) = (&wanted, *current)
        && *part == cur_part
        && *face == cur_face
    {
        return;
    }
    if let Some((_, _, overlay)) = current.take() {
        // 部品ごと作り直したときは一緒に消えている
        commands.entity(overlay).try_despawn();
    }
    if let Some((part, face, view)) = wanted {
        let (positions, normals) = view.display.mesh.face_triangles(face);
        let overlay = commands
            .spawn((
                Mesh3d(meshes.add(triangle_mesh(&positions, &normals))),
                MeshMaterial3d(palette.selected_face.clone()),
                FaceOverlay,
            ))
            .id();
        commands.entity(part).add_child(overlay);
        *current = Some((part, face, overlay));
    }
}

// ---------------------------------------------------------------------------
// 線の描画 (グリッド・選択の外接箱)
// ---------------------------------------------------------------------------

fn draw_grid_and_bounds(
    mut gizmos: Gizmos,
    selection: Res<Selection>,
    settings: Res<ViewSettings>,
    cache: Res<SceneCache>,
) {
    if settings.show_grid {
        draw_grid(&mut gizmos, cache.scene_bounds);
    }

    if settings.show_bounds
        && selection.node.is_some()
        && let Some(bounds) = cache.selection_bounds
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
fn draw_grid(gizmos: &mut Gizmos, scene_bounds: Option<Aabb>) {
    let extent = scene_bounds
        .map(|b| b.size().x.max(b.size().z) as f32)
        .unwrap_or(10.0)
        .max(1e-3);
    let step = 10f32.powf((extent / 4.0).log10().floor());
    let count = 20;
    let half = step * count as f32 / 2.0;
    // 原点の近くに揃える
    let center = scene_bounds
        .map(|b| b.center().as_vec3())
        .unwrap_or(Vec3::ZERO);
    let cx = (center.x / step).round() * step;
    let cz = (center.z / step).round() * step;
    let floor = scene_bounds.map_or(0.0, |b| b.min.y.min(0.0) as f32);

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

/// クリックで部品と面を選ぶ
#[allow(clippy::too_many_arguments)]
fn pick_part(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    viewport: Res<ViewportRect>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    picker: PartPicker,
    model: Res<Model>,
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
    match picker.cast(ray) {
        Some(hit) => {
            // 標準部品は本体ではなく部品全体を選ぶ (面の強調もしない)
            let target = model.pick_target(hit.node);
            selection.node = Some(target);
            selection.face = (target == hit.node).then_some(hit.face);
            selection.reveal_in_tree = true;
        }
        None => selection.clear(),
    }
}

/// レイが最初に当たった部品の面
#[derive(Clone, Copy, Debug)]
pub struct PartHit {
    /// 部品のエンティティ (`SimBody` を持つ)
    pub entity: Entity,
    pub node: NodeId,
    /// 面の番号 (truck の面の並び)
    pub face: usize,
    /// 当たった点 (ワールド座標)
    pub point: Vec3,
}

/// 表示中の部品にレイを当てる。部品の外接箱の BVH → 部品内の三角形の BVH の順に調べるので、
/// 数千部品でも速い。画面上で小さくて粗く描いている部品も詳細な形状で当たりを調べる
/// (穴とフィレットを省略しているときは、省略した形で調べる)。
#[derive(SystemParam)]
pub struct PartPicker<'w, 's> {
    views: Query<'w, 's, (Entity, &'static PartView, &'static GlobalTransform)>,
}

impl PartPicker<'_, '_> {
    pub fn cast(&self, ray: Ray3d) -> Option<PartHit> {
        let parts: Vec<_> = self
            .views
            .iter()
            .filter(|(_, view, _)| view.shown)
            .collect();
        let origin = ray.origin;
        let dir = *ray.direction;
        let world_bounds: Vec<Bounds> = parts
            .iter()
            .map(|(_, view, transform)| {
                let b = view.display.mesh.bounds;
                let corners = Aabb {
                    min: b.min.as_dvec3(),
                    max: b.max.as_dvec3(),
                }
                .corners();
                Bounds::from_points(corners.map(|p| transform.transform_point(p.as_vec3())))
            })
            .collect();
        // 部品の座標系に移したレイ (向きは正規化しないので t は世界座標と同じ尺度のまま)
        let local_ray = |transform: &GlobalTransform| {
            let inverse = transform.affine().inverse();
            (
                inverse.transform_point3(origin),
                inverse.transform_vector3(dir),
            )
        };
        let (index, t) = Bvh::build(&world_bounds).cast(origin, dir, |i, _| {
            let (_, view, transform) = parts[i as usize];
            let (o, d) = local_ray(transform);
            view.display.mesh.ray_cast(o, d).map(|(_, t)| t)
        })?;
        let (entity, view, transform) = parts[index as usize];
        let (o, d) = local_ray(transform);
        let (face, _) = view.display.mesh.ray_cast(o, d)?;
        Some(PartHit {
            entity,
            node: view.node,
            face,
            point: origin + dir * t,
        })
    }
}
