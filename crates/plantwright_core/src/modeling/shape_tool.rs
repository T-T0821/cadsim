//! 3D ビューで辺・面を指して、丸める (フィレット)・面取り・肉抜き (シェル) をする道具の状態と計算
//!
//! 流れは穴あけ ([`super::hole_tool`]) と同じ「部品の辺か面をクリック → 矢印の先の点をドラッグで
//! 半径・幅・肉厚 → 半透明のプレビュー → Enter で確定、Esc で取り消し」。
//!
//! - 丸める・面取り: 辺をクリックで選ぶ。Shift + クリックで足す、選んだ辺をもう一度押すと外す。
//!   面をクリックすると、その面の周りの辺をまとめて選ぶ。
//! - 肉抜き: 開ける面をクリックで選ぶ (選ばなければ閉じた中空)。
//!
//! ここには状態 ([`ShapeTool`])・部品の辺と面の形 ([`PartGeometry`]) と、カーソルから辺を選ぶ・
//! ドラッグ位置から値を決める計算だけを置く。画面への描き方とマウスの読み方はアプリ側
//! (`features/modeling`)。辺と面の番号は部品の表示メッシュの番号 ([`super::ops`] と同じ)。

use glam::{DMat4, DVec2, DVec3};

use super::ops::{chamfer_feature, fillet_feature, shell_feature};
use super::{SolidStore, node_solid};
use crate::history::ModelChange;
use crate::kernel::Brep;
use crate::model::{Model, NodeId, NodeKind};
use crate::shape::tessellate_breps;

/// 値の下限 (モデル単位)
const MIN_VALUE: f64 = 0.001;
/// 箱を表示用にテッセレーションするときの許容誤差 (表示と同じ)
const BOX_TESSELLATION_TOL: f64 = 0.005;

/// 道具の種類
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ShapeOp {
    /// 角を丸める (フィレット)
    #[default]
    Fillet,
    /// 面取り
    Chamfer,
    /// 肉抜き (シェル)
    Shell,
}

impl ShapeOp {
    pub const ALL: [ShapeOp; 3] = [ShapeOp::Fillet, ShapeOp::Chamfer, ShapeOp::Shell];

    /// ボタンの文字
    pub fn label(self) -> &'static str {
        match self {
            ShapeOp::Fillet => "角を丸める",
            ShapeOp::Chamfer => "面取り",
            ShapeOp::Shell => "肉抜き",
        }
    }

    /// 確定したときのお知らせ
    pub fn done_message(self) -> &'static str {
        match self {
            ShapeOp::Fillet => "角を丸めました",
            ShapeOp::Chamfer => "面取りしました",
            ShapeOp::Shell => "肉抜きしました",
        }
    }

    /// ドラッグで決める値の名前
    pub fn value_label(self) -> &'static str {
        match self {
            ShapeOp::Fillet => "半径",
            ShapeOp::Chamfer => "幅",
            ShapeOp::Shell => "肉厚",
        }
    }

    /// 面を選ぶ道具か (でなければ辺を選ぶ)
    pub fn picks_faces(self) -> bool {
        self == ShapeOp::Shell
    }

    /// B-rep にこの操作をかけた形 (プレビュー用。部品の座標系のまま)
    pub fn build(self, solid: &Brep, picks: &[usize], value: f64) -> Result<Brep, String> {
        // 確定 (編集履歴に残す) と同じ編集を作り、同じ道で形にする
        let feature = match self {
            ShapeOp::Fillet => fillet_feature(solid, picks, value),
            ShapeOp::Chamfer => chamfer_feature(solid, picks, value),
            ShapeOp::Shell => shell_feature(solid, value, picks),
        }?;
        feature.apply(solid)
    }

    /// 部品にこの操作をかける。元に戻すための変更を返す
    pub fn apply(
        self,
        model: &mut Model,
        store: &dyn SolidStore,
        id: NodeId,
        picks: &[usize],
        value: f64,
    ) -> Result<ModelChange, String> {
        match self {
            ShapeOp::Fillet => super::ops::fillet_edges(model, store, id, picks, value),
            ShapeOp::Chamfer => super::ops::chamfer_edges(model, store, id, picks, value),
            ShapeOp::Shell => super::ops::shell(model, store, id, value, picks),
        }
    }
}

/// 部品の辺と面の形 (ワールド座標)。道具を使っている間、カーソルで辺を選ぶのに使う
#[derive(bevy_ecs::prelude::Resource, Clone, Debug, Default)]
pub struct PartGeometry {
    pub node: NodeId,
    /// 辺のポリライン (表示メッシュの辺の番号の順)
    pub edges: Vec<Vec<DVec3>>,
    /// 面ごとの三角形 (表示メッシュの面の番号の順)
    pub faces: Vec<Vec<[DVec3; 3]>>,
    pub min: DVec3,
    pub max: DVec3,
}

impl PartGeometry {
    /// 部品の表示メッシュの辺と面をワールド座標で取り出す。箱は表示と同じようにテッセレーションする
    pub fn of(model: &Model, id: NodeId) -> Option<PartGeometry> {
        let node = model.get(id)?;
        let (faces, edges): (Vec<Vec<[DVec3; 3]>>, Vec<Vec<DVec3>>) = match &node.kind {
            NodeKind::Box(part) => {
                let (faces, edges) =
                    tessellate_breps(&[part.build_solid().ok()?], BOX_TESSELLATION_TOL);
                (
                    faces
                        .iter()
                        .map(|f| f.triangle_points().collect::<Vec<_>>())
                        .collect(),
                    edges,
                )
            }
            NodeKind::Imported { shape, .. } => (
                shape
                    .faces
                    .iter()
                    .map(|f| f.triangle_points().collect::<Vec<_>>())
                    .collect(),
                shape.edges.clone(),
            ),
            _ => return None,
        };
        Some(PartGeometry::from_local(
            id,
            model.world_matrix(id),
            &faces,
            &edges,
        ))
    }

    /// 部品の座標系の面と辺を `world` でワールド座標にする
    pub fn from_local(
        node: NodeId,
        world: DMat4,
        faces: &[Vec<[DVec3; 3]>],
        edges: &[Vec<DVec3>],
    ) -> PartGeometry {
        let edges: Vec<Vec<DVec3>> = edges
            .iter()
            .map(|e| e.iter().map(|&p| world.transform_point3(p)).collect())
            .collect();
        let faces: Vec<Vec<[DVec3; 3]>> = faces
            .iter()
            .map(|f| {
                f.iter()
                    .map(|t| t.map(|p| world.transform_point3(p)))
                    .collect()
            })
            .collect();
        let mut min = DVec3::splat(f64::MAX);
        let mut max = DVec3::splat(f64::MIN);
        for p in faces
            .iter()
            .flatten()
            .flatten()
            .chain(edges.iter().flatten())
        {
            min = min.min(*p);
            max = max.max(*p);
        }
        if min.x > max.x {
            (min, max) = (DVec3::ZERO, DVec3::ZERO);
        }
        PartGeometry {
            node,
            edges,
            faces,
            min,
            max,
        }
    }

    pub fn center(&self) -> DVec3 {
        (self.min + self.max) / 2.0
    }

    /// 部品の大きさ (外接箱の対角線)
    pub fn size(&self) -> f64 {
        (self.max - self.min).length().max(MIN_VALUE)
    }

    /// 外接箱のいちばん短い辺
    fn thinnest(&self) -> f64 {
        (self.max - self.min).min_element().max(MIN_VALUE)
    }

    /// カーソルにいちばん近い辺。`ray` はカーソルから飛ばしたレイ (始点, 向き)、`project` はワールドの点を
    /// 画面の点にする。画面上で `max_px` より遠い辺と、`surface` (カーソルの下の部品の面の点) より奥に
    /// 隠れた辺は選ばない。ほぼ同じ距離なら手前の辺を選ぶ
    pub fn pick_edge(
        &self,
        ray: (DVec3, DVec3),
        project: impl Fn(DVec3) -> Option<DVec2>,
        cursor: DVec2,
        max_px: f64,
        surface: Option<DVec3>,
    ) -> Option<usize> {
        let (origin, dir) = (ray.0, ray.1.normalize_or_zero());
        let surface_depth = surface.map(|p| (p - origin).dot(dir));
        let slack = self.size() * 0.01;
        // (画面上の距離, 奥行き, 辺)
        let mut best: Option<(f64, f64, usize)> = None;
        for (i, edge) in self.edges.iter().enumerate() {
            for w in edge.windows(2) {
                // レイにいちばん近い線分の上の点を、画面に写して距離を測る
                let q = closest_on_segment(origin, dir, w[0], w[1]);
                let Some(screen) = project(q) else {
                    continue;
                };
                let distance = cursor.distance(screen);
                if distance > max_px {
                    continue;
                }
                let depth = (q - origin).dot(dir);
                if surface_depth.is_some_and(|s| depth > s + slack) {
                    continue; // 面の裏に隠れている
                }
                let better = best.is_none_or(|(d, z, _)| {
                    if (distance - d).abs() < 0.5 {
                        depth < z
                    } else {
                        distance < d
                    }
                });
                if better {
                    best = Some((distance, depth, i));
                }
            }
        }
        best.map(|(_, _, i)| i)
    }

    /// 面 `face` の周りの辺 (辺の点がすべてその面の上にあるもの)
    pub fn face_edges(&self, face: usize) -> Vec<usize> {
        let Some(triangles) = self.faces.get(face) else {
            return Vec::new();
        };
        let tolerance = self.size() * 0.002;
        // 面の外接箱の外に点がある辺は、三角形を調べるまでもなく外す (大きな部品で重くならないよう)
        let (lo, hi) = triangles.iter().flatten().fold(
            (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)),
            |(lo, hi), p| (lo.min(*p), hi.max(*p)),
        );
        let (lo, hi) = (lo - tolerance, hi + tolerance);
        self.edges
            .iter()
            .enumerate()
            .filter(|(_, edge)| {
                !edge.is_empty()
                    && edge.iter().all(|p| p.cmpge(lo).all() && p.cmple(hi).all())
                    && edge.iter().all(|&p| {
                        triangles
                            .iter()
                            .any(|t| point_triangle_distance(p, t) <= tolerance)
                    })
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// 面の中心 (三角形の重心を面積で重み付けした平均) と外向きの法線
    pub fn face_center_normal(&self, face: usize) -> Option<(DVec3, DVec3)> {
        let triangles = self.faces.get(face)?;
        let mut area_sum = 0.0;
        let mut center = DVec3::ZERO;
        let mut normal = DVec3::ZERO;
        for [a, b, c] in triangles {
            let cross = (*b - *a).cross(*c - *a);
            let area = cross.length() / 2.0;
            area_sum += area;
            center += (*a + *b + *c) / 3.0 * area;
            normal += cross;
        }
        (area_sum > 0.0).then(|| (center / area_sum, normal.normalize_or_zero()))
    }

    /// 辺の長さの真ん中の点と、そこでの辺の向き
    pub fn edge_midpoint(&self, edge: usize) -> Option<(DVec3, DVec3)> {
        let points = self.edges.get(edge)?;
        let length: f64 = points.windows(2).map(|w| w[0].distance(w[1])).sum();
        let mut walked = 0.0;
        for w in points.windows(2) {
            let step = w[0].distance(w[1]);
            if walked + step >= length / 2.0 && step > 0.0 {
                let t = (length / 2.0 - walked) / step;
                return Some((w[0].lerp(w[1], t), (w[1] - w[0]) / step));
            }
            walked += step;
        }
        points.first().map(|&p| (p, DVec3::X))
    }
}

/// レイ (始点 `origin`、単位ベクトル `dir`) にいちばん近い、線分 `a`-`b` の上の点
fn closest_on_segment(origin: DVec3, dir: DVec3, a: DVec3, b: DVec3) -> DVec3 {
    let d = b - a;
    let w = a - origin;
    let dd = d.dot(d);
    if dd < 1e-24 {
        return a;
    }
    let bd = dir.dot(d);
    let denom = dd - bd * bd; // |dir| = 1
    let t = if denom.abs() < 1e-12 * dd {
        // 平行: 始点に近い方の端
        0.0
    } else {
        (bd * w.dot(dir) - w.dot(d)) / denom
    };
    a + d * t.clamp(0.0, 1.0)
}

/// 点と三角形の距離
fn point_triangle_distance(p: DVec3, [a, b, c]: &[DVec3; 3]) -> f64 {
    // Ericson, Real-Time Collision Detection 5.1.5 の最近点
    let (ab, ac, ap) = (*b - *a, *c - *a, p - *a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return p.distance(*a);
    }
    let bp = p - *b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return p.distance(*b);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return p.distance(*a + ab * (d1 / (d1 - d3)));
    }
    let cp = p - *c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return p.distance(*c);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return p.distance(*a + ac * (d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return p.distance(*b + (*c - *b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    p.distance(*a + ab * (vb * denom) + ac * (vc * denom))
}

/// カーソルの下にあるもの
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hover {
    Edge(usize),
    Face(usize),
}

/// 道具の状態
#[derive(bevy_ecs::prelude::Resource, Clone, PartialEq, Debug, Default)]
pub struct ShapeTool {
    pub active: bool,
    pub op: ShapeOp,
    /// 対象の部品
    pub node: Option<NodeId>,
    /// 選んだ辺か面 (表示メッシュの番号)
    pub picks: Vec<usize>,
    /// 半径・幅・肉厚
    pub value: f64,
    /// カーソルの下の辺か面 (クリックすると選ぶもの)
    pub hovered: Option<Hover>,
    /// 矢印の先の点にカーソルが乗っている
    pub handle_hovered: bool,
    /// 矢印の先の点をドラッグ中
    pub dragging: bool,
    /// 前回の値 (丸める・面取り・肉抜きの順。次の初期値)
    pub last_values: [f64; 3],
    /// プレビューか確定の失敗 (理由を一行で)
    pub error: Option<String>,
}

impl ShapeTool {
    /// 部品 `node` に道具 `op` を使い始める。値は前回の値か、部品の大きさから決める
    pub fn start(&mut self, op: ShapeOp, node: NodeId, geometry: &PartGeometry) {
        self.active = true;
        self.op = op;
        self.node = Some(node);
        self.picks.clear();
        self.hovered = None;
        self.handle_hovered = false;
        self.dragging = false;
        self.error = None;
        self.value = self.default_value(geometry);
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.node = None;
        self.picks.clear();
        self.hovered = None;
        self.handle_hovered = false;
        self.dragging = false;
        self.error = None;
    }

    /// 確定した後: 値を次の初期値にして道具をやめる
    pub fn finish(&mut self) {
        self.last_values[self.op_index()] = self.value;
        self.stop();
    }

    /// Esc: 選んだものがあれば外し、なければ道具をやめる
    pub fn cancel(&mut self) {
        if self.picks.is_empty() {
            self.stop();
        } else {
            self.picks.clear();
            self.dragging = false;
            self.error = None;
        }
    }

    fn op_index(&self) -> usize {
        ShapeOp::ALL.iter().position(|&o| o == self.op).unwrap_or(0)
    }

    /// 値の初期値: 前回の値。初回は部品のいちばん薄い寸法の 1 割
    pub fn default_value(&self, geometry: &PartGeometry) -> f64 {
        let last = self.last_values[self.op_index()];
        if last > 0.0 {
            last
        } else {
            nice_round(geometry.thinnest() * 0.1).max(MIN_VALUE)
        }
    }

    /// 確定できる (肉抜きは面を選ばなくてもよい)
    pub fn ready(&self) -> bool {
        self.active && self.node.is_some() && (self.op.picks_faces() || !self.picks.is_empty())
    }

    /// カーソルの下のものをクリックした。Shift なら足す・外す、そうでなければ選び直す。
    /// 選んであるものをもう一度押すと外す。辺の道具で面を押すと、その面の周りの辺をまとめて選ぶ
    pub fn click(&mut self, hover: Hover, shift: bool, geometry: &PartGeometry) {
        let clicked: Vec<usize> = match (hover, self.op.picks_faces()) {
            (Hover::Edge(e), false) => vec![e],
            (Hover::Face(f), false) => geometry.face_edges(f),
            (Hover::Face(f), true) => vec![f],
            (Hover::Edge(_), true) => return,
        };
        if clicked.is_empty() {
            return;
        }
        let all_picked = clicked.iter().all(|c| self.picks.contains(c));
        if all_picked {
            self.picks.retain(|p| !clicked.contains(p));
        } else if shift {
            for c in clicked {
                if !self.picks.contains(&c) {
                    self.picks.push(c);
                }
            }
        } else {
            self.picks = clicked;
        }
        self.error = None;
    }

    /// 矢印の根元と向き (単位ベクトル)。矢印の先 = 根元 + 向き × 値
    ///
    /// - 辺の道具: 最初に選んだ辺の真ん中から、部品の中心の方へ (辺に垂直)
    /// - 肉抜き: 最初に選んだ面の中心から内側へ。面を選んでいなければ部品の上面の中心から下へ
    pub fn arrow(&self, geometry: &PartGeometry) -> Option<(DVec3, DVec3)> {
        let first = self.picks.first().copied();
        if self.op.picks_faces() {
            if let Some((center, normal)) = first.and_then(|f| geometry.face_center_normal(f))
                && normal != DVec3::ZERO
            {
                return Some((center, -normal));
            }
            let c = geometry.center();
            return Some((DVec3::new(c.x, geometry.max.y, c.z), -DVec3::Y));
        }
        let (mid, tangent) = geometry.edge_midpoint(first?)?;
        let to_center = geometry.center() - mid;
        let inward = (to_center - tangent * to_center.dot(tangent))
            .try_normalize()
            .unwrap_or(-DVec3::Y);
        Some((mid, inward))
    }

    /// 矢印の先の点 (ドラッグするつまみ)
    pub fn handle_position(&self, geometry: &PartGeometry) -> Option<DVec3> {
        self.arrow(geometry)
            .map(|(base, dir)| base + dir * self.value)
    }

    /// 矢印の上の点 `p` につまみを合わせる。`step` があればその倍数に丸める
    pub fn set_value_from_point(&mut self, geometry: &PartGeometry, p: DVec3, step: Option<f64>) {
        let Some((base, dir)) = self.arrow(geometry) else {
            return;
        };
        let value = (p - base).dot(dir);
        let value = match step {
            Some(s) if s > 0.0 => (value / s).round() * s,
            _ => value,
        };
        self.value = value.max(MIN_VALUE);
    }

    /// 選んだものの数と値 (画面に出す文字)
    pub fn summary(&self) -> String {
        let what = if self.op.picks_faces() {
            if self.picks.is_empty() {
                "面なし (閉じた中空)".to_owned()
            } else {
                format!("開ける面 {} 枚", self.picks.len())
            }
        } else {
            format!("辺 {} 本", self.picks.len())
        };
        format!("{what} / {} {}", self.op.value_label(), round3(self.value))
    }

    /// 画面の下に出す案内
    pub fn hint(&self) -> String {
        if self.dragging {
            return format!(
                "ドラッグで{}を決めます (Ctrl で刻みにそろえる)",
                self.op.value_label()
            );
        }
        match (self.op, self.picks.is_empty()) {
            (ShapeOp::Shell, true) => {
                "開ける面をクリック。選ばずに Enter で閉じた中空・Esc でやめる".to_owned()
            }
            (ShapeOp::Shell, false) => {
                "矢印の先をドラッグで肉厚。Shift で面を追加・Enter で確定・Esc で取り消し"
                    .to_owned()
            }
            (ShapeOp::Fillet, true) => {
                "丸める辺をクリック (面なら周りの辺をまとめて)・Esc でやめる".to_owned()
            }
            (_, true) => {
                "面取りする辺をクリック (面なら周りの辺をまとめて)・Esc でやめる".to_owned()
            }
            (op, false) => format!(
                "矢印の先をドラッグで{}。Shift で辺を追加・Enter で確定・Esc で取り消し",
                op.value_label()
            ),
        }
    }
}

/// 部品にかけた結果の形 (部品の座標系)。プレビュー用で、モデルは変えない
pub fn preview_solid(
    model: &Model,
    store: &dyn SolidStore,
    tool: &ShapeTool,
) -> Result<Brep, String> {
    let node = tool.node.ok_or("部品が選ばれていません")?;
    let solid = node_solid(model, store, node)?;
    tool.op.build(&solid, &tool.picks, tool.value)
}

/// 部品に道具の操作をかける。元に戻すための変更を返す
pub fn apply(
    model: &mut Model,
    store: &dyn SolidStore,
    tool: &ShapeTool,
) -> Result<ModelChange, String> {
    let node = tool.node.ok_or("部品が選ばれていません")?;
    if !tool.ready() {
        return Err("辺を選んでください".into());
    }
    tool.op.apply(model, store, node, &tool.picks, tool.value)
}

/// 1・2・5 の倍数にそろえる (初期値をきりのよい数にする)
fn nice_round(v: f64) -> f64 {
    if v <= 0.0 {
        return v;
    }
    let p = 10f64.powf(v.log10().floor());
    let m = v / p;
    let n = if m < 1.5 {
        1.0
    } else if m < 3.5 {
        2.0
    } else if m < 7.5 {
        5.0
    } else {
        10.0
    };
    n * p
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::super::tests::MemoryStore;
    use super::*;

    /// 中心が (0, 5, 0) の 10 角の箱
    fn cube() -> (Model, NodeId) {
        let mut model = Model::default();
        let id = model.add_box([10.0, 10.0, 10.0], [0.0, 5.0, 0.0]);
        (model, id)
    }

    /// 正面 (-Z 側から +Z を見る) の平行投影: 画面は (x, -y)
    fn front(p: DVec3) -> Option<DVec2> {
        Some(DVec2::new(p.x, -p.y))
    }

    /// 画面の点 (x, -y) を通る、正面からのレイ
    fn front_ray(screen: DVec2) -> (DVec3, DVec3) {
        (DVec3::new(screen.x, -screen.y, -100.0), DVec3::Z)
    }

    fn edge_between(g: &PartGeometry, f: impl Fn(DVec3) -> bool) -> usize {
        g.edges
            .iter()
            .position(|e| e.iter().all(|&p| f(p)))
            .unwrap()
    }

    #[test]
    fn geometry_is_in_world_coordinates() {
        let (model, id) = cube();
        let g = PartGeometry::of(&model, id).unwrap();
        assert_eq!(g.edges.len(), 12);
        assert_eq!(g.faces.len(), 6);
        assert!((g.min - DVec3::new(-5.0, 0.0, -5.0)).length() < 1e-9);
        assert!((g.max - DVec3::new(5.0, 10.0, 5.0)).length() < 1e-9);
    }

    #[test]
    fn pick_edge_near_cursor_and_skip_hidden() {
        let (model, id) = cube();
        let g = PartGeometry::of(&model, id).unwrap();
        // 手前の上の辺 (y = 10, z = -5) のすぐ近く
        let near_top = DVec2::new(1.0, -9.8);
        let e = g
            .pick_edge(front_ray(near_top), front, near_top, 0.5, None)
            .unwrap();
        assert!(g.edges[e].iter().all(|p| (p.y - 10.0).abs() < 1e-9));
        assert!(
            g.edges[e].iter().all(|p| (p.z + 5.0).abs() < 1e-9),
            "手前の辺"
        );
        // 遠すぎれば選ばない
        let far = DVec2::new(1.0, -7.0);
        assert_eq!(g.pick_edge(front_ray(far), front, far, 0.5, None), None);
        // 手前の面に当たっているとき、奥の縦の辺は隠れているので選ばない
        let side = DVec2::new(5.0, -5.0);
        let hidden = g.pick_edge(
            front_ray(side),
            front,
            side,
            0.5,
            Some(DVec3::new(5.0, 5.0, -5.0)),
        );
        let e = hidden.unwrap();
        assert!(
            g.edges[e].iter().all(|p| (p.z + 5.0).abs() < 1e-9),
            "手前の縦の辺だけ"
        );
    }

    #[test]
    fn clicking_edges_and_faces() {
        let (model, id) = cube();
        let g = PartGeometry::of(&model, id).unwrap();
        let mut tool = ShapeTool::default();
        tool.start(ShapeOp::Fillet, id, &g);
        assert!(!tool.ready());
        tool.click(Hover::Edge(3), false, &g);
        assert_eq!(tool.picks, vec![3]);
        tool.click(Hover::Edge(5), false, &g);
        assert_eq!(tool.picks, vec![5], "Shift なしは選び直す");
        tool.click(Hover::Edge(3), true, &g);
        assert_eq!(tool.picks, vec![5, 3], "Shift で足す");
        tool.click(Hover::Edge(5), true, &g);
        assert_eq!(tool.picks, vec![3], "選んである辺は外す");

        // 上面をクリックすると上面の周りの 4 本
        let top = (0..g.faces.len())
            .find(|&f| g.face_center_normal(f).unwrap().1.y > 0.9)
            .unwrap();
        tool.click(Hover::Face(top), false, &g);
        assert_eq!(tool.picks.len(), 4);
        for &e in &tool.picks {
            assert!(g.edges[e].iter().all(|p| (p.y - 10.0).abs() < 1e-9));
        }
        assert!(tool.ready());

        // Esc: 選択を外す → もう一度で道具をやめる
        tool.cancel();
        assert!(tool.active && tool.picks.is_empty());
        tool.cancel();
        assert!(!tool.active);
    }

    #[test]
    fn shell_picks_faces_and_is_ready_without_any() {
        let (model, id) = cube();
        let g = PartGeometry::of(&model, id).unwrap();
        let mut tool = ShapeTool::default();
        tool.start(ShapeOp::Shell, id, &g);
        assert!(tool.ready(), "面なしでも閉じた中空にできる");
        tool.click(Hover::Edge(0), false, &g);
        assert!(tool.picks.is_empty(), "肉抜きは辺を選ばない");
        tool.click(Hover::Face(2), false, &g);
        assert_eq!(tool.picks, vec![2]);
        // 矢印は開ける面の中心から内へ
        let (base, dir) = tool.arrow(&g).unwrap();
        let (center, normal) = g.face_center_normal(2).unwrap();
        assert!((base - center).length() < 1e-9);
        assert!((dir + normal).length() < 1e-9);
    }

    #[test]
    fn arrow_points_inward_from_edge_and_drag_sets_value() {
        let (model, id) = cube();
        let g = PartGeometry::of(&model, id).unwrap();
        let mut tool = ShapeTool::default();
        tool.start(ShapeOp::Fillet, id, &g);
        assert_eq!(tool.value, 1.0, "10 の 1 割");
        let e = edge_between(&g, |p| {
            (p.x - 5.0).abs() < 1e-9 && (p.y - 10.0).abs() < 1e-9
        });
        tool.click(Hover::Edge(e), false, &g);
        let (base, dir) = tool.arrow(&g).unwrap();
        assert!((base - DVec3::new(5.0, 10.0, 0.0)).length() < 1e-9);
        let expected = DVec3::new(-1.0, -1.0, 0.0).normalize();
        assert!((dir - expected).length() < 1e-9, "{dir}");

        tool.set_value_from_point(&g, base + dir * 2.4 + DVec3::Z, None);
        assert!((tool.value - 2.4).abs() < 1e-9, "矢印に沿った分だけ");
        tool.set_value_from_point(&g, base + dir * 2.4, Some(1.0));
        assert_eq!(tool.value, 2.0);
        tool.set_value_from_point(&g, base - dir, None);
        assert_eq!(tool.value, MIN_VALUE, "0 以下にはならない");
        assert!((tool.handle_position(&g).unwrap() - (base + dir * MIN_VALUE)).length() < 1e-9);
    }

    #[test]
    fn preview_and_apply_round_an_edge() {
        let (mut model, id) = cube();
        let store = MemoryStore::default();
        let g = PartGeometry::of(&model, id).unwrap();
        let mut tool = ShapeTool::default();
        tool.start(ShapeOp::Fillet, id, &g);
        let e = edge_between(&g, |p| {
            (p.x - 5.0).abs() < 1e-9 && (p.y - 10.0).abs() < 1e-9
        });
        tool.click(Hover::Edge(e), false, &g);
        tool.value = 2.0;
        let preview = preview_solid(&model, &store, &tool).unwrap();
        let volume = preview.mass_properties(1.0).unwrap().volume;
        let expected = 1000.0 - (4.0 - std::f64::consts::PI) * 10.0;
        assert!((volume - expected).abs() < 0.1, "{volume}");
        assert!(
            matches!(model.get(id).unwrap().kind, NodeKind::Box(_)),
            "プレビューはモデルを変えない"
        );

        // 大きすぎる半径はプレビューで理由が分かる
        tool.value = 50.0;
        let err = preview_solid(&model, &store, &tool).unwrap_err();
        assert!(err.contains("半径"), "{err}");

        tool.value = 2.0;
        let change = apply(&mut model, &store, &tool).unwrap();
        assert!(change.label.contains("丸める"));
        tool.finish();
        assert!(!tool.active);
        assert_eq!(tool.last_values[0], 2.0, "次の初期値になる");
    }

    #[test]
    fn hints_and_summary() {
        let (model, id) = cube();
        let g = PartGeometry::of(&model, id).unwrap();
        let mut tool = ShapeTool::default();
        tool.start(ShapeOp::Chamfer, id, &g);
        assert!(tool.hint().contains("面取りする辺をクリック"));
        tool.click(Hover::Edge(0), false, &g);
        assert!(tool.hint().contains("Enter"));
        assert_eq!(tool.summary(), "辺 1 本 / 幅 1");
        tool.start(ShapeOp::Shell, id, &g);
        assert_eq!(tool.summary(), "面なし (閉じた中空) / 肉厚 1");
    }

    #[test]
    fn nice_round_values() {
        assert_eq!(nice_round(0.8), 1.0);
        assert_eq!(nice_round(2.7), 2.0);
        assert_eq!(nice_round(40.0), 50.0);
        assert_eq!(nice_round(130.0), 100.0);
    }

    #[test]
    fn closest_point_on_segment_to_ray() {
        let q = closest_on_segment(
            DVec3::new(0.3, 5.0, 0.0),
            -DVec3::Y,
            DVec3::new(-1.0, 0.0, 0.0),
            DVec3::new(1.0, 0.0, 0.0),
        );
        assert!((q - DVec3::new(0.3, 0.0, 0.0)).length() < 1e-12);
        // 線分の外なら端
        let q = closest_on_segment(DVec3::new(3.0, 5.0, 0.0), -DVec3::Y, DVec3::ZERO, DVec3::X);
        assert_eq!(q, DVec3::X);
    }

    #[test]
    fn point_triangle_distance_cases() {
        let t = [DVec3::ZERO, DVec3::X, DVec3::Y];
        assert!((point_triangle_distance(DVec3::new(0.2, 0.2, 3.0), &t) - 3.0).abs() < 1e-12);
        assert!((point_triangle_distance(DVec3::new(-1.0, 0.0, 0.0), &t) - 1.0).abs() < 1e-12);
        assert!(
            (point_triangle_distance(DVec3::new(1.0, 1.0, 0.0), &t) - 0.5f64.sqrt()).abs() < 1e-12
        );
    }
}
