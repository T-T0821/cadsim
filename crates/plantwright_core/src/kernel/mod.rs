//! 形状カーネル: NVIDIA の SMLib (usd-brep, Apache-2.0) を使う B-rep
//!
//! SMLib の C++ のソースは `vendor/smlib/` にあり、`build.rs` がビルドして静的にリンクする。
//! Rust から呼ぶための薄い層が `cpp/kernel.cpp`、その C の関数が [`ffi`]。
//! 形の作成・変形・ブーリアン・テッセレーション・保存・面からの組み立ては [`Brep`] のメソッド。
//! フィレット・面取り・シェル・オフセット・回転・パイプ・スイープ・修復・質量特性・断面は `ops.rs`
//! (C++ 側は `cpp/ops.cpp`)。
//!
//! SMLib は同時に複数のスレッドから呼べないので、呼び出しは1つのロックで順番にする。

mod brep;
mod ffi;
pub mod model;
mod ops;

pub use brep::{BooleanKind, Bounds, Brep, triangulate_planar};
pub use model::{
    BSplineCurve, BSplineSurface, EdgeCurve, FaceMesh, FaceSpec, Frame, MeshedBrep, MeshedFace,
    Surface,
};
pub use ops::MassProperties;

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{DMat4, DVec3};
    use std::f64::consts::PI;

    /// 面の向きをそろえた三角形の符号付き体積 (外向きなら正)
    fn volume(meshed: &MeshedBrep) -> f64 {
        meshed
            .faces
            .iter()
            .filter_map(|f| f.mesh.as_ref())
            .flat_map(|m| m.triangle_points())
            .map(|[a, b, c]| a.dot(b.cross(c)) / 6.0)
            .sum()
    }

    #[test]
    fn cuboid_minus_cylinder() {
        let plate = Brep::cuboid(DVec3::new(10.0, 10.0, 2.0)).unwrap();
        let mut tool = Brep::cylinder_z(2.0, 4.0).unwrap();
        tool.transform(DMat4::from_translation(DVec3::new(5.0, 5.0, -1.0)))
            .unwrap();
        let result = plate.boolean(&tool, BooleanKind::Difference).unwrap();
        assert!(result.is_solid());
        let meshed = result.tessellate(0.001, 10.0);
        assert_eq!(meshed.faces.len(), 7);
        let v = volume(&meshed);
        let expected = 200.0 - PI * 4.0 * 2.0;
        assert!((v - expected).abs() / expected < 1e-3, "{v} {expected}");
        // 元の形は変わらない
        assert!((volume(&plate.tessellate(0.0, 25.0)) - 200.0).abs() < 1e-9);
        // 面の縁の点は辺のポリラインの点と同じ
        let face = meshed.faces.iter().find(|f| f.loops.len() == 2).unwrap();
        let mesh = face.mesh.as_ref().unwrap();
        for &e in face.loops.iter().flatten() {
            for p in &meshed.edges[e] {
                assert!(mesh.positions.iter().any(|q| q.distance(*p) < 1e-12));
            }
        }
    }

    #[test]
    fn text_round_trip_and_transform() {
        let mut solid = Brep::cuboid(DVec3::new(1.0, 2.0, 3.0)).unwrap();
        solid
            .transform(DMat4::from_scale_rotation_translation(
                DVec3::splat(2.0),
                glam::DQuat::from_rotation_z(PI / 2.0),
                DVec3::new(10.0, 0.0, 0.0),
            ))
            .unwrap();
        let text = solid.to_text().unwrap();
        let read = Brep::from_text(&text).unwrap();
        let b = read.bounds().unwrap();
        assert!((b.min - DVec3::new(6.0, 0.0, 0.0)).length() < 1e-6, "{b:?}");
        assert!(
            (b.max - DVec3::new(10.0, 2.0, 6.0)).length() < 1e-6,
            "{b:?}"
        );
        assert!(Brep::from_text("not a brep").is_err());
    }

    #[test]
    fn extrude_l_shape() {
        let points = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(2.0, 0.0, 0.0),
            DVec3::new(2.0, 1.0, 0.0),
            DVec3::new(1.0, 1.0, 0.0),
            DVec3::new(1.0, 2.0, 0.0),
            DVec3::new(0.0, 2.0, 0.0),
        ];
        let solid = Brep::extrude(&points, DVec3::new(0.0, 0.0, 3.0)).unwrap();
        assert!(solid.is_solid());
        let v = volume(&solid.tessellate(0.0, 25.0));
        assert!((v - 9.0).abs() < 1e-9, "{v}");
    }

    /// STEP のように、面を1つずつ足して穴のあいた板を組み立てる
    #[test]
    fn plate_with_hole_from_faces() {
        let (w, h, r) = (10.0, 2.0, 2.0);
        let line = |a: DVec3, b: DVec3| EdgeCurve::Line { start: a, end: b };
        let circle = |z: f64, ccw: bool| EdgeCurve::Arc {
            center: DVec3::new(5.0, 5.0, z),
            x: DVec3::X,
            y: if ccw { DVec3::Y } else { -DVec3::Y },
            radius: r,
            start_deg: 0.0,
            end_deg: 360.0,
        };
        let p = DVec3::new;
        let plane = |origin: DVec3, z: DVec3, x: DVec3| Surface::Plane(Frame { origin, z, x });
        let mut faces = vec![
            // 底面: 曲面の法線は +Z、面は -Z を向く
            FaceSpec {
                surface: plane(DVec3::ZERO, DVec3::Z, DVec3::X),
                same_sense: false,
                loops: vec![
                    vec![
                        line(p(0.0, 0.0, 0.0), p(0.0, w, 0.0)),
                        line(p(0.0, w, 0.0), p(w, w, 0.0)),
                        line(p(w, w, 0.0), p(w, 0.0, 0.0)),
                        line(p(w, 0.0, 0.0), p(0.0, 0.0, 0.0)),
                    ],
                    vec![circle(0.0, true)],
                ],
            },
            FaceSpec {
                surface: plane(p(0.0, 0.0, h), DVec3::Z, DVec3::X),
                same_sense: true,
                loops: vec![
                    vec![
                        line(p(0.0, 0.0, h), p(w, 0.0, h)),
                        line(p(w, 0.0, h), p(w, w, h)),
                        line(p(w, w, h), p(0.0, w, h)),
                        line(p(0.0, w, h), p(0.0, 0.0, h)),
                    ],
                    vec![circle(h, false)],
                ],
            },
            FaceSpec {
                surface: Surface::Cylinder {
                    frame: Frame {
                        origin: p(5.0, 5.0, 0.0),
                        z: DVec3::Z,
                        x: DVec3::X,
                    },
                    radius: r,
                },
                same_sense: false,
                loops: vec![vec![circle(0.0, false)], vec![circle(h, true)]],
            },
        ];
        let corners = [
            p(0.0, 0.0, 0.0),
            p(w, 0.0, 0.0),
            p(w, w, 0.0),
            p(0.0, w, 0.0),
        ];
        for i in 0..4 {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            let (a2, b2) = (a + DVec3::Z * h, b + DVec3::Z * h);
            let d = (b - a).normalize();
            faces.push(FaceSpec {
                surface: plane(a, DVec3::new(d.y, -d.x, 0.0), d),
                same_sense: true,
                loops: vec![vec![line(a, b), line(b, b2), line(b2, a2), line(a2, a)]],
            });
        }
        let (brep, failed) = Brep::from_faces(&faces, 1e-3).unwrap();
        assert_eq!(failed, 0);
        assert!(brep.is_solid());
        let expected = w * w * h - PI * r * r * h;
        let v = volume(&brep.tessellate(0.001, 10.0));
        assert!((v - expected).abs() / expected < 1e-3, "{v} {expected}");
        // 組み立てた形にもブーリアンをかけられる
        let mut tool = Brep::cylinder_z(1.0, 5.0).unwrap();
        tool.transform(DMat4::from_translation(DVec3::new(2.0, 2.0, -1.0)))
            .unwrap();
        let cut = brep.boolean(&tool, BooleanKind::Difference).unwrap();
        let v = volume(&cut.tessellate(0.001, 10.0));
        let expected = expected - PI * h;
        assert!((v - expected).abs() / expected < 1e-3, "{v} {expected}");
    }

    #[test]
    fn planar_polygon_with_hole() {
        let square = vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(4.0, 0.0, 0.0),
            DVec3::new(4.0, 4.0, 0.0),
            DVec3::new(0.0, 4.0, 0.0),
        ];
        let hole = vec![
            DVec3::new(1.0, 1.0, 0.0),
            DVec3::new(1.0, 2.0, 0.0),
            DVec3::new(2.0, 2.0, 0.0),
            DVec3::new(2.0, 1.0, 0.0),
        ];
        let mesh = triangulate_planar(&[square, hole]).unwrap();
        let area: f64 = mesh
            .triangle_points()
            .map(|[a, b, c]| (b - a).cross(c - a).z / 2.0)
            .sum();
        assert!((area - 15.0).abs() < 1e-9, "{area}");
    }
}
