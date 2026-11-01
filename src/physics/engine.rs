//! 物理エンジンのアダプター: [`PhysicsEngine`] を Bevy の物理プラグイン bevy_rapier3d で実装する
//!
//! 剛体は Bevy のエンティティ (`RigidBody` + `Collider` + `Transform`) 1つとして作り、姿勢の受け渡しは
//! プラグインの `Transform` との同期に任せる。物理を1刻み進めるのは、プラグインのシステムを入れた
//! スケジュール [`PhysicsStep`] を1回走らせること。いつ何刻み進めるかはユースケース (`PhysicsRun`) が決める。
//!
//! ベルトコンベアは、接触点の「接線方向の目標相対速度」を書き換えるフック [`BeltHooks`] で表す
//! (ベルト自体は動かない)。ベルトの速度は collider のエンティティに [`Belt`] として持たせる。

use std::collections::HashMap;

use bevy::ecs::schedule::ScheduleLabel;
use bevy::ecs::system::SystemParam;
use bevy::math::{DVec3, Quat, Vec3};
use bevy::prelude::*;
use bevy_rapier3d::pipeline::{BevyPhysicsHooks, ContactModificationContextView};
use bevy_rapier3d::plugin::context::RapierContextSimulation;
use bevy_rapier3d::plugin::{
    DefaultRapierContext, RapierConfiguration, RapierPhysicsPlugin, TimestepMode,
};
use bevy_rapier3d::prelude::{
    ActiveHooks, Ccd, Collider, ColliderMassProperties, Friction, Restitution, RigidBody, Sleeping,
};
use bevy_rapier3d::rapier::prelude::ColliderHandle;

use cadsim_core::physics::world::{
    BodyDesc, BodyKey, BodyKind, Iso, PhysicsEngine, STEP_DT, ShapeDesc,
};

/// 物理を1刻み進めるスケジュール。bevy_rapier3d のシステムはここに入れ、毎フレーム自動では走らせない。
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PhysicsStep;

/// bevy_rapier3d を [`PhysicsStep`] に組み込む
pub struct RapierBackendPlugin;

impl Plugin for RapierBackendPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(
            RapierPhysicsPlugin::<BeltHooks<'static, 'static>>::default().in_schedule(PhysicsStep),
        )
        // 1回走らせると STEP_DT だけ進む
        .insert_resource(TimestepMode::Fixed {
            dt: STEP_DT as f32,
            substeps: 1,
        })
        .init_resource::<Bodies>();
    }
}

/// 剛体の名前 → エンティティ
#[derive(Resource, Default)]
struct Bodies(HashMap<BodyKey, Entity>);

/// 物理の剛体のエンティティに付ける印
#[derive(Component, Clone, Copy, Debug)]
pub struct PhysicsBody(pub BodyKey);

/// ベルト表面の速度 (剛体の座標系)。ベルトの collider のエンティティに付ける。
#[derive(Component, Clone, Copy, Debug)]
pub struct Belt(pub Vec3);

/// Bevy の `World` を借りて [`PhysicsEngine`] として振る舞う
pub struct RapierEngine<'w> {
    world: &'w mut World,
}

impl<'w> RapierEngine<'w> {
    pub fn new(world: &'w mut World) -> Self {
        Self { world }
    }

    /// 剛体をすべて消し、重力と長さの単位を設定し直す。
    /// `gravity` はモデル単位 / 秒²、`meters_per_unit` は長さの単位 1 つが何メートルか。
    pub fn reset(&mut self, gravity: DVec3, meters_per_unit: f64) {
        self.clear();
        let mut contexts = self.world.query_filtered::<(
            &mut RapierConfiguration,
            &mut RapierContextSimulation,
        ), With<DefaultRapierContext>>();
        for (mut config, mut simulation) in contexts.iter_mut(self.world) {
            config.gravity = gravity.as_vec3();
            let parameters = &mut simulation.integration_parameters;
            // 許容めり込みなどの既定値はメートル基準なので、単位に合わせて伸ばす
            parameters.length_unit = (1.0 / meters_per_unit) as f32;
            // 接触を予測する距離 (既定 2 cm) を 2 mm に縮める。広いと、ベルト面のすぐ下に下がった
            // ストッパの角のような「当たっていない」接触でワークが止まってしまう。
            parameters.normalized_prediction_distance = 0.002;
        }
    }

    /// 剛体をすべて消す
    pub fn clear(&mut self) {
        let entities: Vec<Entity> = self
            .world
            .resource_mut::<Bodies>()
            .0
            .drain()
            .map(|(_, e)| e)
            .collect();
        for entity in entities {
            self.world.despawn(entity);
        }
    }

    pub fn world_mut(&mut self) -> &mut World {
        self.world
    }

    fn entity(&self, key: BodyKey) -> Option<Entity> {
        self.world.resource::<Bodies>().0.get(&key).copied()
    }
}

fn to_transform(iso: Iso) -> Transform {
    Transform::from_translation(iso.translation.as_vec3())
        .with_rotation(iso.rotation.as_quat().normalize())
}

fn from_transform(t: &Transform) -> Iso {
    Iso::new(t.rotation.as_dquat(), t.translation.as_dvec3())
}

fn to_vec(points: &[DVec3]) -> Vec<Vec3> {
    points.iter().map(|p| p.as_vec3()).collect()
}

/// 当たり判定の形 (剛体の座標系)
fn collider(shape: &ShapeDesc) -> Option<Collider> {
    Some(match shape {
        ShapeDesc::Cuboid { half, offset } => {
            let h = half.as_vec3();
            let cuboid = Collider::cuboid(h.x, h.y, h.z);
            if *offset == Iso::IDENTITY {
                cuboid
            } else {
                // 剛体の原点からずれた箱は、箱1つの複合形状にしてずらす。子のエンティティに
                // collider を付けてずらすと、機構で動く剛体の目標の姿勢が最初の1回しか効かない。
                let t = to_transform(*offset);
                Collider::compound(vec![(t.translation, t.rotation, cuboid)])
            }
        }
        ShapeDesc::ConvexHull { points } => Collider::convex_hull(&to_vec(points))?,
        ShapeDesc::ConvexDecomposition { vertices, indices } => {
            Collider::convex_decomposition(&to_vec(vertices), indices)
        }
        ShapeDesc::TriMesh { vertices, indices } => {
            Collider::trimesh(to_vec(vertices), indices.clone()).ok()?
        }
    })
}

impl PhysicsEngine for RapierEngine<'_> {
    fn add_body(&mut self, key: BodyKey, desc: &BodyDesc) -> bool {
        let Some(shape) = collider(&desc.shape) else {
            return false;
        };
        self.remove_body(key);
        let rigid_body = match desc.kind {
            BodyKind::Fixed => RigidBody::Fixed,
            BodyKind::Kinematic => RigidBody::KinematicPositionBased,
            BodyKind::Dynamic => RigidBody::Dynamic,
        };
        let mut body = self.world.spawn((
            PhysicsBody(key),
            rigid_body,
            to_transform(desc.pose),
            shape,
            Friction::coefficient(desc.friction as f32),
            Restitution::coefficient(desc.restitution as f32),
            ColliderMassProperties::Density(desc.density as f32),
        ));
        if desc.kind == BodyKind::Dynamic {
            // 速く落ちる小さいワークが薄い板をすり抜けないよう、連続衝突判定を入れる。
            // ベルトに載って一定速度で流れているワークを止まったとみなして眠らせないよう、眠らせない。
            body.insert((Ccd::enabled(), Sleeping::disabled()));
        }
        if let Some(v) = desc.belt_velocity {
            body.insert((Belt(v.as_vec3()), ActiveHooks::MODIFY_SOLVER_CONTACTS));
        }
        let entity = body.id();
        self.world.resource_mut::<Bodies>().0.insert(key, entity);
        true
    }

    fn remove_body(&mut self, key: BodyKey) {
        if let Some(entity) = self.world.resource_mut::<Bodies>().0.remove(&key) {
            self.world.despawn(entity);
        }
    }

    fn set_kinematic_target(&mut self, key: BodyKey, pose: Iso) {
        let Some(entity) = self.entity(key) else {
            return;
        };
        // 1フレームに何刻みも進めるので、刻みごとに変更の時刻を進める。進めないと、前の刻みで
        // プラグインのシステムが見た時刻と同じになり、変更が見落とされる。
        self.world.increment_change_tick();
        let mut body = self.world.entity_mut(entity);
        // プラグインは、機構で動く剛体の Transform の変化を次の刻みの目標として渡す
        if body.get::<RigidBody>() == Some(&RigidBody::KinematicPositionBased)
            && let Some(mut transform) = body.get_mut::<Transform>()
        {
            *transform = to_transform(pose);
        }
    }

    fn step(&mut self, dt: f64) {
        self.world.insert_resource(TimestepMode::Fixed {
            dt: dt as f32,
            substeps: 1,
        });
        self.world.run_schedule(PhysicsStep);
    }

    fn pose(&self, key: BodyKey) -> Option<Iso> {
        let transform = self.world.get::<Transform>(self.entity(key)?)?;
        Some(from_transform(transform))
    }

    fn dynamic_poses(&self) -> Vec<(BodyKey, Iso)> {
        let Some(mut bodies) = self
            .world
            .try_query::<(&PhysicsBody, &RigidBody, &Transform)>()
        else {
            return Vec::new();
        };
        bodies
            .iter(self.world)
            .filter(|(_, rigid_body, _)| **rigid_body == RigidBody::Dynamic)
            .map(|(body, _, transform)| (body.0, from_transform(transform)))
            .collect()
    }
}

/// ベルトに触れている接触の、接線方向の目標相対速度をベルトの速度にする
#[derive(SystemParam)]
pub struct BeltHooks<'w, 's> {
    belts: Query<'w, 's, &'static Belt>,
}

impl BeltHooks<'_, '_> {
    /// collider がベルトなら、表面のワールドでの速度
    fn belt_velocity(
        &self,
        context: &ContactModificationContextView,
        entity: Entity,
        handle: ColliderHandle,
    ) -> Vec3 {
        let Ok(belt) = self.belts.get(entity) else {
            return Vec3::ZERO;
        };
        let raw = &context.raw;
        let collider = &raw.colliders[handle];
        let rotation: Quat = collider
            .parent()
            .and_then(|h| raw.bodies.get(h))
            .map_or(collider.position().rotation, |b| b.position().rotation);
        rotation * belt.0
    }
}

impl BevyPhysicsHooks for BeltHooks<'_, '_> {
    fn modify_solver_contacts(&self, context: ContactModificationContextView) {
        let v1 = self.belt_velocity(&context, context.collider1(), context.raw.collider1);
        let v2 = self.belt_velocity(&context, context.collider2(), context.raw.collider2);
        let Some(manifold) = context.raw.rigid_mut() else {
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

/// 画面なしで物理だけを動かす Bevy の App (テスト用)
#[cfg(test)]
pub fn headless_app(gravity: DVec3, meters_per_unit: f64) -> App {
    let mut app = App::new();
    app.add_plugins((
        TaskPoolPlugin::default(),
        TransformPlugin,
        RapierBackendPlugin,
    ))
    .init_resource::<Time>();
    app.finish();
    app.cleanup();
    // 起動時のシステム (物理の世界の作成) を走らせる
    app.update();
    RapierEngine::new(app.world_mut()).reset(gravity, meters_per_unit);
    app
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

    fn app() -> App {
        headless_app(DVec3::new(0.0, -9810.0, 0.0), 0.001)
    }

    fn run_for(app: &mut App, run: &mut PhysicsRun, seconds: f64) {
        let frames = (seconds * 60.0) as usize;
        for _ in 0..frames {
            run.advance(&mut RapierEngine::new(app.world_mut()), 1.0 / 60.0, &[]);
        }
    }

    fn pose(app: &mut App, key: BodyKey) -> Option<Iso> {
        RapierEngine::new(app.world_mut()).pose(key)
    }

    #[test]
    fn a_box_falls_and_rests_on_the_floor() {
        let mut app = app();
        let mut e = RapierEngine::new(app.world_mut());
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
        let mut run = PhysicsRun::new();
        run_for(&mut app, &mut run, 1.5);
        let y = pose(&mut app, BodyKey::Part(9)).unwrap().translation.y;
        // 床の上面 10 + 半分の高さ 25
        assert!((y - 35.0).abs() < 1.5, "y = {y}");
    }

    #[test]
    fn a_belt_carries_the_work_along_its_axis() {
        let mut app = app();
        let mut e = RapierEngine::new(app.world_mut());
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
        let mut run = PhysicsRun::new();
        run_for(&mut app, &mut run, 2.0);
        let p = pose(&mut app, BodyKey::Part(9)).unwrap().translation;
        // 落ちて加速する時間を除いても 2 秒で 300 mm 以上は進む。速度を超えては進まない。
        assert!(p.x > 300.0 && p.x < 410.0, "x = {}", p.x);
        assert!(p.z.abs() < 5.0);
    }

    #[test]
    fn a_kinematic_pusher_moves_the_work() {
        let mut app = app();
        let mut e = RapierEngine::new(app.world_mut());
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
        let mut run = PhysicsRun::new();
        run_for(&mut app, &mut run, 0.3);
        for frame in 1..=60 {
            let x = start.x + 300.0 * f64::from(frame) / 60.0;
            let target = Iso::from_translation(DVec3::new(x, start.y, 0.0));
            run.advance(
                &mut RapierEngine::new(app.world_mut()),
                1.0 / 60.0,
                &[(BodyKey::Part(2), target)],
            );
        }
        let x = pose(&mut app, BodyKey::Part(9)).unwrap().translation.x;
        // 押し板は x = 200 まで来た。ワークは押し板の前 (板の厚み 10 + ワーク 25) にいる
        assert!(x > 180.0, "x = {x}");
        assert!(run.steps >= 60 * 2 && STEP_DT > 0.0);
    }

    #[test]
    fn convex_hull_and_decomposition_shapes_are_built() {
        let mut app = app();
        let mut e = RapierEngine::new(app.world_mut());
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

    /// 剛体の原点からずれた箱でも、ベルトが運び、床に載る
    #[test]
    fn offset_shapes_carry_and_rest() {
        let mut app = app();
        let mut e = RapierEngine::new(app.world_mut());
        let mut belt = cuboid(BodyKind::Fixed, DVec3::ZERO, DVec3::ONE);
        belt.shape = ShapeDesc::Cuboid {
            half: DVec3::new(2000.0, 10.0, 200.0),
            offset: Iso::from_translation(DVec3::new(0.0, -10.0, 0.0)),
        };
        belt.belt_velocity = Some(DVec3::new(200.0, 0.0, 0.0));
        e.add_body(BodyKey::Part(1), &belt);
        let mut work = cuboid(BodyKind::Dynamic, DVec3::ZERO, DVec3::ONE);
        work.shape = ShapeDesc::Cuboid {
            half: DVec3::splat(25.0),
            offset: Iso::from_translation(DVec3::new(0.0, 30.0, 0.0)),
        };
        e.add_body(BodyKey::Part(9), &work);
        let mut run = PhysicsRun::new();
        run_for(&mut app, &mut run, 2.0);
        let p = pose(&mut app, BodyKey::Part(9))
            .unwrap()
            .apply(DVec3::new(0.0, 30.0, 0.0));
        assert!(p.x > 300.0 && p.x < 410.0, "x = {}", p.x);
        // ベルトの上面 0 + 半分の高さ 25
        assert!((p.y - 25.0).abs() < 1.5, "y = {}", p.y);
    }

    #[test]
    fn removed_and_cleared_bodies_are_gone() {
        let mut app = app();
        let mut e = RapierEngine::new(app.world_mut());
        let work = cuboid(BodyKind::Dynamic, DVec3::ZERO, DVec3::splat(25.0));
        e.add_body(BodyKey::Part(1), &work);
        e.add_body(BodyKey::Part(2), &work);
        e.step(STEP_DT);
        assert_eq!(e.dynamic_poses().len(), 2);
        e.remove_body(BodyKey::Part(1));
        e.step(STEP_DT);
        assert_eq!(e.dynamic_poses().len(), 1);
        e.clear();
        e.step(STEP_DT);
        assert!(e.dynamic_poses().is_empty());
        assert!(e.pose(BodyKey::Part(2)).is_none());
    }
}
