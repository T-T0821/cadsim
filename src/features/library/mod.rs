//! 標準部品ライブラリ
//!
//! - `catalog`: 部品の種類・寸法パラメータ・I/O 点・アクチュエータ動作 (Bevy 非依存)
//! - `geometry`: パラメータから B-rep を作る (Bevy 非依存)
//! - `ui`: 部品パレット (ドラッグで配置)、プロパティ、I/O 一覧、光軸などの表示
//!
//! 配置した部品はモデルの木に `NodeKind::Standard` のノードとして入り、その子に本体
//! (`NodeKind::StandardBody`: チューブ・ロッドなど) を持つ。本体を分けておくと、
//! アクチュエータが動かす単位で表示・干渉チェックができる。部品を動かすときは親の
//! 配置だけを変えればよい (ギズモもプロパティもそうする)。
//!
//! ## 他の機能とのつなぎ方
//!
//! - タイミングチャート: [`Spec::actuators`] の `bodies` が動かす本体の番号。
//!   [`body_node`] で本体のノード id に直せる。`drive` がその動作を起こす出力、
//!   `end_inputs` が原位置 / 動作端の入力。
//! - センサー: [`Spec::sensing`] の光軸 (部品ローカル mm) を [`local_to_world`] でワールド座標にする。
//! - コンベア: [`Spec::conveying_surface`] が搬送面の中心・向き・長さ・幅・速度を返す。
//! - PLC の I/O 割付け: [`io_table`] が全機器の I/O 点を返す。

use plantwright_core::library::{catalog, geometry};
mod panel;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use bevy::math::{DMat4, DQuat, DVec3};
use bevy::prelude::*;

pub use catalog::{IoDir, Motion, PartKind, Spec};
pub use panel::{add_menu, drop_on_viewport, io_list_window, palette_window, properties_section};

use plantwright_core::formats::step::ImportedShape;
use plantwright_core::kernel::Brep;
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::model::NodeKind;
use plantwright_core::shape::tessellate_breps;

/// 標準部品のテッセレーションの許容誤差 (mm)。小さな円柱が多いので細かめにする。
const TESSELLATION_TOL_MM: f64 = 0.3;

pub struct LibraryPlugin;

impl Plugin for LibraryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Jog>()
            .init_resource::<panel::LibraryUiState>()
            .add_systems(Update, (forget_removed_jog, panel::draw_overlays));
    }
}

/// 動作テスト (ジョグ) の値。標準部品のノード id → アクチュエータごとの値 (0..1)。
/// 表示だけを動かし、設計データ (ノードの配置) は変えない。
#[derive(Resource, Default)]
pub struct Jog(pub HashMap<NodeId, Vec<f64>>);

/// 消えた部品のジョグの値を捨てる
fn forget_removed_jog(model: Res<Model>, mut jog: ResMut<Jog>) {
    if model.is_changed() {
        jog.0.retain(|id, _| is_standard(&model, *id));
    }
}

fn is_standard(model: &Model, id: NodeId) -> bool {
    matches!(model.get(id).map(|n| &n.kind), Some(NodeKind::Standard(_)))
}

/// 標準部品を最上位に置き、そのノード id を返す。名前は「エアシリンダ 2」のように通し番号を付ける。
pub fn add_standard(model: &mut Model, kind: PartKind, position: DVec3) -> NodeId {
    let same_kind = standard_nodes(model)
        .into_iter()
        .filter(|&id| {
            matches!(model.get(id).map(|n| &n.kind), Some(NodeKind::Standard(s)) if s.kind == kind)
        })
        .count();
    let name = format!("{} {}", kind.label(), same_kind + 1);
    let id = model.add_node(
        None,
        name,
        DMat4::from_translation(position),
        NodeKind::Standard(Spec::new(kind)),
    );
    rebuild_bodies(model, id);
    id
}

/// 標準部品の本体 (子) をパラメータから作り直す
pub fn rebuild_bodies(model: &mut Model, id: NodeId) {
    let Some(NodeKind::Standard(spec)) = model.get(id).map(|n| n.kind.clone()) else {
        return;
    };
    for child in model
        .get(id)
        .map(|n| n.children.clone())
        .unwrap_or_default()
    {
        model.remove(child);
    }
    let scale = catalog::units_per_mm(&model.unit);
    for (index, (name, shape)) in body_shapes(&spec, scale).into_iter().enumerate() {
        model.add_node(
            Some(id),
            name.into(),
            DMat4::IDENTITY,
            NodeKind::StandardBody { body: index, shape },
        );
    }
}

/// 同じ種類・パラメータ・単位の形状は使い回す (表示のメッシュも共有される)
type ShapeKey = (PartKind, Vec<u64>, u64);
type BodyShapes = Vec<(&'static str, Arc<ImportedShape>)>;
static SHAPE_CACHE: LazyLock<Mutex<HashMap<ShapeKey, BodyShapes>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn body_shapes(spec: &Spec, scale: f64) -> BodyShapes {
    let key = (
        spec.kind,
        spec.params.0.iter().map(|v| v.to_bits()).collect(),
        scale.to_bits(),
    );
    let mut cache = SHAPE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(key)
        .or_insert_with(|| {
            geometry::bodies(spec)
                .into_iter()
                .map(|body| {
                    let solid = if scale == 1.0 {
                        body.solid
                    } else {
                        let s = DMat4::from_scale(DVec3::splat(scale));
                        body.solid.transformed(s).expect("拡大縮小できる")
                    };
                    let shape = tessellate(&solid, body.color, TESSELLATION_TOL_MM * scale);
                    (body.name, Arc::new(shape))
                })
                .collect()
        })
        .clone()
}

/// ソリッドを表示用の面メッシュと稜線にする
fn tessellate(solid: &Brep, color: [f32; 3], tol: f64) -> ImportedShape {
    let (faces, edges) = tessellate_breps(std::slice::from_ref(solid), tol);
    ImportedShape::new(Vec::new(), faces, edges, Some(color))
}

/// モデルにある標準部品のノード (ツリーの表示順)
pub fn standard_nodes(model: &Model) -> Vec<NodeId> {
    model
        .all_nodes()
        .into_iter()
        .filter(|&id| is_standard(model, id))
        .collect()
}

/// 標準部品 `id` の本体 `body` のノード id
// タイミングチャートがアクチュエータの動かす本体を引くときに使う (いまはテストだけ)
#[allow(dead_code)]
pub fn body_node(model: &Model, id: NodeId, body: usize) -> Option<NodeId> {
    model.get(id)?.children.iter().copied().find(|&c| {
        matches!(
            model.get(c).map(|n| &n.kind),
            Some(NodeKind::StandardBody { body: b, .. }) if *b == body
        )
    })
}

/// 標準部品 `id` のローカル座標 (mm) の点をワールド座標にする
pub fn local_to_world(model: &Model, id: NodeId, local_mm: [f64; 3]) -> DVec3 {
    let scale = catalog::units_per_mm(&model.unit);
    model
        .world_matrix(id)
        .transform_point3(DVec3::from(local_mm) * scale)
}

/// 標準部品 `id` のローカル座標の向きをワールド座標の向きにする
pub fn dir_to_world(model: &Model, id: NodeId, local: [f64; 3]) -> DVec3 {
    model
        .world_matrix(id)
        .transform_vector3(DVec3::from(local))
        .normalize_or_zero()
}

/// アクチュエータの値 `value` のときの本体の動き (部品ローカル座標、モデルの単位)
pub fn motion_matrix(motion: &Motion, value: f64, scale: f64) -> DMat4 {
    let (offset_mm, angle_deg) = motion.displacement(value);
    let translation = DMat4::from_translation(DVec3::from(offset_mm) * scale);
    match motion {
        Motion::Rotary { axis, pivot, .. } if angle_deg != 0.0 => {
            let pivot = DVec3::from(*pivot) * scale;
            let axis = DVec3::from(*axis).normalize();
            let rotation = DQuat::from_axis_angle(axis, angle_deg.to_radians());
            // pivot まわりに回す
            translation
                * DMat4::from_translation(pivot)
                * DMat4::from_quat(rotation)
                * DMat4::from_translation(-pivot)
        }
        _ => translation,
    }
}

/// 表示に使うワールド座標の配置。標準部品の本体なら動作テストで動かした分を足す。
pub fn display_matrix(model: &Model, jog: &Jog, id: NodeId) -> DMat4 {
    let world = model.world_matrix(id);
    let Some(node) = model.get(id) else {
        return world;
    };
    let (NodeKind::StandardBody { body, .. }, Some(parent)) = (&node.kind, node.parent) else {
        return world;
    };
    let (Some(values), Some(NodeKind::Standard(spec))) =
        (jog.0.get(&parent), model.get(parent).map(|n| &n.kind))
    else {
        return world;
    };
    let scale = catalog::units_per_mm(&model.unit);
    let offset = spec
        .actuators()
        .iter()
        .zip(values)
        .find(|(act, _)| act.bodies.contains(body))
        .map_or(DMat4::IDENTITY, |(act, &v)| {
            motion_matrix(&act.motion, v, scale)
        });
    // 親の座標系で動かしてから、本体の配置を掛ける
    model.world_matrix(parent) * offset * node.placement
}

/// I/O 一覧の1行
#[derive(Clone, Debug, PartialEq)]
pub struct IoRow {
    pub node: NodeId,
    pub device: String,
    pub point: String,
    pub dir: IoDir,
}

/// 現場機器 (シーケンサー本体とリモート I/O 以外) の I/O 点すべて
pub fn io_table(model: &Model) -> Vec<IoRow> {
    let mut rows = Vec::new();
    for id in standard_nodes(model) {
        let Some(node) = model.get(id) else {
            continue;
        };
        let NodeKind::Standard(spec) = &node.kind else {
            continue;
        };
        if spec.is_io_host() {
            continue;
        }
        rows.extend(spec.io_points().into_iter().map(|p| IoRow {
            node: id,
            device: node.name.clone(),
            point: p.name,
            dir: p.dir,
        }));
    }
    rows
}

/// シーケンサー本体とリモート I/O が持つ I/O 点の数 (入力, 出力)
pub fn io_capacity(model: &Model) -> (usize, usize) {
    let mut cap = (0, 0);
    for id in standard_nodes(model) {
        let Some(NodeKind::Standard(spec)) = model.get(id).map(|n| &n.kind) else {
            continue;
        };
        if !spec.is_io_host() {
            continue;
        }
        for p in spec.io_points() {
            match p.dir {
                IoDir::Input => cap.0 += 1,
                IoDir::Output => cap.1 += 1,
            }
        }
    }
    cap
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_of(model: &Model, id: NodeId) -> Spec {
        match model.get(id).map(|n| &n.kind) {
            Some(NodeKind::Standard(spec)) => spec.clone(),
            _ => panic!("標準部品ではない"),
        }
    }

    fn shape_id(model: &Model, id: NodeId) -> usize {
        match model.get(id).map(|n| &n.kind) {
            Some(NodeKind::StandardBody { shape, .. }) => shape.id,
            _ => panic!("本体ではない"),
        }
    }

    #[test]
    fn add_standard_creates_one_child_per_body() {
        let mut model = Model::default();
        let id = add_standard(&mut model, PartKind::BeltConveyor, DVec3::ZERO);
        let node = model.get(id).unwrap();
        assert_eq!(node.name, "ベルトコンベア 1");
        let bodies = geometry::bodies(&spec_of(&model, id)).len();
        assert_eq!(node.children.len(), bodies);
        assert_eq!(model.measure(id).part_count, bodies);
        let second = add_standard(&mut model, PartKind::BeltConveyor, DVec3::ZERO);
        assert_eq!(model.get(second).unwrap().name, "ベルトコンベア 2");
    }

    #[test]
    fn clicking_a_body_picks_the_whole_part() {
        let mut model = Model::default();
        let id = add_standard(&mut model, PartKind::AirCylinder, DVec3::ZERO);
        let rod = body_node(&model, id, catalog::body::CYL_ROD).unwrap();
        assert_eq!(model.pick_target(rod), id);
        assert_eq!(model.pick_target(id), id);
    }

    #[test]
    fn rebuild_follows_body_count_and_reuses_shapes() {
        let mut model = Model::default();
        let a = add_standard(&mut model, PartKind::Plc, DVec3::ZERO);
        let b = add_standard(&mut model, PartKind::Plc, DVec3::X);
        // 同じパラメータなら形状を共有する
        assert_eq!(
            shape_id(&model, body_node(&model, a, 1).unwrap()),
            shape_id(&model, body_node(&model, b, 1).unwrap())
        );
        let before = model.get(a).unwrap().children.len();
        if let Some(NodeKind::Standard(spec)) = model.get_mut(a).map(|n| &mut n.kind) {
            spec.params.0 = vec![64.0, 64.0];
        }
        rebuild_bodies(&mut model, a);
        assert_eq!(model.get(a).unwrap().children.len(), before + 6);
    }

    #[test]
    fn bodies_are_scaled_to_the_model_unit() {
        let mut model = Model::default();
        model.unit = "m".into();
        let id = add_standard(&mut model, PartKind::BeltConveyor, DVec3::ZERO);
        let bounds = model.world_bounds(id).unwrap();
        // 搬送面の高さ 700 mm = 0.7 m (フレームが 15 mm 上に出る)
        assert!((bounds.max.y - 0.715).abs() < 1e-6, "{}", bounds.max.y);
    }

    #[test]
    fn jog_moves_only_the_actuated_bodies() {
        let mut model = Model::default();
        let id = add_standard(
            &mut model,
            PartKind::AirCylinder,
            DVec3::new(10.0, 0.0, 0.0),
        );
        let mut jog = Jog::default();
        jog.0.insert(id, vec![0.5]);
        let rod = body_node(&model, id, catalog::body::CYL_ROD).unwrap();
        let tube = body_node(&model, id, 0).unwrap();
        let moved = display_matrix(&model, &jog, rod).transform_point3(DVec3::ZERO);
        assert!(moved.distance(DVec3::new(60.0, 0.0, 0.0)) < 1e-9);
        assert_eq!(display_matrix(&model, &jog, tube), model.world_matrix(tube));
    }

    #[test]
    fn rotary_jog_keeps_the_pivot_fixed() {
        let spec = Spec::new(PartKind::Motor);
        let act = &spec.actuators()[0];
        let Motion::Rotary { pivot, .. } = act.motion else {
            panic!("モーターは回転");
        };
        let m = motion_matrix(&act.motion, 0.3, 1.0);
        let p = DVec3::from(pivot);
        assert!(m.transform_point3(p).distance(p) < 1e-9);
    }

    #[test]
    fn io_table_lists_field_devices_and_capacity() {
        let mut model = Model::default();
        add_standard(&mut model, PartKind::Plc, DVec3::ZERO);
        add_standard(&mut model, PartKind::AirCylinder, DVec3::ZERO);
        add_standard(&mut model, PartKind::PhotoSensor, DVec3::ZERO);
        let rows = io_table(&model);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].device, "エアシリンダ 1");
        assert_eq!(rows[3].point, "検出");
        assert_eq!(io_capacity(&model), (16, 16));
    }

    #[test]
    fn sensing_origin_follows_the_part_placement() {
        let mut model = Model::default();
        let id = add_standard(
            &mut model,
            PartKind::PhotoSensor,
            DVec3::new(100.0, 0.0, 0.0),
        );
        let s = Spec::new(PartKind::PhotoSensor).sensing().unwrap();
        assert_eq!(
            local_to_world(&model, id, s.origin),
            DVec3::new(115.0, 25.0, 0.0)
        );
        assert_eq!(dir_to_world(&model, id, s.direction), DVec3::X);
    }

    #[test]
    fn standard_parts_are_counted_as_whole_parts() {
        use bevy::math::DVec3;
        use plantwright_core::bom::make_bom;
        let mut model = Model::default();
        add_standard(&mut model, PartKind::AirCylinder, DVec3::ZERO);
        add_standard(&mut model, PartKind::AirCylinder, DVec3::X * 500.0);
        let bom = make_bom(&model);
        // 本体 (チューブ・ロッドなど) は数えず、同じ寸法のシリンダ2本で1行
        assert_eq!(bom.rows.len(), 1);
        assert_eq!(bom.rows[0].quantity, 2);
        assert_eq!(bom.rows[0].kind, "標準部品");
        assert!(bom.rows[0].size.iter().all(|&v| v > 0.0));
    }
}
