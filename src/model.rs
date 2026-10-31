//! 設計データ (f64) と選択状態
//!
//! モデルはノードの木で表す。ノードは「アセンブリ」「箱 (パラメトリック部品)」
//! 「取り込んだ部品」のどれか。各ノードは親から見た配置 (f64 の行列) を持つ。
//! Bevy のエンティティとは独立していて、表示は viewport.rs が後から追いかける。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use bevy::math::{DMat4, DVec3};
use bevy::prelude::*;
use truck_modeling::{Solid, builder};
// truck の prelude の型名 (Point3 など) を使うが、Bevy と名前がぶつかるものは明示する
use truck_meshalgo::prelude::{EuclideanSpace, Point3, Rad, Vector3};

use crate::library::Spec;
use crate::math::{Aabb, euler_deg_to_quat};
use crate::physics::{Cable, PartPhysics};
use crate::site::model::Site;
use crate::step_import::{ImportNode, ImportedShape};
use cadsim_core::remap::{OutlineNode, plan_remap};

pub type NodeId = u32;

/// 色の指定がない部品の色
pub const DEFAULT_PART_COLOR: [f32; 3] = [0.70, 0.74, 0.80];

#[derive(Clone)]
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
    /// 物理演算での振る舞い (部品だけが使う)
    pub physics: PartPhysics,
    /// 部品スクリプト (Rhai)。PLC 連携の「部品スクリプト」モードで動く。
    pub script: Option<String>,
}

#[derive(Clone)]
pub enum NodeKind {
    /// 子をまとめるだけのノード。`product` は STEP の製品名。
    Assembly { product: Option<String> },
    /// 寸法で作る箱
    Box(BoxPart),
    /// STEP から取り込んだ部品。形状は同じ製品のインスタンスで共有する。
    Imported {
        product: String,
        shape: Arc<ImportedShape>,
        /// 取り込んだ STEP ファイル。プロジェクトを開くときにここから形状を読み直す。
        source: StepSource,
    },
    /// 標準部品 (シリンダ・コンベア・シーケンサーなど)。子に本体 (`StandardBody`) を持ち、
    /// パラメータを変えると library が子を作り直す。
    Standard(Spec),
    /// 標準部品の本体 (チューブ・ロッドなど)。形状は親のパラメータから作ったもの。
    StandardBody {
        /// `library::geometry::bodies` の並びでの番号
        body: usize,
        shape: Arc<ImportedShape>,
    },
}

/// 取り込んだ STEP がどこから来たか
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StepSource {
    /// アプリに埋め込みのサンプル
    Sample,
    File(PathBuf),
    /// 作成メニュー (円柱・押し出し・穴・ブーリアン) で作った形状。B-rep をプロジェクトに埋め込む
    Modeled(Arc<crate::modeling::ModeledSource>),
    /// 負荷テスト用のシーン (部品数)
    Stress(usize),
}

impl NodeKind {
    pub fn label(&self) -> &'static str {
        match self {
            NodeKind::Assembly { .. } => "アセンブリ",
            NodeKind::Box(_) => "部品 (箱)",
            NodeKind::Imported { .. } => "部品",
            NodeKind::Standard(_) => "標準部品",
            NodeKind::StandardBody { .. } => "標準部品の本体",
        }
    }

    pub fn is_part(&self) -> bool {
        !matches!(self, NodeKind::Assembly { .. } | NodeKind::Standard(_))
    }

    pub fn product(&self) -> Option<&str> {
        match self {
            NodeKind::Assembly { product } => product.as_deref(),
            NodeKind::Imported { product, .. } => Some(product),
            NodeKind::Standard(spec) => Some(spec.kind.label()),
            NodeKind::Box(_) | NodeKind::StandardBody { .. } => None,
        }
    }
}

/// 箱1つ分の設計パラメータ。値はすべて f64 で持つ。
#[derive(Clone, PartialEq, Debug)]
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

/// ノードとその子孫を丸ごと写したもの。元に戻す・やり直し (history.rs) で使う。
/// 取り込んだ形状は `Arc` で共有するので、大きなアセンブリでも写すのは軽い。
#[derive(Clone)]
pub struct Snapshot {
    pub root: NodeId,
    pub parent: Option<NodeId>,
    /// 親の children (親がなければ roots) の中での位置
    pub index: usize,
    /// 先頭が root
    pub nodes: Vec<(NodeId, Node)>,
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

/// 形状の差し替えの結果
#[derive(Clone, Debug, Default)]
pub struct ReplaceReport {
    /// 対応がついたノードの数 (一番上を含む)
    pub matched: usize,
    /// 新しい形状に見つからず、古い形状のまま残したノード
    pub unmatched: Vec<NodeId>,
    /// 新しい形状にだけあり、加えたノード
    pub added: Vec<NodeId>,
}

/// 取り込んだ木を深さ優先で並べる (子は持たない)。親は並びの中の番号。
fn flatten_import(
    mut node: ImportNode,
    parent: Option<usize>,
    out: &mut Vec<(ImportNode, Option<usize>)>,
) {
    let children = std::mem::take(&mut node.children);
    let index = out.len();
    out.push((node, parent));
    for child in children {
        flatten_import(child, Some(index), out);
    }
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
    /// 床に敷いた平面図など、部品のツリーの外にある現場のデータ
    pub site: Site,
    /// 部品の間に渡したケーブル・ホース
    pub cables: Vec<Cable>,
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
            site: Site::default(),
            cables: Vec::new(),
        }
    }
}

impl Model {
    // --- 追加と削除 ---------------------------------------------------------

    /// ノードを1つ加える。`parent` が None なら最上位に置く。
    pub fn add_node(
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
                physics: PartPhysics::default(),
                script: None,
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
        let name = format!("箱 {}", self.next_id);
        let part = BoxPart {
            size,
            position,
            rotation_deg: [0.0; 3],
        };
        self.add_node(None, name, DMat4::IDENTITY, NodeKind::Box(part))
    }

    /// 取り込んだ STEP の木をモデルに加え、そのルートの id を返す
    pub fn add_imported(&mut self, root: ImportNode, unit: String, source: StepSource) -> NodeId {
        self.unit = unit;
        let id = self.add_import_node(None, root, &source);
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

    fn add_import_node(
        &mut self,
        parent: Option<NodeId>,
        node: ImportNode,
        source: &StepSource,
    ) -> NodeId {
        let kind = match node.shape {
            Some(shape) => NodeKind::Imported {
                product: node.product,
                shape,
                source: source.clone(),
            },
            None => NodeKind::Assembly {
                product: Some(node.product),
            },
        };
        let id = self.add_node(parent, node.name, node.placement, kind);
        for child in node.children {
            self.add_import_node(Some(id), child, source);
        }
        id
    }

    /// `root` 以下の形状を、取り込み直した `new` の形状に差し替える。
    ///
    /// ノードは名前と構造で対応づけ ([`plan_remap`])、対応したノードは ID を残したまま
    /// 名前・配置・形状だけを入れ替える。ID に結び付いた設定 (アクチュエータ、関節、
    /// センサー、スクリプトなど) はそのまま引き継がれる。
    /// 一番上のノードの配置は、ユーザーが置いた位置なので変えない。
    /// 新しい側にだけあるノードは対応する親の下に加え、古い側にだけあるノードは古い形状のまま残す。
    pub fn replace_geometry(
        &mut self,
        root: NodeId,
        new: ImportNode,
        source: StepSource,
    ) -> Option<ReplaceReport> {
        self.nodes.get(&root)?;
        let old_ids = self.subtree(root);
        let old_index: HashMap<NodeId, usize> =
            old_ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
        let old_outline: Vec<OutlineNode> = old_ids
            .iter()
            .map(|id| {
                let node = &self.nodes[id];
                OutlineNode {
                    name: node.name.clone(),
                    product: node.kind.product().map(str::to_owned),
                    parent: node.parent.and_then(|p| old_index.get(&p).copied()),
                    is_part: node.kind.is_part(),
                }
            })
            .collect();

        let mut flat = Vec::new();
        flatten_import(new, None, &mut flat);
        let new_outline: Vec<OutlineNode> = flat
            .iter()
            .map(|(node, parent)| OutlineNode {
                name: node.name.clone(),
                product: Some(node.product.clone()),
                parent: *parent,
                is_part: node.shape.is_some(),
            })
            .collect();

        let plan = plan_remap(&old_outline, &new_outline);
        let mut report = ReplaceReport {
            matched: plan.matched.len(),
            ..Default::default()
        };

        // 新しい番号 → モデルの ID (対応したもの、または新しく加えたもの)
        let mut id_for_new: Vec<Option<NodeId>> = vec![None; flat.len()];
        for &(o, n, _) in &plan.matched {
            id_for_new[n] = Some(old_ids[o]);
        }
        for (n, (item, parent)) in flat.into_iter().enumerate() {
            let kind = match item.shape {
                Some(shape) => NodeKind::Imported {
                    product: item.product,
                    shape,
                    source: source.clone(),
                },
                None => NodeKind::Assembly {
                    product: Some(item.product),
                },
            };
            match id_for_new[n] {
                Some(id) => {
                    let node = self.nodes.get_mut(&id).expect("対応したノードはある");
                    node.name = item.name;
                    if id != root {
                        node.placement = item.placement;
                    }
                    node.kind = kind;
                    self.dirty.insert(id);
                }
                None => {
                    let parent_id = parent.and_then(|p| id_for_new[p]).unwrap_or(root);
                    // 子の並びは後でつなぎ直す
                    let id = self.add_node(Some(parent_id), item.name, item.placement, kind);
                    id_for_new[n] = Some(id);
                    report.added.push(id);
                }
            }
        }

        // 親子をつなぎ直す。新しいファイルの並び順を先に、残った古いノードを後ろに置く。
        let mut new_parent: HashMap<NodeId, NodeId> = HashMap::new();
        for (n, outline) in new_outline.iter().enumerate() {
            if let (Some(id), Some(p)) = (id_for_new[n], outline.parent) {
                new_parent.insert(id, id_for_new[p].unwrap_or(root));
            }
        }
        for &o in &plan.unmatched_old {
            let id = old_ids[o];
            if let Some(parent) = self.nodes[&id].parent {
                new_parent.insert(id, parent);
            }
            report.unmatched.push(id);
        }
        let mut order: Vec<NodeId> = id_for_new.iter().flatten().copied().collect();
        order.extend(plan.unmatched_old.iter().map(|&o| old_ids[o]));
        for &id in old_ids.iter().chain(&report.added) {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.children.clear();
            }
        }
        for id in order {
            if let Some(&parent) = new_parent.get(&id) {
                self.nodes.get_mut(&id).expect("ある").parent = Some(parent);
                if let Some(p) = self.nodes.get_mut(&parent) {
                    p.children.push(id);
                }
            }
        }
        Some(report)
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

    /// `id` とその子孫を写し取る
    pub fn snapshot(&self, id: NodeId) -> Option<Snapshot> {
        let parent = self.nodes.get(&id)?.parent;
        let siblings = match parent.and_then(|p| self.nodes.get(&p)) {
            Some(parent) => &parent.children,
            None => &self.roots,
        };
        Some(Snapshot {
            root: id,
            parent,
            index: siblings
                .iter()
                .position(|&c| c == id)
                .unwrap_or(siblings.len()),
            nodes: self
                .subtree(id)
                .into_iter()
                .map(|n| (n, self.nodes[&n].clone()))
                .collect(),
        })
    }

    /// 写し取ったノードを元の場所に戻す。同じ id が残っていれば置き換える。
    pub fn restore(&mut self, snapshot: &Snapshot) {
        self.remove(snapshot.root);
        for (id, node) in &snapshot.nodes {
            if node.kind.is_part() {
                self.dirty.insert(*id);
            }
            self.nodes.insert(*id, node.clone());
        }
        let siblings = match snapshot.parent.and_then(|p| self.nodes.get_mut(&p)) {
            Some(parent) => &mut parent.children,
            None => &mut self.roots,
        };
        siblings.insert(snapshot.index.min(siblings.len()), snapshot.root);
    }

    /// 最上位のノードの写しすべて
    pub fn snapshot_all(&self) -> Vec<Snapshot> {
        self.roots
            .iter()
            .filter_map(|&r| self.snapshot(r))
            .collect()
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
        self.cables.clear();
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

    /// "SampleVise / Bracket_L / Upright_1" のような道順
    pub fn path_label(&self, id: NodeId) -> String {
        self.ancestors(id)
            .iter()
            .filter_map(|&a| self.nodes.get(&a).map(|n| n.name.as_str()))
            .collect::<Vec<_>>()
            .join(" / ")
    }

    /// ビューでクリックしたときに選ぶノード。標準部品の本体なら部品全体を選ぶ。
    pub fn pick_target(&self, id: NodeId) -> NodeId {
        match self.nodes.get(&id) {
            Some(Node {
                kind: NodeKind::StandardBody { .. },
                parent: Some(parent),
                ..
            }) => *parent,
            _ => id,
        }
    }

    /// すべてのノードの id (ツリーの表示順)
    pub fn all_nodes(&self) -> Vec<NodeId> {
        self.roots.iter().flat_map(|&r| self.subtree(r)).collect()
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

    /// 表示に使う色 (自分か祖先に付けた色 → STEP の色 → 既定色)。
    /// 3D 表示では `visit_parts` がまとめて求める。
    pub fn display_color(&self, id: NodeId) -> [f32; 3] {
        let assigned = self
            .ancestors(id)
            .iter()
            .rev()
            .find_map(|a| self.nodes.get(a)?.color);
        let from_file = match self.nodes.get(&id).map(|n| &n.kind) {
            Some(NodeKind::Imported { shape, .. } | NodeKind::StandardBody { shape, .. }) => {
                shape.color
            }
            _ => None,
        };
        assigned.or(from_file).unwrap_or(DEFAULT_PART_COLOR)
    }

    /// すべての部品を、ワールド配置・表示/非表示・色を求めながら1回の深さ優先でたどる。
    /// 部品ごとに祖先をたどり直す `world_matrix` などより速いので、毎フレームの表示更新に使う。
    pub fn visit_parts(&self, mut f: impl FnMut(PartVisit)) {
        struct Frame {
            id: NodeId,
            depth: usize,
            parent_world: DMat4,
            parent_shown: bool,
            parent_color: Option<[f32; 3]>,
        }
        let mut path: Vec<NodeId> = Vec::new();
        let mut stack: Vec<Frame> = self
            .roots
            .iter()
            .rev()
            .map(|&id| Frame {
                id,
                depth: 0,
                parent_world: DMat4::IDENTITY,
                parent_shown: true,
                parent_color: None,
            })
            .collect();
        while let Some(frame) = stack.pop() {
            let Some(node) = self.nodes.get(&frame.id) else {
                continue;
            };
            path.truncate(frame.depth);
            path.push(frame.id);
            let world = frame.parent_world * node.placement;
            let shown = frame.parent_shown && node.visible;
            let assigned = node.color.or(frame.parent_color);
            if node.kind.is_part() {
                let from_file = match &node.kind {
                    NodeKind::Imported { shape, .. } | NodeKind::StandardBody { shape, .. } => {
                        shape.color
                    }
                    _ => None,
                };
                f(PartVisit {
                    id: frame.id,
                    path: &path,
                    world,
                    shown,
                    color: assigned.or(from_file).unwrap_or(DEFAULT_PART_COLOR),
                    local_bounds: self.local_bounds(node),
                });
            }
            for &child in node.children.iter().rev() {
                stack.push(Frame {
                    id: child,
                    depth: frame.depth + 1,
                    parent_world: world,
                    parent_shown: shown,
                    parent_color: assigned,
                });
            }
        }
    }

    /// 部品の座標系での外接箱
    fn local_bounds(&self, node: &Node) -> Option<Aabb> {
        match &node.kind {
            NodeKind::Assembly { .. } | NodeKind::Standard(_) => None,
            NodeKind::Box(part) => Some(part.bounds()),
            NodeKind::Imported { shape, .. } | NodeKind::StandardBody { shape, .. } => Some(Aabb {
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
                NodeKind::Assembly { .. } | NodeKind::Standard(_) => continue,
                NodeKind::Box(part) => {
                    let [w, h, d] = part.size;
                    m.face_count += 6;
                    m.edge_count += 12;
                    m.volume += w * h * d;
                    m.area += 2.0 * (w * h + h * d + d * w);
                }
                NodeKind::Imported { shape, .. } | NodeKind::StandardBody { shape, .. } => {
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

/// `Model::visit_parts` が部品ごとに渡す情報
pub struct PartVisit<'a> {
    pub id: NodeId,
    /// ルートから自分までの id (自分を含む)
    pub path: &'a [NodeId],
    pub world: DMat4,
    /// 自分と祖先がすべて表示中
    pub shown: bool,
    /// `display_color` と同じ色
    pub color: [f32; 3],
    /// 部品の座標系での外接箱
    pub local_bounds: Option<Aabb>,
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
        let root = model.add_imported(file.root, file.unit, StepSource::Sample);
        (model, root)
    }

    fn find(model: &Model, name: &str) -> NodeId {
        *model.nodes.iter().find(|(_, n)| n.name == name).unwrap().0
    }

    #[test]
    fn replace_geometry_keeps_ids_and_lists_unmatched() {
        let (mut model, root) = sample_model();
        let moved = DMat4::from_translation(DVec3::new(500.0, 0.0, 0.0));
        model.get_mut(root).unwrap().placement = moved;
        let knob = find(&model, "Knob_1");
        let shaft = find(&model, "Shaft_1");
        let left = find(&model, "Bracket_L");
        let upright_l = model.get(left).unwrap().children[0];

        // 取り込み直した版: Knob の名前が変わり、Shaft がなくなり、Bracket_L が少し動いた
        let mut new = import_step(
            include_str!("../assets/sample_assembly.step"),
            "sample_v2.step",
        )
        .unwrap()
        .root;
        new.children.retain(|c| c.name != "Shaft_1");
        for child in &mut new.children {
            match child.name.as_str() {
                "Knob_1" => child.name = "Knob_rev2".into(),
                "Bracket_L" => {
                    child.placement = DMat4::from_translation(DVec3::Y) * child.placement
                }
                _ => {}
            }
        }
        let left_placement = new
            .children
            .iter()
            .find(|c| c.name == "Bracket_L")
            .unwrap()
            .placement;

        let report = model
            .replace_geometry(root, new, StepSource::File("v2.step".into()))
            .unwrap();
        assert_eq!(report.unmatched, vec![shaft]);
        assert!(report.added.is_empty());
        // ID はそのまま、名前と配置は新しい版になる
        assert_eq!(model.get(knob).unwrap().name, "Knob_rev2");
        assert_eq!(model.get(left).unwrap().placement, left_placement);
        assert_eq!(model.get(left).unwrap().children[0], upright_l);
        // 一番上の配置はユーザーが置いた位置のまま
        assert_eq!(model.get(root).unwrap().placement, moved);
        // 古い Shaft は後ろに残る
        assert_eq!(model.get(root).unwrap().children.last(), Some(&shaft));
        assert_eq!(model.measure(root).part_count, 7);
        assert!(matches!(
            &model.get(knob).unwrap().kind,
            NodeKind::Imported { source: StepSource::File(p), .. } if p.ends_with("v2.step")
        ));
    }

    #[test]
    fn replace_geometry_adds_new_parts_under_matching_parent() {
        let (mut model, root) = sample_model();
        let before = model.subtree(root).len();
        let mut new = import_step(
            include_str!("../assets/sample_assembly.step"),
            "sample_v2.step",
        )
        .unwrap()
        .root;
        let extra = ImportNode {
            name: "Sensor_1".into(),
            product: "Sensor".into(),
            placement: DMat4::IDENTITY,
            children: Vec::new(),
            shape: new.children[0].shape.clone(),
        };
        let bracket = new
            .children
            .iter_mut()
            .find(|c| c.name == "Bracket_R")
            .unwrap();
        bracket.children.push(extra);
        let report = model
            .replace_geometry(root, new, StepSource::Sample)
            .unwrap();
        assert_eq!(report.added.len(), 1);
        assert!(report.unmatched.is_empty());
        assert_eq!(model.subtree(root).len(), before + 1);
        let added = model.get(report.added[0]).unwrap();
        assert_eq!(added.name, "Sensor_1");
        assert_eq!(added.parent, Some(find(&model, "Bracket_R")));
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
    fn visit_parts_matches_per_node_queries() {
        let (mut model, root) = sample_model();
        let bracket = find(&model, "Bracket_L");
        model.get_mut(bracket).unwrap().visible = false;
        model.get_mut(bracket).unwrap().color = Some([1.0, 0.0, 0.0]);
        let mut count = 0;
        model.visit_parts(|v| {
            count += 1;
            assert_eq!(v.path, model.ancestors(v.id).as_slice());
            assert!(v.world.abs_diff_eq(model.world_matrix(v.id), 1e-12));
            assert_eq!(v.shown, model.is_shown(v.id));
            assert_eq!(v.color, model.display_color(v.id));
        });
        assert_eq!(count, model.measure(root).part_count);
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
