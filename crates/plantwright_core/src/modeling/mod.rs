//! モデリング操作 (ドメイン + ユースケース): 円柱・押し出し・穴あけ・ブーリアン。
//! フィレット・面取り・シェル・回転体・パイプ・スイープ・質量特性・断面は [`ops`]。
//! 3D ビューで面を指して穴をあける道具は [`hole_tool`]、辺・面を指して丸める・面取り・肉抜きする道具は [`shape_tool`]
//!
//! 形状は形状カーネル (SMLib) の B-rep ([`Brep`], f64) で作り、ブーリアン (和・差・積) もカーネルで行う。
//! 作った形状はモデルの「部品」として加え、出どころに B-rep を詰めておく
//! ([`ModeledSource`])。詰め方 (プロジェクトファイルに埋め込む JSON) は `modeling::record` が
//! [`SolidStore`] として渡す。開き直すとそこから形状を作り直すので、続けて穴をあけたり
//! ブーリアンをかけたりできる。
//!
//! 演算できるのは箱とここで作った部品だけ。STEP などから取り込んだ部品は、
//! 元のファイルとのつながりを保つため対象外 (エラーで知らせる)。
//! 穴あけ・ブーリアンは、元に戻すための [`ModelChange`] を返す。
//!
//! 形を変えるたびに「何をしたか」を [`features::ShapeHistory`] として部品に残す ([`replace_shape`])。
//! あとから数値を直す・1 つ消す ([`edit_feature`]・[`remove_feature`]) ときは出発点から作り直す。

pub mod extrude_tool;
pub mod features;
pub mod hole_tool;
pub mod ops;
pub mod record;
pub mod shape_tool;

use std::sync::Arc;

use glam::{DMat4, DVec2, DVec3};

use self::features::{Base, Feature, ShapeHistory};
use crate::history::ModelChange;
use crate::kernel::{BooleanKind, Brep};
use crate::math::euler_deg_to_quat;
use crate::model::{BoxPart, Model, ModeledSource, NodeId, NodeKind, StepSource};
use crate::shape::{ImportedShape, tessellate_breps};

/// B-rep を [`ModeledSource`] に詰める・取り出す (トレイト)。実装は `modeling::record`
/// (`crate::modeling::record` の JSON)
pub trait SolidStore {
    /// 形と、その形に至る編集履歴を詰める
    fn save(
        &self,
        name: String,
        solid: &Brep,
        history: &ShapeHistory,
    ) -> Result<ModeledSource, String>;
    fn load(&self, source: &ModeledSource) -> Result<Brep, String>;
    /// 詰めてある編集履歴 (無ければ空)
    fn history(&self, source: &ModeledSource) -> ShapeHistory;
}

/// カーネルの +Z 向きの形を、Y が上のモデルの座標に合わせる回転
fn z_to_y() -> DMat4 {
    DMat4::from_rotation_x(-std::f64::consts::FRAC_PI_2)
}

/// 底面の中心を原点に置いた、Y 方向に立つ円柱
pub fn cylinder(radius: f64, height: f64) -> Result<Brep, String> {
    Brep::cylinder_z(radius, height)?.transformed(z_to_y())
}

/// 床の上の多角形 (上から見た X, Z ではなく、図面と同じ「右 +X・上 -Z」の (u, v)) を Y 方向に押し出す
pub fn extrude(outline: &[DVec2], height: f64) -> Result<Brep, String> {
    let mut points: Vec<DVec2> = Vec::new();
    for &p in outline {
        if points.last().is_none_or(|q| q.distance(p) > 1e-9) {
            points.push(p);
        }
    }
    if points.len() > 2 && points[0].distance(*points.last().unwrap_or(&points[0])) < 1e-9 {
        points.pop();
    }
    if points.len() < 3 {
        return Err("頂点を3つ以上指定してください".into());
    }
    if height <= 0.0 {
        return Err("高さは 0 より大きくしてください".into());
    }
    let area: f64 = (0..points.len())
        .map(|i| points[i].perp_dot(points[(i + 1) % points.len()]))
        .sum::<f64>()
        / 2.0;
    if area.abs() < 1e-12 {
        return Err("面積のない形は押し出せません".into());
    }
    let vertices: Vec<DVec3> = points.iter().map(|p| DVec3::new(p.x, 0.0, -p.y)).collect();
    Brep::extrude(&vertices, DVec3::new(0.0, height, 0.0)).map_err(|_| {
        "多角形の面を作れませんでした (辺が交差していないか確かめてください)".to_owned()
    })
}

/// "0,0; 100,0; 100,50" のような文字から多角形の頂点を読む
pub fn parse_outline(text: &str) -> Result<Vec<DVec2>, String> {
    text.split([';', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|pair| {
            let nums: Vec<f64> = pair
                .split([',', ' ', '\t'])
                .filter(|s| !s.is_empty())
                .map(|s| {
                    s.parse::<f64>()
                        .map_err(|_| format!("数として読めません: {s}"))
                })
                .collect::<Result<_, _>>()?;
            match nums.as_slice() {
                [x, y] => Ok(DVec2::new(*x, *y)),
                _ => Err(format!("「x, y」の形で書いてください: {pair}")),
            }
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanOp {
    Union,
    Subtract,
    Intersect,
}

impl BooleanOp {
    pub fn label(self) -> &'static str {
        match self {
            BooleanOp::Union => "和 (くっつける)",
            BooleanOp::Subtract => "差 (くり抜く)",
            BooleanOp::Intersect => "積 (重なりだけ残す)",
        }
    }
}

/// 2つの形状のブーリアン
pub fn boolean(a: &Brep, b: &Brep, op: BooleanOp) -> Result<Brep, String> {
    let kind = match op {
        BooleanOp::Union => BooleanKind::Union,
        BooleanOp::Subtract => BooleanKind::Difference,
        BooleanOp::Intersect => BooleanKind::Intersection,
    };
    a.boolean(b, kind).map_err(|_| {
        "演算できませんでした。面や辺がぴったり重なっているときは、少しずらしてから試してください"
            .to_owned()
    })
}

/// B-rep を表示用のメッシュと稜線にする
pub fn shape_from_solid(solid: &Brep) -> ImportedShape {
    let (faces, edges) = tessellate_breps(std::slice::from_ref(solid), display_tolerance(solid));
    ImportedShape::new(Vec::new(), faces, edges, None)
}

/// 表示用にメッシュにするときの、曲面からのずれの上限 (形の大きさの 0.05%)
pub(crate) fn display_tolerance(solid: &Brep) -> f64 {
    let size = solid
        .loose_bounds()
        .map_or(1.0, |b| (b.max - b.min).length());
    (size * 0.0005).max(1e-6)
}

/// 部品の形状を、部品の座標系の B-rep で取り出す (箱とモデリングで作った部品だけ)
pub fn node_solid(model: &Model, store: &dyn SolidStore, id: NodeId) -> Result<Brep, String> {
    let node = model.get(id).ok_or("部品が見つかりません")?;
    match &node.kind {
        NodeKind::Box(part) => part.build_solid(),
        NodeKind::Imported {
            source: StepSource::Modeled(source),
            ..
        } => store.load(source),
        NodeKind::Imported { .. } => Err(format!(
            "「{}」は取り込んだ部品なので演算できません。箱か、作成メニューで作った部品を選んでください",
            node.name
        )),
        NodeKind::Assembly { .. } => Err("アセンブリではなく部品を選んでください".into()),
        NodeKind::Standard(_) | NodeKind::StandardBody { .. } => Err(format!(
            "「{}」は標準部品なので演算できません。寸法はプロパティの「標準部品」欄で変えてください",
            node.name
        )),
    }
}

/// 新しい部品として加える (最上位、位置 `position`)。加えたノードの id を返す
pub fn add_solid(
    model: &mut Model,
    store: &dyn SolidStore,
    name: String,
    solid: &Brep,
    position: DVec3,
) -> Result<NodeId, String> {
    let source = Arc::new(store.save(name.clone(), solid, &ShapeHistory::default())?);
    let shape = Arc::new(shape_from_solid(solid));
    Ok(model.add_node(
        None,
        name.clone(),
        DMat4::from_translation(position),
        NodeKind::Imported {
            product: name,
            shape,
            source: StepSource::Modeled(source),
        },
    ))
}

/// 部品 `id` の今の編集履歴。出発点を覚えていない部品は、今の形を出発点にする。
/// 箱は [`normalize_box`] のあとで呼ぶ (原点を中心にした箱が出発点になる)
pub fn history_of(
    model: &Model,
    store: &dyn SolidStore,
    id: NodeId,
) -> Result<ShapeHistory, String> {
    let node = model.get(id).ok_or("部品が見つかりません")?;
    match &node.kind {
        NodeKind::Box(part) => Ok(ShapeHistory {
            base: Some(Base::Box { size: part.size }),
            features: Vec::new(),
        }),
        NodeKind::Imported {
            source: StepSource::Modeled(source),
            ..
        } => {
            let mut history = store.history(source);
            if history.base.is_none() {
                history.base = Some(Base::Solid {
                    smlib: store.load(source)?.to_text()?,
                });
            }
            Ok(history)
        }
        _ => Ok(ShapeHistory::default()),
    }
}

/// 部品 `id` の形を `solid` (部品の座標系) に置き換え、そこに至る編集履歴 `history` を残す。
/// 箱は配置を部品の置き場所に移す
fn replace_shape(
    model: &mut Model,
    store: &dyn SolidStore,
    id: NodeId,
    solid: &Brep,
    history: &ShapeHistory,
) -> Result<(), String> {
    let node = model.get(id).ok_or("部品が見つかりません")?;
    let name = match &node.kind {
        NodeKind::Imported { product, .. } => product.clone(),
        _ => node.name.clone(),
    };
    let source = Arc::new(store.save(name.clone(), solid, history)?);
    let shape = Arc::new(shape_from_solid(solid));
    let node = model.get_mut(id).ok_or("部品が見つかりません")?;
    node.kind = NodeKind::Imported {
        product: name,
        shape,
        source: StepSource::Modeled(source),
    };
    model.dirty.insert(id);
    Ok(())
}

/// 箱は寸法と配置をパラメータで持つので、原点を中心にした形と配置に分けてから演算する
fn normalize_box(model: &mut Model, id: NodeId) {
    let Some(node) = model.get(id) else { return };
    let NodeKind::Box(part) = &node.kind else {
        return;
    };
    let local = DMat4::from_rotation_translation(
        euler_deg_to_quat(part.rotation_deg),
        DVec3::from(part.position),
    );
    let centered = BoxPart {
        size: part.size,
        position: [0.0; 3],
        rotation_deg: [0.0; 3],
    };
    let placement = node.placement * local;
    if let Some(node) = model.get_mut(id) {
        node.placement = placement;
        node.kind = NodeKind::Box(centered);
    }
}

/// 穴の向き
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HoleAxis {
    X,
    Y,
    Z,
}

impl HoleAxis {
    pub fn label(self) -> &'static str {
        match self {
            HoleAxis::X => "X (左右)",
            HoleAxis::Y => "Y (上下)",
            HoleAxis::Z => "Z (前後)",
        }
    }

    fn dir(self) -> DVec3 {
        match self {
            HoleAxis::X => DVec3::X,
            HoleAxis::Y => DVec3::Y,
            HoleAxis::Z => DVec3::Z,
        }
    }
}

/// 穴の指定 (ワールド座標)
#[derive(Clone, Copy, Debug)]
pub struct HoleSpec {
    /// 穴の中心線が通る点。深さを指定するときは入口の中心
    pub center: DVec3,
    pub axis: HoleAxis,
    pub diameter: f64,
    /// 0 なら貫通。正なら軸の + 向きから、負なら - 向きから掘る深さ
    pub depth: f64,
}

/// 部品に穴をあける。元に戻すための変更を返す
pub fn drill_hole(
    model: &mut Model,
    store: &dyn SolidStore,
    id: NodeId,
    hole: &HoleSpec,
) -> Result<ModelChange, String> {
    if hole.diameter <= 0.0 {
        return Err("穴の直径は 0 より大きくしてください".into());
    }
    node_solid(model, store, id)?;
    let before = model.snapshot(id);
    normalize_box(model, id);
    let solid = node_solid(model, store, id)?;
    let world = model.world_matrix(id);
    let to_local = world.inverse();
    let bounds = model.world_bounds(id).ok_or("部品の大きさが分かりません")?;
    let dir = hole.axis.dir();
    // 貫通なら部品の外まで、深さ指定なら入口から深さぶん。
    // 面がぴったり重なると演算できないので、外側へ少しはみ出させる
    let margin = bounds.size().max_element() * 0.05 + hole.diameter;
    let (start, drill_dir, length) = if hole.depth == 0.0 {
        let lo = bounds.min.dot(dir) - margin;
        let hi = bounds.max.dot(dir) + margin;
        (
            hole.center + dir * (lo - hole.center.dot(dir)),
            dir,
            hi - lo,
        )
    } else if hole.depth > 0.0 {
        // + 側の入口から - 向きに掘る
        (hole.center + dir * margin, -dir, hole.depth + margin)
    } else {
        (hole.center - dir * margin, dir, -hole.depth + margin)
    };
    // 穴を部品の座標系で覚える (あとから直せるように)
    let local_start = to_local.transform_point3(start);
    let local_end = to_local.transform_point3(start + drill_dir * length);
    let local_length = local_start.distance(local_end);
    let feature = Feature::Hole {
        start: local_start.to_array(),
        dir: ((local_end - local_start) / local_length).to_array(),
        length: local_length,
        diameter: hole.diameter * local_length / length,
    };
    let result = history_of(model, store, id).and_then(|mut history| {
        history.features.push(feature);
        feature_result(&history, &solid)
    });
    let (result, history) = match result {
        Ok(r) => r,
        Err(e) => {
            if let Some(b) = &before {
                model.restore(b);
            }
            return Err(e);
        }
    };
    replace_shape(model, store, id, &result, &history)?;
    let name = model.get(id).map(|n| n.name.clone()).unwrap_or_default();
    Ok(ModelChange {
        label: format!("{name} に穴をあける"),
        changes: vec![(before, model.snapshot(id))],
    })
}

/// 履歴の最後の編集を `solid` にかける。履歴ごと返す
fn feature_result(history: &ShapeHistory, solid: &Brep) -> Result<(Brep, ShapeHistory), String> {
    let last = history.features.last().ok_or("編集がありません")?;
    Ok((last.apply(solid)?, history.clone()))
}

/// `start` から `dir` 方向へ長さ `length` の円柱 (ワールド)
fn oriented_cylinder(radius: f64, length: f64, start: DVec3, dir: DVec3) -> Result<Brep, String> {
    let base = cylinder(radius, length)?;
    let rotation = glam::DQuat::from_rotation_arc(DVec3::Y, dir.normalize());
    base.transformed(DMat4::from_rotation_translation(rotation, start))
}

/// 部品 `a` と `b` のブーリアン。結果は `a` に入れ、`b` は消す。元に戻すための変更を返す
pub fn apply_boolean(
    model: &mut Model,
    store: &dyn SolidStore,
    a: NodeId,
    b: NodeId,
    op: BooleanOp,
) -> Result<ModelChange, String> {
    if a == b {
        return Err("違う部品を2つ選んでください".into());
    }
    node_solid(model, store, a)?;
    let solid_b = node_solid(model, store, b)?;
    let before_a = model.snapshot(a);
    let before_b = model.snapshot(b);
    normalize_box(model, a);
    let solid_a = node_solid(model, store, a)?;
    // b を a の座標系に持ってくる
    let b_to_a = model.world_matrix(a).inverse() * model.world_matrix(b);
    let result = solid_b.transformed(b_to_a).and_then(|solid_b| {
        let mut history = history_of(model, store, a)?;
        history.features.push(Feature::Boolean {
            other: solid_b.to_text()?,
            op,
        });
        feature_result(&history, &solid_a)
    });
    let (result, history) = match result {
        Ok(r) => r,
        Err(e) => {
            if let Some(s) = &before_a {
                model.restore(s);
            }
            return Err(e);
        }
    };
    replace_shape(model, store, a, &result, &history)?;
    let names = [a, b].map(|id| model.get(id).map(|n| n.name.clone()).unwrap_or_default());
    model.remove(b);
    Ok(ModelChange {
        label: format!("{} と {} の{}", names[0], names[1], op.label()),
        changes: vec![(before_a, model.snapshot(a)), (before_b, None)],
    })
}

/// 部品 `id` の編集履歴を `change` で書き換え、出発点から形を作り直す。元に戻すための変更を返す
fn rebuild_with(
    model: &mut Model,
    store: &dyn SolidStore,
    id: NodeId,
    what: &str,
    change: impl FnOnce(&mut ShapeHistory) -> Result<(), String>,
) -> Result<ModelChange, String> {
    let mut history = history_of(model, store, id)?;
    if !history.editable() {
        return Err("この部品は形の記録を持っていないので編集できません".into());
    }
    change(&mut history)?;
    let solid = history.rebuild()?;
    let before = model.snapshot(id);
    normalize_box(model, id);
    replace_shape(model, store, id, &solid, &history)?;
    let name = model.get(id).map(|n| n.name.clone()).unwrap_or_default();
    Ok(ModelChange {
        label: format!("{name} の{what}"),
        changes: vec![(before, model.snapshot(id))],
    })
}

/// 編集履歴の `index` 番目を `feature` に差し替えて形を作り直す
pub fn edit_feature(
    model: &mut Model,
    store: &dyn SolidStore,
    id: NodeId,
    index: usize,
    feature: Feature,
) -> Result<ModelChange, String> {
    let what = format!("{}を直す", feature.label());
    rebuild_with(model, store, id, &what, |history| {
        *history
            .features
            .get_mut(index)
            .ok_or("その編集は見つかりません")? = feature;
        Ok(())
    })
}

/// 編集履歴の `index` 番目を消して形を作り直す
pub fn remove_feature(
    model: &mut Model,
    store: &dyn SolidStore,
    id: NodeId,
    index: usize,
) -> Result<ModelChange, String> {
    let label = history_of(model, store, id)?
        .features
        .get(index)
        .map(Feature::label)
        .ok_or("その編集は見つかりません")?;
    rebuild_with(model, store, id, &format!("{label}を消す"), |history| {
        history.features.remove(index);
        Ok(())
    })
}

/// 編集履歴の出発点を `base` に差し替えて形を作り直す (箱の寸法を直す)
pub fn edit_base(
    model: &mut Model,
    store: &dyn SolidStore,
    id: NodeId,
    base: Base,
) -> Result<ModelChange, String> {
    rebuild_with(model, store, id, "出発点の形を直す", |history| {
        history.base = Some(base);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用: B-rep と履歴をそのまま覚えておく
    #[derive(Default)]
    pub(super) struct MemoryStore(std::sync::Mutex<Vec<(Brep, ShapeHistory)>>);

    impl SolidStore for MemoryStore {
        fn save(
            &self,
            name: String,
            solid: &Brep,
            history: &ShapeHistory,
        ) -> Result<ModeledSource, String> {
            let mut solids = self.0.lock().unwrap();
            solids.push((solid.clone(), history.clone()));
            Ok(ModeledSource {
                id: crate::shape::next_shape_id(),
                name,
                json: (solids.len() - 1).to_string(),
            })
        }

        fn load(&self, source: &ModeledSource) -> Result<Brep, String> {
            let i: usize = source.json.parse().map_err(|_| "?".to_owned())?;
            Ok(self.0.lock().unwrap()[i].0.clone())
        }

        fn history(&self, source: &ModeledSource) -> ShapeHistory {
            source
                .json
                .parse::<usize>()
                .ok()
                .and_then(|i| self.0.lock().unwrap().get(i).map(|s| s.1.clone()))
                .unwrap_or_default()
        }
    }

    /// 向きをそろえた三角形から求めた符号付き体積 (外向きなら正)
    fn signed_volume(shape: &ImportedShape) -> f64 {
        shape
            .faces
            .iter()
            .flat_map(|poly| poly.triangle_points())
            .map(|[a, b, c]| a.dot(b.cross(c)) / 6.0)
            .sum()
    }

    #[test]
    fn cylinder_volume_and_orientation() {
        let solid = cylinder(10.0, 50.0).unwrap();
        let shape = shape_from_solid(&solid);
        let expected = std::f64::consts::PI * 100.0 * 50.0;
        let v = signed_volume(&shape);
        assert!((v - expected).abs() / expected < 0.01, "{v} {expected}");
        assert!((shape.stats.max.y - 50.0).abs() < 1e-9);
        assert!(cylinder(0.0, 1.0).is_err());
    }

    #[test]
    fn extrude_either_winding() {
        let square = parse_outline("0,0; 100,0; 100,50; 0,50").unwrap();
        for outline in [square.clone(), square.iter().rev().copied().collect()] {
            let shape = shape_from_solid(&extrude(&outline, 20.0).unwrap());
            let v = signed_volume(&shape);
            assert!((v - 100_000.0).abs() < 1e-6, "{v}");
            // 図面の上 (+v) はワールドの -Z
            assert!((shape.stats.min.z + 50.0).abs() < 1e-9);
        }
        assert!(parse_outline("0,0; 1").is_err());
        assert!(extrude(&square[..2], 1.0).is_err());
    }

    #[test]
    fn hole_through_box_removes_volume() {
        let mut model = Model::default();
        let store = MemoryStore::default();
        let id = model.add_box([100.0, 40.0, 100.0], [200.0, 20.0, 0.0]);
        let change = drill_hole(
            &mut model,
            &store,
            id,
            &HoleSpec {
                center: DVec3::new(200.0, 0.0, 0.0),
                axis: HoleAxis::Y,
                diameter: 20.0,
                depth: 0.0,
            },
        )
        .unwrap();
        let NodeKind::Imported { shape, .. } = &model.get(id).unwrap().kind else {
            panic!("穴をあけた箱は形状の部品になる");
        };
        let expected = 100.0 * 40.0 * 100.0 - std::f64::consts::PI * 100.0 * 40.0;
        let v = signed_volume(shape);
        assert!((v - expected).abs() / expected < 0.01, "{v} {expected}");
        // 位置は変わらない
        let b = model.world_bounds(id).unwrap();
        assert!((b.center() - DVec3::new(200.0, 20.0, 0.0)).length() < 1e-6);
        assert!(change.label.contains("穴"));
        change.undo(&mut model);
        assert!(matches!(model.get(id).unwrap().kind, NodeKind::Box(_)));
    }

    #[test]
    fn boolean_union() {
        let mut model = Model::default();
        let store = MemoryStore::default();
        let a = model.add_box([100.0, 100.0, 100.0], [0.0, 50.0, 0.0]);
        let cyl = cylinder(20.0, 160.0).unwrap();
        let b = add_solid(
            &mut model,
            &store,
            "円柱".into(),
            &cyl,
            DVec3::new(10.0, -30.0, 7.0),
        )
        .unwrap();
        let change = apply_boolean(&mut model, &store, a, b, BooleanOp::Union).unwrap();
        assert!(model.get(b).is_none());
        let NodeKind::Imported { shape, .. } = &model.get(a).unwrap().kind else {
            panic!();
        };
        let cube = 1_000_000.0;
        let outside = std::f64::consts::PI * 400.0 * 60.0;
        let v = signed_volume(shape);
        assert!((v - (cube + outside)).abs() / cube < 0.02, "{v}");
        // 戻すと円柱が戻る
        change.undo(&mut model);
        assert!(model.get(b).is_some());
        // 無い部品は演算できない
        assert!(node_solid(&model, &store, 999).is_err());
    }

    #[test]
    fn hole_can_be_edited_and_removed_later() {
        let mut model = Model::default();
        let store = MemoryStore::default();
        let id = model.add_box([100.0, 40.0, 100.0], [200.0, 20.0, 0.0]);
        let hole = HoleSpec {
            center: DVec3::new(210.0, 0.0, 5.0),
            axis: HoleAxis::Y,
            diameter: 20.0,
            depth: 0.0,
        };
        drill_hole(&mut model, &store, id, &hole).unwrap();
        let history = history_of(&model, &store, id).unwrap();
        assert!(matches!(history.base, Some(Base::Box { .. })));
        assert_eq!(history.features.len(), 1);
        let Feature::Hole {
            start, diameter, ..
        } = &history.features[0]
        else {
            panic!("穴が記録される");
        };
        // 部品の座標系で覚える (箱の中心が原点)
        assert!((start[0] - 10.0).abs() < 1e-6 && (start[2] - 5.0).abs() < 1e-6);
        assert!((diameter - 20.0).abs() < 1e-9);

        // 直径を直す
        let mut edited = history.features[0].clone();
        edited.set_value(40.0).unwrap();
        let change = edit_feature(&mut model, &store, id, 0, edited).unwrap();
        assert!(change.label.contains("穴 φ40"));
        let volume = |model: &Model| {
            let NodeKind::Imported { shape, .. } = &model.get(id).unwrap().kind else {
                panic!()
            };
            shape.stats.volume
        };
        let expected = 400_000.0 - std::f64::consts::PI * 400.0 * 40.0;
        assert!((volume(&model) - expected).abs() / expected < 0.01);
        // 位置は変わらない
        let b = model.world_bounds(id).unwrap();
        assert!((b.center() - DVec3::new(200.0, 20.0, 0.0)).length() < 1e-6);

        // 消すと箱の形に戻る (ノードは形状の部品のまま)
        remove_feature(&mut model, &store, id, 0).unwrap();
        assert!((volume(&model) - 400_000.0).abs() < 1e-3);
        assert!(history_of(&model, &store, id).unwrap().features.is_empty());
        assert!(remove_feature(&mut model, &store, id, 5).is_err());

        // 出発点の箱の寸法も直せる
        edit_base(
            &mut model,
            &store,
            id,
            Base::Box {
                size: [50.0, 40.0, 100.0],
            },
        )
        .unwrap();
        assert!((volume(&model) - 200_000.0).abs() < 1e-3);
        change.undo(&mut model);
    }

    #[test]
    fn parts_without_a_record_start_from_their_current_shape() {
        let mut model = Model::default();
        let store = MemoryStore::default();
        let cyl = cylinder(20.0, 100.0).unwrap();
        let id = add_solid(&mut model, &store, "円柱".into(), &cyl, DVec3::ZERO).unwrap();
        assert!(history_of(&model, &store, id).unwrap().features.is_empty());
        drill_hole(
            &mut model,
            &store,
            id,
            &HoleSpec {
                center: DVec3::new(0.0, 50.0, 0.0),
                axis: HoleAxis::X,
                diameter: 10.0,
                depth: 0.0,
            },
        )
        .unwrap();
        let history = history_of(&model, &store, id).unwrap();
        assert!(matches!(history.base, Some(Base::Solid { .. })));
        assert_eq!(history.features.len(), 1);
        // 作り直しても同じ形になる
        let again = history.rebuild().unwrap();
        let now = node_solid(&model, &store, id).unwrap();
        let (a, b) = (
            again.mass_properties(1.0).unwrap().volume,
            now.mass_properties(1.0).unwrap().volume,
        );
        assert!((a - b).abs() / b < 1e-6, "{a} {b}");
    }
}
