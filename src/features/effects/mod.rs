//! 状態のエフェクト: 詰まり・故障・安全停止を光と煙で、溶接と干渉 (チャートの再生中) を火花で 3D ビューに出す
//!
//! どこに・どの見た目を出すか、粒の動きは `plantwright_core::effects`。ここでは毎フレーム
//! 「工程・安全機器・干渉の状態を集める → 出す場所を決めて粒を進める → 表示に写す」だけをする。
//!
//! - 光: 加算合成の球 (きっかけごとに材質を 1 つ持ち、脈打つ強さを毎フレーム書き換える)
//! - 煙: 半透明の球。濃さを 8 段に分けた材質を使い回す
//! - 火花・波紋: ギズモの線 (メッシュを作らないので軽い)
//!
//! 光と煙の球は使い回しの入れ物 (エンティティ) に載せ、要らない分は隠す。毎フレーム作り直さない。
//! オン・オフは表示メニューの「状態のエフェクト」([`ViewSettings::effects`])。

use std::collections::HashMap;

use bevy::light::NotShadowCaster;
use bevy::prelude::*;

use plantwright_core::effects::{
    Cue, Emitter, EmitterKey, ParticleField, ParticleKind, collision_point, flow_emitters,
    ripple_phase, safety_beacon,
};
use plantwright_core::simulation::interference::PairStatus;

use crate::features::flow::{FlowRes, FlowRun};
use crate::features::motion::Playback;
use crate::features::safety::{SafetyLayoutRes, SafetyState};
use crate::features::simulation::InterferenceCheck;
use crate::viewport::parts::ViewSettings;
use plantwright_core::model::Model;

/// 煙の球の入れ物の上限 (粒の上限とは別に、描く数を抑える)
const MAX_SMOKE_VISUALS: usize = 300;
/// 煙の濃さの段数
const SMOKE_LEVELS: usize = 8;
/// 煙のいちばん濃いときの不透明度
const SMOKE_ALPHA: f32 = 0.55;

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EffectState>()
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (update_effects, show_glows, show_smoke, draw_lines).chain(),
            );
    }
}

/// 今出しているエフェクト
#[derive(Resource, Default)]
pub struct EffectState {
    emitters: Vec<Emitter>,
    field: ParticleField,
    /// 光の脈打ちに使う時刻 (秒)
    clock: f64,
}

/// 光と煙の球の形と材質
#[derive(Resource)]
struct EffectAssets {
    sphere: Handle<Mesh>,
    glow: HashMap<Cue, Handle<StandardMaterial>>,
    smoke: Vec<Handle<StandardMaterial>>,
}

/// 光の球の入れ物 (番号順に使う)
#[derive(Component)]
struct GlowVisual(usize);

/// 煙の球の入れ物 (番号順に使う)
#[derive(Component)]
struct SmokeVisual(usize);

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let sphere = meshes.add(Sphere::new(1.0).mesh().ico(3).expect("ico 3 は作れる"));
    let glow = Cue::ALL
        .into_iter()
        .map(|cue| {
            let material = materials.add(StandardMaterial {
                base_color: Color::NONE,
                alpha_mode: AlphaMode::Add,
                unlit: true,
                cull_mode: None,
                ..default()
            });
            (cue, material)
        })
        .collect();
    let smoke = (0..SMOKE_LEVELS)
        .map(|level| {
            let alpha = SMOKE_ALPHA * (level + 1) as f32 / SMOKE_LEVELS as f32;
            materials.add(StandardMaterial {
                base_color: Color::srgba(0.12, 0.12, 0.13, alpha),
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                ..default()
            })
        })
        .collect();
    commands.insert_resource(EffectAssets {
        sphere,
        glow,
        smoke,
    });
}

/// 状態を集めてエフェクトを出す場所を決め、粒を進める
#[allow(clippy::too_many_arguments)]
fn update_effects(
    time: Res<Time>,
    settings: Res<ViewSettings>,
    flow: Res<FlowRes>,
    run: Res<FlowRun>,
    model: Res<Model>,
    safety_layout: Res<SafetyLayoutRes>,
    safety: Res<SafetyState>,
    check: Res<InterferenceCheck>,
    playback: Res<Playback>,
    mut state: ResMut<EffectState>,
) {
    let settings = settings.effects;
    let state = &mut *state;
    state.emitters.clear();
    if settings.enabled {
        let bounds = |part| model.is_shown(part).then(|| model.world_bounds(part))?;
        if let Some(sim) = run.sim.as_ref().filter(|_| !run.is_stale(&flow)) {
            state.emitters = flow_emitters(&flow, |id| sim.state_of(id), bounds);
        }
        for id in safety.stop_devices() {
            if let Some(device) = safety_layout.devices.iter().find(|d| d.id == id) {
                state.emitters.push(safety_beacon(device));
            }
        }
        // 干渉の火花は、タイミングチャートで動かしている間だけ (はめあいなどで止まったまま重なっている部品には出さない)
        let pairs = if playback.playing {
            &check.report.pairs[..]
        } else {
            &[]
        };
        for pair in pairs {
            if !matches!(pair.status, PairStatus::Interfering { .. }) {
                continue;
            }
            if let Some((point, radius)) = collision_point(&pair.curves) {
                let key = EmitterKey::Collision(pair.a, pair.b);
                state
                    .emitters
                    .push(Emitter::at(key, Cue::Collision, point, radius));
            }
        }
    }
    let dt = time.delta_secs_f64();
    state.clock += dt;
    state.field.step(&state.emitters, &settings, dt);
}

/// 光の球を出す場所に置き、きっかけごとの材質の明るさを脈打たせる
fn show_glows(
    mut commands: Commands,
    settings: Res<ViewSettings>,
    state: Res<EffectState>,
    assets: Res<EffectAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut visuals: Query<(
        &GlowVisual,
        &mut Transform,
        &mut Visibility,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
) {
    let glows: Vec<_> = state
        .emitters
        .iter()
        .filter_map(|e| Some((e, e.cue.look(&settings.effects).glow?)))
        .collect();
    // 使っているきっかけの材質だけ明るさを書き換える
    for cue in Cue::ALL {
        let Some(glow) = glows.iter().find(|(e, _)| e.cue == cue).map(|(_, g)| g) else {
            continue;
        };
        let Some(mut material) = materials.get_mut(&assets.glow[&cue]) else {
            continue;
        };
        let s = glow.strength(state.clock) * 0.55;
        let [r, g, b] = glow.rgb;
        material.base_color = Color::srgba(r * s, g * s, b * s, 1.0);
    }
    let mut used = 0;
    for (visual, mut transform, mut visibility, mut material) in &mut visuals {
        match glows.get(visual.0) {
            Some((e, glow)) => {
                let swell = 1.0 + 0.06 * glow.strength(state.clock);
                *transform = Transform::from_translation(e.center.as_vec3())
                    .with_scale(Vec3::splat((e.radius * 1.1) as f32 * swell));
                visibility.set_if_neq(Visibility::Visible);
                let handle = &assets.glow[&e.cue];
                if material.0 != *handle {
                    material.0 = handle.clone();
                }
                used += 1;
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    for (i, (e, _)) in glows.iter().enumerate().skip(used) {
        commands.spawn((
            GlowVisual(i),
            Mesh3d(assets.sphere.clone()),
            MeshMaterial3d(assets.glow[&e.cue].clone()),
            Transform::from_scale(Vec3::ZERO),
            Visibility::Hidden,
            NotShadowCaster,
        ));
    }
}

/// 煙の粒を球で描く
fn show_smoke(
    mut commands: Commands,
    state: Res<EffectState>,
    assets: Res<EffectAssets>,
    mut visuals: Query<(
        &SmokeVisual,
        &mut Transform,
        &mut Visibility,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
    mut spawned: Local<usize>,
) {
    let smoke: Vec<_> = state
        .field
        .particles()
        .iter()
        .filter(|p| p.kind == ParticleKind::Smoke)
        .take(MAX_SMOKE_VISUALS)
        .collect();
    for (visual, mut transform, mut visibility, mut material) in &mut visuals {
        match smoke.get(visual.0) {
            Some(p) => {
                *transform = Transform::from_translation(p.pos.as_vec3())
                    .with_scale(Vec3::splat(p.smoke_radius() as f32));
                // 生まれてすぐは薄く、上がるにつれて薄れて消える
                let fade_in = (p.age / 0.3).min(1.0);
                let density = p.fade() * fade_in;
                let level = ((density * SMOKE_LEVELS as f64) as usize).min(SMOKE_LEVELS - 1);
                let handle = &assets.smoke[level];
                if material.0 != *handle {
                    material.0 = handle.clone();
                }
                visibility.set_if_neq(Visibility::Visible);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    while *spawned < smoke.len() {
        commands.spawn((
            SmokeVisual(*spawned),
            Mesh3d(assets.sphere.clone()),
            MeshMaterial3d(assets.smoke[0].clone()),
            Transform::from_scale(Vec3::ZERO),
            Visibility::Hidden,
            NotShadowCaster,
        ));
        *spawned += 1;
    }
}

/// 火花と床の波紋をギズモの線で描く
fn draw_lines(mut gizmos: Gizmos, settings: Res<ViewSettings>, state: Res<EffectState>) {
    for p in state.field.particles() {
        if p.kind != ParticleKind::Spark {
            continue;
        }
        // 白に近い黄色から、冷えるにつれて橙・赤へ
        let t = p.progress() as f32;
        let color = Color::srgb(1.0, 0.95 - 0.6 * t, 0.7 - 0.7 * t.min(1.0));
        gizmos.line(p.pos.as_vec3(), p.spark_tail().as_vec3(), color);
    }
    let phase = ripple_phase(state.clock);
    for e in &state.emitters {
        let look = e.cue.look(&settings.effects);
        if !look.ripple {
            continue;
        }
        let Some(glow) = look.glow else { continue };
        let [r, g, b] = glow.rgb;
        // 2 本の輪を半周期ずらして広げる
        for offset in [0.0, 0.5] {
            let k = (phase + offset).rem_euclid(1.0);
            let radius = e.radius * (0.9 + 1.2 * k);
            let alpha = (1.0 - k) as f32;
            let center = Vec3::new(e.center.x as f32, e.floor_y as f32 + 2.0, e.center.z as f32);
            gizmos.circle(
                Isometry3d::new(center, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                radius as f32,
                Color::srgba(r, g, b, alpha),
            );
        }
    }
}
