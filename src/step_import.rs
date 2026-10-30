//! STEP ファイル (AP203 / AP214 / AP242) の読み込み
//!
//! Bevy に依存しない純粋な変換処理だけを置く。読み込みは別スレッドで動かす前提。
//!
//! 1. ruststep で STEP をパースし、エンティティを id で引ける表にする。
//! 2. 製品構成 (PRODUCT_DEFINITION と NEXT_ASSEMBLY_USAGE_OCCURRENCE) からアセンブリの木を作る。
//! 3. 各部品の形状 (MANIFOLD_SOLID_BREP など) を truck-stepio で B-rep (f64) に変換し、
//!    表示用にテッセレーションする。同じ部品を何度使っていても形状は1回だけ作る。
//!    テッセレーションは部品 (製品) ごとに CPU のコア数ぶん並列に行う。
//! 4. 色 (STYLED_ITEM) と長さの単位も読む。
//!
//! STEP は Z 軸が上なので、Y 軸が上の Bevy に合わせる回転はルートの配置に入れる。

use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use bevy::math::{DMat4, DVec3};
use truck_meshalgo::prelude::*;

use crate::interchange::Animation;
use truck_stepio::r#in::{
    Table,
    alias::{Curve3D, Surface},
    ruststep::{
        self,
        ast::{EntityInstance, Name, Parameter, Record},
    },
};
use truck_topology::compress::CompressedShell;

use crate::part_mesh::PartMesh;

/// truck の B-rep (f64)。表示用メッシュの元であり、今後の計測や干渉チェックにも使う。
pub type Brep = CompressedShell<Point3, Curve3D, Surface>;

/// 読み込んだファイル全体
pub struct ImportedFile {
    /// アセンブリの木のルート (ファイル名のノード)
    pub root: ImportNode,
    /// 長さの単位 ("mm" など)
    pub unit: String,
    /// 読めなかった形状など、ユーザーに伝えたい注意
    pub warnings: Vec<String>,
    /// ノードの動き (glTF などから)。トラックのノード番号は木を深さ優先でたどった順 (ルートが 0)
    pub animations: Vec<Animation>,
}

/// アセンブリの木の1ノード。子を持つならアセンブリ、shape を持つなら部品。
pub struct ImportNode {
    /// インスタンス名 (例: "Bracket_L")
    pub name: String,
    /// 製品名 (例: "Bracket")。同じ製品の別インスタンスは同じ名前になる。
    pub product: String,
    /// 親の座標系から見た配置
    pub placement: DMat4,
    pub children: Vec<ImportNode>,
    pub shape: Option<Arc<ImportedShape>>,
}

/// 1つの部品 (製品) の形状。複数のインスタンスで共有する。
pub struct ImportedShape {
    /// 共有されている形状を見分けるための番号 (メッシュのキャッシュに使う)
    pub id: usize,
    /// 元の B-rep。いまは表示に使っていないが、今後の計測や干渉チェックのために持っておく。
    #[allow(dead_code)]
    pub breps: Vec<Brep>,
    /// 面ごとの三角形メッシュ (部品の座標系, f64)
    pub faces: Vec<PolygonMesh>,
    /// 稜線のポリライン (部品の座標系, f64)
    pub edges: Vec<Vec<Point3>>,
    /// STEP に書かれていた色 (sRGB 0..1)
    pub color: Option<[f32; 3]>,
    pub stats: ShapeStats,
    /// 表示用のまとめたメッシュ・BVH・簡略形状。重いので読み込みのスレッドで作っておく。
    pub mesh: Arc<PartMesh>,
}

/// 部品の計測値 (メッシュから計算するので近似値)
#[derive(Clone, Copy, Default)]
pub struct ShapeStats {
    pub min: DVec3,
    pub max: DVec3,
    pub volume: f64,
    pub area: f64,
    pub face_count: usize,
    pub edge_count: usize,
}

/// STEP の文字列を読み込む。`file_name` はルートノードの名前になる。
pub fn import_step(text: &str, file_name: &str) -> Result<ImportedFile, String> {
    let exchange =
        ruststep::parser::parse(text).map_err(|e| format!("STEP の構文を読めませんでした: {e}"))?;
    let instances: Vec<&EntityInstance> = exchange.data.iter().flat_map(|d| &d.entities).collect();
    let entities = Entities::new(&instances);
    let table = Table::from_iter(instances.iter().copied());

    let mut builder = TreeBuilder {
        entities: &entities,
        table: &table,
        structure: ProductStructure::read(&entities),
        colors: read_colors(&entities),
        shapes: HashMap::new(),
        warnings: Vec::new(),
    };
    let mut tops = builder.build_roots();
    if tops.is_empty() {
        return Err("STEP の中に形状が見つかりませんでした".into());
    }

    // ファイル名のノードをルートにし、Z 上 → Y 上の回転を入れる
    let z_up_to_y_up = DMat4::from_rotation_x(-std::f64::consts::FRAC_PI_2);
    let root = if tops.len() == 1 && tops[0].shape.is_none() {
        let mut top = tops.remove(0);
        top.placement = z_up_to_y_up * top.placement;
        top
    } else {
        ImportNode {
            name: file_name.to_owned(),
            product: file_name.to_owned(),
            placement: z_up_to_y_up,
            children: tops,
            shape: None,
        }
    };
    Ok(ImportedFile {
        root,
        unit: read_length_unit(&entities),
        warnings: builder.warnings,
        animations: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// エンティティの表
// ---------------------------------------------------------------------------

/// `#id` からレコードを引く表。複合エンティティ `( A() B() )` は複数のレコードを持つ。
struct Entities<'a> {
    records: HashMap<u64, Vec<&'a Record>>,
}

impl<'a> Entities<'a> {
    fn new(instances: &[&'a EntityInstance]) -> Self {
        let records = instances
            .iter()
            .map(|instance| match instance {
                EntityInstance::Simple { id, record } => (*id, vec![record]),
                EntityInstance::Complex { id, subsuper } => (*id, subsuper.0.iter().collect()),
            })
            .collect();
        Self { records }
    }

    /// `id` のエンティティが `name` 型のレコードを持っていれば、その引数を返す
    fn get(&self, id: u64, name: &str) -> Option<&'a [Parameter]> {
        self.records
            .get(&id)?
            .iter()
            .find(|r| r.name == name)
            .map(|r| args(r))
    }

    /// `name` 型のレコードを持つエンティティを (id, 引数) で列挙する
    fn all(&self, name: &'a str) -> impl Iterator<Item = (u64, &'a [Parameter])> + '_ {
        self.records.iter().filter_map(move |(id, records)| {
            let record = records.iter().find(|r| r.name == name)?;
            Some((*id, args(record)))
        })
    }

    fn type_names(&self, id: u64) -> impl Iterator<Item = &'a str> + '_ {
        self.records
            .get(&id)
            .into_iter()
            .flatten()
            .map(|r| r.name.as_str())
    }
}

fn args(record: &Record) -> &[Parameter] {
    match &record.parameter {
        Parameter::List(list) => list,
        _ => &[],
    }
}

fn as_ref(p: &Parameter) -> Option<u64> {
    match p {
        Parameter::Ref(Name::Entity(id)) => Some(*id),
        _ => None,
    }
}

fn as_str(p: &Parameter) -> Option<&str> {
    match p {
        Parameter::String(s) => Some(s),
        _ => None,
    }
}

fn as_real(p: &Parameter) -> Option<f64> {
    match p {
        Parameter::Real(x) => Some(*x),
        Parameter::Integer(x) => Some(*x as f64),
        _ => None,
    }
}

fn as_refs(p: &Parameter) -> Vec<u64> {
    match p {
        Parameter::List(list) => list.iter().filter_map(as_ref).collect(),
        _ => as_ref(p).into_iter().collect(),
    }
}

fn arg_ref(params: &[Parameter], index: usize) -> Option<u64> {
    params.get(index).and_then(as_ref)
}

// ---------------------------------------------------------------------------
// 製品構成
// ---------------------------------------------------------------------------

/// 親子関係1本分 (NEXT_ASSEMBLY_USAGE_OCCURRENCE)
struct Occurrence {
    name: String,
    parent: u64,
    child: u64,
    /// 子の座標系 → 親の座標系
    placement: DMat4,
}

struct ProductStructure {
    /// PRODUCT_DEFINITION の id → 製品名
    product_names: HashMap<u64, String>,
    /// PRODUCT_DEFINITION の id → 形状の表現 (REPRESENTATION) の id
    representations: HashMap<u64, Vec<u64>>,
    occurrences: Vec<Occurrence>,
}

impl ProductStructure {
    fn read(e: &Entities) -> Self {
        // PRODUCT_DEFINITION → PRODUCT_DEFINITION_FORMATION → PRODUCT の名前
        let mut product_names = HashMap::new();
        for (pd, params) in e.all("PRODUCT_DEFINITION") {
            let name = arg_ref(params, 2)
                .and_then(|pdf| e.get(pdf, "PRODUCT_DEFINITION_FORMATION"))
                .and_then(|pdf| arg_ref(pdf, 2))
                .and_then(|product| e.get(product, "PRODUCT"))
                .and_then(|product| {
                    let name = as_str(&product[1]).filter(|s| !s.is_empty());
                    name.or_else(|| as_str(&product[0]))
                })
                .unwrap_or("(名前なし)");
            product_names.insert(pd, name.to_owned());
        }

        // PRODUCT_DEFINITION_SHAPE の id → それが指すもの (製品定義か親子関係)
        let shape_of: HashMap<u64, u64> = e
            .all("PRODUCT_DEFINITION_SHAPE")
            .filter_map(|(id, params)| Some((id, arg_ref(params, 2)?)))
            .collect();

        // SHAPE_DEFINITION_REPRESENTATION: 製品定義に形状の表現をひも付ける
        let mut representations: HashMap<u64, Vec<u64>> = HashMap::new();
        for (_, params) in e.all("SHAPE_DEFINITION_REPRESENTATION") {
            if let (Some(pds), Some(rep)) = (arg_ref(params, 0), arg_ref(params, 1))
                && let Some(&pd) = shape_of.get(&pds)
            {
                representations.entry(pd).or_default().push(rep);
            }
        }
        // 変換なしの SHAPE_REPRESENTATION_RELATIONSHIP でつながった表現も同じ製品のもの
        let links = plain_representation_links(e);
        for reps in representations.values_mut() {
            let mut stack = reps.clone();
            let mut seen: HashSet<u64> = reps.iter().copied().collect();
            while let Some(rep) = stack.pop() {
                for &next in links.get(&rep).into_iter().flatten() {
                    if seen.insert(next) {
                        reps.push(next);
                        stack.push(next);
                    }
                }
            }
        }

        // 親子関係と、その配置 (CONTEXT_DEPENDENT_SHAPE_REPRESENTATION)
        let placement_of_occurrence: HashMap<u64, u64> = e
            .all("CONTEXT_DEPENDENT_SHAPE_REPRESENTATION")
            .filter_map(|(_, params)| {
                let relation = arg_ref(params, 0)?;
                let occurrence = shape_of.get(&arg_ref(params, 1)?)?;
                Some((*occurrence, relation))
            })
            .collect();
        let mut occurrences: Vec<(u64, Occurrence)> = e
            .all("NEXT_ASSEMBLY_USAGE_OCCURRENCE")
            .filter_map(|(id, params)| {
                let parent = arg_ref(params, 3)?;
                let child = arg_ref(params, 4)?;
                let name = [1, 0]
                    .iter()
                    .filter_map(|&i| params.get(i).and_then(as_str))
                    .find(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| product_names.get(&child).cloned().unwrap_or_default());
                let child_reps = representations
                    .get(&child)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let placement = placement_of_occurrence
                    .get(&id)
                    .and_then(|&relation| relation_transform(e, relation, child_reps))
                    .unwrap_or(DMat4::IDENTITY);
                let occurrence = Occurrence {
                    name,
                    parent,
                    child,
                    placement,
                };
                Some((id, occurrence))
            })
            .collect();
        // ファイルに書かれた順 (= 多くの CAD での並び順) にする
        occurrences.sort_by_key(|(id, _)| *id);
        let occurrences = occurrences.into_iter().map(|(_, o)| o).collect();

        Self {
            product_names,
            representations,
            occurrences,
        }
    }
}

/// 変換を伴わない表現どうしのつながり (両方向)
fn plain_representation_links(e: &Entities) -> HashMap<u64, Vec<u64>> {
    let mut links: HashMap<u64, Vec<u64>> = HashMap::new();
    for (id, params) in e.all("REPRESENTATION_RELATIONSHIP") {
        let with_transform = e
            .type_names(id)
            .any(|n| n == "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION");
        if with_transform {
            continue;
        }
        if let (Some(a), Some(b)) = (arg_ref(params, 2), arg_ref(params, 3)) {
            links.entry(a).or_default().push(b);
            links.entry(b).or_default().push(a);
        }
    }
    // 単独の SHAPE_REPRESENTATION_RELATIONSHIP('', '', a, b) という書き方もある
    for (id, params) in e.all("SHAPE_REPRESENTATION_RELATIONSHIP") {
        if e.get(id, "REPRESENTATION_RELATIONSHIP").is_some() {
            continue;
        }
        if let (Some(a), Some(b)) = (arg_ref(params, 2), arg_ref(params, 3)) {
            links.entry(a).or_default().push(b);
            links.entry(b).or_default().push(a);
        }
    }
    links
}

/// REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION から「子 → 親」の変換を求める
fn relation_transform(e: &Entities, relation: u64, child_reps: &[u64]) -> Option<DMat4> {
    let rel = e.get(relation, "REPRESENTATION_RELATIONSHIP")?;
    let rep_1 = arg_ref(rel, 2)?;
    let with_transform = e.get(relation, "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION")?;
    let idt = e.get(arg_ref(with_transform, 0)?, "ITEM_DEFINED_TRANSFORMATION")?;
    let item_1 = axis2_placement(e, arg_ref(idt, 2)?)?;
    let item_2 = axis2_placement(e, arg_ref(idt, 3)?)?;
    // 通常は rep_1 が子、item_1 が子側の基準、item_2 が親側の配置
    if child_reps.is_empty() || child_reps.contains(&rep_1) {
        Some(item_2 * item_1.inverse())
    } else {
        Some(item_1 * item_2.inverse())
    }
}

/// AXIS2_PLACEMENT_3D を座標変換行列にする
fn axis2_placement(e: &Entities, id: u64) -> Option<DMat4> {
    let params = e.get(id, "AXIS2_PLACEMENT_3D")?;
    let location = arg_ref(params, 1).and_then(|p| cartesian_point(e, p))?;
    let z = arg_ref(params, 2)
        .and_then(|d| direction(e, d))
        .unwrap_or(DVec3::Z)
        .normalize();
    let x_hint = arg_ref(params, 3)
        .and_then(|d| direction(e, d))
        .unwrap_or_else(|| z.any_orthonormal_vector());
    let x = (x_hint - z * x_hint.dot(z)).normalize();
    let y = z.cross(x);
    Some(DMat4::from_cols(
        x.extend(0.0),
        y.extend(0.0),
        z.extend(0.0),
        location.extend(1.0),
    ))
}

fn cartesian_point(e: &Entities, id: u64) -> Option<DVec3> {
    vec3(e.get(id, "CARTESIAN_POINT")?.get(1)?)
}

fn direction(e: &Entities, id: u64) -> Option<DVec3> {
    vec3(e.get(id, "DIRECTION")?.get(1)?)
}

fn vec3(p: &Parameter) -> Option<DVec3> {
    let Parameter::List(values) = p else {
        return None;
    };
    let c: Vec<f64> = values.iter().filter_map(as_real).collect();
    match c[..] {
        [x, y, z, ..] => Some(DVec3::new(x, y, z)),
        [x, y] => Some(DVec3::new(x, y, 0.0)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 木の組み立て
// ---------------------------------------------------------------------------

struct TreeBuilder<'a, 'e> {
    entities: &'a Entities<'e>,
    table: &'a Table,
    structure: ProductStructure,
    /// 形状アイテム (ソリッドやシェル) の id → 色
    colors: HashMap<u64, [f32; 3]>,
    /// 製品定義の id → 形状 (インスタンス間で共有する)
    shapes: HashMap<u64, Option<Arc<ImportedShape>>>,
    warnings: Vec<String>,
}

impl TreeBuilder<'_, '_> {
    /// どの親子関係でも子になっていない製品定義をルートとして木を作る
    fn build_roots(&mut self) -> Vec<ImportNode> {
        self.prepare_shapes();
        let children: HashSet<u64> = self.structure.occurrences.iter().map(|o| o.child).collect();
        let parents: HashSet<u64> = self
            .structure
            .occurrences
            .iter()
            .map(|o| o.parent)
            .collect();
        let mut roots: Vec<u64> = self
            .structure
            .product_names
            .keys()
            .copied()
            .filter(|pd| !children.contains(pd))
            .filter(|pd| parents.contains(pd) || self.structure.representations.contains_key(pd))
            .collect();
        roots.sort_unstable();

        let mut nodes: Vec<ImportNode> = roots
            .into_iter()
            .filter_map(|pd| {
                let name = self.structure.product_names[&pd].clone();
                self.build_node(pd, name, DMat4::IDENTITY, &mut Vec::new())
            })
            .collect();

        if nodes.is_empty() {
            // 製品構成が書かれていないファイル: シェルをそのまま部品として並べる
            let mut shells: Vec<u64> = self.table.shell.keys().copied().collect();
            shells.sort_unstable();
            let jobs: Vec<(u64, ShapeJob)> = shells
                .into_iter()
                .filter_map(|shell| {
                    let job = self.convert_shells(&[shell], &format!("Shell #{shell}"))?;
                    Some((shell, job))
                })
                .collect();
            nodes = self
                .tessellate_all(jobs)
                .into_iter()
                .filter_map(|(shell, shape)| {
                    Some(ImportNode {
                        name: format!("Shell #{shell}"),
                        product: format!("Shell #{shell}"),
                        placement: DMat4::IDENTITY,
                        children: Vec::new(),
                        shape: Some(shape?),
                    })
                })
                .collect();
        }
        nodes
    }

    /// 製品定義 `pd` のノードを作る。`path` は循環参照を防ぐための祖先リスト。
    fn build_node(
        &mut self,
        pd: u64,
        name: String,
        placement: DMat4,
        path: &mut Vec<u64>,
    ) -> Option<ImportNode> {
        if path.contains(&pd) {
            self.warnings
                .push(format!("循環したアセンブリ構成を飛ばしました: {name}"));
            return None;
        }
        path.push(pd);
        let product = self.structure.product_names[&pd].clone();
        let occurrences: Vec<(String, u64, DMat4)> = self
            .structure
            .occurrences
            .iter()
            .filter(|o| o.parent == pd)
            .map(|o| (o.name.clone(), o.child, o.placement))
            .collect();
        let mut children: Vec<ImportNode> = occurrences
            .into_iter()
            .filter_map(|(child_name, child, child_placement)| {
                self.build_node(child, child_name, child_placement, path)
            })
            .collect();
        path.pop();

        let shape = self.shape_of_product(pd);
        // 形状と子の両方を持つ製品は、形状を子ノードとして足す
        let shape = match (shape, children.is_empty()) {
            (Some(shape), false) => {
                children.insert(
                    0,
                    ImportNode {
                        name: format!("{product} (本体)"),
                        product: product.clone(),
                        placement: DMat4::IDENTITY,
                        children: Vec::new(),
                        shape: Some(shape),
                    },
                );
                None
            }
            (shape, _) => shape,
        };
        if shape.is_none() && children.is_empty() {
            return None;
        }
        Some(ImportNode {
            name,
            product,
            placement,
            children,
            shape,
        })
    }

    fn shape_of_product(&mut self, pd: u64) -> Option<Arc<ImportedShape>> {
        if let Some(shape) = self.shapes.get(&pd) {
            return shape.clone();
        }
        let shells = self.product_shells(pd);
        let name = self.structure.product_names[&pd].clone();
        let shape = if shells.is_empty() {
            None
        } else {
            self.make_shape(&shells, name)
        };
        self.shapes.insert(pd, shape.clone());
        shape
    }

    /// 製品定義の形状を作るシェル
    fn product_shells(&self, pd: u64) -> Vec<u64> {
        self.structure
            .representations
            .get(&pd)
            .into_iter()
            .flatten()
            .flat_map(|&rep| representation_items(self.entities, rep))
            .flat_map(|item| shells_of_item(self.entities, item))
            .collect()
    }

    /// 形状を持つ製品をすべて先にまとめて作る。いちばん重いテッセレーションを
    /// CPU のコア数ぶん並列に行い、木を組み立てるときはできたものを引くだけにする。
    fn prepare_shapes(&mut self) {
        let mut products: Vec<u64> = self.structure.representations.keys().copied().collect();
        products.sort_unstable();
        let mut jobs = Vec::new();
        for pd in products {
            let shells = self.product_shells(pd);
            let name = self
                .structure
                .product_names
                .get(&pd)
                .cloned()
                .unwrap_or_default();
            match self.convert_shells(&shells, &name) {
                Some(job) => jobs.push((pd, job)),
                None => {
                    self.shapes.insert(pd, None);
                }
            }
        }
        for (pd, shape) in self.tessellate_all(jobs) {
            self.shapes.insert(pd, shape);
        }
    }

    /// シェルを B-rep に変換してテッセレーションし、1つの部品形状にまとめる
    fn make_shape(&mut self, shells: &[u64], name: String) -> Option<Arc<ImportedShape>> {
        let job = self.convert_shells(shells, &name)?;
        self.tessellate_all(vec![((), job)]).pop()?.1
    }

    /// シェルを B-rep に変換する (テッセレーションはまだしない)。読めたものが無ければ None。
    fn convert_shells(&mut self, shells: &[u64], name: &str) -> Option<ShapeJob> {
        let mut breps = Vec::new();
        for &id in shells {
            let converted = self
                .table
                .shell
                .get(&id)
                .ok_or_else(|| "未対応のシェル".to_owned())
                .and_then(|holder| {
                    self.table
                        .to_compressed_shell(holder)
                        .map_err(|e| e.to_string())
                });
            match converted {
                Ok(brep) => breps.push(brep),
                Err(err) => self
                    .warnings
                    .push(format!("{name}: 形状 #{id} を読めませんでした ({err})")),
            }
        }
        if breps.is_empty() {
            return None;
        }
        Some(ShapeJob {
            name: name.to_owned(),
            color: shells.iter().find_map(|s| self.colors.get(s).copied()),
            breps,
        })
    }

    /// 変換した B-rep を並列にテッセレーションする。`key` は結果を見分けるための値。
    fn tessellate_all<K: Send>(
        &mut self,
        jobs: Vec<(K, ShapeJob)>,
    ) -> Vec<(K, Option<Arc<ImportedShape>>)> {
        parallel_map(jobs, |(key, job)| {
            let name = job.name;
            (key, name, tessellate(job.breps, job.color))
        })
        .into_iter()
        .map(|(key, name, shape)| {
            if shape.faces.is_empty() {
                self.warnings
                    .push(format!("{name}: 面をメッシュにできませんでした"));
                (key, None)
            } else {
                (key, Some(Arc::new(shape)))
            }
        })
        .collect()
    }
}

/// テッセレーション待ちの形状
struct ShapeJob {
    name: String,
    breps: Vec<Brep>,
    color: Option<[f32; 3]>,
}

/// `items` を CPU のコア数ぶんのスレッドで `f` にかけ、元の順に並べて返す
fn parallel_map<T: Send, R: Send>(items: Vec<T>, f: impl Fn(T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(items.len());
    if threads <= 1 {
        return items.into_iter().map(f).collect();
    }
    let count = items.len();
    // 重さが形状ごとにばらばらなので、空いたスレッドが次の1つを取りに来る形にする
    let queue = std::sync::Mutex::new(items.into_iter().enumerate());
    let results = std::sync::Mutex::new(Vec::with_capacity(count));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().map(|mut q| q.next()).ok().flatten();
                    let Some((index, item)) = next else {
                        break;
                    };
                    let result = f(item);
                    if let Ok(mut results) = results.lock() {
                        results.push((index, result));
                    }
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap_or_default();
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, r)| r).collect()
}

/// 表現 (REPRESENTATION の仲間) が持つアイテムの id
fn representation_items(e: &Entities, rep: u64) -> Vec<u64> {
    e.records
        .get(&rep)
        .into_iter()
        .flatten()
        .find_map(|r| {
            let params = args(r);
            matches!(params.get(1), Some(Parameter::List(_))).then(|| as_refs(&params[1]))
        })
        .unwrap_or_default()
}

/// 形状アイテムに含まれるシェルの id
fn shells_of_item(e: &Entities, item: u64) -> Vec<u64> {
    for name in ["MANIFOLD_SOLID_BREP", "FACETED_BREP"] {
        if let Some(params) = e.get(item, name) {
            return arg_ref(params, 1).into_iter().collect();
        }
    }
    if let Some(params) = e.get(item, "BREP_WITH_VOIDS") {
        let mut shells: Vec<u64> = arg_ref(params, 1).into_iter().collect();
        shells.extend(params.get(2).map(as_refs).unwrap_or_default());
        return shells;
    }
    if let Some(params) = e.get(item, "SHELL_BASED_SURFACE_MODEL") {
        return params.get(1).map(as_refs).unwrap_or_default();
    }
    if e.get(item, "CLOSED_SHELL").is_some() || e.get(item, "OPEN_SHELL").is_some() {
        return vec![item];
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// 色と単位
// ---------------------------------------------------------------------------

/// STYLED_ITEM から「形状アイテム (とその中のシェル) → 色」の表を作る
fn read_colors(e: &Entities) -> HashMap<u64, [f32; 3]> {
    let mut colors = HashMap::new();
    for kind in ["STYLED_ITEM", "OVER_RIDING_STYLED_ITEM"] {
        for (_, params) in e.all(kind) {
            let Some(item) = arg_ref(params, 2) else {
                continue;
            };
            let styles = params.get(1).map(as_refs).unwrap_or_default();
            let Some(color) = styles.into_iter().find_map(|s| find_color(e, s, 0)) else {
                continue;
            };
            colors.insert(item, color);
            for shell in shells_of_item(e, item) {
                colors.entry(shell).or_insert(color);
            }
        }
    }
    colors
}

/// スタイルの参照をたどって最初に見つかった色を返す
fn find_color(e: &Entities, id: u64, depth: usize) -> Option<[f32; 3]> {
    if depth > 8 {
        return None;
    }
    if let Some(params) = e.get(id, "COLOUR_RGB") {
        let c: Vec<f32> = params[1..]
            .iter()
            .filter_map(as_real)
            .map(|x| x as f32)
            .collect();
        return match c[..] {
            [r, g, b, ..] => Some([r, g, b]),
            _ => None,
        };
    }
    if let Some(params) = e.get(id, "DRAUGHTING_PRE_DEFINED_COLOUR") {
        return predefined_color(as_str(params.first()?)?);
    }
    e.records
        .get(&id)?
        .iter()
        .flat_map(|r| args(r))
        .flat_map(nested_refs)
        .find_map(|next| find_color(e, next, depth + 1))
}

/// 引数の中の参照を、リストの中も含めて全部取り出す
fn nested_refs(p: &Parameter) -> Vec<u64> {
    match p {
        Parameter::Ref(Name::Entity(id)) => vec![*id],
        Parameter::List(list) => list.iter().flat_map(nested_refs).collect(),
        Parameter::Typed { parameter, .. } => nested_refs(parameter),
        _ => Vec::new(),
    }
}

fn predefined_color(name: &str) -> Option<[f32; 3]> {
    Some(match name {
        "red" => [1.0, 0.0, 0.0],
        "green" => [0.0, 1.0, 0.0],
        "blue" => [0.0, 0.0, 1.0],
        "yellow" => [1.0, 1.0, 0.0],
        "magenta" => [1.0, 0.0, 1.0],
        "cyan" => [0.0, 1.0, 1.0],
        "black" => [0.0, 0.0, 0.0],
        "white" => [1.0, 1.0, 1.0],
        _ => return None,
    })
}

/// 長さの単位を読む。見つからなければ "mm" とみなす (STEP で最も多い)。
fn read_length_unit(e: &Entities) -> String {
    for (id, _) in e.all("LENGTH_UNIT") {
        if let Some(params) = e.get(id, "CONVERSION_BASED_UNIT") {
            let name = params
                .first()
                .and_then(as_str)
                .unwrap_or("?")
                .to_lowercase();
            return match name.as_str() {
                "inch" => "in".into(),
                "foot" => "ft".into(),
                _ => name,
            };
        }
        if let Some(params) = e.get(id, "SI_UNIT") {
            let prefix = match params.first() {
                Some(Parameter::Enumeration(p)) => p.as_str(),
                _ => "",
            };
            return match prefix {
                "MILLI" => "mm",
                "CENTI" => "cm",
                "MICRO" => "µm",
                "KILO" => "km",
                _ => "m",
            }
            .into();
        }
    }
    "mm".into()
}

// ---------------------------------------------------------------------------
// テッセレーション
// ---------------------------------------------------------------------------

/// B-rep を表示用の面メッシュと稜線に変換し、寸法・体積・表面積を求める
fn tessellate(breps: Vec<Brep>, color: Option<[f32; 3]>) -> ImportedShape {
    // 許容誤差は部品の大きさに合わせる (頂点の範囲で大きさを見積もる)
    let mut min = DVec3::splat(f64::MAX);
    let mut max = DVec3::splat(f64::MIN);
    for p in breps.iter().flat_map(|b| &b.vertices) {
        min = min.min(DVec3::new(p.x, p.y, p.z));
        max = max.max(DVec3::new(p.x, p.y, p.z));
    }
    let tol = tessellation_tolerance(min, max);

    let mut faces = Vec::new();
    let mut edges = Vec::new();
    for brep in &breps {
        let meshed = brep.robust_triangulation(tol);
        for face in meshed.faces {
            if let Some(mut poly) = face.surface {
                if !face.orientation {
                    poly.invert();
                }
                faces.push(poly);
            }
        }
        edges.extend(meshed.edges.into_iter().map(|e| e.curve.0));
    }

    ImportedShape::new(breps, faces, edges, color)
}

/// 部品の大きさ (頂点の範囲) に見合うテッセレーションの許容誤差
pub fn tessellation_tolerance(min: DVec3, max: DVec3) -> f64 {
    let (diameter, thinnest) = if min.x <= max.x {
        ((max - min).length(), (max - min).min_element())
    } else {
        (1.0, 1.0)
    };
    // 細長い部品 (軸など) でも丸みが角張らないよう、いちばん薄い方向の寸法も見る
    (diameter * 0.002)
        .min(thinnest * 0.005)
        .max(diameter * 1e-5)
        .max(1e-6)
}

impl ImportedShape {
    /// メッシュから形状を作り、計測値と共有用の番号を付ける
    pub fn new(
        breps: Vec<Brep>,
        faces: Vec<PolygonMesh>,
        edges: Vec<Vec<Point3>>,
        color: Option<[f32; 3]>,
    ) -> Self {
        let mut stats = mesh_stats(&faces);
        stats.edge_count = edges.len();
        let mesh = Arc::new(PartMesh::new(&faces, &edges));
        ImportedShape {
            id: next_shape_id(),
            breps,
            faces,
            edges,
            color,
            stats,
            mesh,
        }
    }
}

/// 形状の番号を払い出す。ファイルをまたいでも重ならない
pub fn next_shape_id() -> usize {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// 三角形メッシュから外接箱・体積・表面積を計算する
pub fn mesh_stats(faces: &[PolygonMesh]) -> ShapeStats {
    let mut stats = ShapeStats {
        min: DVec3::splat(f64::MAX),
        max: DVec3::splat(f64::MIN),
        face_count: faces.len(),
        ..Default::default()
    };
    for poly in faces {
        let positions = poly.positions();
        for p in positions {
            let p = DVec3::new(p.x, p.y, p.z);
            stats.min = stats.min.min(p);
            stats.max = stats.max.max(p);
        }
        for tri in poly.faces().triangle_iter() {
            let [a, b, c] = tri.map(|v| {
                let p = positions[v.pos];
                DVec3::new(p.x, p.y, p.z)
            });
            stats.area += (b - a).cross(c - a).length() / 2.0;
            // 原点と三角形で作る四面体の符号付き体積を足し合わせる
            stats.volume += a.dot(b.cross(c)) / 6.0;
        }
    }
    if stats.min.x > stats.max.x {
        stats.min = DVec3::ZERO;
        stats.max = DVec3::ZERO;
    }
    stats.volume = stats.volume.abs();
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_map_keeps_order() {
        let out = parallel_map((0..100).collect(), |i: u64| i * i);
        assert_eq!(out, (0..100).map(|i| i * i).collect::<Vec<_>>());
    }

    const SAMPLE: &str = include_str!("../assets/sample_assembly.step");

    fn find<'a>(node: &'a ImportNode, name: &str) -> Option<&'a ImportNode> {
        if node.name == name {
            return Some(node);
        }
        node.children.iter().find_map(|c| find(c, name))
    }

    #[test]
    fn sample_assembly_structure() {
        let file = import_step(SAMPLE, "sample_assembly.step").unwrap();
        assert!(file.warnings.is_empty(), "{:?}", file.warnings);
        assert_eq!(file.unit, "mm");
        let root = &file.root;
        assert_eq!(root.name, "SampleVise");
        let names: Vec<&str> = root.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["BasePlate_1", "Bracket_L", "Bracket_R", "Shaft_1", "Knob_1"]
        );
        let bracket = find(root, "Bracket_L").unwrap();
        assert_eq!(bracket.product, "Bracket");
        assert_eq!(bracket.children.len(), 2);
        // 同じ製品のインスタンスは形状を共有する
        let left = find(&root.children[1], "Bushing_1").unwrap();
        let right = find(&root.children[2], "Bushing_1").unwrap();
        assert!(Arc::ptr_eq(
            left.shape.as_ref().unwrap(),
            right.shape.as_ref().unwrap()
        ));
    }

    #[test]
    fn sample_shapes_have_expected_size_and_color() {
        let file = import_step(SAMPLE, "sample").unwrap();
        let base = find(&file.root, "BasePlate_1").unwrap();
        let shape = base.shape.as_ref().unwrap();
        let size = shape.stats.max - shape.stats.min;
        assert!((size - DVec3::new(240.0, 100.0, 20.0)).length() < 1e-6);
        assert!((shape.stats.volume - 240.0 * 100.0 * 20.0).abs() < 1e-3);
        assert_eq!(shape.color, Some([0.55, 0.6, 0.68]));

        // 円柱の体積はメッシュ近似なので 1% 以内
        let shaft = find(&file.root, "Shaft_1").unwrap().shape.as_ref().unwrap();
        let exact = std::f64::consts::PI * 8.0 * 8.0 * 260.0;
        let error = (shaft.stats.volume - exact).abs() / exact;
        assert!(error < 0.01, "誤差 {:.2}%", error * 100.0);
    }

    #[test]
    fn placement_moves_child_into_parent_frame() {
        let file = import_step(SAMPLE, "sample").unwrap();
        let shaft = find(&file.root, "Shaft_1").unwrap();
        // 円柱の軸 (+Z) が +X を向き、(-130, 0, 70) に置かれている
        let origin = shaft.placement.transform_point3(DVec3::ZERO);
        let tip = shaft.placement.transform_point3(DVec3::Z);
        assert!((origin - DVec3::new(-130.0, 0.0, 70.0)).length() < 1e-9);
        assert!((tip - origin - DVec3::X).length() < 1e-9);
    }
}
