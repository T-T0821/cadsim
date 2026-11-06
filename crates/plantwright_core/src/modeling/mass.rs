//! 部品・アセンブリの質量特性 (プロパティの「質量」欄と、3D の重心の球)
//!
//! 部品を選ぶだけで体積・質量・重心・慣性モーメントが出るよう、計算は形状カーネルの
//! [`Brep::mass_properties`](crate::kernel::Brep::mass_properties) で行い、結果を部品ごとに
//! [`MassCache`] に覚えておく。覚える鍵は「形・密度・回転と拡大縮小」なので、ギズモで部品を
//! 平行移動している間は計算し直さず、重心をずらすだけで済む。
//!
//! 密度は部品の物理設定 (`PartPhysics::density`, g/cm³) をそのまま使う。材質を選ぶと
//! ([`MATERIALS`]) その密度が入り、物理演算の重さにも同じ値が効く。
//! アセンブリを選べば、中の部品をまとめた値 (慣性モーメントは平行軸の定理で全体の重心まわり) になる。

use std::collections::HashMap;
use std::hash::Hash;

use glam::{DMat4, DVec3};

use super::SolidStore;
use super::ops::{combine, part_solids};
use crate::kernel::MassProperties;
use crate::model::{Model, NodeId, NodeKind};
use crate::physics::settings::meters_per_unit;

/// 材質の名前と密度 (g/cm³)。プロパティのドロップダウンに並ぶ
pub const MATERIALS: [(&str, f64); 9] = [
    ("鉄 (SS400)", 7.85),
    ("ステンレス (SUS304)", 7.93),
    ("アルミ (A5052)", 2.68),
    ("真鍮", 8.47),
    ("銅", 8.96),
    ("樹脂 (ABS)", 1.05),
    ("樹脂 (POM)", 1.41),
    ("ゴム", 1.2),
    ("水 (既定)", 1.0),
];

/// 密度 (g/cm³) に合う材質の名前。どれにも合わなければ None
pub fn material_name(density: f64) -> Option<&'static str> {
    MATERIALS
        .iter()
        .find(|(_, d)| (d - density).abs() < 1e-9)
        .map(|(name, _)| *name)
}

/// 部品・アセンブリの質量特性 (ワールド座標)
#[derive(Clone, Debug, PartialEq)]
pub struct MassSummary {
    /// 体積 (モデル単位³)
    pub volume: f64,
    /// 表面積 (モデル単位²)
    pub area: f64,
    /// 質量 (kg)
    pub mass_kg: f64,
    /// 重心 (ワールド座標、モデル単位)
    pub centroid: DVec3,
    /// 重心を通りワールドの軸に平行な軸まわりの慣性モーメント (kg·m²)
    pub inertia: DVec3,
    /// 計算に入った部品の数
    pub parts: usize,
    /// 閉じた形でないなどで計算できなかった部品の名前
    pub skipped: Vec<String>,
}

/// 部品の形を見分ける鍵 (同じ形なら同じ値)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum ShapeKey {
    /// 箱は寸法と箱自身の位置・回転から形が決まる
    Box([u64; 9]),
    /// 取り込んだ部品・モデリングの部品・標準部品の本体は、形状の番号
    Shape(usize),
}

/// 部品1つ分の計算結果を引く鍵
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct PartKey {
    shape: ShapeKey,
    /// 密度 (g/cm³) のビット
    density: u64,
    /// ワールド行列の回転・拡大縮小の部分 (3×3) のビット
    linear: [u64; 9],
    /// 長さの単位 (m) のビット
    unit: u64,
}

fn shape_key(kind: &NodeKind) -> Option<ShapeKey> {
    match kind {
        NodeKind::Box(part) => {
            let mut v = [0u64; 9];
            for (i, x) in part
                .size
                .iter()
                .chain(&part.position)
                .chain(&part.rotation_deg)
                .enumerate()
            {
                v[i] = x.to_bits();
            }
            Some(ShapeKey::Box(v))
        }
        NodeKind::Imported { shape, .. } | NodeKind::StandardBody { shape, .. } => {
            Some(ShapeKey::Shape(shape.id))
        }
        NodeKind::Assembly { .. } | NodeKind::Standard(_) => None,
    }
}

fn linear_bits(world: DMat4) -> [u64; 9] {
    let cols = [world.x_axis, world.y_axis, world.z_axis];
    let mut v = [0u64; 9];
    for (i, c) in cols.iter().enumerate() {
        v[i * 3] = c.x.to_bits();
        v[i * 3 + 1] = c.y.to_bits();
        v[i * 3 + 2] = c.z.to_bits();
    }
    v
}

/// 部品ごとの質量特性を覚えておく。プロパティの欄が毎フレーム引くので、形・密度・向きが
/// 変わったときだけ計算する。重心の球を出す部品もここに置く
#[derive(bevy_ecs::prelude::Resource, Default)]
pub struct MassCache {
    /// 平行移動を除いたワールド座標での結果 (計算できなければ理由)
    parts: HashMap<PartKey, Result<MassProperties, String>>,
    /// 3D に重心の球を出すノードと、その重心 (ワールド座標)
    pub centroid: Option<(NodeId, DVec3)>,
}

/// 覚えておく部品の数の上限。超えたら忘れて計算し直す
const CACHE_LIMIT: usize = 4096;

impl MassCache {
    /// ノード `id` (部品かアセンブリ) の質量特性。中に形を持つ部品が無ければ None
    pub fn summary(
        &mut self,
        model: &Model,
        store: &dyn SolidStore,
        id: NodeId,
    ) -> Option<MassSummary> {
        let m_per_unit = meters_per_unit(&model.unit);
        // 部品ごとの (名前, 鍵, ワールド行列, 密度)
        let mut parts = Vec::new();
        model.visit_parts(|part| {
            if !part.path.contains(&id) {
                return;
            }
            let Some(node) = model.get(part.id) else {
                return;
            };
            if let Some(shape) = shape_key(&node.kind) {
                let key = PartKey {
                    shape,
                    density: node.physics.density.to_bits(),
                    linear: linear_bits(part.world),
                    unit: m_per_unit.to_bits(),
                };
                parts.push((part.id, key, part.world, node.physics.density));
            }
        });
        if parts.is_empty() {
            return None;
        }
        if self.parts.len() > CACHE_LIMIT {
            self.parts.clear();
        }
        let mut found = Vec::new();
        let mut skipped = Vec::new();
        for (part, key, world, density) in parts {
            let result = self.parts.entry(key).or_insert_with(|| {
                let linear =
                    DMat4::from_cols(world.x_axis, world.y_axis, world.z_axis, glam::DVec4::W);
                part_mass(model, store, part, linear, density, m_per_unit)
            });
            match result {
                Ok(p) => {
                    let mut p = *p;
                    p.centroid += world.w_axis.truncate();
                    found.push(p);
                }
                Err(_) => skipped.push(model.get(part).map(|n| n.name.clone()).unwrap_or_default()),
            }
        }
        let total = combine(&found);
        let m2 = m_per_unit * m_per_unit;
        Some(MassSummary {
            volume: total.volume,
            area: total.area,
            mass_kg: total.mass,
            centroid: total.centroid,
            inertia: total.inertia * m2,
            parts: found.len(),
            skipped,
        })
    }
}

/// 部品1つの質量特性。`matrix` で形を置き、質量は kg、長さはモデル単位で返す
fn part_mass(
    model: &Model,
    store: &dyn SolidStore,
    id: NodeId,
    matrix: DMat4,
    density_g_cm3: f64,
    m_per_unit: f64,
) -> Result<MassProperties, String> {
    // g/cm³ → kg/m³ → kg/(モデル単位)³
    let density = density_g_cm3 * 1000.0 * m_per_unit.powi(3);
    let parts: Vec<MassProperties> = part_solids(model, store, id)?
        .iter()
        .map(|b| b.transformed(matrix)?.mass_properties(density))
        .collect::<Result<_, _>>()
        .map_err(|_| "閉じた形でないので質量を計算できません".to_owned())?;
    Ok(combine(&parts))
}

/// 質量 (kg) を読みやすい単位の文字列にする
pub fn format_mass(kg: f64) -> String {
    if kg >= 1000.0 {
        format!("{:.3} t", kg / 1000.0)
    } else if kg >= 1.0 {
        format!("{kg:.3} kg")
    } else if kg * 1000.0 >= 1.0 - 1e-9 {
        format!("{:.1} g", kg * 1000.0)
    } else {
        format!("{:.2} mg", kg * 1e6)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::MemoryStore;
    use super::*;
    use crate::model::BoxPart;

    #[test]
    fn steel_box_mass_and_centroid() {
        let store = MemoryStore::default();
        let mut model = Model::default();
        model.unit = "mm".into();
        // 100 mm 角の鉄の箱 = 1000 cm³ × 7.85 g/cm³ = 7.85 kg
        let id = model.add_box([100.0, 100.0, 100.0], [10.0, 50.0, -20.0]);
        model.get_mut(id).unwrap().physics.density = 7.85;
        let mut cache = MassCache::default();
        let s = cache.summary(&model, &store, id).unwrap();
        assert!((s.volume - 1.0e6).abs() < 1.0, "{}", s.volume);
        assert!((s.mass_kg - 7.85).abs() < 1e-3, "{}", s.mass_kg);
        assert!((s.centroid - DVec3::new(10.0, 50.0, -20.0)).length() < 1e-6);
        // 立方体の慣性モーメント m a² / 6 (a = 0.1 m)
        let i = 7.85 * 0.01 / 6.0;
        assert!((s.inertia.x - i).abs() < 1e-6, "{}", s.inertia.x);
        assert_eq!(s.parts, 1);
        assert!(s.skipped.is_empty());
    }

    #[test]
    fn assembly_sums_parts_and_moving_does_not_recompute() {
        let store = MemoryStore::default();
        let mut model = Model::default();
        model.unit = "mm".into();
        let group = model.add_node(
            None,
            "組".into(),
            DMat4::IDENTITY,
            NodeKind::Assembly { product: None },
        );
        let cube = |position| {
            NodeKind::Box(BoxPart {
                size: [100.0; 3],
                position,
                rotation_deg: [0.0; 3],
            })
        };
        let a = model.add_node(Some(group), "a".into(), DMat4::IDENTITY, cube([0.0; 3]));
        // b は組の中で +X に 200 ずらす
        model.add_node(
            Some(group),
            "b".into(),
            DMat4::from_translation(DVec3::new(200.0, 0.0, 0.0)),
            cube([0.0; 3]),
        );
        let mut cache = MassCache::default();
        let s = cache.summary(&model, &store, group).unwrap();
        assert_eq!(s.parts, 2);
        assert!((s.mass_kg - 2.0).abs() < 1e-3, "{}", s.mass_kg);
        assert!((s.centroid.x - 100.0).abs() < 1e-6);
        // 2 つの箱は同じ形・同じ向きなので、計算は 1 回分しか覚えていない
        assert_eq!(cache.parts.len(), 1);
        // 組ごと動かしても計算し直さず、重心だけ動く
        model.get_mut(group).unwrap().placement =
            DMat4::from_translation(DVec3::new(0.0, 0.0, 500.0));
        let s = cache.summary(&model, &store, group).unwrap();
        assert!((s.centroid - DVec3::new(100.0, 0.0, 500.0)).length() < 1e-6);
        assert_eq!(cache.parts.len(), 1);
        // 材質を変えると質量が変わる
        model.get_mut(a).unwrap().physics.density = 2.68;
        let s = cache.summary(&model, &store, group).unwrap();
        assert!((s.mass_kg - 3.68).abs() < 1e-3, "{}", s.mass_kg);
    }

    #[test]
    fn material_names_and_mass_text() {
        assert_eq!(material_name(7.85), Some("鉄 (SS400)"));
        assert_eq!(material_name(3.3), None);
        assert_eq!(format_mass(7.85), "7.850 kg");
        assert_eq!(format_mass(0.0125), "12.5 g");
        assert_eq!(format_mass(2500.0), "2.500 t");
        assert_eq!(format_mass(1e-6), "1.00 mg");
        assert_eq!(format_mass(0.001), "1.0 g");
    }
}
