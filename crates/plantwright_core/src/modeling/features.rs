//! 形の編集履歴: 出発点の形 ([`Base`]) と、その後に加えた編集 ([`Feature`]) の列
//!
//! 穴あけ・フィレット・面取り・シェル・オフセット・ブーリアンをかけるたびに、できた B-rep と
//! 一緒に「何をしたか」を [`ShapeHistory`] として部品に残す ([`super::replace_shape`])。
//! あとから数値を直したり 1 つ消したりするときは、出発点から編集を順にやり直して形を作り直す
//! ([`ShapeHistory::rebuild`])。
//!
//! 辺と面は番号で覚えると前の編集を変えたときにずれるので、部品座標の「辺の中点と長さ」
//! ([`EdgeKey`]) と「面の中心と法線」([`FaceKey`]) で覚え、やり直すときに最寄りの辺・面に当て直す。
//! すべて部品の座標系 (箱は原点を中心にした向き) で持つ。

use glam::{DMat4, DVec3};
use serde::{Deserialize, Serialize};

use super::{BooleanOp, boolean, display_tolerance, oriented_cylinder};
use crate::kernel::{Brep, FaceMesh, MeshedBrep};
use crate::shape::TESSELLATION_ANGLE_DEG;

/// 編集を加える前の、出発点の形
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Base {
    /// 原点を中心にした箱 (幅 / 高さ / 奥行)
    Box { size: [f64; 3] },
    /// そのまま覚えた形 (円柱・押し出し・回転体など、記録を始めたときの形)
    Solid { smlib: String },
}

impl Base {
    pub fn build(&self) -> Result<Brep, String> {
        match self {
            Base::Box { size } => {
                let size = DVec3::from(*size);
                Brep::cuboid(size)?.transformed(DMat4::from_translation(-size / 2.0))
            }
            Base::Solid { smlib } => Brep::from_text(smlib),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Base::Box { size } => format!(
                "箱 {} × {} × {}",
                round3(size[0]),
                round3(size[1]),
                round3(size[2])
            ),
            Base::Solid { .. } => "元の形".to_owned(),
        }
    }
}

/// 辺を見分ける印: 中点と長さ (部品座標)
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeKey {
    pub mid: [f64; 3],
    pub length: f64,
}

/// 面を見分ける印: 面積で重み付けした中心と法線 (部品座標)。閉じた曲面は法線がほぼ 0
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FaceKey {
    pub center: [f64; 3],
    pub normal: [f64; 3],
}

/// 形に加えた 1 つの編集 (部品座標)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Feature {
    /// `start` から `dir` 方向に長さ `length` の円柱をくり抜く
    Hole {
        start: [f64; 3],
        dir: [f64; 3],
        length: f64,
        diameter: f64,
    },
    Fillet {
        edges: Vec<EdgeKey>,
        radius: f64,
    },
    Chamfer {
        edges: Vec<EdgeKey>,
        distance: f64,
    },
    Shell {
        open_faces: Vec<FaceKey>,
        thickness: f64,
    },
    Offset {
        distance: f64,
    },
    RemoveFillets,
    /// 部品座標に置いた相手の形 (SMLib のテキスト) との演算
    Boolean {
        other: String,
        op: BooleanOp,
    },
}

impl Feature {
    /// 履歴に出す短い名前
    pub fn label(&self) -> String {
        match self {
            Feature::Hole { diameter, .. } => format!("穴 φ{}", round3(*diameter)),
            Feature::Fillet { edges, radius } => {
                format!("フィレット R{} ({} 辺)", round3(*radius), edges.len())
            }
            Feature::Chamfer { edges, distance } => {
                format!("面取り {} ({} 辺)", round3(*distance), edges.len())
            }
            Feature::Shell {
                open_faces,
                thickness,
            } => {
                if open_faces.is_empty() {
                    format!("シェル 厚み {}", round3(*thickness))
                } else {
                    format!(
                        "シェル 厚み {} ({} 面を開ける)",
                        round3(*thickness),
                        open_faces.len()
                    )
                }
            }
            Feature::Offset { distance } => format!("オフセット {:+}", round3(*distance)),
            Feature::RemoveFillets => "フィレットを消す".to_owned(),
            Feature::Boolean { op, .. } => format!("ブーリアン {}", op.label()),
        }
    }

    /// あとから直せる数値 (名前と値)。無いものは直せない (消すだけ)
    pub fn value(&self) -> Option<(&'static str, f64)> {
        match self {
            Feature::Hole { diameter, .. } => Some(("直径", *diameter)),
            Feature::Fillet { radius, .. } => Some(("半径", *radius)),
            Feature::Chamfer { distance, .. } => Some(("大きさ", *distance)),
            Feature::Shell { thickness, .. } => Some(("厚み", *thickness)),
            Feature::Offset { distance } => Some(("距離", *distance)),
            Feature::RemoveFillets | Feature::Boolean { .. } => None,
        }
    }

    /// [`value`](Self::value) の数値を書き換える。オフセット以外は 0 より大きい値だけ受け付ける
    pub fn set_value(&mut self, value: f64) -> Result<(), String> {
        let target = match self {
            Feature::Hole { diameter, .. } => diameter,
            Feature::Fillet { radius, .. } => radius,
            Feature::Chamfer { distance, .. } => distance,
            Feature::Shell { thickness, .. } => thickness,
            Feature::Offset { distance } => {
                *distance = value;
                return Ok(());
            }
            Feature::RemoveFillets | Feature::Boolean { .. } => {
                return Err("この編集に直せる数値はありません".into());
            }
        };
        if value <= 0.0 {
            return Err("0 より大きい値にしてください".into());
        }
        *target = value;
        Ok(())
    }

    /// 形 `solid` にこの編集を加える
    pub fn apply(&self, solid: &Brep) -> Result<Brep, String> {
        match self {
            Feature::Hole {
                start,
                dir,
                length,
                diameter,
            } => {
                if *diameter <= 0.0 {
                    return Err("穴の直径は 0 より大きくしてください".into());
                }
                let tool = oriented_cylinder(
                    *diameter / 2.0,
                    *length,
                    DVec3::from(*start),
                    DVec3::from(*dir),
                )?;
                boolean(solid, &tool, BooleanOp::Subtract)
            }
            Feature::Fillet { edges, radius } => solid
                .fillet_edges(&resolve_edges(solid, edges)?, *radius)
                .map_err(|_| too_large("半径")),
            Feature::Chamfer { edges, distance } => solid
                .chamfer_edges(&resolve_edges(solid, edges)?, *distance)
                .map_err(|_| too_large("面取りの大きさ")),
            Feature::Shell {
                open_faces,
                thickness,
            } => solid
                .shell(*thickness, &resolve_faces(solid, open_faces)?)
                .map_err(|_| "中空にできませんでした。厚みを薄くしてみてください".to_owned()),
            Feature::Offset { distance } => solid.offset(*distance),
            Feature::RemoveFillets => solid.remove_fillets(),
            Feature::Boolean { other, op } => boolean(solid, &Brep::from_text(other)?, *op),
        }
    }
}

fn too_large(what: &str) -> String {
    format!("できませんでした。{what}が隣の面より大きくないか確かめてください")
}

/// 出発点の形と編集の列。`base` が無い部品 (取り込んだ形・古いファイル) は作り直せない
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShapeHistory {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<Base>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<Feature>,
}

impl ShapeHistory {
    /// 編集をあとから直したり消したりできる (出発点の形を覚えている)
    pub fn editable(&self) -> bool {
        self.base.is_some()
    }

    /// 出発点から編集を順にやり直して形を作る。失敗したら何番目の編集で止まったかを含めて返す
    pub fn rebuild(&self) -> Result<Brep, String> {
        let base = self
            .base
            .as_ref()
            .ok_or("この部品は形の記録を持っていないので作り直せません")?;
        let mut solid = base.build()?;
        for (i, feature) in self.features.iter().enumerate() {
            solid = feature
                .apply(&solid)
                .map_err(|e| format!("{} 番目の「{}」: {e}", i + 1, feature.label()))?;
        }
        Ok(solid)
    }
}

// ---------------------------------------------------------------------------
// 辺・面の印
// ---------------------------------------------------------------------------

/// 折れ線の辺の印 (長さの半分の位置の点と、全体の長さ)
pub fn edge_key(polyline: &[DVec3]) -> EdgeKey {
    let length: f64 = polyline.windows(2).map(|w| w[0].distance(w[1])).sum();
    let mut remaining = length / 2.0;
    let mut mid = polyline.first().copied().unwrap_or_default();
    for w in polyline.windows(2) {
        let seg = w[0].distance(w[1]);
        if seg >= remaining {
            mid = if seg > 0.0 {
                w[0].lerp(w[1], remaining / seg)
            } else {
                w[0]
            };
            break;
        }
        remaining -= seg;
    }
    EdgeKey {
        mid: mid.to_array(),
        length,
    }
}

/// 面メッシュの印 (面積で重み付けした中心と法線)
pub fn face_key(mesh: &FaceMesh) -> FaceKey {
    let mut area_sum = 0.0;
    let mut center = DVec3::ZERO;
    let mut normal = DVec3::ZERO;
    for [a, b, c] in mesh.triangle_points() {
        let n = (b - a).cross(c - a) / 2.0;
        let area = n.length();
        area_sum += area;
        center += (a + b + c) / 3.0 * area;
        normal += n;
    }
    if area_sum > 0.0 {
        center /= area_sum;
        normal /= area_sum;
    }
    FaceKey {
        center: center.to_array(),
        normal: normal.to_array(),
    }
}

fn meshed(solid: &Brep) -> MeshedBrep {
    solid.tessellate(display_tolerance(solid), TESSELLATION_ANGLE_DEG)
}

fn size_of(solid: &Brep) -> f64 {
    solid
        .loose_bounds()
        .map_or(1.0, |b| (b.max - b.min).length())
        .max(1e-6)
}

/// カーネルの辺の番号 → 印
pub fn edge_keys(solid: &Brep, kernel_edges: &[usize]) -> Result<Vec<EdgeKey>, String> {
    let meshed = meshed(solid);
    kernel_edges
        .iter()
        .map(|&e| {
            meshed
                .edges
                .get(e)
                .map(|poly| edge_key(poly))
                .ok_or_else(|| format!("辺 {e} が見つかりません"))
        })
        .collect()
}

/// カーネルの面の番号 → 印
pub fn face_keys(solid: &Brep, kernel_faces: &[usize]) -> Result<Vec<FaceKey>, String> {
    let meshed = meshed(solid);
    kernel_faces
        .iter()
        .map(|&f| {
            meshed
                .faces
                .get(f)
                .and_then(|face| face.mesh.as_ref())
                .map(face_key)
                .ok_or_else(|| format!("面 {f} が見つかりません"))
        })
        .collect()
}

/// 印から、今の形で最寄りの辺 (カーネルの番号) を探す。遠すぎれば形が変わりすぎたとしてエラー
pub fn resolve_edges(solid: &Brep, keys: &[EdgeKey]) -> Result<Vec<usize>, String> {
    let meshed = meshed(solid);
    let tolerance = size_of(solid) * 0.05;
    let candidates: Vec<(usize, EdgeKey)> = meshed
        .edges
        .iter()
        .enumerate()
        .filter(|(_, poly)| poly.len() >= 2)
        .map(|(i, poly)| (i, edge_key(poly)))
        .collect();
    keys.iter()
        .map(|key| {
            let mid = DVec3::from(key.mid);
            candidates
                .iter()
                .map(|(i, c)| {
                    (
                        *i,
                        mid.distance(DVec3::from(c.mid)) + (c.length - key.length).abs() * 0.5,
                    )
                })
                .filter(|(_, score)| *score <= tolerance)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
                .ok_or_else(|| "辺が見つかりません (前の編集で形が変わりすぎています)".to_owned())
        })
        .collect()
}

/// 印から、今の形で最寄りの面 (カーネルの番号) を探す
pub fn resolve_faces(solid: &Brep, keys: &[FaceKey]) -> Result<Vec<usize>, String> {
    let meshed = meshed(solid);
    let size = size_of(solid);
    let tolerance = size * 0.05;
    let candidates: Vec<(usize, FaceKey)> = meshed
        .faces
        .iter()
        .enumerate()
        .filter_map(|(i, face)| face.mesh.as_ref().map(|m| (i, face_key(m))))
        .collect();
    keys.iter()
        .map(|key| {
            let center = DVec3::from(key.center);
            let normal = DVec3::from(key.normal);
            candidates
                .iter()
                .map(|(i, c)| {
                    let score = center.distance(DVec3::from(c.center))
                        + normal.distance(DVec3::from(c.normal)) * size * 0.01;
                    (*i, score)
                })
                .filter(|(_, score)| *score <= tolerance)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
                .ok_or_else(|| "面が見つかりません (前の編集で形が変わりすぎています)".to_owned())
        })
        .collect()
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(solid: &Brep) -> f64 {
        solid.mass_properties(1.0).unwrap().volume
    }

    #[test]
    fn box_base_matches_centered_box_part() {
        let base = Base::Box {
            size: [100.0, 40.0, 60.0],
        };
        let solid = base.build().unwrap();
        let b = solid.bounds().unwrap();
        assert!((b.min + DVec3::new(50.0, 20.0, 30.0)).length() < 1e-6);
        assert!((b.max - DVec3::new(50.0, 20.0, 30.0)).length() < 1e-6);
        assert_eq!(base.label(), "箱 100 × 40 × 60");
    }

    #[test]
    fn rebuild_replays_hole_and_edit_changes_it() {
        let mut history = ShapeHistory {
            base: Some(Base::Box {
                size: [100.0, 40.0, 100.0],
            }),
            features: vec![Feature::Hole {
                start: [0.0, -30.0, 0.0],
                dir: [0.0, 1.0, 0.0],
                length: 60.0,
                diameter: 20.0,
            }],
        };
        let v = volume(&history.rebuild().unwrap());
        let expected = 400_000.0 - std::f64::consts::PI * 100.0 * 40.0;
        assert!((v - expected).abs() / expected < 0.01, "{v} {expected}");

        history.features[0].set_value(40.0).unwrap();
        let v = volume(&history.rebuild().unwrap());
        let expected = 400_000.0 - std::f64::consts::PI * 400.0 * 40.0;
        assert!((v - expected).abs() / expected < 0.01, "{v} {expected}");
        assert_eq!(history.features[0].label(), "穴 φ40");
        assert!(history.features[0].set_value(0.0).is_err());

        history.features.clear();
        assert!((volume(&history.rebuild().unwrap()) - 400_000.0).abs() < 1e-3);
        assert!(!ShapeHistory::default().editable());
        assert!(ShapeHistory::default().rebuild().is_err());
    }

    #[test]
    fn edge_keys_find_the_same_edge_after_a_change() {
        let base = Base::Box {
            size: [100.0, 40.0, 100.0],
        };
        let solid = base.build().unwrap();
        // 全部の辺の印を取り、同じ形で解くと同じ番号に戻る
        let all: Vec<usize> = (0..12).collect();
        let keys = edge_keys(&solid, &all).unwrap();
        assert_eq!(resolve_edges(&solid, &keys).unwrap(), all);
        // 穴をあけたあとでも箱の辺は見つかる
        let drilled = Feature::Hole {
            start: [0.0, -30.0, 0.0],
            dir: [0.0, 1.0, 0.0],
            length: 60.0,
            diameter: 20.0,
        }
        .apply(&solid)
        .unwrap();
        let found = resolve_edges(&drilled, &keys[..1]).unwrap();
        let again = edge_keys(&drilled, &found).unwrap();
        assert!((DVec3::from(again[0].mid) - DVec3::from(keys[0].mid)).length() < 1e-6);
        // 遠い印は見つからない
        let far = EdgeKey {
            mid: [1000.0, 0.0, 0.0],
            length: 10.0,
        };
        assert!(resolve_edges(&solid, &[far]).is_err());
    }

    #[test]
    fn fillet_feature_replays_through_edge_keys() {
        let base = Base::Box {
            size: [100.0, 40.0, 100.0],
        };
        let solid = base.build().unwrap();
        let keys = edge_keys(&solid, &[0]).unwrap();
        let history = ShapeHistory {
            base: Some(base),
            features: vec![Feature::Fillet {
                edges: keys,
                radius: 5.0,
            }],
        };
        let v = volume(&history.rebuild().unwrap());
        // 角 1 本を丸めた分 (辺の長さ × (r² − πr²/4)) だけ減る
        let r = 5.0;
        let removed_per_len = r * r - std::f64::consts::PI * r * r / 4.0;
        assert!(
            v < 400_000.0 && v > 400_000.0 - 100.0 * removed_per_len * 1.01,
            "{v}"
        );
    }

    #[test]
    fn face_keys_identify_faces() {
        let solid = Base::Box {
            size: [100.0, 40.0, 100.0],
        }
        .build()
        .unwrap();
        let all: Vec<usize> = (0..6).collect();
        let keys = face_keys(&solid, &all).unwrap();
        assert_eq!(resolve_faces(&solid, &keys).unwrap(), all);
        // 上の面の印は中心が y=20、法線が +Y
        let top = keys
            .iter()
            .find(|k| k.normal[1] > 0.9)
            .expect("上の面がある");
        assert!((top.center[1] - 20.0).abs() < 1e-6);
    }

    #[test]
    fn history_round_trips_through_json() {
        let history = ShapeHistory {
            base: Some(Base::Box {
                size: [1.0, 2.0, 3.0],
            }),
            features: vec![
                Feature::Offset { distance: -1.5 },
                Feature::RemoveFillets,
                Feature::Boolean {
                    other: "x".into(),
                    op: BooleanOp::Union,
                },
            ],
        };
        let json = serde_json::to_string(&history).unwrap();
        let back: ShapeHistory = serde_json::from_str(&json).unwrap();
        assert_eq!(back, history);
        assert_eq!(history.features[0].label(), "オフセット -1.5");
        assert!(history.features[1].value().is_none());
    }
}
