//! 作業者と AGV の 3D モデルの表示
//!
//! 形と動きは `cadsim_core::floor::{figure, figure_motion}` が決める。ここでは
//!
//! - 形ごとにメッシュを 1回だけ作り、1体ぶんを「親 (位置・向き) + 形ごとの子」として並べる
//! - 動きのクリップ (作業者は歩く・作業・待つ、AGV は車輪・荷) を Bevy の `AnimationClip` にし、
//!   親に `AnimationPlayer` を付けて、子は `AnimationTargetId` で動かす
//! - 毎フレーム、親の位置・向きと、クリップごとの時刻と重み (core が決める) を写す。
//!   重みを Bevy の `AnimationGraph` が混ぜるので、歩きから作業へなめらかに移る
//! - 色と見える/見えない (選択・ランプ・荷) は動きではないので、マテリアルと `Visibility` に写す
//!
//! クリップの時刻は再生に任せず、床の時計から毎フレーム決める (止める・巻き戻すと動きも止まる・戻る)。

use std::collections::HashMap;

use bevy::animation::{AnimatedBy, AnimationTargetId, animated_field};
use bevy::curve::UnevenSampleAutoCurve;
use bevy::math::DVec2;
use bevy::prelude::*;

use super::agv_sim::{AgvSim, Phase, Vehicle};
use super::figure::{
    AgvPose, AgvStatus, FigurePart, Paint, PersonPose, Shape, agv_local_parts, heading_rotation,
    person_local_parts,
};
use super::figure_motion::{
    AgvDims, FigureClip, PERSON_ACTIVITIES, agv_cargo_amount, agv_cargo_clip, agv_roll_clip,
    agv_roll_time, person_clip, person_weights,
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

/// 形 `i` を動かすクリップの宛先 (同じ形の番号なら、どの作業者でも同じ ID。
/// どの親のプレイヤーが動かすかは `AnimatedBy` で分ける)
fn part_name(i: usize) -> Name {
    Name::new(format!("形{i}"))
}

/// core のクリップを Bevy のクリップにする
fn to_animation_clip(clip: &FigureClip) -> AnimationClip {
    let mut out = AnimationClip::default();
    let times: Vec<f32> = clip.times.iter().map(|&t| t as f32).collect();
    for (i, keys) in clip.parts.iter().enumerate() {
        let Some(keys) = keys else { continue };
        let target = AnimationTargetId::from_name(&part_name(i));
        fn zip<'a, T: 'a>(times: &'a [f32], values: Vec<T>) -> impl Iterator<Item = (f32, T)> + 'a {
            times.iter().copied().zip(values)
        }
        if let Ok(curve) = UnevenSampleAutoCurve::new(zip(
            &times,
            keys.translations.iter().map(|v| v.as_vec3()).collect(),
        )) {
            out.add_curve_to_target(
                target,
                AnimatableCurve::new(animated_field!(Transform::translation), curve),
            );
        }
        if let Ok(curve) = UnevenSampleAutoCurve::new(zip(
            &times,
            keys.rotations.iter().map(|q| q.as_quat()).collect(),
        )) {
            out.add_curve_to_target(
                target,
                AnimatableCurve::new(animated_field!(Transform::rotation), curve),
            );
        }
        if let Ok(curve) = UnevenSampleAutoCurve::new(zip(
            &times,
            keys.scales.iter().map(|v| v.as_vec3()).collect(),
        )) {
            out.add_curve_to_target(
                target,
                AnimatableCurve::new(animated_field!(Transform::scale), curve),
            );
        }
    }
    out.set_duration(clip.duration as f32);
    out
}

/// クリップを混ぜるグラフと、クリップごとの節
struct FigureAnimations {
    graph: Handle<AnimationGraph>,
    nodes: Vec<AnimationNodeIndex>,
}

impl FigureAnimations {
    fn new(
        clips: &[FigureClip],
        animation_clips: &mut Assets<AnimationClip>,
        graphs: &mut Assets<AnimationGraph>,
    ) -> Self {
        let handles: Vec<Handle<AnimationClip>> = clips
            .iter()
            .map(|c| animation_clips.add(to_animation_clip(c)))
            .collect();
        let (graph, nodes) = AnimationGraph::from_clips(handles);
        Self {
            graph: graphs.add(graph),
            nodes,
        }
    }
}

/// 1体ぶん: 親 (位置・向き・プレイヤー) と形ごとの子
struct Spawned {
    root: Entity,
    parts: Vec<Entity>,
}

/// メッシュとマテリアルとクリップの使い回し、作業者・AGV ごとの部品
#[derive(Resource, Default)]
pub struct FigureAssets {
    meshes: HashMap<ShapeKey, Handle<Mesh>>,
    materials: HashMap<Paint, Handle<StandardMaterial>>,
    spawned: HashMap<Owner, Spawned>,
    person: Option<FigureAnimations>,
    /// AGV のクリップは寸法で変わるので、作ったときの寸法と一緒に持つ
    agv: Option<(AgvDims, FigureAnimations)>,
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

    fn despawn(&mut self, owner: Owner, commands: &mut Commands) {
        if let Some(old) = self.spawned.remove(&owner) {
            commands.entity(old.root).despawn();
        }
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

/// 床の位置と向きを親の置き方にする
fn root_transform(position: DVec2, heading: DVec2) -> Transform {
    Transform {
        translation: Vec3::new(position.x as f32, 0.0, position.y as f32),
        rotation: heading_rotation(heading).as_quat(),
        scale: Vec3::ONE,
    }
}

fn agv_status(v: &Vehicle) -> AgvStatus {
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

/// 1体ぶんの、いまの置き方と動き
struct Figure {
    owner: Owner,
    root: Transform,
    /// 色と見える/見えないを取る形 (置き方は使わない)
    parts: Vec<FigurePart>,
    /// クリップごとの (時刻, 重み)。並びは `FigureAnimations::nodes` と同じ
    clips: Vec<(f64, f64)>,
}

fn agv_dims(sim: &AgvSim) -> AgvDims {
    AgvDims {
        length: sim.params().length,
        width: sim.params().width,
        height: AGV_HEIGHT,
    }
}

/// いまの作業者と AGV を並べる
fn current_figures(
    clock: &FloorClock,
    operators: &OperatorsRes,
    state: &FloorState,
) -> Vec<Figure> {
    let mut figures = Vec::new();
    for op in &operators.operators {
        let plan = state.plans.get(&op.id);
        let position = operator_position(state, op, clock.time);
        let heading = plan.map_or(DVec2::X, |p| p.heading_at(clock.time));
        let pose = PersonPose {
            position,
            heading,
            activity: plan
                .and_then(|p| p.segment_at(clock.time))
                .map_or(Activity::Wait, |s| s.activity),
            walked: plan.map_or(0.0, |p| p.walked_at(clock.time)),
            time: clock.time,
            selected: state.selected_operator == Some(op.id),
        };
        figures.push(Figure {
            owner: Owner::Person(op.id),
            root: root_transform(position, heading),
            parts: person_local_parts(&pose),
            clips: person_weights(plan, clock.time)
                .iter()
                .map(|c| (c.time, c.weight))
                .collect(),
        });
    }
    if let Some(sim) = state.agv_sim() {
        for v in &sim.vehicles {
            figures.push(agv_figure(sim, v, clock.time));
        }
    }
    figures
}

fn agv_figure(sim: &AgvSim, v: &Vehicle, time: f64) -> Figure {
    let (position, heading) = v.pose(sim.graph());
    let total = match v.phase {
        Phase::Loading => sim.params().load_time,
        _ => sim.params().unload_time,
    };
    let cargo = agv_cargo_amount(v.phase, v.station_remaining(), total);
    let dims = agv_dims(sim);
    let parts = agv_local_parts(&AgvPose {
        position,
        heading,
        length: dims.length,
        width: dims.width,
        height: dims.height,
        status: agv_status(v),
        loaded: cargo > 0.0,
        cargo_lift: 0.0,
        distance: v.distance,
        time,
    });
    Figure {
        owner: Owner::Agv(v.id),
        root: root_transform(position, heading),
        parts,
        // 車輪と荷は別の形を動かすので、どちらも重み 1 で重ねる
        clips: vec![(agv_roll_time(v.distance), 1.0), (cargo, 1.0)],
    }
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
    mut animation_clips: ResMut<Assets<AnimationClip>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut roots: Query<(&mut Transform, &mut AnimationPlayer), Without<FloorFigure>>,
    mut parts: Query<(&mut MeshMaterial3d<StandardMaterial>, &mut Visibility), With<FloorFigure>>,
) {
    let figures = current_figures(&clock, &operators, &state);

    // クリップは最初に 1回だけ作る。AGV は寸法が変わったら作り直し、いまの AGV も並べ直す
    if assets.person.is_none() {
        let clips: Vec<FigureClip> = PERSON_ACTIVITIES.iter().map(|&a| person_clip(a)).collect();
        assets.person = Some(FigureAnimations::new(
            &clips,
            &mut animation_clips,
            &mut graphs,
        ));
    }
    if let Some(dims) = state.agv_sim().map(agv_dims)
        && assets.agv.as_ref().is_none_or(|(d, _)| *d != dims)
    {
        let clips = [agv_roll_clip(dims), agv_cargo_clip(dims)];
        assets.agv = Some((
            dims,
            FigureAnimations::new(&clips, &mut animation_clips, &mut graphs),
        ));
        let agvs: Vec<Owner> = assets
            .spawned
            .keys()
            .copied()
            .filter(|o| matches!(o, Owner::Agv(_)))
            .collect();
        for owner in agvs {
            assets.despawn(owner, &mut commands);
        }
    }

    // いなくなった作業者・AGV を消す
    let alive: Vec<Owner> = figures.iter().map(|f| f.owner).collect();
    let gone: Vec<Owner> = assets
        .spawned
        .keys()
        .copied()
        .filter(|o| !alive.contains(o))
        .collect();
    for owner in gone {
        assets.despawn(owner, &mut commands);
    }

    for figure in figures {
        let (graph, nodes) = {
            let animations = match figure.owner {
                Owner::Person(_) => assets.person.as_ref(),
                Owner::Agv(_) => assets.agv.as_ref().map(|(_, a)| a),
            };
            let Some(animations) = animations else {
                continue;
            };
            (animations.graph.clone(), animations.nodes.clone())
        };
        // 形の数が同じなら、置き方・動き・色だけ写す
        if let Some(spawned) = assets.spawned.get(&figure.owner)
            && spawned.parts.len() == figure.parts.len()
        {
            let (root, entities) = (spawned.root, spawned.parts.clone());
            if let Ok((mut transform, mut player)) = roots.get_mut(root) {
                transform.set_if_neq(figure.root);
                for (&node, &(time, weight)) in nodes.iter().zip(&figure.clips) {
                    player
                        .play(node)
                        .pause()
                        .set_seek_time(time as f32)
                        .set_weight(weight as f32);
                }
            }
            for (e, part) in entities.iter().zip(&figure.parts) {
                let material = assets.material(part.paint, &mut materials);
                let Ok((mut mat, mut visibility)) = parts.get_mut(*e) else {
                    continue;
                };
                if mat.0 != material {
                    mat.0 = material;
                }
                visibility.set_if_neq(visibility_of(part));
            }
            continue;
        }
        assets.despawn(figure.owner, &mut commands);
        let root = commands
            .spawn((
                Name::new(match figure.owner {
                    Owner::Person(id) => format!("作業者 {id}"),
                    Owner::Agv(id) => format!("AGV {id}"),
                }),
                figure.root,
                Visibility::default(),
                AnimationPlayer::default(),
                AnimationGraphHandle(graph),
            ))
            .id();
        let entities = figure
            .parts
            .iter()
            .enumerate()
            .map(|(i, part)| {
                let mesh = assets.mesh(part.shape, &mut meshes);
                let material = assets.material(part.paint, &mut materials);
                let name = part_name(i);
                let target = AnimationTargetId::from_name(&name);
                commands
                    .spawn((
                        FloorFigure,
                        name,
                        Mesh3d(mesh),
                        MeshMaterial3d(material),
                        transform_of(part),
                        visibility_of(part),
                        target,
                        AnimatedBy(root),
                        ChildOf(root),
                    ))
                    .id()
            })
            .collect();
        assets.spawned.insert(
            figure.owner,
            Spawned {
                root,
                parts: entities,
            },
        );
    }
}
