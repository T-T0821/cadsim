//! 物理の設定のファイル上の形 (.plantwright の JSON)。アダプター層。
//!
//! 内部の型 (`settings.rs` / `cable.rs`) と分けておき、内部を変えてもファイルの互換を保つ。
//! 部品は「最上位から順にたどる子の番号」(寸法・注記と同じ) で書く。

use serde::{Deserialize, Serialize};

use plantwright_core::physics::cable::{Cable, CableAnchor, CableKind};
use plantwright_core::physics::settings::{
    BeltDrive, BodyMode, ColliderShape, LocalAxis, PartPhysics,
};

/// 部品1つ分の物理の設定。既定値の項目は省く。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PartPhysicsRecord {
    #[serde(default)]
    pub mode: BodyModeRecord,
    #[serde(default)]
    pub shape: ShapeRecord,
    #[serde(default = "default_friction")]
    pub friction: f64,
    #[serde(default)]
    pub restitution: f64,
    #[serde(default = "default_density")]
    pub density: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub belt: Option<BeltRecord>,
}

fn default_friction() -> f64 {
    PartPhysics::default().friction
}

fn default_density() -> f64 {
    PartPhysics::default().density
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BodyModeRecord {
    Fixed,
    #[default]
    Mechanism,
    Dynamic,
    Ignored,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShapeRecord {
    #[default]
    Auto,
    Box,
    ConvexHull,
    ConvexDecomposition,
    Mesh,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AxisRecord {
    #[default]
    X,
    Y,
    Z,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct BeltRecord {
    pub axis: AxisRecord,
    #[serde(default)]
    pub reversed: bool,
    pub speed: f64,
}

impl PartPhysicsRecord {
    /// 既定値なら None (ファイルに書かない)
    pub fn from_settings(s: &PartPhysics) -> Option<Self> {
        if s.is_default() {
            return None;
        }
        Some(Self {
            mode: match s.mode {
                BodyMode::Fixed => BodyModeRecord::Fixed,
                BodyMode::Mechanism => BodyModeRecord::Mechanism,
                BodyMode::Dynamic => BodyModeRecord::Dynamic,
                BodyMode::Ignored => BodyModeRecord::Ignored,
            },
            shape: match s.shape {
                ColliderShape::Auto => ShapeRecord::Auto,
                ColliderShape::Box => ShapeRecord::Box,
                ColliderShape::ConvexHull => ShapeRecord::ConvexHull,
                ColliderShape::ConvexDecomposition => ShapeRecord::ConvexDecomposition,
                ColliderShape::Mesh => ShapeRecord::Mesh,
            },
            friction: s.friction,
            restitution: s.restitution,
            density: s.density,
            belt: s.belt.map(|b| BeltRecord {
                axis: match b.axis {
                    LocalAxis::X => AxisRecord::X,
                    LocalAxis::Y => AxisRecord::Y,
                    LocalAxis::Z => AxisRecord::Z,
                },
                reversed: b.reversed,
                speed: b.speed,
            }),
        })
    }

    pub fn to_settings(&self) -> PartPhysics {
        PartPhysics {
            mode: match self.mode {
                BodyModeRecord::Fixed => BodyMode::Fixed,
                BodyModeRecord::Mechanism => BodyMode::Mechanism,
                BodyModeRecord::Dynamic => BodyMode::Dynamic,
                BodyModeRecord::Ignored => BodyMode::Ignored,
            },
            shape: match self.shape {
                ShapeRecord::Auto => ColliderShape::Auto,
                ShapeRecord::Box => ColliderShape::Box,
                ShapeRecord::ConvexHull => ColliderShape::ConvexHull,
                ShapeRecord::ConvexDecomposition => ColliderShape::ConvexDecomposition,
                ShapeRecord::Mesh => ColliderShape::Mesh,
            },
            friction: self.friction,
            restitution: self.restitution,
            density: self.density,
            belt: self.belt.map(|b| BeltDrive {
                axis: match b.axis {
                    AxisRecord::X => LocalAxis::X,
                    AxisRecord::Y => LocalAxis::Y,
                    AxisRecord::Z => LocalAxis::Z,
                },
                reversed: b.reversed,
                speed: b.speed,
            }),
        }
    }

    /// 物理的にありえない値 (負の摩擦など) がないか
    pub fn is_valid(&self) -> bool {
        let finite = [self.friction, self.restitution, self.density]
            .iter()
            .all(|v| v.is_finite());
        finite
            && self.friction >= 0.0
            && (0.0..=1.0).contains(&self.restitution)
            && self.density > 0.0
            && self.belt.is_none_or(|b| b.speed.is_finite())
    }
}

/// ケーブル・ホース1本
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CableRecord {
    pub name: String,
    #[serde(default)]
    pub hose: bool,
    pub start: CableAnchorRecord,
    pub end: CableAnchorRecord,
    pub length: f64,
    pub diameter: f64,
    pub min_bend_radius: f64,
}

/// 紐の端。`node` がなければ `local` はワールド座標。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CableAnchorRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<Vec<usize>>,
    pub local: [f64; 3],
}

impl CableRecord {
    /// `path` は部品のノード id → 番号の道筋。部品が消えた端はワールドの位置にする。
    pub fn from_cable(cable: &Cable, path: impl Fn(u32) -> Option<Vec<usize>>) -> Self {
        let anchor = |a: &CableAnchor| CableAnchorRecord {
            node: a.node.and_then(&path),
            local: a.local.to_array(),
        };
        Self {
            name: cable.name.clone(),
            hose: cable.kind == CableKind::Hose,
            start: anchor(&cable.start),
            end: anchor(&cable.end),
            length: cable.length,
            diameter: cable.diameter,
            min_bend_radius: cable.min_bend_radius,
        }
    }

    /// 端の部品が見つからなければ None
    pub fn to_cable(&self, node: impl Fn(&[usize]) -> Option<u32>) -> Option<Cable> {
        let anchor = |a: &CableAnchorRecord| -> Option<CableAnchor> {
            let node = match &a.node {
                Some(path) => Some(node(path)?),
                None => None,
            };
            Some(CableAnchor {
                node,
                local: a.local.into(),
            })
        };
        Some(Cable {
            name: self.name.clone(),
            kind: if self.hose {
                CableKind::Hose
            } else {
                CableKind::Cable
            },
            start: anchor(&self.start)?,
            end: anchor(&self.end)?,
            length: self.length,
            diameter: self.diameter,
            min_bend_radius: self.min_bend_radius,
        })
    }

    pub fn is_valid(&self) -> bool {
        let values = [self.length, self.diameter, self.min_bend_radius];
        values.iter().all(|v| v.is_finite() && *v > 0.0)
            && self
                .start
                .local
                .iter()
                .chain(&self.end.local)
                .all(|v| v.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;

    #[test]
    fn default_settings_are_not_written() {
        assert!(PartPhysicsRecord::from_settings(&PartPhysics::default()).is_none());
    }

    #[test]
    fn settings_round_trip() {
        let s = PartPhysics {
            mode: BodyMode::Dynamic,
            shape: ColliderShape::ConvexDecomposition,
            friction: 0.3,
            restitution: 0.2,
            density: 7.8,
            belt: Some(BeltDrive {
                axis: LocalAxis::Z,
                reversed: true,
                speed: 150.0,
            }),
        };
        let record = PartPhysicsRecord::from_settings(&s).unwrap();
        let json = serde_json::to_string(&record).unwrap();
        let back: PartPhysicsRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.to_settings(), s);
        assert!(back.is_valid());
        // 省いた項目は既定値になる
        let minimal: PartPhysicsRecord = serde_json::from_str(r#"{"mode":"Fixed"}"#).unwrap();
        assert_eq!(
            minimal.to_settings().friction,
            PartPhysics::default().friction
        );
    }

    #[test]
    fn cable_anchors_use_node_paths() {
        let cable = Cable {
            name: "エアホース".into(),
            kind: CableKind::Hose,
            start: CableAnchor {
                node: Some(5),
                local: DVec3::new(1.0, 2.0, 3.0),
            },
            end: CableAnchor {
                node: None,
                local: DVec3::ZERO,
            },
            length: 500.0,
            diameter: 8.0,
            min_bend_radius: 40.0,
        };
        let record = CableRecord::from_cable(&cable, |id| (id == 5).then(|| vec![0, 2]));
        assert_eq!(record.start.node, Some(vec![0, 2]));
        assert!(record.is_valid());
        let back = record
            .to_cable(|path| (path == [0, 2]).then_some(9))
            .unwrap();
        assert_eq!(back.start.node, Some(9));
        assert_eq!(back.kind, CableKind::Hose);
        assert!(record.to_cable(|_| None).is_none());
    }
}
