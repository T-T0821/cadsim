//! モデリングで作った形状 (B-rep) を `.plantwright` に埋め込む JSON にする
//!
//! `crate::modeling::SolidStore` の実装 ([`JsonSolids`]) と、
//! `ModeledSource` から形状を作り直すメソッド (`solid`・`to_imported`)。
//!
//! JSON は `{"smlib": "<SMLib の .smb のテキスト>"}`。形状カーネルを SMLib に替える前の
//! ファイルは truck の B-rep をそのまま JSON にしていたので、読むときはその形も受け付けて
//! 面を1つずつ SMLib の B-rep に組み立て直す ([`legacy_truck_solid`])。

use std::collections::HashMap;
use std::sync::Arc;

use glam::{DMat4, DVec3};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::kernel::{BSplineCurve, BSplineSurface, Brep, EdgeCurve, FaceSpec, Frame, Surface};
use crate::model::ModeledSource;
use crate::modeling::{SolidStore, shape_from_solid};
use crate::shape::{ImportNode, ImportedFile, next_shape_id};

/// B-rep を JSON にして `ModeledSource` に詰める
pub struct JsonSolids;

impl SolidStore for JsonSolids {
    fn save(&self, name: String, solid: &Brep) -> Result<ModeledSource, String> {
        ModeledSource::new(name, solid)
    }

    fn load(&self, source: &ModeledSource) -> Result<Brep, String> {
        source.solid()
    }
}

/// 埋め込む JSON の形
#[derive(Serialize, Deserialize)]
struct SolidRecord {
    /// SMLib の .smb のテキスト
    smlib: String,
}

/// `ModeledSource` (B-rep を JSON で持つ) と B-rep の変換
impl ModeledSource {
    pub fn new(name: String, solid: &Brep) -> Result<Self, String> {
        let record = SolidRecord {
            smlib: solid.to_text()?,
        };
        let json =
            serde_json::to_string(&record).map_err(|e| format!("形状を保存できません: {e}"))?;
        Ok(Self {
            id: next_shape_id(),
            name,
            json,
        })
    }

    pub fn solid(&self) -> Result<Brep, String> {
        let value: Value =
            serde_json::from_str(&self.json).map_err(|e| format!("形状を読めませんでした: {e}"))?;
        if let Some(text) = value.get("smlib").and_then(Value::as_str) {
            return Brep::from_text(text);
        }
        if value.get("boundaries").is_some() {
            return legacy_truck_solid(&value);
        }
        Err("形状を読めませんでした (知らない形式です)".into())
    }

    /// モデルに取り込める形 (部品1つの木) にする
    pub fn to_imported(&self) -> Result<ImportedFile, String> {
        let shape = shape_from_solid(&self.solid()?);
        Ok(ImportedFile {
            root: ImportNode {
                name: self.name.clone(),
                product: self.name.clone(),
                placement: DMat4::IDENTITY,
                children: Vec::new(),
                shape: Some(Arc::new(shape)),
            },
            unit: String::new(),
            warnings: Vec::new(),
            animations: Vec::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// 以前の形式 (truck の B-rep の JSON)
// ---------------------------------------------------------------------------

fn point(v: &Value) -> Option<DVec3> {
    Some(DVec3::new(
        v.get("x")?.as_f64()?,
        v.get("y")?.as_f64()?,
        v.get("z")?.as_f64()?,
    ))
}

/// 同次座標 (x, y, z は重みを掛けた値) の点を、点と重みにする
fn homogeneous(v: &Value) -> Option<(DVec3, f64)> {
    let w = v.get("w").and_then(Value::as_f64).unwrap_or(1.0);
    let p = point(v)?;
    (w.abs() > 1e-300).then(|| (p / w, w))
}

/// 重なりを含む節点の列を、重なりを除いた節点と重なりの数にする
fn compress_knots(knots: &[f64]) -> (Vec<f64>, Vec<u32>) {
    let mut values: Vec<f64> = Vec::new();
    let mut mults: Vec<u32> = Vec::new();
    for &k in knots {
        match values.last() {
            Some(&last) if (k - last).abs() <= 1e-12 * (1.0 + last.abs()) => {
                if let Some(m) = mults.last_mut() {
                    *m += 1;
                }
            }
            _ => {
                values.push(k);
                mults.push(1);
            }
        }
    }
    (values, mults)
}

fn knot_list(v: &Value) -> Option<Vec<f64>> {
    v.as_array()?.iter().map(Value::as_f64).collect()
}

/// truck の NurbsCurve / BSplineCurve
fn spline_curve(v: &Value) -> Option<BSplineCurve> {
    let knots = knot_list(v.get("knot_vec")?)?;
    let mut points = Vec::new();
    let mut weights = Vec::new();
    for cp in v.get("control_points")?.as_array()? {
        let (p, w) = homogeneous(cp)?;
        points.push(p);
        weights.push(w);
    }
    let degree = knots.len().checked_sub(points.len() + 1)? as u32;
    let (knots, multiplicities) = compress_knots(&knots);
    let rational = weights.iter().any(|w| (w - 1.0).abs() > 1e-12);
    Some(BSplineCurve {
        degree,
        points,
        weights: rational.then_some(weights),
        knots,
        multiplicities,
    })
}

/// 点の列を通る折れ線 (1次の B スプライン)
fn polyline(points: Vec<DVec3>) -> Option<BSplineCurve> {
    if points.len() < 2 {
        return None;
    }
    let n = points.len();
    let mut multiplicities = vec![1; n];
    multiplicities[0] = 2;
    multiplicities[n - 1] = 2;
    Some(BSplineCurve {
        degree: 1,
        points,
        weights: None,
        knots: (0..n).map(|i| i as f64).collect(),
        multiplicities,
    })
}

/// 辺の曲線 (`start` → `end` が辺の向き) を、輪郭の向き (`forward` なら辺と同じ) で返す
fn legacy_edge(curve: &Value, start: DVec3, end: DVec3, forward: bool) -> EdgeCurve {
    let (from, to) = if forward { (start, end) } else { (end, start) };
    let spline = if let Some(c) = curve
        .get("NurbsCurve")
        .or_else(|| curve.get("BSplineCurve"))
    {
        spline_curve(c)
    } else if let Some(c) = curve.get("IntersectionCurve") {
        // 交線は近似の折れ線 (leader) を使う
        c.get("leader")
            .and_then(|l| l.get("Polyline"))
            .and_then(Value::as_array)
            .and_then(|points| polyline(points.iter().filter_map(point).collect()))
    } else {
        None
    };
    match spline {
        Some(curve) => EdgeCurve::BSpline {
            curve,
            start: from,
            end: to,
            full: start.distance(end) <= 1e-9 * (1.0 + start.length()),
            reverse: !forward,
        },
        None => EdgeCurve::Line {
            start: from,
            end: to,
        },
    }
}

fn legacy_surface(v: &Value) -> Option<Surface> {
    if let Some(p) = v.get("Plane") {
        let o = point(p.get("o")?)?;
        let u = point(p.get("p")?)? - o;
        let w = point(p.get("q")?)? - o;
        return Some(Surface::Plane(Frame {
            origin: o,
            z: u.cross(w).try_normalize()?,
            x: u.try_normalize()?,
        }));
    }
    let s = v.get("NurbsSurface").or_else(|| v.get("BSplineSurface"))?;
    let knot_vecs = s.get("knot_vecs")?.as_array()?;
    let u_knots = knot_list(knot_vecs.first()?)?;
    let v_knots = knot_list(knot_vecs.get(1)?)?;
    let rows = s.get("control_points")?.as_array()?;
    let nu = rows.len();
    let nv = rows.first()?.as_array()?.len();
    let mut points = Vec::new();
    let mut weights = Vec::new();
    for row in rows {
        let row = row.as_array()?;
        if row.len() != nv {
            return None;
        }
        for cp in row {
            let (p, w) = homogeneous(cp)?;
            points.push(p);
            weights.push(w);
        }
    }
    let u_degree = u_knots.len().checked_sub(nu + 1)? as u32;
    let v_degree = v_knots.len().checked_sub(nv + 1)? as u32;
    let (u_knots, u_multiplicities) = compress_knots(&u_knots);
    let (v_knots, v_multiplicities) = compress_knots(&v_knots);
    let rational = weights.iter().any(|w| (w - 1.0).abs() > 1e-12);
    Some(Surface::BSpline(BSplineSurface {
        u_degree,
        v_degree,
        nu,
        nv,
        points,
        weights: rational.then_some(weights),
        u_knots,
        u_multiplicities,
        v_knots,
        v_multiplicities,
    }))
}

/// truck の `Solid` を serde_json で書いたもの (以前の形式) から B-rep を組み立てる
fn legacy_truck_solid(value: &Value) -> Result<Brep, String> {
    let bad = || "以前の形式の形状を読めませんでした".to_owned();
    let mut faces = Vec::new();
    let mut min = DVec3::splat(f64::MAX);
    let mut max = DVec3::splat(f64::MIN);
    for shell in value
        .get("boundaries")
        .and_then(Value::as_array)
        .ok_or_else(bad)?
    {
        let vertices: Vec<DVec3> = shell
            .get("vertices")
            .and_then(Value::as_array)
            .ok_or_else(bad)?
            .iter()
            .map(|v| point(v).ok_or_else(bad))
            .collect::<Result<_, _>>()?;
        for &p in &vertices {
            min = min.min(p);
            max = max.max(p);
        }
        let edges = shell
            .get("edges")
            .and_then(Value::as_array)
            .ok_or_else(bad)?;
        let mut edge_ends: HashMap<usize, (DVec3, DVec3, &Value)> = HashMap::new();
        for (i, e) in edges.iter().enumerate() {
            let ends = e
                .get("vertices")
                .and_then(Value::as_array)
                .ok_or_else(bad)?;
            let at = |k: usize| {
                ends.get(k)
                    .and_then(Value::as_u64)
                    .and_then(|v| vertices.get(v as usize).copied())
            };
            let (Some(a), Some(b), Some(curve)) = (at(0), at(1), e.get("curve")) else {
                return Err(bad());
            };
            edge_ends.insert(i, (a, b, curve));
        }
        for face in shell
            .get("faces")
            .and_then(Value::as_array)
            .ok_or_else(bad)?
        {
            let Some(surface) = face.get("surface").and_then(legacy_surface) else {
                continue;
            };
            let orientation = face.get("orientation").and_then(Value::as_bool) != Some(false);
            let mut loops = Vec::new();
            for wire in face
                .get("boundaries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let mut curves = Vec::new();
                for use_ in wire.as_array().into_iter().flatten() {
                    let index = use_.get("index").and_then(Value::as_u64).ok_or_else(bad)?;
                    let forward = use_.get("orientation").and_then(Value::as_bool) != Some(false);
                    let (a, b, curve) = edge_ends.get(&(index as usize)).ok_or_else(bad)?;
                    curves.push((*a, *b, *curve, forward));
                }
                // truck は輪郭を曲面の向きで持つ。面が裏返っていれば、面の外向きに合わせて逆にたどる
                if !orientation {
                    curves.reverse();
                    for c in &mut curves {
                        c.3 = !c.3;
                    }
                }
                loops.push(
                    curves
                        .into_iter()
                        .map(|(a, b, curve, forward)| legacy_edge(curve, a, b, forward))
                        .collect(),
                );
            }
            faces.push(FaceSpec {
                surface,
                same_sense: orientation,
                loops,
            });
        }
    }
    let size = if min.x <= max.x {
        (max - min).length()
    } else {
        1.0
    };
    let (brep, _) = Brep::from_faces(&faces, (size * 2e-4).max(1e-6))?;
    Ok(brep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Model, NodeKind, StepSource};
    use crate::modeling::{BooleanOp, add_solid, apply_boolean, cylinder};

    #[test]
    fn modeled_shape_round_trips_through_json() {
        let mut model = Model::default();
        let a = model.add_box([100.0, 100.0, 100.0], [0.0, 50.0, 0.0]);
        let cyl = cylinder(20.0, 160.0).unwrap();
        let b = add_solid(
            &mut model,
            &JsonSolids,
            "円柱".into(),
            &cyl,
            glam::DVec3::new(10.0, -30.0, 7.0),
        )
        .unwrap();
        apply_boolean(&mut model, &JsonSolids, a, b, BooleanOp::Union).unwrap();
        let NodeKind::Imported { shape, source, .. } = &model.get(a).unwrap().kind else {
            panic!();
        };
        // 保存した JSON から同じ形を作り直せる
        let StepSource::Modeled(source) = source else {
            panic!()
        };
        assert!(source.json.starts_with("{\"smlib\":"));
        let again = source.to_imported().unwrap().root.shape.unwrap();
        assert_eq!(again.faces.len(), shape.faces.len());
        assert!((again.stats.max - shape.stats.max).length() < 1e-6);
        assert!((again.stats.volume - shape.stats.volume).abs() < 1e-6 * shape.stats.volume);
    }

    /// 形状カーネルを替える前のファイル (truck の B-rep の JSON) も読める
    #[test]
    fn reads_legacy_truck_json() {
        let cases = [
            (include_str!("testdata/truck_cylinder.json"), 15_707.96),
            (include_str!("testdata/truck_extrude.json"), 150_000.0),
            (include_str!("testdata/truck_box_hole.json"), 387_433.6),
            (include_str!("testdata/truck_union.json"), 1_075_398.2),
        ];
        for (json, expected) in cases {
            let source = ModeledSource {
                id: next_shape_id(),
                name: "old".into(),
                json: json.to_owned(),
            };
            let solid = source.solid().unwrap();
            assert!(solid.is_solid());
            let shape = shape_from_solid(&solid);
            let error = (shape.stats.volume - expected).abs() / expected;
            assert!(error < 0.003, "{} {expected}", shape.stats.volume);
        }
    }
}
