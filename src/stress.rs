//! 負荷テスト用のシーン: 同じ形の設備ユニットを格子状に並べ、数千部品のアセンブリを作る
//!
//! 実際の工場ラインのデータに近づけるため、少ない種類の形状 (板・脚・モーター・ローラー・ボルト)
//! を何度も使い回す。形状は1種類につき1回だけテッセレーションし、インスタンスで共有する。
//! `cargo run --release -- --stress 5000` やメニューの「負荷テスト用シーン」で開く。

use std::f64::consts::TAU;
use std::sync::Arc;

use bevy::math::{DMat4, DVec3};
use truck_meshalgo::prelude::*;
use truck_modeling::{Solid, builder};

use plantwright_core::formats::step::ImportNode;
use plantwright_core::formats::step::ImportedFile;
use plantwright_core::formats::step::ImportedShape;
use plantwright_core::formats::step::tessellation_tolerance;
use plantwright_core::part_mesh::shell_edges;

/// 1ユニットの部品数 (板1 + 脚4 + モーター1 + ローラー8 + ボルト16)
pub const PARTS_PER_STATION: usize = 30;
/// 横に並べるユニットの数
const STATIONS_PER_ROW: usize = 10;

/// 部品数が `part_count` 個以上になるだけユニットを並べたシーンを作る
pub fn stress_scene(part_count: usize) -> ImportedFile {
    let shapes = Shapes::new();
    let stations = part_count.div_ceil(PARTS_PER_STATION).max(1);
    let lines: Vec<ImportNode> = (0..stations.div_ceil(STATIONS_PER_ROW))
        .map(|row| {
            let in_row = (stations - row * STATIONS_PER_ROW).min(STATIONS_PER_ROW);
            let children = (0..in_row)
                .map(|col| {
                    let offset = DVec3::new(col as f64 * 800.0, 0.0, row as f64 * 900.0);
                    station(&shapes, format!("Station_{row}_{col}"), offset)
                })
                .collect();
            assembly(format!("Line_{row}"), DMat4::IDENTITY, children)
        })
        .collect();
    ImportedFile {
        root: assembly(
            format!("負荷テスト ({stations} ユニット)"),
            DMat4::IDENTITY,
            lines,
        ),
        unit: "mm".into(),
        warnings: Vec::new(),
        animations: Vec::new(),
    }
}

/// 使い回す形状
struct Shapes {
    plate: Arc<ImportedShape>,
    leg: Arc<ImportedShape>,
    motor: Arc<ImportedShape>,
    roller: Arc<ImportedShape>,
    bolt: Arc<ImportedShape>,
}

impl Shapes {
    fn new() -> Self {
        let shape = |solids: Vec<Solid>, rgb: [f32; 3]| Arc::new(tessellate(&solids, rgb));
        Self {
            plate: shape(vec![cuboid([600.0, 20.0, 400.0])], [0.62, 0.66, 0.70]),
            leg: shape(vec![cuboid([40.0, 500.0, 40.0])], [0.45, 0.48, 0.52]),
            motor: shape(
                vec![
                    cylinder(50.0, 120.0, DVec3::Y, DVec3::ZERO),
                    cylinder(8.0, 40.0, DVec3::Y, DVec3::new(0.0, 120.0, 0.0)),
                ],
                [0.25, 0.42, 0.75],
            ),
            roller: shape(
                vec![cylinder(
                    25.0,
                    380.0,
                    DVec3::Z,
                    DVec3::new(0.0, 0.0, -190.0),
                )],
                [0.80, 0.80, 0.78],
            ),
            bolt: shape(
                vec![
                    cylinder(5.0, 30.0, DVec3::Y, DVec3::new(0.0, -30.0, 0.0)),
                    cylinder(9.0, 6.0, DVec3::Y, DVec3::ZERO),
                ],
                [0.20, 0.20, 0.22],
            ),
        }
    }
}

/// 脚の付いた台にモーターとローラーを載せ、ボルトで留めた1ユニット (30部品)
fn station(shapes: &Shapes, name: String, offset: DVec3) -> ImportNode {
    let mut children = Vec::with_capacity(PARTS_PER_STATION);
    let top = 510.0;
    children.push(part("Plate", &shapes.plate, DVec3::new(0.0, 500.0, 0.0)));
    for (i, (x, z)) in [
        (-270.0, -170.0),
        (270.0, -170.0),
        (-270.0, 170.0),
        (270.0, 170.0),
    ]
    .into_iter()
    .enumerate()
    {
        children.push(part(
            &format!("Leg_{i}"),
            &shapes.leg,
            DVec3::new(x, 240.0, z),
        ));
    }
    children.push(part("Motor", &shapes.motor, DVec3::new(-220.0, top, 0.0)));
    for i in 0..8 {
        let x = -120.0 + i as f64 * 55.0;
        children.push(part(
            &format!("Roller_{i}"),
            &shapes.roller,
            DVec3::new(x, top + 25.0, 0.0),
        ));
    }
    for i in 0..16 {
        let x = -280.0 + (i % 8) as f64 * 80.0;
        let z = if i < 8 { -185.0 } else { 185.0 };
        children.push(part(
            &format!("Bolt_{i}"),
            &shapes.bolt,
            DVec3::new(x, top, z),
        ));
    }
    assembly(name, DMat4::from_translation(offset), children)
}

fn part(name: &str, shape: &Arc<ImportedShape>, position: DVec3) -> ImportNode {
    ImportNode {
        name: name.to_owned(),
        product: name
            .trim_end_matches(|c: char| c.is_ascii_digit() || c == '_')
            .to_owned(),
        placement: DMat4::from_translation(position),
        children: Vec::new(),
        shape: Some(shape.clone()),
    }
}

fn assembly(name: String, placement: DMat4, children: Vec<ImportNode>) -> ImportNode {
    ImportNode {
        product: name.clone(),
        name,
        placement,
        children,
        shape: None,
    }
}

// ---------------------------------------------------------------------------
// 形状の作成 (truck)
// ---------------------------------------------------------------------------

/// 原点を中心にした直方体
fn cuboid([w, h, d]: [f64; 3]) -> Solid {
    let v = builder::vertex(Point3::new(-w / 2.0, -h / 2.0, -d / 2.0));
    let e = builder::tsweep(&v, Vector3::new(w, 0.0, 0.0));
    let f = builder::tsweep(&e, Vector3::new(0.0, h, 0.0));
    builder::tsweep(&f, Vector3::new(0.0, 0.0, d))
}

/// `base` から `axis` (Y か Z) の向きに伸びる円柱
fn cylinder(radius: f64, height: f64, axis: DVec3, base: DVec3) -> Solid {
    let axis_v = Vector3::new(axis.x, axis.y, axis.z);
    // 軸に垂直な方向に半径ぶん離した点を、軸のまわりに1周させて円を作る
    let start = if axis.x.abs() > 0.9 {
        DVec3::Y
    } else {
        DVec3::X
    } * radius
        + base;
    let v = builder::vertex(Point3::new(start.x, start.y, start.z));
    let circle = builder::rsweep(&v, Point3::new(base.x, base.y, base.z), axis_v, Rad(TAU));
    let disk = builder::try_attach_plane(&[circle]).expect("円は平面に貼れる");
    builder::tsweep(&disk, axis_v * height)
}

/// truck のソリッドを表示用の面メッシュと稜線にする
fn tessellate(solids: &[Solid], color: [f32; 3]) -> ImportedShape {
    let mut min = DVec3::splat(f64::MAX);
    let mut max = DVec3::splat(f64::MIN);
    for v in solids.iter().flat_map(|s| s.vertex_iter()) {
        let p = v.point();
        min = min.min(DVec3::new(p.x, p.y, p.z));
        max = max.max(DVec3::new(p.x, p.y, p.z));
    }
    let tol = tessellation_tolerance(min, max);

    let mut faces = Vec::new();
    let mut edges = Vec::new();
    for solid in solids {
        let meshed = solid.triangulation(tol);
        for shell in meshed.boundaries() {
            for face in shell.face_iter() {
                let mut poly = face.surface().unwrap_or_default();
                if !face.orientation() {
                    poly.invert();
                }
                faces.push(poly);
            }
            edges.extend(shell_edges(shell));
        }
    }
    ImportedShape::new(Vec::new(), faces, edges, Some(color))
}

#[cfg(test)]
mod tests {
    use super::*;
    use plantwright_core::model::Model;
    use plantwright_core::model::StepSource;

    #[test]
    fn scene_has_requested_part_count() {
        let file = stress_scene(95);
        let mut model = Model::default();
        let root = model.add_imported(file.root, file.unit, StepSource::Stress(95));
        // 30 部品のユニットを 4 つ (95 以上になる最小の数)
        assert_eq!(model.measure(root).part_count, 4 * PARTS_PER_STATION);
    }

    #[test]
    fn cylinder_shape_is_round() {
        let shape = tessellate(&[cylinder(10.0, 5.0, DVec3::Y, DVec3::ZERO)], [1.0; 3]);
        let size = shape.stats.max - shape.stats.min;
        assert!((size.x - 20.0).abs() < 0.1, "{size}");
        assert!((size.y - 5.0).abs() < 1e-9, "{size}");
        // 体積 π r² h (多角形近似なのでわずかに小さい)
        let expected = std::f64::consts::PI * 100.0 * 5.0;
        assert!((shape.stats.volume - expected).abs() / expected < 0.01);
    }
}
