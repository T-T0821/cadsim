//! 3D 上の寸法と注記
//!
//! 3D ビュー右下の「寸法・注記」パネルで道具を選び、ビュー上をクリックして置く。
//! - 点間距離: 2点をクリック。頂点・辺の中点・面の中心の近くでは吸着する
//! - 辺の長さ: 辺をクリック (円弧などの曲線は線に沿った長さ)
//! - 面間距離: 面を2つクリック。平行な平面なら距離、平行でなければ面どうしの角度
//! - 注記: 部品の上をクリックして文字を置く (文字はパネルで編集)
//!
//! 寸法は「どの部品のどこか」で覚えるので、部品を動かすと寸法線も付いて動く。
//! 箱の点は箱の半寸法で割った -1..1 の値で覚えるので、箱の寸法を変えても同じ頂点や面に付いたまま。
//! 値は B-rep をテッセレーションした f64 のメッシュから計算し、表示のときだけ f32 にする。
//!
//! 寸法と注記はプロジェクトファイル (.plantwright) に保存され、追加・削除・文字の編集は
//! 元に戻す (Ctrl+Z) の対象になる。部品を消しても寸法は消さずに隠すだけなので、
//! 部品の削除を元に戻せば寸法も戻る (保存するときは部品のない寸法を書かない)。

use crate::ui::dock::{self, Dock};
use plantwright_core::workspace::Tool;
use std::collections::HashMap;
use std::sync::Arc;

use bevy::{
    math::{DMat4, DVec3},
    prelude::*,
    window::PrimaryWindow,
};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use plantwright_core::kernel::FaceMesh;
use plantwright_core::shape::tessellate_breps;
use serde::{Deserialize, Serialize};

use crate::document::history::{Command, History};
use crate::document::selection::Selection;
use crate::features::simulation::measure::MeasureTool;
use crate::ui::font::install_japanese_font;
use crate::ui::widgets::length;
use crate::viewport::camera::{ViewportRect, alt_pressed, keyboard_free};
use crate::viewport::gizmo::transform_gizmo_system;
use crate::viewport::picking::PartUnderPointer;
use plantwright_core::math::euler_deg_to_quat;
use plantwright_core::model::BoxPart;
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::model::NodeKind;

/// 吸着する距離 (画面上のピクセル)
const SNAP_PX: f32 = 12.0;
/// 辺を拾う距離 (画面上のピクセル)
const EDGE_PICK_PX: f32 = 10.0;
/// これより角度差が小さい面は平行とみなす (ラジアン)
const PARALLEL_TOL: f64 = 1e-6;

const DIM_COLOR: Color = Color::srgb(0.10, 0.80, 0.90);
const SELECTED_COLOR: Color = Color::srgb(1.0, 0.35, 0.80);
const NOTE_COLOR: Color = Color::srgb(0.95, 0.80, 0.25);
const PREVIEW_COLOR: Color = Color::srgb(0.40, 1.0, 0.45);

pub struct AnnotationPlugin;

impl Plugin for AnnotationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Annotations>()
            .init_resource::<AnnotateTool>()
            .init_resource::<GeometryCache>()
            .init_gizmo_group::<AnnotationGizmos>()
            .add_systems(Startup, configure_gizmos)
            .add_systems(
                EguiPrimaryContextPass,
                (annotation_panel, draw_labels)
                    .chain()
                    .after(install_japanese_font)
                    // UI がビューの領域とカーソルの位置を決めた後で描く
                    .after(transform_gizmo_system),
            )
            .add_systems(
                Update,
                (
                    update_geometry_cache,
                    forget_missing_picks,
                    yield_to_measure,
                    cancel_with_escape.run_if(keyboard_free),
                    annotate_input,
                    draw_annotations,
                )
                    .chain(),
            );
    }
}

// ---------------------------------------------------------------------------
// データ (保存の対象)
// ---------------------------------------------------------------------------

/// 部品上の1点。`local` は部品の「アンカー座標系」での位置 (`anchor_frame` を参照)。
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Anchor {
    pub node: NodeId,
    pub local: [f64; 3],
}

/// 部品の面。番号はビューのピッキング (`PartHit::face`) と同じ並び。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FaceRef {
    pub node: NodeId,
    pub face: usize,
}

/// 部品の稜線。番号は `PartGeometry::edges` の並び。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EdgeRef {
    pub node: NodeId,
    pub edge: usize,
}

#[derive(Clone, PartialEq, Debug)]
pub enum AnnotationKind {
    PointDistance { a: Anchor, b: Anchor },
    EdgeLength { edge: EdgeRef },
    FaceDistance { a: FaceRef, b: FaceRef },
    Note { at: Anchor, text: String },
}

impl AnnotationKind {
    fn label(&self) -> &'static str {
        match self {
            AnnotationKind::PointDistance { .. } => "点間距離",
            AnnotationKind::EdgeLength { .. } => "辺の長さ",
            AnnotationKind::FaceDistance { .. } => "面間距離",
            AnnotationKind::Note { .. } => "注記",
        }
    }

    fn nodes(&self) -> Vec<NodeId> {
        match self {
            AnnotationKind::PointDistance { a, b } => vec![a.node, b.node],
            AnnotationKind::EdgeLength { edge } => vec![edge.node],
            AnnotationKind::FaceDistance { a, b } => vec![a.node, b.node],
            AnnotationKind::Note { at, .. } => vec![at.node],
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Annotation {
    pub id: u32,
    pub kind: AnnotationKind,
}

#[derive(Resource)]
pub struct Annotations {
    pub items: Vec<Annotation>,
    pub next_id: u32,
    /// false なら寸法と注記をまとめて隠す
    pub visible: bool,
    /// パネルやラベルで選ばれている寸法・注記
    pub selected: Option<u32>,
}

impl Default for Annotations {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
            visible: true,
            selected: None,
        }
    }
}

impl Annotations {
    pub fn add(&mut self, kind: AnnotationKind) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(Annotation { id, kind });
        id
    }

    /// 寸法・注記の一覧を差し替える (元に戻す・やり直し)
    fn set_items(&mut self, items: Vec<Annotation>) {
        self.next_id = self
            .next_id
            .max(items.iter().map(|a| a.id + 1).max().unwrap_or(1));
        self.items = items;
        let selected = self.selected;
        if !self.items.iter().any(|a| Some(a.id) == selected) {
            self.selected = None;
        }
    }
}

/// 寸法・注記の変更1回分 (変更前と変更後の一覧)
struct AnnotationChange {
    label: String,
    before: Vec<Annotation>,
    after: Vec<Annotation>,
}

impl Command for AnnotationChange {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn undo(&self, world: &mut World) {
        world
            .resource_mut::<Annotations>()
            .set_items(self.before.clone());
    }

    fn redo(&self, world: &mut World) {
        world
            .resource_mut::<Annotations>()
            .set_items(self.after.clone());
    }
}

fn record_change(
    history: &mut History,
    label: &str,
    before: Vec<Annotation>,
    after: &[Annotation],
) {
    if before != after {
        history.push(AnnotationChange {
            label: label.to_string(),
            before,
            after: after.to_vec(),
        });
    }
}

// ---------------------------------------------------------------------------
// ファイルに書く形 (.plantwright の "annotations")
// ---------------------------------------------------------------------------

/// 部品はアセンブリの木の中の位置 (最上位から順にたどる子の番号) で指す。
/// ノードの id は開き直すと変わるため。
pub type NodePath = Vec<usize>;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AnchorRecord {
    pub node: NodePath,
    /// 箱なら半寸法で割った値 (-1..1)、取り込んだ部品なら部品の座標系
    pub local: [f64; 3],
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FaceRecord {
    pub node: NodePath,
    pub face: usize,
}

/// 寸法・注記1つ分。JSON では `"type"` で見分ける。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum AnnotationRecord {
    PointDistance { a: AnchorRecord, b: AnchorRecord },
    EdgeLength { node: NodePath, edge: usize },
    FaceDistance { a: FaceRecord, b: FaceRecord },
    Note { at: AnchorRecord, text: String },
}

pub fn node_path(model: &Model, id: NodeId) -> Option<NodePath> {
    model
        .ancestors(id)
        .iter()
        .map(|&a| {
            let siblings = match model.get(a)?.parent {
                Some(parent) => &model.get(parent)?.children,
                None => &model.roots,
            };
            siblings.iter().position(|&s| s == a)
        })
        .collect()
}

pub fn node_at_path(model: &Model, path: &[usize]) -> Option<NodeId> {
    let (&first, rest) = path.split_first()?;
    let mut id = *model.roots[..].get(first)?;
    for &index in rest {
        id = *model.get(id)?.children[..].get(index)?;
    }
    Some(id)
}

/// 保存する形にする。部品が消えている寸法・注記は書かない。
pub fn to_records(annotations: &Annotations, model: &Model) -> Vec<AnnotationRecord> {
    let path = |id: NodeId| {
        model
            .get(id)
            .is_some_and(|n| n.kind.is_part())
            .then(|| node_path(model, id))
            .flatten()
    };
    let anchor = |a: &Anchor| {
        Some(AnchorRecord {
            node: path(a.node)?,
            local: a.local,
        })
    };
    let face = |f: &FaceRef| {
        Some(FaceRecord {
            node: path(f.node)?,
            face: f.face,
        })
    };
    annotations
        .items
        .iter()
        .filter_map(|a| match &a.kind {
            AnnotationKind::PointDistance { a, b } => Some(AnnotationRecord::PointDistance {
                a: anchor(a)?,
                b: anchor(b)?,
            }),
            AnnotationKind::EdgeLength { edge } => Some(AnnotationRecord::EdgeLength {
                node: path(edge.node)?,
                edge: edge.edge,
            }),
            AnnotationKind::FaceDistance { a, b } => Some(AnnotationRecord::FaceDistance {
                a: face(a)?,
                b: face(b)?,
            }),
            AnnotationKind::Note { at, text } => Some(AnnotationRecord::Note {
                at: anchor(at)?,
                text: text.clone(),
            }),
        })
        .collect()
}

impl Annotations {
    /// ファイルから読む。指している部品が見つからないものは飛ばし、その件数を返す。
    pub fn from_records(records: &[AnnotationRecord], model: &Model) -> (Self, usize) {
        let part = |path: &NodePath| {
            node_at_path(model, path).filter(|&id| model.get(id).is_some_and(|n| n.kind.is_part()))
        };
        let anchor = |a: &AnchorRecord| {
            Some(Anchor {
                node: part(&a.node)?,
                local: a.local,
            })
            .filter(|a| a.local.iter().all(|v| v.is_finite()))
        };
        let face = |f: &FaceRecord| {
            Some(FaceRef {
                node: part(&f.node)?,
                face: f.face,
            })
        };
        let mut annotations = Self::default();
        let mut skipped = 0;
        for record in records {
            let kind = match record {
                AnnotationRecord::PointDistance { a, b } => anchor(a)
                    .zip(anchor(b))
                    .map(|(a, b)| AnnotationKind::PointDistance { a, b }),
                AnnotationRecord::EdgeLength { node, edge } => {
                    part(node).map(|node| AnnotationKind::EdgeLength {
                        edge: EdgeRef { node, edge: *edge },
                    })
                }
                AnnotationRecord::FaceDistance { a, b } => face(a)
                    .zip(face(b))
                    .map(|(a, b)| AnnotationKind::FaceDistance { a, b }),
                AnnotationRecord::Note { at, text } => anchor(at).map(|at| AnnotationKind::Note {
                    at,
                    text: text.clone(),
                }),
            };
            match kind {
                Some(kind) => {
                    annotations.add(kind);
                }
                None => skipped += 1,
            }
        }
        (annotations, skipped)
    }
}

// ---------------------------------------------------------------------------
// 部品の形状 (頂点・稜線・面)
// ---------------------------------------------------------------------------

/// アンカー座標系 → ワールド。
/// 箱は「箱の中心・箱の向き・半寸法で割った」座標系なので、寸法を変えても同じ頂点に付いて行く。
/// 取り込んだ部品は部品の座標系そのもの。
fn anchor_frame(model: &Model, node: NodeId) -> Option<DMat4> {
    let world = model.world_matrix(node);
    match &model.get(node)?.kind {
        NodeKind::Box(part) => Some(world * box_frame(part)),
        NodeKind::Imported { .. } | NodeKind::StandardBody { .. } => Some(world),
        NodeKind::Assembly { .. } | NodeKind::Standard(_) => None,
    }
}

fn box_frame(part: &BoxPart) -> DMat4 {
    DMat4::from_scale_rotation_translation(
        DVec3::from(part.size) / 2.0,
        euler_deg_to_quat(part.rotation_deg),
        DVec3::from(part.position),
    )
}

impl Anchor {
    fn world(&self, model: &Model) -> Option<DVec3> {
        Some(anchor_frame(model, self.node)?.transform_point3(DVec3::from(self.local)))
    }

    fn from_world(model: &Model, node: NodeId, world: DVec3) -> Option<Self> {
        let frame = anchor_frame(model, node)?;
        Some(Self {
            node,
            local: frame.inverse().transform_point3(world).to_array(),
        })
    }
}

#[derive(Clone, Debug)]
struct FaceGeometry {
    /// 面の輪郭の線分 (強調表示用)
    outline: Vec<[DVec3; 2]>,
    /// 面積で重み付けした中心
    center: DVec3,
    /// 単位法線 (向きは問わない)
    normal: DVec3,
    /// 平面か (曲面なら面間距離は中心どうしで測る)
    planar: bool,
}

/// 部品の座標系 (ノードの配置を掛ける前) での頂点・稜線・面
#[derive(Clone, Debug, Default)]
struct PartGeometry {
    vertices: Vec<DVec3>,
    /// 稜線のポリライン
    edges: Vec<Vec<DVec3>>,
    faces: Vec<FaceGeometry>,
}

fn polyline_length(points: &[DVec3]) -> f64 {
    points.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// 線に沿って半分の長さの位置
fn polyline_midpoint(points: &[DVec3]) -> DVec3 {
    let half = polyline_length(points) / 2.0;
    let mut walked = 0.0;
    for w in points.windows(2) {
        let step = w[0].distance(w[1]);
        if walked + step >= half && step > 0.0 {
            return w[0].lerp(w[1], (half - walked) / step);
        }
        walked += step;
    }
    points.first().copied().unwrap_or_default()
}

fn same_point(a: DVec3, b: DVec3) -> bool {
    a.distance_squared(b) < 1e-18
}

fn push_unique(points: &mut Vec<DVec3>, p: DVec3) {
    if !points.iter().any(|&q| same_point(q, p)) {
        points.push(p);
    }
}

/// 箱の許容誤差 (平面だけなので三角形の数には効かない)
const BOX_TOL: f64 = 0.005;

/// 三角形メッシュの面から中心・法線・輪郭を求める
fn mesh_face_geometry(poly: &FaceMesh) -> FaceGeometry {
    let positions = &poly.positions;
    let mut area_sum = 0.0;
    let mut center = DVec3::ZERO;
    let mut normal_sum = DVec3::ZERO;
    let mut normals = Vec::new();
    // 1つの三角形にしか使われていない辺が輪郭
    let mut edge_use: HashMap<(usize, usize), (usize, [DVec3; 2])> = HashMap::new();
    for tri in &poly.triangles {
        let idx = tri.map(|v| v as usize);
        let [a, b, c] = idx.map(|i| positions[i]);
        let cross = (b - a).cross(c - a);
        let area = cross.length() / 2.0;
        area_sum += area;
        center += (a + b + c) / 3.0 * area;
        normal_sum += cross;
        normals.push(cross.normalize_or_zero());
        for (i, j) in [(0, 1), (1, 2), (2, 0)] {
            let key = (idx[i].min(idx[j]), idx[i].max(idx[j]));
            edge_use
                .entry(key)
                .or_insert((0, [positions[idx[i]], positions[idx[j]]]))
                .0 += 1;
        }
    }
    let normal = normal_sum.normalize_or_zero();
    let planar = normals
        .iter()
        .all(|n| *n == DVec3::ZERO || n.dot(normal) > 1.0 - 1e-6);
    FaceGeometry {
        outline: edge_use
            .into_values()
            .filter(|(count, _)| *count == 1)
            .map(|(_, seg)| seg)
            .collect(),
        center: if area_sum > 0.0 {
            center / area_sum
        } else {
            positions.iter().copied().sum::<DVec3>() / positions.len().max(1) as f64
        },
        normal,
        planar,
    }
}

/// 部品の形状のメッシュから取り出す。面の並びは `ImportedShape::faces` の並び
/// (= ビューのピッキングの面番号)。
fn imported_geometry(faces: &[FaceMesh], edges: &[Vec<DVec3>]) -> PartGeometry {
    let mut g = PartGeometry {
        faces: faces.iter().map(mesh_face_geometry).collect(),
        ..default()
    };
    for line in edges {
        let points = line.clone();
        if let (Some(&first), Some(&last)) = (points.first(), points.last()) {
            push_unique(&mut g.vertices, first);
            push_unique(&mut g.vertices, last);
            g.edges.push(points);
        }
    }
    g
}

fn node_geometry(kind: &NodeKind) -> Option<PartGeometry> {
    match kind {
        NodeKind::Box(part) => {
            let (faces, edges) = tessellate_breps(&[part.build_solid().ok()?], BOX_TOL);
            Some(imported_geometry(&faces, &edges))
        }
        NodeKind::Imported { shape, .. } | NodeKind::StandardBody { shape, .. } => {
            Some(imported_geometry(&shape.faces, &shape.edges))
        }
        NodeKind::Assembly { .. } | NodeKind::Standard(_) => None,
    }
}

#[derive(PartialEq)]
enum CacheKey {
    Box([[f64; 3]; 3]),
    Shape(usize),
}

/// 部品ごとの形状のキャッシュ。箱はパラメータが変わったときだけ作り直し、
/// 取り込んだ部品は同じ形状のインスタンスで共有する。
#[derive(Resource, Default)]
struct GeometryCache {
    nodes: HashMap<NodeId, (CacheKey, Arc<PartGeometry>)>,
    shapes: HashMap<usize, Arc<PartGeometry>>,
}

impl GeometryCache {
    fn get(&self, node: NodeId) -> Option<&PartGeometry> {
        self.nodes.get(&node).map(|(_, g)| g.as_ref())
    }

    fn face(&self, f: FaceRef) -> Option<&FaceGeometry> {
        self.get(f.node)?.faces.get(f.face)
    }

    fn edge(&self, e: EdgeRef) -> Option<&[DVec3]> {
        self.get(e.node)?.edges.get(e.edge).map(Vec::as_slice)
    }

    /// モデルの変更に合わせて、変わった部品の形状だけ作り直す
    fn update(&mut self, model: &Model) {
        let ids = part_ids(model);
        self.nodes.retain(|id, _| ids.contains(id));
        for id in ids {
            let Some(node) = model.get(id) else {
                continue;
            };
            let key = match &node.kind {
                NodeKind::Box(part) => CacheKey::Box([part.size, part.position, part.rotation_deg]),
                NodeKind::Imported { shape, .. } | NodeKind::StandardBody { shape, .. } => {
                    CacheKey::Shape(shape.id)
                }
                NodeKind::Assembly { .. } | NodeKind::Standard(_) => continue,
            };
            if self.nodes.get(&id).is_some_and(|(k, _)| *k == key) {
                continue;
            }
            let geometry = match (&node.kind, &key) {
                (
                    NodeKind::Imported { .. } | NodeKind::StandardBody { .. },
                    CacheKey::Shape(shape),
                ) => self
                    .shapes
                    .entry(*shape)
                    .or_insert_with(|| Arc::new(node_geometry(&node.kind).unwrap_or_default()))
                    .clone(),
                _ => Arc::new(node_geometry(&node.kind).unwrap_or_default()),
            };
            self.nodes.insert(id, (key, geometry));
        }
    }
}

/// モデル中のすべての部品 (アセンブリを除く)
fn part_ids(model: &Model) -> Vec<NodeId> {
    model
        .roots
        .iter()
        .flat_map(|&r| model.subtree(r))
        .filter(|&id| model.get(id).is_some_and(|n| n.kind.is_part()))
        .collect()
}

fn update_geometry_cache(model: Res<Model>, mut cache: ResMut<GeometryCache>) {
    if model.is_changed() {
        cache.update(&model);
    }
}

/// 部品がすべてあるか (部品を消した寸法は、元に戻すときのために残して隠す)
fn parts_exist(kind: &AnnotationKind, model: &Model, cache: &GeometryCache) -> bool {
    let refs_ok = match *kind {
        AnnotationKind::FaceDistance { a, b } => cache.face(a).is_some() && cache.face(b).is_some(),
        AnnotationKind::EdgeLength { edge } => cache.edge(edge).is_some(),
        _ => true,
    };
    refs_ok
        && kind
            .nodes()
            .into_iter()
            .all(|id| model.get(id).is_some_and(|n| n.kind.is_part()))
}

/// 置いている途中の1点目の部品が消えたら取り消す
fn forget_missing_picks(model: Res<Model>, mut tool: ResMut<AnnotateTool>) {
    if model.is_changed() && tool.first.is_some_and(|p| model.get(p.node()).is_none()) {
        tool.first = None;
    }
}

/// シミュレーションの距離計測 (M) を始めたら、寸法の道具はやめる
fn yield_to_measure(measure: Res<MeasureTool>, mut tool: ResMut<AnnotateTool>) {
    if measure.is_changed() && measure.active && tool.mode != Mode::Off {
        tool.mode = Mode::Off;
    }
}

// ---------------------------------------------------------------------------
// 寸法の値と描く位置 (ワールド座標)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Measure {
    Length(f64),
    /// 度
    Angle(f64),
}

impl Measure {
    fn text(self, unit: &str) -> String {
        match self {
            Measure::Length(v) => length(v, unit),
            Measure::Angle(v) => format!("{v:.1}°"),
        }
    }
}

/// ワールド座標に置いた面
struct WorldFace {
    center: DVec3,
    normal: DVec3,
    planar: bool,
}

fn world_face(model: &Model, cache: &GeometryCache, f: FaceRef) -> Option<WorldFace> {
    let face = cache.face(f)?;
    let m = model.world_matrix(f.node);
    Some(WorldFace {
        center: m.transform_point3(face.center),
        normal: m.transform_vector3(face.normal).normalize_or_zero(),
        planar: face.planar,
    })
}

/// 2つの面の関係。平行な平面なら面 a の中心から面 b へ下ろした垂線の長さ、
/// 平行でない平面なら角度、曲面を含むなら中心どうしの距離。
fn face_measure(a: &WorldFace, b: &WorldFace) -> (Measure, DVec3, DVec3) {
    if !(a.planar && b.planar) {
        return (
            Measure::Length(a.center.distance(b.center)),
            a.center,
            b.center,
        );
    }
    let angle = a.normal.dot(b.normal).abs().min(1.0).acos();
    if angle < PARALLEL_TOL {
        let d = (a.center - b.center).dot(b.normal);
        (Measure::Length(d.abs()), a.center, a.center - d * b.normal)
    } else {
        (Measure::Angle(angle.to_degrees()), a.center, b.center)
    }
}

/// 描画用に、寸法・注記をワールド座標に解決したもの
struct Resolved {
    measure: Option<Measure>,
    /// 寸法線 (始点, 終点)
    line: Option<(DVec3, DVec3)>,
    /// 曲がった辺の形 (辺の長さで、辺が直線でないとき)
    curve: Option<Vec<DVec3>>,
    /// ラベルを置く位置 (注記なら注記の付く点)
    label_at: DVec3,
}

fn resolve(kind: &AnnotationKind, model: &Model, cache: &GeometryCache) -> Option<Resolved> {
    if !parts_exist(kind, model, cache) || !kind.nodes().into_iter().all(|n| model.is_shown(n)) {
        return None;
    }
    match kind {
        AnnotationKind::PointDistance { a, b } => {
            let (a, b) = (a.world(model)?, b.world(model)?);
            Some(Resolved {
                measure: Some(Measure::Length(a.distance(b))),
                line: Some((a, b)),
                curve: None,
                label_at: (a + b) / 2.0,
            })
        }
        AnnotationKind::EdgeLength { edge } => {
            let m = model.world_matrix(edge.node);
            let points: Vec<DVec3> = cache
                .edge(*edge)?
                .iter()
                .map(|&p| m.transform_point3(p))
                .collect();
            let (first, last) = (*points.first()?, *points.last()?);
            let length = polyline_length(&points);
            let straight = (length - first.distance(last)).abs() < 1e-9 * length.max(1.0);
            Some(Resolved {
                measure: Some(Measure::Length(length)),
                line: straight.then_some((first, last)),
                label_at: polyline_midpoint(&points),
                curve: (!straight).then_some(points),
            })
        }
        AnnotationKind::FaceDistance { a, b } => {
            let (a, b) = (world_face(model, cache, *a)?, world_face(model, cache, *b)?);
            let (measure, start, end) = face_measure(&a, &b);
            Some(Resolved {
                measure: Some(measure),
                line: Some((start, end)),
                curve: None,
                label_at: (start + end) / 2.0,
            })
        }
        AnnotationKind::Note { at, .. } => Some(Resolved {
            measure: None,
            line: None,
            curve: None,
            label_at: at.world(model)?,
        }),
    }
}

/// 一覧に出す値の文字列
fn value_text(kind: &AnnotationKind, model: &Model, cache: &GeometryCache) -> String {
    if let AnnotationKind::Note { text, .. } = kind {
        // 一覧の幅が広がりすぎないよう、1行目の先頭だけ出す
        let first = text.lines().next().unwrap_or("");
        let mut short: String = first.chars().take(12).collect();
        if short.len() < first.len() {
            short.push('…');
        }
        return short;
    }
    resolve(kind, model, cache)
        .and_then(|r| r.measure)
        .map_or("-".to_string(), |m| m.text(&model.unit))
}

// ---------------------------------------------------------------------------
// 置くときの操作
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Mode {
    #[default]
    Off,
    PointDistance,
    EdgeLength,
    FaceDistance,
    Note,
}

impl Mode {
    const ALL: [Mode; 5] = [
        Mode::Off,
        Mode::PointDistance,
        Mode::EdgeLength,
        Mode::FaceDistance,
        Mode::Note,
    ];

    fn label(self) -> &'static str {
        match self {
            Mode::Off => "選択",
            Mode::PointDistance => "点間距離",
            Mode::EdgeLength => "辺の長さ",
            Mode::FaceDistance => "面間距離",
            Mode::Note => "注記",
        }
    }
}

/// カーソルの下で拾えたもの
#[derive(Clone, Copy, Debug)]
enum Pick {
    Point {
        anchor: Anchor,
        world: DVec3,
        /// 頂点などに吸着したか
        snapped: bool,
    },
    Edge(EdgeRef),
    Face(FaceRef),
}

impl Pick {
    fn node(&self) -> NodeId {
        match self {
            Pick::Point { anchor, .. } => anchor.node,
            Pick::Edge(e) => e.node,
            Pick::Face(f) => f.node,
        }
    }
}

#[derive(Resource, Default)]
pub struct AnnotateTool {
    pub mode: Mode,
    /// 2つ選ぶ寸法の1つ目
    first: Option<Pick>,
    /// カーソルの下にあるもの (吸着の目印と仮の寸法を出す)
    hover: Option<Pick>,
    /// 置いたばかりの注記の文字入力にフォーカスを移す
    focus_note: bool,
    /// 注記の文字を打ち始めたときの一覧 (元に戻す用)
    note_before: Option<Vec<Annotation>>,
}

/// 寸法を置いている間は、クリックで部品を選択しない
pub fn annotate_tool_off(tool: Res<AnnotateTool>) -> bool {
    tool.mode == Mode::Off
}

fn cancel_with_escape(keys: Res<ButtonInput<KeyCode>>, mut tool: ResMut<AnnotateTool>) {
    if keys.just_pressed(KeyCode::Escape) {
        if tool.first.is_some() {
            tool.first = None;
        } else {
            tool.mode = Mode::Off;
        }
    }
}

/// 点 p から線分 ab までの距離 (画面上)
fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(f32::EPSILON)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

#[allow(clippy::too_many_arguments)]
fn annotate_input(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    viewport: Res<ViewportRect>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    under_pointer: PartUnderPointer,
    model: Res<Model>,
    cache: Res<GeometryCache>,
    mut tool: ResMut<AnnotateTool>,
    mut annotations: ResMut<Annotations>,
    mut history: ResMut<History>,
) {
    if tool.mode == Mode::Off {
        tool.hover = None;
        tool.first = None;
        return;
    }
    let cursor = window
        .cursor_position()
        .filter(|&c| viewport.contains(c) && !viewport.pointer_on_ui);
    let Some(cursor) = cursor else {
        tool.hover = None;
        return;
    };
    let (camera, camera_transform) = *camera;
    let to_screen = |p: DVec3| camera.world_to_viewport(camera_transform, p.as_vec3()).ok();

    let hit = under_pointer.hit().map(|hit| (hit, hit.point));

    // 吸着や辺の候補は、画面上でカーソルの近くにある部品だけから探す
    let near_cursor = |id: NodeId| {
        let Some(bounds) = model.world_bounds(id) else {
            return false;
        };
        let corners: Vec<Vec2> = bounds.corners().into_iter().filter_map(to_screen).collect();
        let min = corners.iter().copied().reduce(Vec2::min);
        let max = corners.iter().copied().reduce(Vec2::max);
        min.zip(max).is_some_and(|(min, max)| {
            let margin = Vec2::splat(SNAP_PX);
            Rect::from_corners(min - margin, max + margin).contains(cursor)
        })
    };
    let candidates: Vec<NodeId> = part_ids(&model)
        .into_iter()
        .filter(|&id| model.is_shown(id) && near_cursor(id))
        .collect();

    tool.hover = match tool.mode {
        Mode::Off => None,
        Mode::PointDistance | Mode::Note => {
            // 頂点・辺の中点・(当たった面の) 中心のうち、カーソルに一番近いものに吸着する
            let mut best: Option<(f32, NodeId, DVec3)> = None;
            for &id in &candidates {
                let Some(geometry) = cache.get(id) else {
                    continue;
                };
                let m = model.world_matrix(id);
                let mut points = geometry.vertices.clone();
                points.extend(geometry.edges.iter().map(|e| polyline_midpoint(e)));
                if let Some((tag, _)) = hit
                    && tag.node == id
                    && let Some(face) = geometry.faces.get(tag.face)
                {
                    points.push(face.center);
                }
                for p in points {
                    let p = m.transform_point3(p);
                    if let Some(s) = to_screen(p) {
                        let d = s.distance(cursor);
                        if d < SNAP_PX && best.is_none_or(|(bd, ..)| d < bd) {
                            best = Some((d, id, p));
                        }
                    }
                }
            }
            let snapped = best.map(|(_, id, p)| (id, p, true));
            let free = hit.map(|(tag, p)| (tag.node, p.as_dvec3(), false));
            snapped.or(free).and_then(|(id, world, snapped)| {
                Some(Pick::Point {
                    anchor: Anchor::from_world(&model, id, world)?,
                    world,
                    snapped,
                })
            })
        }
        Mode::EdgeLength => {
            let mut best: Option<(f32, EdgeRef)> = None;
            for &id in &candidates {
                let Some(geometry) = cache.get(id) else {
                    continue;
                };
                let m = model.world_matrix(id);
                for (index, edge) in geometry.edges.iter().enumerate() {
                    let screen: Vec<Option<Vec2>> = edge
                        .iter()
                        .map(|&p| to_screen(m.transform_point3(p)))
                        .collect();
                    for w in screen.windows(2) {
                        let (Some(a), Some(b)) = (w[0], w[1]) else {
                            continue;
                        };
                        let d = segment_distance(cursor, a, b);
                        if d < EDGE_PICK_PX && best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((
                                d,
                                EdgeRef {
                                    node: id,
                                    edge: index,
                                },
                            ));
                        }
                    }
                }
            }
            best.map(|(_, edge)| Pick::Edge(edge))
        }
        Mode::FaceDistance => hit.map(|(tag, _)| {
            Pick::Face(FaceRef {
                node: tag.node,
                face: tag.face,
            })
        }),
    };

    if !buttons.just_pressed(MouseButton::Left) || alt_pressed(&keys) {
        return;
    }
    let Some(pick) = tool.hover else {
        return;
    };
    let created = match (tool.mode, tool.first, pick) {
        (Mode::PointDistance, None, Pick::Point { .. })
        | (Mode::FaceDistance, None, Pick::Face(_)) => {
            tool.first = Some(pick);
            None
        }
        (
            Mode::PointDistance,
            Some(Pick::Point {
                anchor: a,
                world: wa,
                ..
            }),
            Pick::Point {
                anchor: b,
                world: wb,
                ..
            },
        ) if wa.distance(wb) > 1e-9 => {
            tool.first = None;
            Some(AnnotationKind::PointDistance { a, b })
        }
        (Mode::FaceDistance, Some(Pick::Face(a)), Pick::Face(b)) if a != b => {
            tool.first = None;
            Some(AnnotationKind::FaceDistance { a, b })
        }
        (Mode::EdgeLength, _, Pick::Edge(edge)) => Some(AnnotationKind::EdgeLength { edge }),
        (Mode::Note, _, Pick::Point { anchor, .. }) => {
            tool.focus_note = true;
            Some(AnnotationKind::Note {
                at: anchor,
                text: "メモ".to_string(),
            })
        }
        _ => None,
    };
    if let Some(kind) = created {
        let before = annotations.items.clone();
        let label = format!("{}を追加", kind.label());
        let id = annotations.add(kind);
        annotations.selected = Some(id);
        record_change(&mut history, &label, before, &annotations.items);
    }
}

// ---------------------------------------------------------------------------
// 描画
// ---------------------------------------------------------------------------

/// 寸法線用のギズモ。形状に隠れないよう手前に描く。
#[derive(Default, Reflect, GizmoConfigGroup)]
struct AnnotationGizmos;

fn configure_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<AnnotationGizmos>();
    config.depth_bias = -1.0;
    config.line.width = 2.5;
}

fn dimension_line(gizmos: &mut Gizmos<AnnotationGizmos>, a: DVec3, b: DVec3, color: Color) {
    let (a, b) = (a.as_vec3(), b.as_vec3());
    let length = a.distance(b);
    if length < 1e-6 {
        return;
    }
    // 矢じりは寸法線の長さに対して控えめな大きさにする
    gizmos
        .arrow(a, b, color)
        .with_double_end()
        .with_tip_length(length * 0.08);
}

fn draw_face(
    gizmos: &mut Gizmos<AnnotationGizmos>,
    model: &Model,
    cache: &GeometryCache,
    f: FaceRef,
    color: Color,
) {
    let Some(face) = cache.face(f) else {
        return;
    };
    let m = model.world_matrix(f.node);
    for [a, b] in &face.outline {
        gizmos.line(
            m.transform_point3(*a).as_vec3(),
            m.transform_point3(*b).as_vec3(),
            color,
        );
    }
}

fn draw_edge(
    gizmos: &mut Gizmos<AnnotationGizmos>,
    model: &Model,
    cache: &GeometryCache,
    e: EdgeRef,
    color: Color,
) {
    if let Some(points) = cache.edge(e) {
        let m = model.world_matrix(e.node);
        gizmos.linestrip(
            points.iter().map(|&p| m.transform_point3(p).as_vec3()),
            color,
        );
    }
}

fn draw_annotations(
    mut gizmos: Gizmos<AnnotationGizmos>,
    model: Res<Model>,
    cache: Res<GeometryCache>,
    annotations: Res<Annotations>,
    tool: Res<AnnotateTool>,
) {
    if annotations.visible {
        for annotation in &annotations.items {
            let Some(r) = resolve(&annotation.kind, &model, &cache) else {
                continue;
            };
            let selected = annotations.selected == Some(annotation.id);
            let color = if selected { SELECTED_COLOR } else { DIM_COLOR };
            match (r.line, r.measure) {
                // 平行でない面の角度は、中心どうしを細い線で結ぶだけ
                (Some((a, b)), Some(Measure::Angle(_))) => {
                    gizmos.line(a.as_vec3(), b.as_vec3(), color.with_alpha(0.6));
                }
                (Some((a, b)), _) => dimension_line(&mut gizmos, a, b, color),
                _ => {}
            }
            if let Some(curve) = &r.curve {
                gizmos.linestrip(curve.iter().map(|p| p.as_vec3()), color);
            }
            if selected && let AnnotationKind::FaceDistance { a, b } = annotation.kind {
                draw_face(&mut gizmos, &model, &cache, a, color);
                draw_face(&mut gizmos, &model, &cache, b, color);
            }
        }
    }

    // 置いている途中の仮表示
    for pick in [tool.first, tool.hover].into_iter().flatten() {
        match pick {
            Pick::Edge(e) => draw_edge(&mut gizmos, &model, &cache, e, PREVIEW_COLOR),
            Pick::Face(f) => draw_face(&mut gizmos, &model, &cache, f, PREVIEW_COLOR),
            Pick::Point { .. } => {}
        }
    }
    if let Some((_, a, b)) = preview_measure(&tool, &model, &cache) {
        dimension_line(&mut gizmos, a, b, PREVIEW_COLOR);
    }
}

/// 置いている途中の寸法 (値, 始点, 終点)
fn preview_measure(
    tool: &AnnotateTool,
    model: &Model,
    cache: &GeometryCache,
) -> Option<(Measure, DVec3, DVec3)> {
    match (tool.first, tool.hover?) {
        (Some(Pick::Point { world: a, .. }), Pick::Point { world: b, .. }) => {
            Some((Measure::Length(a.distance(b)), a, b))
        }
        (_, Pick::Edge(edge)) => {
            let r = resolve(&AnnotationKind::EdgeLength { edge }, model, cache)?;
            let (a, b) = r.line.unwrap_or((r.label_at, r.label_at));
            Some((r.measure?, a, b))
        }
        (Some(Pick::Face(a)), Pick::Face(b)) if a != b => {
            let (a, b) = (world_face(model, cache, a)?, world_face(model, cache, b)?);
            Some(face_measure(&a, &b))
        }
        _ => None,
    }
}

fn to_egui(c: Color) -> egui::Color32 {
    let [r, g, b, a] = c.to_srgba().to_u8_array();
    egui::Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// 寸法の値と注記の文字を、ビュー上にラベルとして描く。ラベルをクリックすると選択できる。
fn draw_labels(
    mut contexts: EguiContexts,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut viewport: ResMut<ViewportRect>,
    model: Res<Model>,
    cache: Res<GeometryCache>,
    mut annotations: ResMut<Annotations>,
    tool: Res<AnnotateTool>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let (camera, camera_transform) = *camera;
    let view = viewport.rect.map(|r| {
        egui::Rect::from_min_max(egui::pos2(r.min.x, r.min.y), egui::pos2(r.max.x, r.max.y))
    });
    let to_screen = |p: DVec3| {
        camera
            .world_to_viewport(camera_transform, p.as_vec3())
            .ok()
            .map(|v| egui::pos2(v.x, v.y))
            .filter(|p| view.is_none_or(|r| r.contains(*p)))
    };
    let mut painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("annotation_leaders"),
    ));
    if let Some(view) = view {
        painter.set_clip_rect(view);
    }

    let mut clicked = None;
    if annotations.visible {
        for annotation in &annotations.items {
            let Some(r) = resolve(&annotation.kind, &model, &cache) else {
                continue;
            };
            let Some(at) = to_screen(r.label_at) else {
                continue;
            };
            let selected = annotations.selected == Some(annotation.id);
            let accent = to_egui(if selected { SELECTED_COLOR } else { DIM_COLOR });
            let (text, fill, text_color, pos, pivot) = match (&annotation.kind, r.measure) {
                (AnnotationKind::Note { text, .. }, _) => {
                    // 注記は付く点から右上に引き出して置く
                    let pos = at + egui::vec2(36.0, -36.0);
                    let leader = to_egui(if selected { SELECTED_COLOR } else { NOTE_COLOR });
                    painter.line_segment([at, pos], egui::Stroke::new(1.5, leader));
                    painter.circle_filled(at, 3.5, leader);
                    let fill = egui::Color32::from_rgb(255, 243, 170);
                    let text_color = egui::Color32::from_gray(20);
                    (
                        text.clone(),
                        fill,
                        text_color,
                        pos,
                        egui::Align2::LEFT_BOTTOM,
                    )
                }
                (_, Some(m)) => {
                    let fill = egui::Color32::from_rgba_unmultiplied(20, 24, 30, 230);
                    let text = m.text(&model.unit);
                    let pivot = egui::Align2::CENTER_CENTER;
                    (text, fill, egui::Color32::WHITE, at, pivot)
                }
                _ => continue,
            };
            // ラベルはパネルより奥に描き、ラベルの上ではビューのクリックが効かないようにする (下で)
            let response = egui::Area::new(egui::Id::new(("annotation_label", annotation.id)))
                .fixed_pos(pos)
                .pivot(pivot)
                .order(egui::Order::Background)
                .constrain(false)
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(fill)
                        .stroke(egui::Stroke::new(if selected { 2.0 } else { 1.0 }, accent))
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(6, 3))
                        .show(ui, |ui| {
                            let text = egui::RichText::new(text).color(text_color);
                            let label = egui::Label::new(text)
                                .wrap_mode(egui::TextWrapMode::Extend)
                                .selectable(false)
                                .sense(egui::Sense::click());
                            ui.add(label).clicked()
                        })
                        .inner
                });
            if response.inner {
                clicked = Some(annotation.id);
            }
            if response.response.contains_pointer() {
                viewport.pointer_on_ui = true;
            }
        }
    }
    if clicked.is_some() {
        annotations.selected = clicked;
    }

    // 吸着の目印と、置いている途中の値
    if let Some(Pick::Point { world, snapped, .. }) = tool.hover
        && let Some(p) = to_screen(world)
    {
        let stroke = egui::Stroke::new(2.0, to_egui(PREVIEW_COLOR));
        if snapped {
            let rect = egui::Rect::from_center_size(p, egui::vec2(11.0, 11.0));
            painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Middle);
        } else {
            painter.circle_stroke(p, 4.0, stroke);
        }
    }
    if let Some((measure, a, b)) = preview_measure(&tool, &model, &cache)
        && let Some(p) = to_screen((a + b) / 2.0)
    {
        let galley = painter.layout_no_wrap(
            measure.text(&model.unit),
            egui::FontId::proportional(15.0),
            to_egui(PREVIEW_COLOR),
        );
        let rect = egui::Align2::CENTER_BOTTOM
            .anchor_size(p + egui::vec2(0.0, -12.0), galley.size())
            .expand2(egui::vec2(6.0, 3.0));
        let fill = egui::Color32::from_rgba_unmultiplied(20, 24, 30, 230);
        painter.rect_filled(rect, 4.0, fill);
        painter.galley(
            rect.shrink2(egui::vec2(6.0, 3.0)).min,
            galley,
            egui::Color32::WHITE,
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// パネル
// ---------------------------------------------------------------------------

fn mode_hint(tool: &AnnotateTool) -> &'static str {
    match (tool.mode, tool.first.is_some()) {
        (Mode::Off, _) => "道具を選んでビューをクリックすると、寸法や注記を置けます",
        (Mode::PointDistance, false) => "1点目をクリック (頂点・辺の中点・面の中心に吸着)",
        (Mode::PointDistance, true) => "2点目をクリック (Esc で取り消し)",
        (Mode::EdgeLength, _) => "長さを測る辺をクリック",
        (Mode::FaceDistance, false) => "1つ目の面をクリック",
        (Mode::FaceDistance, true) => "2つ目の面をクリック (Esc で取り消し)",
        (Mode::Note, _) => "注記を付ける場所をクリック。文字は下の欄で編集",
    }
}

#[allow(clippy::too_many_arguments)]
fn annotation_panel(
    mut contexts: EguiContexts,
    model: Res<Model>,
    cache: Res<GeometryCache>,
    mut annotations: ResMut<Annotations>,
    mut tool: ResMut<AnnotateTool>,
    mut selection: ResMut<Selection>,
    mut measure: ResMut<MeasureTool>,
    mut history: ResMut<History>,
    mut dock: ResMut<Dock>,
) -> Result {
    let Some(window) = dock::panel(contexts.ctx_mut()?, Tool::Annotation, &dock) else {
        // パネルを閉じたら、置きかけの寸法もやめる (クリックで部品を選べるように戻す)
        if tool.mode != Mode::Off {
            tool.mode = Mode::Off;
            tool.first = None;
        }
        return Ok(());
    };
    let mut open = true;
    window.open(&mut open).show(contexts.ctx_mut()?, |ui| {
        ui.horizontal_wrapped(|ui| {
            for mode in Mode::ALL {
                if ui
                    .selectable_label(tool.mode == mode, mode.label())
                    .clicked()
                {
                    tool.mode = mode;
                    tool.first = None;
                    if mode != Mode::Off {
                        // ギズモがクリックを横取りしないよう、部品の選択を外す
                        selection.clear();
                        if measure.active {
                            measure.set_active(false);
                        }
                    }
                }
            }
        });
        ui.weak(mode_hint(&tool));
        ui.checkbox(&mut annotations.visible, "寸法と注記を表示");
        ui.separator();

        // 部品を消した寸法は、元に戻すときのために残っているが一覧には出さない
        let shown = |a: &&Annotation| parts_exist(&a.kind, &model, &cache);
        if !annotations.items.iter().any(|a| shown(&a)) {
            ui.weak("寸法・注記はまだありません");
            return;
        }
        let mut delete = None;
        let mut select = None;
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                for annotation in annotations.items.iter().filter(shown) {
                    ui.horizontal(|ui| {
                        let text = format!(
                            "{}  {}",
                            annotation.kind.label(),
                            value_text(&annotation.kind, &model, &cache)
                        );
                        let selected = annotations.selected == Some(annotation.id);
                        if ui.selectable_label(selected, text).clicked() {
                            select = Some((!selected).then_some(annotation.id));
                        }
                        if ui.small_button("削除").clicked() {
                            delete = Some(annotation.id);
                        }
                    });
                }
            });
        if let Some(selected) = select {
            annotations.selected = selected;
        }
        let before = annotations.items.clone();
        if let Some(id) = delete {
            let label = before
                .iter()
                .find(|a| a.id == id)
                .map_or("", |a| a.kind.label());
            let label = format!("{label}を削除");
            annotations.items.retain(|a| a.id != id);
            if annotations.selected == Some(id) {
                annotations.selected = None;
            }
            record_change(&mut history, &label, before.clone(), &annotations.items);
        }

        let selected = annotations.selected;
        if let Some(Annotation {
            kind: AnnotationKind::Note { text, .. },
            ..
        }) = annotations
            .items
            .iter_mut()
            .find(|a| Some(a.id) == selected)
        {
            ui.separator();
            ui.label("注記の文字");
            let response = ui.add(
                egui::TextEdit::multiline(text)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            if tool.focus_note {
                response.request_focus();
                tool.focus_note = false;
            }
            // 打ち始めの一覧を覚えておき、入力欄から離れたときに1件の履歴にする
            if response.gained_focus() {
                tool.note_before = Some(before.clone());
            }
            if response.lost_focus()
                && let Some(note_before) = tool.note_before.take()
            {
                record_change(&mut history, "注記を編集", note_before, &annotations.items);
            }
        }
        ui.separator();
        if ui.button("すべて削除").clicked() {
            let before = annotations.items.clone();
            annotations.items.clear();
            annotations.selected = None;
            record_change(
                &mut history,
                "寸法と注記をすべて削除",
                before,
                &annotations.items,
            );
        }
    });
    if !open {
        dock.set_open(Tool::Annotation, false);
    }
    Ok(())
}

impl AnnotateTool {
    /// ビューの下に出す案内 (寸法・注記を置いている間だけ)
    pub fn hint(&self) -> Option<String> {
        let text = match (self.mode, self.first.is_some()) {
            (Mode::Off, _) => return None,
            (Mode::PointDistance, false) => "1点目の頂点か面をクリックしてください (Esc でやめる)",
            (Mode::PointDistance, true) => {
                "2点目をクリックすると点間の寸法を置きます (Esc で1点目を取り消し)"
            }
            (Mode::EdgeLength, _) => "辺をクリックすると長さの寸法を置きます (Esc でやめる)",
            (Mode::FaceDistance, false) => "1つ目の面をクリックしてください (Esc でやめる)",
            (Mode::FaceDistance, true) => {
                "2つ目の面をクリックすると面間の寸法を置きます (Esc で1つ目を取り消し)"
            }
            (Mode::Note, _) => "注記を付ける場所をクリックしてください (Esc でやめる)",
        };
        Some(text.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plantwright_core::formats::step::import_step;

    fn approx(a: DVec3, b: DVec3) -> bool {
        a.distance(b) < 1e-9
    }

    type BoxSpec = ([f64; 3], [f64; 3], [f64; 3]);

    /// (寸法, 位置, 回転) の箱を並べたモデル
    fn box_model(boxes: &[BoxSpec]) -> (Model, Vec<NodeId>, GeometryCache) {
        let mut model = Model::default();
        let ids: Vec<NodeId> = boxes
            .iter()
            .map(|&(size, position, rotation)| {
                let id = model.add_box(size, position);
                box_part(&mut model, id).rotation_deg = rotation;
                id
            })
            .collect();
        let mut cache = GeometryCache::default();
        cache.update(&model);
        (model, ids, cache)
    }

    fn box_part(model: &mut Model, id: NodeId) -> &mut BoxPart {
        match &mut model.get_mut(id).unwrap().kind {
            NodeKind::Box(part) => part,
            _ => unreachable!(),
        }
    }

    #[test]
    fn anchor_round_trip_with_rotation() {
        let (model, ids, _) = box_model(&[([2.0, 1.0, 3.0], [1.0, 2.0, -1.0], [15.0, 30.0, 45.0])]);
        let world = DVec3::new(1.3, 2.2, -0.4);
        let anchor = Anchor::from_world(&model, ids[0], world).unwrap();
        assert!(approx(anchor.world(&model).unwrap(), world));
    }

    #[test]
    fn anchor_follows_resize_and_move() {
        let (mut model, ids, _) = box_model(&[([2.0, 1.0, 1.0], [0.0; 3], [0.0; 3])]);
        let corner = Anchor::from_world(&model, ids[0], DVec3::new(1.0, 0.5, 0.5)).unwrap();
        assert!(approx(DVec3::from(corner.local), DVec3::ONE));
        let part = box_part(&mut model, ids[0]);
        part.size = [4.0, 2.0, 2.0];
        part.position = [10.0, 0.0, 0.0];
        assert!(approx(
            corner.world(&model).unwrap(),
            DVec3::new(12.0, 1.0, 1.0)
        ));
    }

    #[test]
    fn box_vertices_are_unit_corners_in_anchor_frame() {
        // 回した箱の頂点が、アンカー座標系ではちょうど ±1 の角になる
        let (model, ids, cache) =
            box_model(&[([2.0, 1.0, 3.0], [1.0, 0.5, 0.0], [20.0, -35.0, 70.0])]);
        let geometry = cache.get(ids[0]).unwrap();
        assert_eq!(geometry.vertices.len(), 8);
        for &v in &geometry.vertices {
            let anchor = Anchor::from_world(&model, ids[0], v).unwrap();
            for c in anchor.local {
                assert!((c.abs() - 1.0).abs() < 1e-9, "{:?}", anchor.local);
            }
        }
    }

    #[test]
    fn box_geometry_counts() {
        let (_, ids, cache) = box_model(&[([2.0, 1.0, 1.0], [0.0; 3], [0.0; 3])]);
        let g = cache.get(ids[0]).unwrap();
        assert_eq!((g.vertices.len(), g.edges.len(), g.faces.len()), (8, 12, 6));
        let mut lengths: Vec<f64> = g.edges.iter().map(|e| polyline_length(e)).collect();
        lengths.sort_by(f64::total_cmp);
        assert!((lengths[0] - 1.0).abs() < 1e-12);
        assert!((lengths[11] - 2.0).abs() < 1e-12);
    }

    fn find_face(cache: &GeometryCache, node: NodeId, pred: impl Fn(DVec3) -> bool) -> FaceRef {
        let faces = &cache.get(node).unwrap().faces;
        let face = faces.iter().position(|f| pred(f.center)).unwrap();
        FaceRef { node, face }
    }

    #[test]
    fn distance_between_parallel_faces_of_two_boxes() {
        let (model, ids, cache) = box_model(&[
            ([1.0; 3], [0.0; 3], [0.0; 3]),
            ([1.0; 3], [3.0, 0.2, 0.0], [0.0; 3]),
        ]);
        // 1つ目の +X 面と 2つ目の -X 面のすき間は 3 - 0.5 - 0.5 = 2
        let a = find_face(&cache, ids[0], |c| c.x > 0.49);
        let b = find_face(&cache, ids[1], |c| c.x < 2.51);
        let kind = AnnotationKind::FaceDistance { a, b };
        let r = resolve(&kind, &model, &cache).unwrap();
        assert!(matches!(r.measure, Some(Measure::Length(d)) if (d - 2.0).abs() < 1e-12));
        let (start, end) = r.line.unwrap();
        assert!(approx(start, DVec3::new(0.5, 0.0, 0.0)));
        assert!(approx(end, DVec3::new(2.5, 0.0, 0.0)));
    }

    #[test]
    fn angle_between_tilted_faces() {
        let (model, ids, cache) = box_model(&[
            ([1.0; 3], [0.0; 3], [0.0; 3]),
            ([1.0; 3], [3.0, 0.0, 0.0], [0.0, 0.0, 30.0]),
        ]);
        let top = find_face(&cache, ids[0], |c| c.y > 0.49);
        let faces = &cache.get(ids[1]).unwrap().faces;
        let highest = (0..faces.len())
            .max_by(|&x, &y| faces[x].center.y.total_cmp(&faces[y].center.y))
            .unwrap();
        let kind = AnnotationKind::FaceDistance {
            a: top,
            b: FaceRef {
                node: ids[1],
                face: highest,
            },
        };
        let r = resolve(&kind, &model, &cache).unwrap();
        assert!(matches!(r.measure, Some(Measure::Angle(d)) if (d - 30.0).abs() < 1e-9));
    }

    #[test]
    fn box_face_order_matches_picking() {
        // ビューのピッキングの面番号は表示メッシュの面の並び。寸法側の面番号と同じ面を指すこと
        let part = BoxPart {
            size: [2.0, 1.0, 3.0],
            position: [0.5, 0.0, 0.0],
            rotation_deg: [0.0, 25.0, 0.0],
        };
        let geometry = node_geometry(&NodeKind::Box(part.clone())).unwrap();
        let (faces, _) = tessellate_breps(&[part.build_solid().unwrap()], 0.001);
        assert_eq!(faces.len(), 6);
        assert_eq!(faces.len(), geometry.faces.len());
        for (face, ours) in faces.iter().zip(&geometry.faces) {
            assert!(approx(mesh_face_geometry(face).center, ours.center));
            assert!(ours.planar);
        }
    }

    #[test]
    fn imported_part_edges_and_faces() {
        let file = import_step(
            include_str!("../../../assets/sample_assembly.step"),
            "sample_assembly.step",
        )
        .unwrap();
        let mut model = Model::default();
        model.add_imported(
            file.root,
            file.unit,
            plantwright_core::model::StepSource::Sample,
        );
        let mut cache = GeometryCache::default();
        cache.update(&model);
        let parts = part_ids(&model);
        assert!(!parts.is_empty());
        for id in parts {
            let g = cache.get(id).unwrap();
            assert!(!g.faces.is_empty() && !g.edges.is_empty());
            assert!(g.edges.iter().all(|e| polyline_length(e) > 0.0));
            assert!(
                g.faces
                    .iter()
                    .all(|f| (f.normal.length() - 1.0).abs() < 1e-9)
            );
            // アンカーはワールドに戻すと同じ点になる
            let center = model.world_matrix(id).transform_point3(g.faces[0].center);
            let anchor = Anchor::from_world(&model, id, center).unwrap();
            assert!(approx(anchor.world(&model).unwrap(), center));
        }
    }

    #[test]
    fn curved_edge_length_follows_curve() {
        let quarter: Vec<DVec3> = (0..=90)
            .map(|d| {
                let r = (d as f64).to_radians();
                DVec3::new(r.cos(), r.sin(), 0.0)
            })
            .collect();
        let length = polyline_length(&quarter);
        assert!((length - std::f64::consts::FRAC_PI_2).abs() < 1e-4);
        let mid = polyline_midpoint(&quarter);
        assert!(approx(mid, DVec3::new(0.5f64.sqrt(), 0.5f64.sqrt(), 0.0)));
    }

    #[test]
    fn removed_part_hides_annotation_until_undo() {
        let (mut model, ids, cache) = box_model(&[([1.0; 3], [0.0; 3], [0.0; 3])]);
        let kind = AnnotationKind::EdgeLength {
            edge: EdgeRef {
                node: ids[0],
                edge: 0,
            },
        };
        let snapshot = model.snapshot(ids[0]).unwrap();
        model.remove(ids[0]);
        assert!(resolve(&kind, &model, &cache).is_none());
        // 部品の削除を元に戻すと同じ id で戻るので、寸法も見える
        model.restore(&snapshot);
        assert!(resolve(&kind, &model, &cache).is_some());
    }

    #[test]
    fn records_round_trip_through_node_paths() {
        let file = import_step(
            include_str!("../../../assets/sample_assembly.step"),
            "sample_assembly.step",
        )
        .unwrap();
        let mut model = Model::default();
        model.add_imported(
            file.root,
            file.unit,
            plantwright_core::model::StepSource::Sample,
        );
        let box_id = model.add_box([10.0; 3], [300.0, 5.0, 0.0]);
        let deep = part_ids(&model)
            .into_iter()
            .find(|&id| model.ancestors(id).len() >= 3)
            .unwrap();
        let anchor = |node, local| Anchor { node, local };
        let mut annotations = Annotations::default();
        annotations.add(AnnotationKind::PointDistance {
            a: anchor(deep, [1.0, 2.0, 3.0]),
            b: anchor(box_id, [1.0, 1.0, -1.0]),
        });
        annotations.add(AnnotationKind::FaceDistance {
            a: FaceRef {
                node: deep,
                face: 1,
            },
            b: FaceRef {
                node: box_id,
                face: 2,
            },
        });
        annotations.add(AnnotationKind::EdgeLength {
            edge: EdgeRef {
                node: box_id,
                edge: 3,
            },
        });
        annotations.add(AnnotationKind::Note {
            at: anchor(deep, [0.0; 3]),
            text: "要確認".into(),
        });
        let records = to_records(&annotations, &model);
        assert_eq!(records.len(), 4);
        let json = serde_json::to_string(&records).unwrap();
        let parsed: Vec<AnnotationRecord> = serde_json::from_str(&json).unwrap();
        let (restored, skipped) = Annotations::from_records(&parsed, &model);
        assert_eq!(skipped, 0);
        let kinds = |a: &Annotations| a.items.iter().map(|a| a.kind.clone()).collect::<Vec<_>>();
        assert_eq!(kinds(&restored), kinds(&annotations));

        // 部品を消したら、その寸法は保存しない。読むときに見つからない部品は飛ばす
        model.remove(box_id);
        assert_eq!(to_records(&annotations, &model).len(), 1);
        let (restored, skipped) = Annotations::from_records(&parsed, &model);
        assert_eq!((restored.items.len(), skipped), (1, 3));
    }

    #[test]
    fn undo_and_redo_annotation_changes() {
        let mut app = App::new();
        app.init_resource::<Annotations>()
            .init_resource::<History>()
            .init_resource::<Model>()
            .init_resource::<Selection>();
        let world = app.world_mut();
        let before = world.resource::<Annotations>().items.clone();
        let mut annotations = world.resource_mut::<Annotations>();
        annotations.add(AnnotationKind::EdgeLength {
            edge: EdgeRef { node: 1, edge: 0 },
        });
        let after = annotations.items.clone();
        record_change(
            &mut world.resource_mut::<History>(),
            "辺の長さを追加",
            before,
            &after,
        );
        crate::document::history::run(world, plantwright_core::history::Request::Undo);
        assert!(world.resource::<Annotations>().items.is_empty());
        crate::document::history::run(world, plantwright_core::history::Request::Redo);
        assert_eq!(world.resource::<Annotations>().items, after);
    }

    #[test]
    fn note_value_text_is_first_line() {
        let (model, ids, cache) = box_model(&[([1.0; 3], [0.0; 3], [0.0; 3])]);
        let kind = AnnotationKind::Note {
            at: Anchor {
                node: ids[0],
                local: [0.0, 1.0, 0.0],
            },
            text: "チャック位置\n要確認".to_string(),
        };
        assert_eq!(value_text(&kind, &model, &cache), "チャック位置");
        let long = AnnotationKind::Note {
            at: Anchor {
                node: ids[0],
                local: [0.0, 1.0, 0.0],
            },
            text: "ワーク搬入側のストッパー位置を要確認".to_string(),
        };
        assert_eq!(
            value_text(&long, &model, &cache),
            "ワーク搬入側のストッパー…"
        );
    }
}
