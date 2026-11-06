//! STEP のシェル (面の集まり) を、形状カーネルで組み立てられる面の記述 ([`FaceSpec`]) にする
//!
//! 面ごとに曲面と輪郭の辺を読み、[`Brep::from_faces`] で縫い合わせて1つの B-rep にする。
//! 輪郭は STEP の決まりどおり、面の外向きに見て外周が反時計回り・穴が時計回りになるように
//! FACE_BOUND の向きと ORIENTED_EDGE の向きを反映してから渡す。
//! 円柱などの継ぎ目の辺 (同じ輪郭を行きと帰りで2回通る辺) は取り除き、残りを別々の輪郭にする。

use glam::DVec3;
use ruststep::ast::Parameter;

use super::{Entities, arg_ref, args, as_real, as_refs, as_str, axis2_placement, cartesian_point};
use crate::kernel::{BSplineCurve, BSplineSurface, Brep, EdgeCurve, FaceSpec, Frame, Surface};

/// 辺1本分: EDGE_CURVE の id (POLY_LOOP の辺は None) と、輪郭の向きの曲線
type LoopEdge = (Option<u64>, EdgeCurve);

/// STEP の形状を読むための表と設定
pub(super) struct Converter<'a, 'e> {
    e: &'a Entities<'e>,
    /// ファイルの角度の単位 → 度
    deg_per_angle: f64,
    /// 無限に続く直線を有限の線分にするときの長さ (ファイル全体の頂点の広がり)
    extent: f64,
}

impl<'a, 'e> Converter<'a, 'e> {
    pub fn new(e: &'a Entities<'e>) -> Self {
        let (lo, hi) = e
            .all("VERTEX_POINT")
            .filter_map(|(_, p)| arg_ref(p, 1).and_then(|id| cartesian_point(e, id)))
            .fold(
                (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)),
                |(lo, hi), p| (lo.min(p), hi.max(p)),
            );
        let extent = if lo.x <= hi.x {
            (hi - lo).length().max(1.0)
        } else {
            1.0
        };
        Self {
            e,
            deg_per_angle: read_angle_unit(e),
            extent,
        }
    }

    /// シェル (CLOSED_SHELL / OPEN_SHELL) を B-rep にする。
    /// 戻り値の文字列は、読めなかった面があったときの注意
    pub fn shell(&self, id: u64) -> Result<(Brep, Option<String>), String> {
        let params = self
            .e
            .get(id, "CLOSED_SHELL")
            .or_else(|| self.e.get(id, "OPEN_SHELL"))
            .ok_or("未対応のシェル")?;
        let face_ids = params.get(1).map(as_refs).unwrap_or_default();
        let mut faces = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        for face in &face_ids {
            match self.face(*face) {
                Ok(spec) => faces.push(spec),
                Err(err) => errors.push(err),
            }
        }
        if faces.is_empty() {
            return Err(errors.first().cloned().unwrap_or("面がありません".into()));
        }
        let tol = (loop_size(&faces) * 1e-5).max(1e-6);
        let (brep, failed) = Brep::from_faces(&faces, tol)?;
        let skipped = errors.len() + failed;
        let warning = (skipped > 0).then(|| {
            let reason = errors
                .first()
                .map(|e| format!(" ({e})"))
                .unwrap_or_default();
            format!(
                "{skipped} / {} 枚の面を読めませんでした{reason}",
                face_ids.len()
            )
        });
        Ok((brep, warning))
    }

    fn face(&self, id: u64) -> Result<FaceSpec, String> {
        let params = self
            .e
            .get(id, "ADVANCED_FACE")
            .or_else(|| self.e.get(id, "FACE_SURFACE"))
            .ok_or_else(|| format!("未対応の面 #{id}"))?;
        let surface = self.surface(arg_ref(params, 2).ok_or("面に曲面がありません")?)?;
        let same_sense = as_bool(params.get(3)).unwrap_or(true);
        // (外周か, 辺)
        let mut loops: Vec<(bool, Vec<LoopEdge>)> = Vec::new();
        for bound in params.get(1).map(as_refs).unwrap_or_default() {
            let (bp, outer) = match self.e.get(bound, "FACE_OUTER_BOUND") {
                Some(p) => (p, true),
                None => match self.e.get(bound, "FACE_BOUND") {
                    Some(p) => (p, false),
                    None => continue,
                },
            };
            let Some(loop_id) = arg_ref(bp, 1) else {
                continue;
            };
            let mut edges = self.edge_loop(loop_id)?;
            if edges.is_empty() {
                continue; // VERTEX_LOOP (円錐の頂点など) は輪郭にならない
            }
            if !as_bool(bp.get(2)).unwrap_or(true) {
                edges = reverse_loop(edges);
            }
            for part in split_seams(edges) {
                loops.push((outer, part));
            }
        }
        if loops.is_empty() {
            return Err("面に輪郭がありません".into());
        }
        // 外周を最初にする。FACE_OUTER_BOUND が無ければ、いちばん大きい輪郭を外周とみなす
        if !loops.iter().any(|(outer, _)| *outer) {
            let largest = (0..loops.len())
                .max_by(|&a, &b| curves_size(&loops[a].1).total_cmp(&curves_size(&loops[b].1)))
                .unwrap_or(0);
            loops[largest].0 = true;
        }
        loops.sort_by_key(|(outer, _)| !*outer);
        Ok(FaceSpec {
            surface,
            same_sense,
            loops: loops
                .into_iter()
                .map(|(_, edges)| edges.into_iter().map(|(_, c)| c).collect())
                .collect(),
        })
    }

    /// 輪郭 (EDGE_LOOP / POLY_LOOP) の辺を、輪郭の向きの曲線にする
    fn edge_loop(&self, id: u64) -> Result<Vec<LoopEdge>, String> {
        if let Some(params) = self.e.get(id, "EDGE_LOOP") {
            let mut edges = Vec::new();
            for oriented in params.get(1).map(as_refs).unwrap_or_default() {
                let (edge, curves) = self.oriented_edge(oriented)?;
                edges.extend(curves.into_iter().map(|c| (Some(edge), c)));
            }
            return Ok(edges);
        }
        if let Some(params) = self.e.get(id, "POLY_LOOP") {
            let points = self.points(params.get(1))?;
            let n = points.len();
            return Ok((0..n)
                .map(|i| (points[i], points[(i + 1) % n]))
                .filter(|(a, b)| a != b)
                .map(|(start, end)| (None, EdgeCurve::Line { start, end }))
                .collect());
        }
        if self.e.get(id, "VERTEX_LOOP").is_some() {
            return Ok(Vec::new());
        }
        Err(format!("未対応の輪郭 #{id}"))
    }

    fn oriented_edge(&self, id: u64) -> Result<(u64, Vec<EdgeCurve>), String> {
        let params = self
            .e
            .get(id, "ORIENTED_EDGE")
            .ok_or_else(|| format!("未対応の辺 #{id}"))?;
        let edge = arg_ref(params, 3).ok_or("辺がありません")?;
        let orientation = as_bool(params.get(4)).unwrap_or(true);
        let ec = self
            .e
            .get(edge, "EDGE_CURVE")
            .ok_or_else(|| format!("未対応の辺 #{edge}"))?;
        let v1 = self.vertex(arg_ref(ec, 1))?;
        let v2 = self.vertex(arg_ref(ec, 2))?;
        let geometry = arg_ref(ec, 3).ok_or("辺に曲線がありません")?;
        let same_sense = as_bool(ec.get(4)).unwrap_or(true);
        let (start, end) = if orientation { (v1, v2) } else { (v2, v1) };
        let curves = self.curve(geometry, start, end, orientation == same_sense)?;
        Ok((edge, curves))
    }

    fn vertex(&self, id: Option<u64>) -> Result<DVec3, String> {
        id.and_then(|v| self.e.get(v, "VERTEX_POINT"))
            .and_then(|p| arg_ref(p, 1))
            .and_then(|p| cartesian_point(self.e, p))
            .ok_or_else(|| "頂点を読めません".into())
    }

    /// 曲線 `id` の `start` から `end` までの部分。`forward` は曲線のパラメータと同じ向きにたどるか
    fn curve(
        &self,
        id: u64,
        start: DVec3,
        end: DVec3,
        forward: bool,
    ) -> Result<Vec<EdgeCurve>, String> {
        let e = self.e;
        if e.get(id, "LINE").is_some() {
            return Ok(vec![EdgeCurve::Line { start, end }]);
        }
        if let Some(p) = e.get(id, "CIRCLE") {
            let frame = self.frame(arg_ref(p, 1))?;
            let radius = real(p, 2)?;
            return Ok(vec![circle_arc(frame, radius, start, end, forward)]);
        }
        if let Some(p) = e.get(id, "ELLIPSE") {
            let frame = self.frame(arg_ref(p, 1))?;
            return Ok(vec![ellipse_arc(
                frame,
                real(p, 2)?,
                real(p, 3)?,
                start,
                end,
                forward,
            )]);
        }
        for name in ["SURFACE_CURVE", "SEAM_CURVE", "BOUNDED_SURFACE_CURVE"] {
            if let Some(p) = e.get(id, name) {
                let curve_3d = arg_ref(p, 1).ok_or("曲線がありません")?;
                return self.curve(curve_3d, start, end, forward);
            }
        }
        if let Some(p) = e.get(id, "TRIMMED_CURVE") {
            let basis = arg_ref(p, 1).ok_or("曲線がありません")?;
            let sense = as_bool(p.get(4)).unwrap_or(true);
            return self.curve(basis, start, end, forward == sense);
        }
        if let Some(p) = e.get(id, "POLYLINE") {
            let mut points = self.points(p.get(1))?;
            if !forward {
                points.reverse();
            }
            if points.len() < 2 {
                return Err("点が足りない折れ線".into());
            }
            let n = points.len();
            points[0] = start;
            points[n - 1] = end;
            return Ok(points
                .windows(2)
                .filter(|w| w[0] != w[1])
                .map(|w| EdgeCurve::Line {
                    start: w[0],
                    end: w[1],
                })
                .collect());
        }
        if e.get(id, "B_SPLINE_CURVE_WITH_KNOTS").is_some() {
            let curve = self.bspline_curve(id)?;
            let full = start.distance(end) <= curve_size(&curve.points) * 1e-9;
            return Ok(vec![EdgeCurve::BSpline {
                curve,
                start,
                end,
                full,
                reverse: !forward,
            }]);
        }
        Err(format!("未対応の曲線 {}", self.type_name(id)))
    }

    /// 回転面・押し出し面の母線 (曲線全体)
    fn profile(&self, id: u64) -> Result<EdgeCurve, String> {
        let e = self.e;
        if let Some(p) = e.get(id, "LINE") {
            let origin = arg_ref(p, 1)
                .and_then(|c| cartesian_point(e, c))
                .ok_or("直線の点がありません")?;
            let dir = arg_ref(p, 2)
                .and_then(|v| self.vector(v))
                .ok_or("直線の向きがありません")?
                .normalize();
            return Ok(EdgeCurve::Line {
                start: origin - dir * self.extent,
                end: origin + dir * self.extent,
            });
        }
        if let Some(p) = e.get(id, "CIRCLE") {
            let f = self.frame(arg_ref(p, 1))?;
            return Ok(EdgeCurve::Arc {
                center: f.origin,
                x: f.x,
                y: f.z.cross(f.x),
                radius: real(p, 2)?,
                start_deg: 0.0,
                end_deg: 360.0,
            });
        }
        if let Some(p) = e.get(id, "ELLIPSE") {
            let f = self.frame(arg_ref(p, 1))?;
            return Ok(EdgeCurve::Ellipse {
                center: f.origin,
                x: f.x,
                y: f.z.cross(f.x),
                rx: real(p, 2)?,
                ry: real(p, 3)?,
                start_deg: 0.0,
                end_deg: 360.0,
            });
        }
        if let Some(p) = e.get(id, "TRIMMED_CURVE") {
            // 曲面は面の輪郭で切り取るので、もとの曲線全体を使えばよい (向きだけ合わせる)
            let basis = self.profile(arg_ref(p, 1).ok_or("曲線がありません")?)?;
            return Ok(if as_bool(p.get(4)).unwrap_or(true) {
                basis
            } else {
                reverse_curve(basis)
            });
        }
        for name in ["SURFACE_CURVE", "SEAM_CURVE", "BOUNDED_SURFACE_CURVE"] {
            if let Some(p) = e.get(id, name) {
                return self.profile(arg_ref(p, 1).ok_or("曲線がありません")?);
            }
        }
        if e.get(id, "B_SPLINE_CURVE_WITH_KNOTS").is_some() {
            let curve = self.bspline_curve(id)?;
            let start = *curve.points.first().ok_or("制御点がありません")?;
            let end = *curve.points.last().ok_or("制御点がありません")?;
            return Ok(EdgeCurve::BSpline {
                curve,
                start,
                end,
                full: true,
                reverse: false,
            });
        }
        Err(format!("未対応の母線 {}", self.type_name(id)))
    }

    fn bspline_curve(&self, id: u64) -> Result<BSplineCurve, String> {
        let e = self.e;
        let knots = e
            .get(id, "B_SPLINE_CURVE_WITH_KNOTS")
            .ok_or("節点がありません")?;
        // 単独のレコードなら名前から並ぶ。複合エンティティなら型ごとに分かれる
        let (degree, points, knots) = if knots.len() >= 9 {
            (&knots[1], &knots[2], &knots[6..8])
        } else {
            let base = e
                .get(id, "B_SPLINE_CURVE")
                .ok_or("B スプラインの次数がありません")?;
            (
                base.first().ok_or("次数がありません")?,
                base.get(1).ok_or("制御点がありません")?,
                knots,
            )
        };
        let points = self.points(Some(points))?;
        let weights = e
            .get(id, "RATIONAL_B_SPLINE_CURVE")
            .map(|p| reals(p.first()))
            .filter(|w| w.len() == points.len());
        Ok(BSplineCurve {
            degree: as_real(degree).ok_or("次数がありません")? as u32,
            points,
            weights,
            multiplicities: reals(knots.first()).into_iter().map(|m| m as u32).collect(),
            knots: reals(knots.get(1)),
        })
    }

    fn surface(&self, id: u64) -> Result<Surface, String> {
        let e = self.e;
        if let Some(p) = e.get(id, "PLANE") {
            return Ok(Surface::Plane(self.frame(arg_ref(p, 1))?));
        }
        if let Some(p) = e.get(id, "CYLINDRICAL_SURFACE") {
            return Ok(Surface::Cylinder {
                frame: self.frame(arg_ref(p, 1))?,
                radius: real(p, 2)?,
            });
        }
        if let Some(p) = e.get(id, "CONICAL_SURFACE") {
            return Ok(Surface::Cone {
                frame: self.frame(arg_ref(p, 1))?,
                radius: real(p, 2)?,
                semi_angle_deg: real(p, 3)? * self.deg_per_angle,
            });
        }
        if let Some(p) = e.get(id, "SPHERICAL_SURFACE") {
            return Ok(Surface::Sphere {
                frame: self.frame(arg_ref(p, 1))?,
                radius: real(p, 2)?,
            });
        }
        for name in ["TOROIDAL_SURFACE", "DEGENERATE_TOROIDAL_SURFACE"] {
            if let Some(p) = e.get(id, name) {
                return Ok(Surface::Torus {
                    frame: self.frame(arg_ref(p, 1))?,
                    major: real(p, 2)?,
                    minor: real(p, 3)?,
                });
            }
        }
        if e.get(id, "B_SPLINE_SURFACE_WITH_KNOTS").is_some() {
            return self.bspline_surface(id).map(Surface::BSpline);
        }
        if let Some(p) = e.get(id, "SURFACE_OF_REVOLUTION") {
            let profile = self.profile(arg_ref(p, 1).ok_or("母線がありません")?)?;
            let axis = arg_ref(p, 2)
                .and_then(|a| e.get(a, "AXIS1_PLACEMENT"))
                .ok_or("回転軸がありません")?;
            let origin = arg_ref(axis, 1)
                .and_then(|c| cartesian_point(e, c))
                .ok_or("回転軸の点がありません")?;
            let axis = arg_ref(axis, 2)
                .and_then(|d| super::direction(e, d))
                .unwrap_or(DVec3::Z)
                .normalize();
            return Ok(Surface::Revolution {
                profile,
                origin,
                axis,
            });
        }
        if let Some(p) = e.get(id, "SURFACE_OF_LINEAR_EXTRUSION") {
            let profile = self.profile(arg_ref(p, 1).ok_or("母線がありません")?)?;
            let direction = arg_ref(p, 2)
                .and_then(|v| self.vector(v))
                .ok_or("押し出しの向きがありません")?;
            return Ok(Surface::Extrusion { profile, direction });
        }
        Err(format!("未対応の曲面 {}", self.type_name(id)))
    }

    fn bspline_surface(&self, id: u64) -> Result<BSplineSurface, String> {
        let e = self.e;
        let knots = e
            .get(id, "B_SPLINE_SURFACE_WITH_KNOTS")
            .ok_or("節点がありません")?;
        let (degrees, rows, knots) = if knots.len() >= 13 {
            (&knots[1..3], &knots[3], &knots[8..12])
        } else {
            let base = e
                .get(id, "B_SPLINE_SURFACE")
                .ok_or("B スプラインの次数がありません")?;
            if base.len() < 3 {
                return Err("B スプライン曲面を読めません".into());
            }
            (&base[0..2], &base[2], knots)
        };
        if degrees.len() < 2 || knots.len() < 4 {
            return Err("B スプライン曲面を読めません".into());
        }
        let Parameter::List(rows) = rows else {
            return Err("制御点がありません".into());
        };
        let mut points = Vec::new();
        let mut nv = 0;
        for row in rows {
            let row = self.points(Some(row))?;
            if nv != 0 && row.len() != nv {
                return Err("制御点の並びがそろっていません".into());
            }
            nv = row.len();
            points.extend(row);
        }
        let weights = e
            .get(id, "RATIONAL_B_SPLINE_SURFACE")
            .and_then(|p| match p.first() {
                Some(Parameter::List(rows)) => {
                    let w: Vec<f64> = rows.iter().flat_map(|r| reals(Some(r))).collect();
                    (w.len() == points.len()).then_some(w)
                }
                _ => None,
            });
        let mults = |p: &Parameter| reals(Some(p)).into_iter().map(|m| m as u32).collect();
        Ok(BSplineSurface {
            u_degree: as_real(&degrees[0]).ok_or("次数がありません")? as u32,
            v_degree: as_real(&degrees[1]).ok_or("次数がありません")? as u32,
            nu: rows.len(),
            nv,
            points,
            weights,
            u_multiplicities: mults(&knots[0]),
            v_multiplicities: mults(&knots[1]),
            u_knots: reals(Some(&knots[2])),
            v_knots: reals(Some(&knots[3])),
        })
    }

    fn frame(&self, id: Option<u64>) -> Result<Frame, String> {
        let m = id
            .and_then(|id| axis2_placement(self.e, id))
            .ok_or("座標系を読めません")?;
        Ok(Frame {
            origin: m.w_axis.truncate(),
            z: m.z_axis.truncate(),
            x: m.x_axis.truncate(),
        })
    }

    /// VECTOR (向きと長さ)
    fn vector(&self, id: u64) -> Option<DVec3> {
        let p = self.e.get(id, "VECTOR")?;
        let dir = super::direction(self.e, arg_ref(p, 1)?)?.normalize();
        Some(dir * as_real(p.get(2)?)?)
    }

    fn points(&self, list: Option<&Parameter>) -> Result<Vec<DVec3>, String> {
        list.map(as_refs)
            .unwrap_or_default()
            .into_iter()
            .map(|id| cartesian_point(self.e, id).ok_or_else(|| format!("点 #{id} を読めません")))
            .collect()
    }

    fn type_name(&self, id: u64) -> String {
        self.e.type_names(id).collect::<Vec<_>>().join(" ")
    }
}

fn as_bool(p: Option<&Parameter>) -> Option<bool> {
    match p? {
        Parameter::Enumeration(v) => match v.as_str() {
            "T" => Some(true),
            "F" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn real(params: &[Parameter], index: usize) -> Result<f64, String> {
    params
        .get(index)
        .and_then(as_real)
        .ok_or_else(|| "数値を読めません".into())
}

fn reals(p: Option<&Parameter>) -> Vec<f64> {
    match p {
        Some(Parameter::List(list)) => list.iter().filter_map(as_real).collect(),
        _ => Vec::new(),
    }
}

/// 角度の単位を読む。度で書かれたファイルもある (ふつうはラジアン)
fn read_angle_unit(e: &Entities) -> f64 {
    for (id, _) in e.all("PLANE_ANGLE_UNIT") {
        if let Some(params) = e.get(id, "CONVERSION_BASED_UNIT") {
            let name = params.first().and_then(as_str).unwrap_or("");
            if name.to_lowercase().contains("deg") {
                return 1.0;
            }
            // 単位の定義 (MEASURE_WITH_UNIT) に書かれたラジアンの値から求める
            let factor = arg_ref(params, 1)
                .and_then(|m| e.records.get(&m))
                .and_then(|records| {
                    records.iter().find_map(|r| match args(r).first()? {
                        Parameter::Typed { parameter, .. } => as_real(parameter),
                        p => as_real(p),
                    })
                });
            if let Some(f) = factor {
                return f.to_degrees();
            }
        }
    }
    1f64.to_degrees()
}

/// 円の `start` から `end` まで (`forward` なら軸のまわりに反時計回り)
fn circle_arc(f: Frame, radius: f64, start: DVec3, end: DVec3, forward: bool) -> EdgeCurve {
    let normal = if forward { f.z } else { -f.z };
    let to_start = start - f.origin;
    let x = (to_start - normal * to_start.dot(normal))
        .try_normalize()
        .unwrap_or(f.x);
    let y = normal.cross(x);
    let to_end = end - f.origin;
    let mut sweep = to_end.dot(y).atan2(to_end.dot(x)).to_degrees();
    if sweep <= 1e-7 {
        sweep += 360.0;
    }
    EdgeCurve::Arc {
        center: f.origin,
        x,
        y,
        radius,
        start_deg: 0.0,
        end_deg: sweep,
    }
}

/// 楕円の `start` から `end` まで
fn ellipse_arc(f: Frame, rx: f64, ry: f64, start: DVec3, end: DVec3, forward: bool) -> EdgeCurve {
    let x = f.x;
    let y = f.z.cross(f.x);
    let angle = |p: DVec3| {
        let d = p - f.origin;
        (d.dot(y) / ry).atan2(d.dot(x) / rx).to_degrees()
    };
    // 逆向きは y を反転してパラメータの符号を変える
    let (y, s0, mut s1) = if forward {
        (y, angle(start), angle(end))
    } else {
        (-y, -angle(start), -angle(end))
    };
    while s1 <= s0 + 1e-7 {
        s1 += 360.0;
    }
    while s1 > s0 + 360.0 + 1e-7 {
        s1 -= 360.0;
    }
    EdgeCurve::Ellipse {
        center: f.origin,
        x,
        y,
        rx,
        ry,
        start_deg: s0,
        end_deg: s1,
    }
}

/// 曲線を逆向きにたどるものにする
fn reverse_curve(curve: EdgeCurve) -> EdgeCurve {
    match curve {
        EdgeCurve::Line { start, end } => EdgeCurve::Line {
            start: end,
            end: start,
        },
        // y を反転すると、パラメータ t は -t になる
        EdgeCurve::Arc {
            center,
            x,
            y,
            radius,
            start_deg,
            end_deg,
        } => EdgeCurve::Arc {
            center,
            x,
            y: -y,
            radius,
            start_deg: -end_deg,
            end_deg: -start_deg,
        },
        EdgeCurve::Ellipse {
            center,
            x,
            y,
            rx,
            ry,
            start_deg,
            end_deg,
        } => EdgeCurve::Ellipse {
            center,
            x,
            y: -y,
            rx,
            ry,
            start_deg: -end_deg,
            end_deg: -start_deg,
        },
        EdgeCurve::BSpline {
            curve,
            start,
            end,
            full,
            reverse,
        } => EdgeCurve::BSpline {
            curve,
            start: end,
            end: start,
            full,
            reverse: !reverse,
        },
    }
}

fn reverse_loop(edges: Vec<LoopEdge>) -> Vec<LoopEdge> {
    edges
        .into_iter()
        .rev()
        .map(|(id, c)| (id, reverse_curve(c)))
        .collect()
}

/// 継ぎ目の辺 (同じ輪郭で2回使われる辺) を除き、残りのひと続きの辺をそれぞれ輪郭にする。
/// 分けた輪郭が閉じないときは元のまま返す
fn split_seams(edges: Vec<LoopEdge>) -> Vec<Vec<LoopEdge>> {
    let is_seam = |id: &Option<u64>| {
        id.is_some() && edges.iter().filter(|(other, _)| other == id).count() >= 2
    };
    let seams: Vec<bool> = edges.iter().map(|(id, _)| is_seam(id)).collect();
    let Some(first_seam) = seams.iter().position(|&s| s) else {
        return vec![edges];
    };
    // 継ぎ目の直後から1周たどって、継ぎ目で区切る
    let n = edges.len();
    let mut parts: Vec<Vec<LoopEdge>> = Vec::new();
    let mut current = Vec::new();
    for k in 1..=n {
        let i = (first_seam + k) % n;
        if seams[i] {
            if !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
        } else {
            current.push(edges[i].clone());
        }
    }
    let size = curves_size(&edges).max(1e-9);
    let closed = |part: &Vec<LoopEdge>| {
        let (a, b) = (start_point(&part[0].1), end_point(&part[part.len() - 1].1));
        a.distance(b) <= size * 1e-6
    };
    if parts.is_empty() || !parts.iter().all(closed) {
        return vec![edges];
    }
    parts
}

fn start_point(c: &EdgeCurve) -> DVec3 {
    match c {
        EdgeCurve::Line { start, .. } | EdgeCurve::BSpline { start, .. } => *start,
        EdgeCurve::Arc {
            center,
            x,
            y,
            radius,
            start_deg,
            ..
        } => {
            let t = start_deg.to_radians();
            *center + *radius * (t.cos() * *x + t.sin() * *y)
        }
        EdgeCurve::Ellipse {
            center,
            x,
            y,
            rx,
            ry,
            start_deg,
            ..
        } => {
            let t = start_deg.to_radians();
            *center + *rx * t.cos() * *x + *ry * t.sin() * *y
        }
    }
}

fn end_point(c: &EdgeCurve) -> DVec3 {
    match c {
        EdgeCurve::Line { end, .. } | EdgeCurve::BSpline { end, .. } => *end,
        _ => start_point(&reverse_curve(c.clone())),
    }
}

/// 辺の端点が広がる大きさ (対角線の長さ)
fn curves_size(edges: &[LoopEdge]) -> f64 {
    let points: Vec<DVec3> = edges
        .iter()
        .flat_map(|(_, c)| [start_point(c), end_point(c)])
        .collect();
    let size = curve_size(&points);
    // 円1本だけの輪郭は端点が1点なので半径で測る
    edges
        .iter()
        .map(|(_, c)| match c {
            EdgeCurve::Arc { radius, .. } => 2.0 * radius,
            EdgeCurve::Ellipse { rx, ry, .. } => 2.0 * rx.max(*ry),
            _ => 0.0,
        })
        .fold(size, f64::max)
}

fn curve_size(points: &[DVec3]) -> f64 {
    let (lo, hi) = points.iter().fold(
        (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)),
        |(lo, hi), &p| (lo.min(p), hi.max(p)),
    );
    if lo.x <= hi.x {
        (hi - lo).length()
    } else {
        0.0
    }
}

/// 面の輪郭全体の大きさ
fn loop_size(faces: &[FaceSpec]) -> f64 {
    let points: Vec<DVec3> = faces
        .iter()
        .flat_map(|f| f.loops.iter().flatten())
        .flat_map(|c| [start_point(c), end_point(c)])
        .collect();
    curve_size(&points)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame {
            origin: DVec3::ZERO,
            z: DVec3::Z,
            x: DVec3::X,
        }
    }

    #[test]
    fn arc_follows_loop_direction() {
        // 反時計回りに (1,0) → (0,1) は 90°、時計回りなら 270°
        let (a, b) = (DVec3::X, DVec3::Y);
        let EdgeCurve::Arc { end_deg, .. } = circle_arc(frame(), 1.0, a, b, true) else {
            unreachable!()
        };
        assert!((end_deg - 90.0).abs() < 1e-9);
        let arc = circle_arc(frame(), 1.0, a, b, false);
        let EdgeCurve::Arc { end_deg, .. } = arc else {
            unreachable!()
        };
        assert!((end_deg - 270.0).abs() < 1e-9);
        assert!(start_point(&arc).distance(a) < 1e-12);
        assert!(end_point(&arc).distance(b) < 1e-12);
        // 閉じた円は1周
        let EdgeCurve::Arc { end_deg, .. } = circle_arc(frame(), 1.0, a, a, true) else {
            unreachable!()
        };
        assert!((end_deg - 360.0).abs() < 1e-9);
    }

    #[test]
    fn reversed_ellipse_keeps_end_points() {
        let (a, b) = (DVec3::new(2.0, 0.0, 0.0), DVec3::new(0.0, 1.0, 0.0));
        for forward in [true, false] {
            let arc = ellipse_arc(frame(), 2.0, 1.0, a, b, forward);
            assert!(start_point(&arc).distance(a) < 1e-12);
            assert!(end_point(&arc).distance(b) < 1e-12);
            let back = reverse_curve(arc);
            assert!(start_point(&back).distance(b) < 1e-12);
        }
    }

    #[test]
    fn seam_edges_split_cylinder_loop() {
        let circle = |z: f64, forward: bool| {
            circle_arc(
                Frame {
                    origin: DVec3::new(0.0, 0.0, z),
                    ..frame()
                },
                1.0,
                DVec3::new(1.0, 0.0, z),
                DVec3::new(1.0, 0.0, z),
                forward,
            )
        };
        let seam = |up: bool| {
            let (a, b) = (DVec3::X, DVec3::new(1.0, 0.0, 2.0));
            let (start, end) = if up { (a, b) } else { (b, a) };
            EdgeCurve::Line { start, end }
        };
        let edges = vec![
            (Some(1), circle(0.0, true)),
            (Some(2), seam(true)),
            (Some(3), circle(2.0, false)),
            (Some(2), seam(false)),
        ];
        let parts = split_seams(edges);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0][0].0, Some(3));
        assert_eq!(parts[1][0].0, Some(1));
    }
}
