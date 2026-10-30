//! 設計データ (f64) と選択状態
//!
//! モデルはノードの木で表す。ノードは「アセンブリ」「箱 (パラメトリック部品)」
//! 「取り込んだ部品」のどれか。各ノードは親から見た配置 (f64 の行列) を持つ。
//! Bevy のエンティティとは独立していて、表示は viewport.rs が後から追いかける。

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use bevy::math::{DMat4, DVec3};
use bevy::prelude::*;
use truck_modeling::{Solid, builder};
// truck の prelude の型名 (Point3 など) を使うが、Bevy と名前がぶつかるものは明示する
use truck_meshalgo::prelude::{EuclideanSpace, Point3, Rad, Vector3};

use crate::math::{Aabb, euler_deg_to_quat};
use crate::step_import::{ImportNode, ImportedShape};

pub type NodeId = u32;

/// 色の指定がない部品の色
pub const DEFAULT_PART_COLOR: [f32; 3] = [0.70, 0.74, 0.80];

pub struct Node {
    pub name: String,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    /// 親の座標系から見た配置
    pub placement: DMat4,
    /// 目のアイコンで切り替える表示/非表示 (親が非表示なら子も見えない)
    pub visible: bool,
    /// ユーザーが付けた色。子孫の部品にも効く。
    pub color: Option<[f32; 3]>,
    pub kind: NodeKind,
}

pub enum NodeKind {
    /// 子をまとめるだけのノード。`product` は STEP の製品名。
    Assembly { product: Option<String> },
    /// 寸法で作る箱
    Box(BoxPart),
    /// STEP から取り込んだ部品。形状は同じ製品のインスタンスで共有する。
    Imported {
        product: String,
        shape: Arc<ImportedShape>,
    },
}

impl NodeKind {
    pub fn label(&self) -> &'static str {
        match self {
            NodeKind::Assembly { .. } => "アセンブリ",
            NodeKind::Box(_) => "部品 (箱)",
            NodeKind::Imported { .. } => "部品",
        }
    }

    pub fn is_part(&self) -> bool {
        !matches!(self, NodeKind::Assembly { .. })
    }

    pub fn product(&self) -> Option<&str> {
        match self {
            NodeKind::Assembly { product } => product.as_deref(),
            NodeKind::Imported { product, .. } => Some(product),
            NodeKind::Box(_) => None,
        }
    }
}

/// 箱1つ分の設計パラメータ。値はすべて f64 で持つ。
#[derive(Clone)]
pub struct BoxPart {
    /// 幅 (X), 高さ (Y), 奥行 (Z)
    pub size: [f64; 3],
    pub position: [f64; 3],
    /// X → Y → Z の順に回す角度 (度)
    pub rotation_deg: [f64; 3],
}

impl BoxPart {
    /// パラメータから truck の B-rep ソリッドを作る。
    pub fn build_solid(&self) -> Solid {
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

    /// 回転後の外接箱 (箱の8つの角から厳密に求める)
    fn bounds(&self) -> Aabb {
        let half = DVec3::from(self.size) / 2.0;
        let m = DMat4::from_rotation_translation(
            euler_deg_to_quat(self.rotation_deg),
            DVec3::from(self.position),
        );
        Aabb {
            min: -half,
            max: half,
        }
        .transformed(m)
    }
}

/// 部品やアセンブリの計測値の合計
#[derive(Clone, Copy, Default)]
pub struct Measurements {
    pub part_count: usize,
    pub face_count: usize,
    pub edge_count: usize,
    pub volume: f64,
    pub area: f64,
}

#[derive(Resource)]
pub struct Model {
    nodes: BTreeMap<NodeId, Node>,
    /// 最上位のノード (ツリーの表示順)
    pub roots: Vec<NodeId>,
    next_id: NodeId,
    /// 表示を作り直す必要がある部品 (形状が変わった・削除された)
    pub dirty: HashSet<NodeId>,
    /// 長さの単位 (最後に取り込んだ STEP の単位)
    pub unit: String,
    /// 新規作成フォームの寸法
    pub new_box_size: [f64; 3],
}

impl Default for Model {
    fn default() -> Self {
        Self {
            nodes: BTreeMap::new(),
            roots: Vec::new(),
            next_id: 1,
            dirty: HashSet::new(),
            unit: "mm".into(),
            new_box_size: [1.0, 1.0, 1.0],
        }
    }
}

impl Model {
    // --- 追加と削除 ---------------------------------------------------------

    fn add_node(
        &mut self,
        parent: Option<NodeId>,
        name: String,
        placement: DMat4,
        kind: NodeKind,
    ) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        let is_part = kind.is_part();
        self.nodes.insert(
            id,
            Node {
                name,
                parent,
                children: Vec::new(),
                placement,
                visible: true,
                color: None,
                kind,
            },
        );
        match parent.and_then(|p| self.nodes.get_mut(&p)) {
            Some(parent) => parent.children.push(id),
            None => self.roots.push(id),
        }
        if is_part {
            self.dirty.insert(id);
        }
        id
    }

    pub fn add_box(&mut self, size: [f64; 3], position: [f64; 3]) -> NodeId {
        let name = format!("Box {}", self.next_id);
        let part = BoxPart {
            size,
            position,
            rotation_deg: [0.0; 3],
        };
        self.add_node(None, name, DMat4::IDENTITY, NodeKind::Box(part))
    }

    /// 取り込んだ STEP の木をモデルに加え、そのルートの id を返す
    pub fn add_imported(&mut self, root: ImportNode, unit: String) -> NodeId {
        self.unit = unit;
        let id = self.add_import_node(None, root);
        // 新しく作る箱の寸法を、取り込んだものの大きさに見合うきりのいい値にする
        if let Some(bounds) = self.world_bounds(id) {
            let step = 10f64.powf((bounds.size().max_element() / 4.0).log10().floor());
            self.new_box_size = [step; 3];
        }
        id
    }

    /// 既存のものと重ならないよう、全体の右 (+X 側) の床の上に置く位置
    pub fn free_spot(&self, size: [f64; 3]) -> [f64; 3] {
        match self.scene_bounds() {
            Some(b) => [
                b.max.x + size[0] * 1.5,
                b.min.y.min(0.0) + size[1] / 2.0,
                b.center().z,
            ],
            None => [0.0, size[1] / 2.0, 0.0],
        }
    }

    fn add_import_node(&mut self, parent: Option<NodeId>, node: ImportNode) -> NodeId {
        let kind = match node.shape {
            Some(shape) => NodeKind::Imported {
                product: node.product,
                shape,
            },
            None => NodeKind::Assembly {
                product: Some(node.product),
            },
        };
        let id = self.add_node(parent, node.name, node.placement, kind);
        for child in node.children {
            self.add_import_node(Some(id), child);
        }
        id
    }

    /// ノードとその子孫を消す
    pub fn remove(&mut self, id: NodeId) {
        let Some(parent) = self.nodes.get(&id).map(|n| n.parent) else {
            return;
        };
        match parent.and_then(|p| self.nodes.get_mut(&p)) {
            Some(parent) => parent.children.retain(|&c| c != id),
            None => self.roots.retain(|&r| r != id),
        }
        for removed in self.subtree(id) {
            self.nodes.remove(&removed);
            self.dirty.insert(removed);
        }
    }

    /// 非表示にしたものをすべて表示に戻す
    pub fn show_all(&mut self) {
        for node in self.nodes.values_mut() {
            node.visible = true;
        }
    }

    /// `id` だけが見えるよう、`id` と祖先の兄弟を隠す
    pub fn isolate(&mut self, id: NodeId) {
        for a in self.ancestors(id) {
            let siblings = match self.nodes.get(&a).and_then(|n| n.parent) {
                Some(parent) => self.nodes[&parent].children.clone(),
                None => self.roots.clone(),
            };
            for s in siblings {
                if let Some(node) = self.nodes.get_mut(&s) {
                    node.visible = s == a;
                }
            }
        }
    }

    pub fn clear(&mut self) {
        for root in self.roots.clone() {
            self.remove(root);
        }
    }

    // --- 参照 ---------------------------------------------------------------

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// 自分と子孫の id (深さ優先、自分が先頭)
    pub fn subtree(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            if let Some(node) = self.nodes.get(&id) {
                out.push(id);
                stack.extend(node.children.iter().rev());
            }
        }
        out
    }

    /// ルートから自分までの祖先 (自分を含む、ルートが先頭)
    pub fn ancestors(&self, id: NodeId) -> Vec<NodeId> {
        let mut chain = Vec::new();
        let mut current = Some(id);
        while let Some(id) = current {
            let Some(node) = self.nodes.get(&id) else {
                break;
            };
            chain.push(id);
            current = node.parent;
        }
        chain.reverse();
        chain
    }

    pub fn is_in_subtree(&self, id: NodeId, root: NodeId) -> bool {
        self.ancestors(id).contains(&root)
    }

    /// "SampleVise / Bracket_L / Upright_1" のような道順
    pub fn path_label(&self, id: NodeId) -> String {
        self.ancestors(id)
            .iter()
            .filter_map(|&a| self.nodes.get(&a).map(|n| n.name.as_str()))
            .collect::<Vec<_>>()
            .join(" / ")
    }

    /// ワールド座標系での配置 (祖先の配置を順に掛ける)
    pub fn world_matrix(&self, id: NodeId) -> DMat4 {
        self.ancestors(id)
            .iter()
            .filter_map(|a| self.nodes.get(a))
            .fold(DMat4::IDENTITY, |m, n| m * n.placement)
    }

    /// 自分と祖先がすべて表示中なら true
    pub fn is_shown(&self, id: NodeId) -> bool {
        self.ancestors(id)
            .iter()
            .all(|a| self.nodes.get(a).is_some_and(|n| n.visible))
    }

    /// 表示に使う色 (自分か祖先に付けた色 → STEP の色 → 既定色)
    pub fn display_color(&self, id: NodeId) -> [f32; 3] {
        let assigned = self
            .ancestors(id)
            .iter()
            .rev()
            .find_map(|a| self.nodes.get(a)?.color);
        let from_file = match self.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::Imported { shape, .. }) => shape.color,
            _ => None,
        };
        assigned.or(from_file).unwrap_or(DEFAULT_PART_COLOR)
    }

    /// 部品の座標系での外接箱
    fn local_bounds(&self, node: &Node) -> Option<Aabb> {
        match &node.kind {
            NodeKind::Assembly { .. } => None,
            NodeKind::Box(part) => Some(part.bounds()),
            NodeKind::Imported { shape, .. } => Some(Aabb {
                min: shape.stats.min,
                max: shape.stats.max,
            }),
        }
    }

    /// ノード (アセンブリなら子孫すべて) のワールド座標の外接箱
    pub fn world_bounds(&self, id: NodeId) -> Option<Aabb> {
        self.subtree(id)
            .into_iter()
            .filter_map(|n| {
                let node = self.nodes.get(&n)?;
                Some(self.local_bounds(node)?.transformed(self.world_matrix(n)))
            })
            .reduce(Aabb::union)
    }

    /// 表示中のものすべての外接箱
    pub fn scene_bounds(&self) -> Option<Aabb> {
        self.roots
            .iter()
            .filter(|&&r| self.is_shown(r))
            .filter_map(|&r| self.world_bounds(r))
            .reduce(Aabb::union)
    }

    /// 部品数・面数・体積などの合計 (配置は剛体移動なので体積は変わらない)
    pub fn measure(&self, id: NodeId) -> Measurements {
        let mut m = Measurements::default();
        for n in self.subtree(id) {
            let Some(node) = self.nodes.get(&n) else {
                continue;
            };
            match &node.kind {
                NodeKind::Assembly { .. } => continue,
                NodeKind::Box(part) => {
                    let [w, h, d] = part.size;
                    m.face_count += 6;
                    m.edge_count += 12;
                    m.volume += w * h * d;
                    m.area += 2.0 * (w * h + h * d + d * w);
                }
                NodeKind::Imported { shape, .. } => {
                    m.face_count += shape.stats.face_count;
                    m.edge_count += shape.stats.edge_count;
                    m.volume += shape.stats.volume;
                    m.area += shape.stats.area;
                }
            }
            m.part_count += 1;
        }
        m
    }
}

/// いま選んでいるもの
#[derive(Resource, Default)]
pub struct Selection {
    /// 選択中のノード (部品でもアセンブリでもよい)
    pub node: Option<NodeId>,
    /// ビューで面をクリックしたときの面番号
    pub face: Option<usize>,
    /// ツリーの行にカーソルが乗っているノード (ビューで薄く強調する)
    pub hovered: Option<NodeId>,
    /// ビューで選んだとき、ツリーをそこまで開いてスクロールする
    pub reveal_in_tree: bool,
}

impl Selection {
    pub fn select(&mut self, node: NodeId) {
        self.node = Some(node);
        self.face = None;
    }

    pub fn clear(&mut self) {
        self.node = None;
        self.face = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::step_import::import_step;

    fn sample_model() -> (Model, NodeId) {
        let file = import_step(
            include_str!("../assets/sample_assembly.step"),
            "sample_assembly.step",
        )
        .unwrap();
        let mut model = Model::default();
        let root = model.add_imported(file.root, file.unit);
        (model, root)
    }

    fn find(model: &Model, name: &str) -> NodeId {
        *model.nodes.iter().find(|(_, n)| n.name == name).unwrap().0
    }

    #[test]
    fn assembly_bounds_cover_all_parts() {
        let (model, root) = sample_model();
        // STEP の Z 上は Y 上に回して取り込む
        let size = model.world_bounds(root).unwrap().size();
        // X: 軸 -130..130 とノブ 16 → 276 / 高さ: ベース下端 -20 〜 ノブ上端 70 + 22
        // 円柱の外接箱はメッシュから求めるので少しだけ小さくなる
        assert!((size.x - 276.0).abs() < 1e-6, "{size}");
        assert!((size.y - 112.0).abs() < 0.1, "{size}");
        assert!((size.z - 100.0).abs() < 1e-6, "{size}");
        assert_eq!(model.measure(root).part_count, 7);
    }

    #[test]
    fn hiding_assembly_hides_children() {
        let (mut model, _) = sample_model();
        let bracket = find(&model, "Bracket_L");
        let upright = model.get(bracket).unwrap().children[0];
        assert!(model.is_shown(upright));
        model.get_mut(bracket).unwrap().visible = false;
        assert!(!model.is_shown(upright));
    }

    #[test]
    fn remove_subtree() {
        let (mut model, root) = sample_model();
        model.dirty.clear();
        let bracket = find(&model, "Bracket_R");
        model.remove(bracket);
        assert_eq!(model.measure(root).part_count, 5);
        assert_eq!(model.dirty.len(), 3);
    }

    #[test]
    fn color_inherits_from_assembly() {
        let (mut model, _) = sample_model();
        let bracket = find(&model, "Bracket_L");
        let upright = model.get(bracket).unwrap().children[0];
        assert_eq!(model.display_color(upright), [0.4, 0.62, 0.48]);
        model.get_mut(bracket).unwrap().color = Some([1.0, 0.0, 0.0]);
        assert_eq!(model.display_color(upright), [1.0, 0.0, 0.0]);
    }
}
