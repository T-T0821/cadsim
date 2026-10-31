//! 物理エンジンのアダプター: [`PhysicsEngine`] を Rapier (rapier3d, 純 Rust) で実装する
//!
//! Rapier は f32 で計算するので、ここで f64 ⇔ f32 を変換する。ベルトコンベアは
//! 接触点の「接線方向の目標相対速度」を書き換えるフックで表す (ベルト自体は動かない)。

use std::collections::HashMap;

use glam::{DVec3, Quat, Vec3};
use rapier3d::prelude::*;

use cadsim_core::physics::world::{BodyDesc, BodyKey, BodyKind, Iso, PhysicsEngine, ShapeDesc};

/// ベルトの速度を collider の user_data に詰めたときの印 (最上位ビット)
const BELT_FLAG: u128 = 1 << 127;

pub struct RapierEngine {
    world: PhysicsWorld,
    bodies: HashMap<BodyKey, RigidBodyHandle>,
}

impl RapierEngine {
    /// `gravity` はモデル単位 / 秒²、`meters_per_unit` は長さの単位 1 つが何メートルか。
    pub fn new(gravity: DVec3, meters_per_unit: f64) -> Self {
        let mut world = PhysicsWorld::new();
        world.gravity = gravity.as_vec3();
        // 許容めり込みなどの既定値はメートル基準なので、単位に合わせて伸ばす
        world.integration_parameters.length_unit = (1.0 / meters_per_unit) as f32;
        // 接触を予測する距離 (既定 2 cm) を 2 mm に縮める。広いと、ベルト面のすぐ下に下がった
        // ストッパの角のような「当たっていない」接触でワークが止まってしまう。
        world.integration_parameters.normalized_prediction_distance = 0.002;
        Self {
            world,
            bodies: HashMap::new(),
        }
    }
}

fn to_pose(iso: Iso) -> Pose {
    Pose::from_parts(
        iso.translation.as_vec3(),
        iso.rotation.as_quat().normalize(),
    )
}

fn from_pose(pose: &Pose) -> Iso {
    Iso::new(pose.rotation.as_dquat(), pose.translation.as_dvec3())
}

fn to_vec(points: &[DVec3]) -> Vec<Vec3> {
    points.iter().map(|p| p.as_vec3()).collect()
}

fn collider(shape: &ShapeDesc) -> Option<ColliderBuilder> {
    Some(match shape {
        ShapeDesc::Cuboid { half, offset } => {
            let h = half.as_vec3();
            ColliderBuilder::cuboid(h.x, h.y, h.z).position(to_pose(*offset))
        }
        ShapeDesc::ConvexHull { points } => ColliderBuilder::convex_hull(&to_vec(points))?,
        ShapeDesc::ConvexDecomposition { vertices, indices } => {
            ColliderBuilder::convex_decomposition(&to_vec(vertices), indices)
        }
        ShapeDesc::TriMesh { vertices, indices } => {
            ColliderBuilder::trimesh(to_vec(vertices), indices.clone()).ok()?
        }
    })
}

/// ベルトの速度 (剛体の座標系, f32 × 3) を user_data に詰める
fn pack_belt(v: Vec3) -> u128 {
    let bits = |f: f32| u128::from(f.to_bits());
    BELT_FLAG | bits(v.x) | (bits(v.y) << 32) | (bits(v.z) << 64)
}

fn unpack_belt(data: u128) -> Option<Vec3> {
    if data & BELT_FLAG == 0 {
        return None;
    }
    let f = |shift: u32| f32::from_bits((data >> shift) as u32);
    Some(Vec3::new(f(0), f(32), f(64)))
}

impl PhysicsEngine for RapierEngine {
    fn add_body(&mut self, key: BodyKey, desc: &BodyDesc) -> bool {
        let Some(builder) = collider(&desc.shape) else {
            return false;
        };
        self.remove_body(key);
        let body = match desc.kind {
            BodyKind::Fixed => RigidBodyBuilder::fixed(),
            BodyKind::Kinematic => RigidBodyBuilder::kinematic_position_based(),
            // 速く落ちる小さいワークが薄い板をすり抜けないよう、連続衝突判定を入れる。
            // ベルトに載って一定速度で流れているワークを止まったとみなして眠らせないよう、眠らせない。
            BodyKind::Dynamic => RigidBodyBuilder::dynamic()
                .ccd_enabled(true)
                .can_sleep(false),
        }
        .pose(to_pose(desc.pose));
        let mut builder = builder
            .friction(desc.friction as f32)
            .restitution(desc.restitution as f32)
            .density(desc.density as f32);
        if let Some(v) = desc.belt_velocity {
            builder = builder
                .user_data(pack_belt(v.as_vec3()))
                .active_hooks(ActiveHooks::MODIFY_SOLVER_CONTACTS);
        }
        let (handle, _) = self.world.insert(body, builder);
        self.bodies.insert(key, handle);
        true
    }

    fn remove_body(&mut self, key: BodyKey) {
        if let Some(handle) = self.bodies.remove(&key) {
            self.world.remove_body(handle);
        }
    }

    fn set_kinematic_target(&mut self, key: BodyKey, pose: Iso) {
        if let Some(body) = self
            .bodies
            .get(&key)
            .and_then(|h| self.world.bodies.get_mut(*h))
            && body.is_kinematic()
        {
            body.set_next_kinematic_position(to_pose(pose));
        }
    }

    fn step(&mut self, dt: f64) {
        self.world.integration_parameters.dt = dt as f32;
        self.world.step_with_events(&BeltHooks, &());
    }

    fn pose(&self, key: BodyKey) -> Option<Iso> {
        let body = self.world.bodies.get(*self.bodies.get(&key)?)?;
        Some(from_pose(body.position()))
    }

    fn dynamic_poses(&self) -> Vec<(BodyKey, Iso)> {
        self.bodies
            .iter()
            .filter_map(|(key, handle)| {
                let body = self.world.bodies.get(*handle)?;
                body.is_dynamic()
                    .then(|| (*key, from_pose(body.position())))
            })
            .collect()
    }
}

/// ベルトに触れている接触の、接線方向の目標相対速度をベルトの速度にする
struct BeltHooks;

impl BeltHooks {
    /// collider がベルトなら、表面のワールドでの速度
    fn belt_velocity(context: &ContactModificationContext, handle: ColliderHandle) -> Vec3 {
        let collider = &context.colliders[handle];
        let Some(local) = unpack_belt(collider.user_data) else {
            return Vec3::ZERO;
        };
        let rotation: Quat = collider
            .parent()
            .and_then(|h| context.bodies.get(h))
            .map_or(collider.position().rotation, |b| b.position().rotation);
        rotation * local
    }
}

impl PhysicsHooks for BeltHooks {
    fn modify_solver_contacts(&self, context: &mut ContactModificationContext) {
        let v1 = Self::belt_velocity(context, context.collider1);
        let v2 = Self::belt_velocity(context, context.collider2);
        let Some(manifold) = context.rigid_mut() else {
            return;
        };
        let normal = *manifold.normal;
        // Rapier の目標相対速度は「1 の表面 − 2 の表面」の向き。法線方向の成分は捨てる。
        let relative = v1 - v2;
        let tangent = relative - normal * relative.dot(normal);
        for contact in manifold.solver_contacts.iter_mut() {
            contact.tangent_velocity = tangent;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadsim_core::physics::world::{PhysicsRun, STEP_DT, box_triangles, indexed_mesh};

    fn cuboid(kind: BodyKind, center: DVec3, half: DVec3) -> BodyDesc {
        BodyDesc {
            kind,
            pose: Iso::from_translation(center),
            shape: ShapeDesc::Cuboid {
                half,
                offset: Iso::IDENTITY,
            },
            friction: 0.8,
            restitution: 0.0,
            density: 1e-6,
            belt_velocity: None,
        }
    }

    fn engine() -> RapierEngine {
        RapierEngine::new(DVec3::new(0.0, -9810.0, 0.0), 0.001)
    }

    fn run_for(run: &mut PhysicsRun<RapierEngine>, seconds: f64) {
        let frames = (seconds * 60.0) as usize;
        for _ in 0..frames {
            run.advance(1.0 / 60.0, &[]);
        }
    }

    #[test]
    fn belt_data_round_trips() {
        let v = Vec3::new(1.5, -2.0, 300.0);
        assert_eq!(unpack_belt(pack_belt(v)), Some(v));
        assert_eq!(unpack_belt(0), None);
    }

    #[test]
    fn a_box_falls_and_rests_on_the_floor() {
        let mut e = engine();
        let floor = cuboid(
            BodyKind::Fixed,
            DVec3::ZERO,
            DVec3::new(1000.0, 10.0, 1000.0),
        );
        assert!(e.add_body(BodyKey::Part(1), &floor));
        let work = cuboid(
            BodyKind::Dynamic,
            DVec3::new(0.0, 300.0, 0.0),
            DVec3::splat(25.0),
        );
        assert!(e.add_body(BodyKey::Part(9), &work));
        let mut run = PhysicsRun::new(e);
        run_for(&mut run, 1.5);
        let y = run.engine.pose(BodyKey::Part(9)).unwrap().translation.y;
        // 床の上面 10 + 半分の高さ 25
        assert!((y - 35.0).abs() < 1.5, "y = {y}");
    }

    #[test]
    fn a_belt_carries_the_work_along_its_axis() {
        let mut e = engine();
        let mut belt = cuboid(
            BodyKind::Fixed,
            DVec3::ZERO,
            DVec3::new(2000.0, 10.0, 200.0),
        );
        belt.belt_velocity = Some(DVec3::new(200.0, 0.0, 0.0));
        e.add_body(BodyKey::Part(1), &belt);
        let work = cuboid(
            BodyKind::Dynamic,
            DVec3::new(0.0, 40.0, 0.0),
            DVec3::splat(25.0),
        );
        e.add_body(BodyKey::Part(9), &work);
        let mut run = PhysicsRun::new(e);
        run_for(&mut run, 2.0);
        let p = run.engine.pose(BodyKey::Part(9)).unwrap().translation;
        // 落ちて加速する時間を除いても 2 秒で 300 mm 以上は進む。速度を超えては進まない。
        assert!(p.x > 300.0 && p.x < 410.0, "x = {}", p.x);
        assert!(p.z.abs() < 5.0);
    }

    #[test]
    fn a_kinematic_pusher_moves_the_work() {
        let mut e = engine();
        let floor = cuboid(
            BodyKind::Fixed,
            DVec3::ZERO,
            DVec3::new(2000.0, 10.0, 2000.0),
        );
        e.add_body(BodyKey::Part(1), &floor);
        let start = DVec3::new(-100.0, 60.0, 0.0);
        let pusher = cuboid(BodyKind::Kinematic, start, DVec3::new(10.0, 50.0, 100.0));
        e.add_body(BodyKey::Part(2), &pusher);
        let work = cuboid(
            BodyKind::Dynamic,
            DVec3::new(0.0, 35.0, 0.0),
            DVec3::splat(25.0),
        );
        e.add_body(BodyKey::Part(9), &work);
        let mut run = PhysicsRun::new(e);
        run_for(&mut run, 0.3);
        for frame in 1..=60 {
            let x = start.x + 300.0 * f64::from(frame) / 60.0;
            let target = Iso::from_translation(DVec3::new(x, start.y, 0.0));
            run.advance(1.0 / 60.0, &[(BodyKey::Part(2), target)]);
        }
        let x = run.engine.pose(BodyKey::Part(9)).unwrap().translation.x;
        // 押し板は x = 200 まで来た。ワークは押し板の前 (板の厚み 10 + ワーク 25) にいる
        assert!(x > 180.0, "x = {x}");
        assert!(run.steps >= 60 * 2 && STEP_DT > 0.0);
    }

    #[test]
    fn convex_hull_and_decomposition_shapes_are_built() {
        let mut e = engine();
        let corners: Vec<DVec3> = (0..8)
            .map(|i| {
                DVec3::new(
                    if i & 1 == 0 { -1.0 } else { 1.0 },
                    if i & 2 == 0 { -1.0 } else { 1.0 },
                    if i & 4 == 0 { -1.0 } else { 1.0 },
                ) * 20.0
            })
            .collect();
        let mut desc = cuboid(BodyKind::Dynamic, DVec3::ZERO, DVec3::ONE);
        desc.shape = ShapeDesc::ConvexHull {
            points: corners.clone(),
        };
        assert!(e.add_body(BodyKey::Part(9), &desc));
        desc.shape = ShapeDesc::ConvexHull {
            points: vec![DVec3::ZERO; 3],
        };
        assert!(!e.add_body(BodyKey::Part(2), &desc));
        assert!(e.pose(BodyKey::Part(9)).is_some() && e.pose(BodyKey::Part(2)).is_none());

        // L 字の溝 (凹んだ形) も凸分解なら作れる
        let mut triangles = box_triangles(
            DVec3::new(100.0, 20.0, 50.0),
            Iso::from_translation(DVec3::new(0.0, 10.0, 0.0)),
        );
        triangles.extend(box_triangles(
            DVec3::new(20.0, 80.0, 50.0),
            Iso::from_translation(DVec3::new(-40.0, 60.0, 0.0)),
        ));
        let (vertices, indices) = indexed_mesh(&triangles);
        desc.shape = ShapeDesc::ConvexDecomposition { vertices, indices };
        assert!(e.add_body(BodyKey::Part(3), &desc));
    }
}
