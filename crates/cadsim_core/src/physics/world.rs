//! 物理演算を進めるユースケース (Bevy に依存しない)
//!
//! 物理エンジンそのものはポート [`PhysicsEngine`] の向こうにあり、ここは
//! 「部品の設定から剛体を作る」「一定の刻みで進める」「機構で動く部品を目標の位置へ動かす」だけを受け持つ。
//! エンジンの実装はアプリ側の `physics/rapier.rs` (アダプター)。

use std::collections::HashMap;

use glam::{DQuat, DVec3};

use super::settings::{BodyMode, ColliderShape, PartPhysics};

/// 剛体の名前。部品はノード id で引く。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BodyKey {
    Part(u32),
}

/// 回転してから平行移動する剛体変換 (ワールド座標)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Iso {
    pub rotation: DQuat,
    pub translation: DVec3,
}

impl Default for Iso {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Iso {
    pub const IDENTITY: Iso = Iso {
        rotation: DQuat::IDENTITY,
        translation: DVec3::ZERO,
    };

    pub fn new(rotation: DQuat, translation: DVec3) -> Self {
        Self {
            rotation,
            translation,
        }
    }

    pub fn from_translation(translation: DVec3) -> Self {
        Self::new(DQuat::IDENTITY, translation)
    }

    pub fn apply(&self, p: DVec3) -> DVec3 {
        self.rotation * p + self.translation
    }

    /// `a` から `b` への途中の姿勢 (t = 0..1)
    pub fn lerp(a: Iso, b: Iso, t: f64) -> Iso {
        Iso {
            rotation: a.rotation.slerp(b.rotation, t),
            translation: a.translation.lerp(b.translation, t),
        }
    }
}

/// エンジンでの剛体の種類
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    Fixed,
    /// 目標の姿勢を毎刻み与えて動かす。ぶつかったものは押すが、押し返されない。
    Kinematic,
    Dynamic,
}

/// 当たり判定の形 (剛体の座標系)
#[derive(Clone, Debug, PartialEq)]
pub enum ShapeDesc {
    Cuboid {
        half: DVec3,
        offset: Iso,
    },
    ConvexHull {
        points: Vec<DVec3>,
    },
    ConvexDecomposition {
        vertices: Vec<DVec3>,
        indices: Vec<[u32; 3]>,
    },
    TriMesh {
        vertices: Vec<DVec3>,
        indices: Vec<[u32; 3]>,
    },
}

/// エンジンに剛体を1つ足すための情報
#[derive(Clone, Debug, PartialEq)]
pub struct BodyDesc {
    pub kind: BodyKind,
    pub pose: Iso,
    pub shape: ShapeDesc,
    pub friction: f64,
    pub restitution: f64,
    /// 密度 (kg / モデル単位³)
    pub density: f64,
    /// ベルト表面の速度 (剛体の座標系)。上に載ったものを運ぶ。
    pub belt_velocity: Option<DVec3>,
}

/// 物理エンジンのポート。アダプター (`rapier.rs`) が実装する。
pub trait PhysicsEngine {
    /// 剛体を足す。形が作れなかったら (頂点が足りないなど) false。同じ名前があれば置き換える。
    fn add_body(&mut self, key: BodyKey, desc: &BodyDesc) -> bool;
    fn remove_body(&mut self, key: BodyKey);
    /// 機構で動く剛体の、次の刻みの終わりでの姿勢
    fn set_kinematic_target(&mut self, key: BodyKey, pose: Iso);
    /// 時間 `dt` 秒だけ進める
    fn step(&mut self, dt: f64);
    fn pose(&self, key: BodyKey) -> Option<Iso>;
    /// 物理で動く剛体すべての姿勢
    fn dynamic_poses(&self) -> Vec<(BodyKey, Iso)>;
}

/// 物理の刻み (秒)。ベルトの上を滑らかに流れ、100 個程度なら 1 フレームに数回進めても軽い。
pub const STEP_DT: f64 = 1.0 / 120.0;
/// 1 回の `advance` で進める刻みの上限。重いフレームが続いても固まらないよう、超えた分は捨てる。
const MAX_STEPS_PER_ADVANCE: u32 = 8;

/// 一定の刻みで物理を進める。画面のフレームの長さが揺れても、結果は刻みの数だけで決まる。
pub struct PhysicsRun<E> {
    pub engine: E,
    /// 開始からの時刻 (秒)
    pub time: f64,
    pub steps: u64,
    accumulator: f64,
    /// 機構で動く剛体の、前の刻みでの目標 (刻みの間を補間して滑らかに動かす)
    last_targets: HashMap<BodyKey, Iso>,
}

impl<E: PhysicsEngine> PhysicsRun<E> {
    pub fn new(engine: E) -> Self {
        Self {
            engine,
            time: 0.0,
            steps: 0,
            accumulator: 0.0,
            last_targets: HashMap::new(),
        }
    }

    /// `dt` 秒進める。`targets` は機構で動く剛体の、この時間の終わりでの姿勢。
    /// 進めた刻みの数を返す。
    pub fn advance(&mut self, dt: f64, targets: &[(BodyKey, Iso)]) -> u32 {
        self.accumulator += dt.max(0.0);
        let wanted = (self.accumulator / STEP_DT + 1e-9).floor() as u32;
        let steps = wanted.min(MAX_STEPS_PER_ADVANCE);
        if steps == 0 {
            return 0;
        }
        self.accumulator = if wanted > steps {
            0.0
        } else {
            (self.accumulator - f64::from(steps) * STEP_DT).max(0.0)
        };
        for i in 1..=steps {
            let t = f64::from(i) / f64::from(steps);
            for &(key, target) in targets {
                let from = self
                    .last_targets
                    .get(&key)
                    .copied()
                    .or_else(|| self.engine.pose(key))
                    .unwrap_or(target);
                self.engine
                    .set_kinematic_target(key, Iso::lerp(from, target, t));
            }
            self.engine.step(STEP_DT);
            self.time += STEP_DT;
            self.steps += 1;
        }
        self.last_targets.extend(targets.iter().copied());
        steps
    }
}

// ---------------------------------------------------------------------------
// 部品 → 剛体
// ---------------------------------------------------------------------------

/// 部品の形 (部品の座標系)
pub enum PartGeometry<'a> {
    /// 箱の部品。`offset` は部品の座標系の中での箱の位置と向き。
    Box { size: DVec3, offset: Iso },
    /// 三角形の集まり (取り込んだ部品の表示用メッシュ)
    Mesh(&'a [[DVec3; 3]]),
}

/// 部品1つ分の入力
pub struct PartInput<'a> {
    pub settings: PartPhysics,
    pub geometry: PartGeometry<'a>,
    /// 開始時のワールドでの姿勢
    pub pose: Iso,
    /// 長さの単位 1 つが何メートルか (密度の換算に使う)
    pub meters_per_unit: f64,
}

/// 部品の設定から剛体を作る。当たらない部品や、形のない部品は None。
pub fn part_body(part: &PartInput) -> Option<BodyDesc> {
    let s = &part.settings;
    let kind = match s.mode {
        BodyMode::Ignored => return None,
        BodyMode::Fixed => BodyKind::Fixed,
        BodyMode::Mechanism => BodyKind::Kinematic,
        BodyMode::Dynamic => BodyKind::Dynamic,
    };
    let is_box = matches!(part.geometry, PartGeometry::Box { .. });
    let shape = match (s.shape.resolve(s.mode, is_box), &part.geometry) {
        (ColliderShape::Box, PartGeometry::Box { size, offset }) => ShapeDesc::Cuboid {
            half: *size / 2.0,
            offset: *offset,
        },
        (ColliderShape::Box, PartGeometry::Mesh(triangles)) => {
            let (min, max) = bounds(triangles)?;
            ShapeDesc::Cuboid {
                half: (max - min) / 2.0,
                offset: Iso::from_translation((min + max) / 2.0),
            }
        }
        (shape, geometry) => {
            let triangles = match geometry {
                PartGeometry::Box { size, offset } => box_triangles(*size, *offset),
                PartGeometry::Mesh(triangles) => triangles.to_vec(),
            };
            let (vertices, indices) = indexed_mesh(&triangles);
            if vertices.len() < 4 {
                return None;
            }
            match shape {
                ColliderShape::ConvexDecomposition => {
                    ShapeDesc::ConvexDecomposition { vertices, indices }
                }
                ColliderShape::Mesh => ShapeDesc::TriMesh { vertices, indices },
                _ => ShapeDesc::ConvexHull { points: vertices },
            }
        }
    };
    // g/cm³ → kg/m³ → kg/モデル単位³
    let density = s.density * 1000.0 * part.meters_per_unit.powi(3);
    Some(BodyDesc {
        kind,
        pose: part.pose,
        shape,
        friction: s.friction,
        restitution: s.restitution,
        density,
        belt_velocity: s.belt.map(|b| b.local_velocity()),
    })
}

fn bounds(triangles: &[[DVec3; 3]]) -> Option<(DVec3, DVec3)> {
    let mut points = triangles.iter().flatten();
    let first = *points.next()?;
    Some(points.fold((first, first), |(min, max), p| (min.min(*p), max.max(*p))))
}

/// 箱の12枚の三角形
pub fn box_triangles(size: DVec3, offset: Iso) -> Vec<[DVec3; 3]> {
    let h = size / 2.0;
    let corner = |i: usize| {
        offset.apply(DVec3::new(
            if i & 1 == 0 { -h.x } else { h.x },
            if i & 2 == 0 { -h.y } else { h.y },
            if i & 4 == 0 { -h.z } else { h.z },
        ))
    };
    const QUADS: [[usize; 4]; 6] = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ];
    QUADS
        .iter()
        .flat_map(|q| {
            [
                [corner(q[0]), corner(q[1]), corner(q[2])],
                [corner(q[0]), corner(q[2]), corner(q[3])],
            ]
        })
        .collect()
}

/// 三角形の並びを、重なった頂点をまとめた頂点 + 番号の形にする。
/// 面ごとに分かれたメッシュでも、凸分解やメッシュの当たり判定がつながった形として扱えるようにする。
pub fn indexed_mesh(triangles: &[[DVec3; 3]]) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let Some((min, max)) = bounds(triangles) else {
        return (Vec::new(), Vec::new());
    };
    // 形の大きさに対して十分小さい格子で丸め、同じ点とみなす
    let tolerance = ((max - min).max_element() * 1e-6).max(1e-12);
    let mut lookup: HashMap<[i64; 3], u32> = HashMap::new();
    let mut vertices = Vec::new();
    let mut indices = Vec::with_capacity(triangles.len());
    for triangle in triangles {
        let ids = triangle.map(|p| {
            let key = ((p - min) / tolerance).round().as_i64vec3().to_array();
            *lookup.entry(key).or_insert_with(|| {
                vertices.push(p);
                (vertices.len() - 1) as u32
            })
        });
        // 潰れた三角形は捨てる
        if ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2] {
            indices.push(ids);
        }
    }
    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::settings::BeltDrive;

    /// 呼ばれた操作を覚えるだけのエンジン
    #[derive(Default)]
    struct Recorder {
        steps: Vec<f64>,
        targets: Vec<(BodyKey, Iso)>,
    }

    impl PhysicsEngine for Recorder {
        fn add_body(&mut self, _: BodyKey, _: &BodyDesc) -> bool {
            true
        }
        fn remove_body(&mut self, _: BodyKey) {}
        fn set_kinematic_target(&mut self, key: BodyKey, pose: Iso) {
            self.targets.push((key, pose));
        }
        fn step(&mut self, dt: f64) {
            self.steps.push(dt);
        }
        fn pose(&self, _: BodyKey) -> Option<Iso> {
            None
        }
        fn dynamic_poses(&self) -> Vec<(BodyKey, Iso)> {
            Vec::new()
        }
    }

    #[test]
    fn advances_in_fixed_steps_and_keeps_the_remainder() {
        let mut run = PhysicsRun::new(Recorder::default());
        assert_eq!(run.advance(STEP_DT * 0.6, &[]), 0);
        assert_eq!(run.advance(STEP_DT * 0.6, &[]), 1);
        assert_eq!(run.advance(STEP_DT * 2.0, &[]), 2);
        assert!(run.engine.steps.iter().all(|&dt| dt == STEP_DT));
        assert_eq!(run.steps, 3);
    }

    #[test]
    fn long_frames_are_capped() {
        let mut run = PhysicsRun::new(Recorder::default());
        assert_eq!(run.advance(1.0, &[]), MAX_STEPS_PER_ADVANCE);
        // 捨てた分は次に持ち越さない
        assert_eq!(run.advance(0.0, &[]), 0);
    }

    #[test]
    fn kinematic_targets_are_interpolated_between_frames() {
        let key = BodyKey::Part(1);
        let mut run = PhysicsRun::new(Recorder::default());
        run.advance(STEP_DT, &[(key, Iso::IDENTITY)]);
        run.engine.targets.clear();
        let goal = Iso::from_translation(DVec3::new(4.0, 0.0, 0.0));
        run.advance(STEP_DT * 4.0, &[(key, goal)]);
        let xs: Vec<f64> = run
            .engine
            .targets
            .iter()
            .map(|(_, p)| p.translation.x)
            .collect();
        assert_eq!(xs, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn shared_corners_are_merged() {
        let triangles = box_triangles(DVec3::ONE, Iso::IDENTITY);
        let (vertices, indices) = indexed_mesh(&triangles);
        assert_eq!(vertices.len(), 8);
        assert_eq!(indices.len(), 12);
    }

    fn input(settings: PartPhysics, geometry: PartGeometry) -> PartInput {
        PartInput {
            settings,
            geometry,
            pose: Iso::IDENTITY,
            meters_per_unit: 0.001,
        }
    }

    #[test]
    fn parts_become_bodies_by_mode_and_shape() {
        let cube = PartGeometry::Box {
            size: DVec3::splat(100.0),
            offset: Iso::IDENTITY,
        };
        let dynamic = PartPhysics {
            mode: BodyMode::Dynamic,
            ..Default::default()
        };
        let body = part_body(&input(dynamic, cube)).unwrap();
        assert_eq!(body.kind, BodyKind::Dynamic);
        assert!(matches!(body.shape, ShapeDesc::Cuboid { half, .. } if half == DVec3::splat(50.0)));
        // 1 g/cm³ の 100 mm 角は 1 kg
        let volume = 100.0f64.powi(3);
        assert!((body.density * volume - 1.0).abs() < 1e-9);

        let ignored = PartPhysics {
            mode: BodyMode::Ignored,
            ..Default::default()
        };
        let cube = PartGeometry::Box {
            size: DVec3::ONE,
            offset: Iso::IDENTITY,
        };
        assert!(part_body(&input(ignored, cube)).is_none());

        // 取り込んだ部品は、機構で動くならメッシュ、物理で動くなら凸包
        let triangles = box_triangles(DVec3::ONE, Iso::IDENTITY);
        let body = part_body(&input(
            PartPhysics::default(),
            PartGeometry::Mesh(&triangles),
        ))
        .unwrap();
        assert_eq!(body.kind, BodyKind::Kinematic);
        assert!(matches!(body.shape, ShapeDesc::TriMesh { .. }));
        let body = part_body(&input(dynamic, PartGeometry::Mesh(&triangles))).unwrap();
        assert!(matches!(body.shape, ShapeDesc::ConvexHull { ref points } if points.len() == 8));
    }

    #[test]
    fn belt_speed_is_passed_in_part_coordinates() {
        let belt = PartPhysics {
            belt: Some(BeltDrive::default()),
            ..Default::default()
        };
        let cube = PartGeometry::Box {
            size: DVec3::ONE,
            offset: Iso::IDENTITY,
        };
        let body = part_body(&input(belt, cube)).unwrap();
        assert_eq!(body.belt_velocity, Some(DVec3::new(200.0, 0.0, 0.0)));
    }
}
