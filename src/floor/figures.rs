//! 作業者と AGV の 3D モデルの表示
//!
//! 形と姿勢は `cadsim_core::floor::figure` が決める。ここでは形ごとにメッシュを 1回だけ作り、
//! 毎フレーム、位置・向き・色・見える/見えないを写すだけにする。
//! 作業者や AGV の数・形の数が変わったときだけ、その 1体ぶんを作り直す。

use std::collections::HashMap;

use bevy::math::DVec2;
use bevy::prelude::*;

use super::agv_sim::{AgvSim, Phase};
use super::figure::{
    AgvPose, AgvStatus, FigurePart, Paint, PersonPose, Shape, agv_parts, person_parts,
};
use super::operator_plan::Activity;
use super::{AGV_HEIGHT, FloorClock, FloorState, OperatorsRes, operator_position};

/// 作業者と AGV の 3D モデルの部品 (ライブラリのドロップ先の判定などから外すための目印)
#[derive(Component)]
pub struct FloorFigure;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Owner {
    Person(u32),
    Agv(usize),
}

/// 形の寸法を、メッシュを使い回すための鍵にする
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ShapeKey {
    Box([u32; 3]),
    Sphere(u32),
    Cylinder(u32, u32),
    Capsule(u32, u32),
}

impl ShapeKey {
    fn of(shape: Shape) -> Self {
        let b = |v: f64| (v as f32).to_bits();
        match shape {
            Shape::Box { size } => Self::Box([b(size.x), b(size.y), b(size.z)]),
            Shape::Sphere { radius } => Self::Sphere(b(radius)),
            Shape::Cylinder { radius, height } => Self::Cylinder(b(radius), b(height)),
            Shape::Capsule { radius, length } => Self::Capsule(b(radius), b(length)),
        }
    }
}

fn mesh_of(shape: Shape) -> Mesh {
    match shape {
        Shape::Box { size } => Cuboid::new(size.x as f32, size.y as f32, size.z as f32).into(),
        Shape::Sphere { radius } => Sphere::new(radius as f32).mesh().uv(24, 16),
        Shape::Cylinder { radius, height } => Cylinder::new(radius as f32, height as f32)
            .mesh()
            .resolution(24)
            .into(),
        Shape::Capsule { radius, length } => Capsule3d::new(radius as f32, length as f32)
            .mesh()
            .longitudes(20)
            .latitudes(10)
            .into(),
    }
}

fn color_of(paint: Paint) -> (Color, f32) {
    // (色, 自ら光る強さ)
    match paint {
        Paint::Skin => (Color::srgb(0.93, 0.76, 0.62), 0.0),
        Paint::Hair => (Color::srgb(0.16, 0.12, 0.10), 0.0),
        Paint::Helmet => (Color::srgb(0.98, 0.98, 0.96), 0.0),
        Paint::Vest => (Color::srgb(1.0, 0.55, 0.08), 0.0),
        Paint::Highlight => (Color::srgb(0.25, 0.55, 1.0), 0.0),
        Paint::Reflective => (Color::srgb(0.85, 0.88, 0.9), 0.0),
        Paint::Shirt => (Color::srgb(0.22, 0.32, 0.5), 0.0),
        Paint::Pants => (Color::srgb(0.17, 0.2, 0.3), 0.0),
        Paint::Shoe => (Color::srgb(0.12, 0.12, 0.13), 0.0),
        Paint::Eye => (Color::srgb(0.08, 0.08, 0.1), 0.0),
        Paint::AgvBody => (Color::srgb(0.88, 0.9, 0.92), 0.0),
        Paint::AgvDeck => (Color::srgb(0.28, 0.3, 0.33), 0.0),
        Paint::Stripe => (Color::srgb(1.0, 0.8, 0.1), 0.0),
        Paint::StripeAlert => (Color::srgb(0.95, 0.2, 0.2), 0.0),
        Paint::Bumper => (Color::srgb(0.12, 0.12, 0.13), 0.0),
        Paint::Tire => (Color::srgb(0.1, 0.1, 0.11), 0.0),
        Paint::Hub => (Color::srgb(0.6, 0.62, 0.65), 0.0),
        Paint::Headlight => (Color::srgb(0.95, 0.97, 1.0), 1.0),
        Paint::Taillight => (Color::srgb(0.9, 0.1, 0.1), 0.6),
        Paint::Sensor => (Color::srgb(0.15, 0.15, 0.17), 0.0),
        Paint::LampGreen => (Color::srgb(0.2, 0.95, 0.35), 1.2),
        Paint::LampAmber => (Color::srgb(1.0, 0.65, 0.1), 1.2),
        Paint::LampRed => (Color::srgb(1.0, 0.15, 0.15), 1.2),
        Paint::LampOff => (Color::srgb(0.35, 0.37, 0.4), 0.0),
        Paint::Cargo => (Color::srgb(0.75, 0.56, 0.36), 0.0),
    }
}

/// メッシュとマテリアルの使い回し、作業者・AGV ごとの部品
#[derive(Resource, Default)]
pub struct FigureAssets {
    meshes: HashMap<ShapeKey, Handle<Mesh>>,
    materials: HashMap<Paint, Handle<StandardMaterial>>,
    spawned: HashMap<Owner, Vec<Entity>>,
}

impl FigureAssets {
    fn mesh(&mut self, shape: Shape, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        self.meshes
            .entry(ShapeKey::of(shape))
            .or_insert_with(|| meshes.add(mesh_of(shape)))
            .clone()
    }

    fn material(
        &mut self,
        paint: Paint,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        self.materials
            .entry(paint)
            .or_insert_with(|| {
                let (color, glow) = color_of(paint);
                materials.add(StandardMaterial {
                    base_color: color,
                    emissive: color.to_linear() * glow,
                    perceptual_roughness: 0.55,
                    ..default()
                })
            })
            .clone()
    }
}

fn transform_of(part: &FigurePart) -> Transform {
    Transform {
        translation: part.translation.as_vec3(),
        rotation: part.rotation.as_quat(),
        scale: part.scale.as_vec3(),
    }
}

fn visibility_of(part: &FigurePart) -> Visibility {
    if part.visible {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    }
}

fn agv_status(v: &super::agv_sim::Vehicle) -> AgvStatus {
    if v.blocked {
        AgvStatus::Blocked
    } else {
        match v.phase {
            Phase::Idle => AgvStatus::Idle,
            Phase::Loading | Phase::Unloading => AgvStatus::Handling,
            Phase::ToPickup | Phase::ToDrop => AgvStatus::Moving,
        }
    }
}

/// いまの作業者と AGV の形を並べる
fn current_figures(
    clock: &FloorClock,
    operators: &OperatorsRes,
    state: &FloorState,
) -> Vec<(Owner, Vec<FigurePart>)> {
    let mut figures = Vec::new();
    for op in &operators.operators {
        let plan = state.plans.get(&op.id);
        let pose = PersonPose {
            position: operator_position(state, op, clock.time),
            heading: plan.map_or(DVec2::X, |p| p.heading_at(clock.time)),
            activity: plan
                .and_then(|p| p.segment_at(clock.time))
                .map_or(Activity::Wait, |s| s.activity),
            walked: plan.map_or(0.0, |p| p.walked_at(clock.time)),
            time: clock.time,
            selected: state.selected_operator == Some(op.id),
        };
        figures.push((Owner::Person(op.id), person_parts(&pose)));
    }
    if let Some(sim) = state.agv_sim() {
        for v in &sim.vehicles {
            figures.push((Owner::Agv(v.id), agv_figure(sim, v, clock.time)));
        }
    }
    figures
}

fn agv_figure(sim: &AgvSim, v: &super::agv_sim::Vehicle, time: f64) -> Vec<FigurePart> {
    let (position, heading) = v.pose(sim.graph());
    agv_parts(&AgvPose {
        position,
        heading,
        length: sim.params().length,
        width: sim.params().width,
        height: AGV_HEIGHT,
        status: agv_status(v),
        loaded: matches!(v.phase, Phase::ToDrop | Phase::Unloading),
        distance: v.distance,
        time,
    })
}

/// 作業者と AGV の 3D モデルを、いまの位置・姿勢に合わせる
#[allow(clippy::too_many_arguments)]
pub fn sync_figures(
    mut commands: Commands,
    clock: Res<FloorClock>,
    operators: Res<OperatorsRes>,
    state: Res<FloorState>,
    mut assets: ResMut<FigureAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut parts: Query<
        (
            &mut Transform,
            &mut MeshMaterial3d<StandardMaterial>,
            &mut Visibility,
        ),
        With<FloorFigure>,
    >,
) {
    let figures = current_figures(&clock, &operators, &state);

    // いなくなった作業者・AGV を消す
    let alive: Vec<Owner> = figures.iter().map(|(o, _)| *o).collect();
    assets.spawned.retain(|owner, entities| {
        let keep = alive.contains(owner);
        if !keep {
            for &e in entities.iter() {
                commands.entity(e).despawn();
            }
        }
        keep
    });

    for (owner, figure) in figures {
        // 形の数が同じなら姿勢と色だけ写す
        if let Some(entities) = assets.spawned.get(&owner).cloned()
            && entities.len() == figure.len()
        {
            for (e, part) in entities.iter().zip(&figure) {
                let material = assets.material(part.paint, &mut materials);
                let Ok((mut transform, mut mat, mut visibility)) = parts.get_mut(*e) else {
                    continue;
                };
                transform.set_if_neq(transform_of(part));
                if mat.0 != material {
                    mat.0 = material;
                }
                visibility.set_if_neq(visibility_of(part));
            }
            continue;
        }
        if let Some(old) = assets.spawned.remove(&owner) {
            for e in old {
                commands.entity(e).despawn();
            }
        }
        let entities = figure
            .iter()
            .map(|part| {
                let mesh = assets.mesh(part.shape, &mut meshes);
                let material = assets.material(part.paint, &mut materials);
                commands
                    .spawn((
                        FloorFigure,
                        Mesh3d(mesh),
                        MeshMaterial3d(material),
                        transform_of(part),
                        visibility_of(part),
                    ))
                    .id()
            })
            .collect();
        assets.spawned.insert(owner, entities);
    }
}
