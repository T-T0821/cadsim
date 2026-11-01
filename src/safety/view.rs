//! 安全機器と動作範囲の 3D 表示
//!
//! - 機器の形は半透明のメッシュ (柵・検知範囲・ライトカーテンの光の面・非常停止ボタン)
//! - 検知の状態は線で描く。検知していなければ緑、検知中・押されていれば赤
//! - 動作範囲は半透明の橙の箱。距離が足りない柵とライトカーテンは、近い動作範囲との間に赤い線を引く

use bevy::prelude::*;

use super::{SafetyLayoutRes, SafetyState};
use plantwright_core::floor::geom::Rect2;
use plantwright_core::safety::device::{DeviceKind, SafetyDevice};

const OK_COLOR: Color = Color::srgb(0.2, 0.85, 0.35);
const ALERT_COLOR: Color = Color::srgb(0.95, 0.15, 0.15);
const FENCE_COLOR: Color = Color::srgb(0.95, 0.8, 0.1);
const SELECTED_COLOR: Color = Color::srgb(0.25, 0.55, 1.0);

/// 安全機器の線
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct SafetyGizmos;

/// 機器と動作範囲のメッシュ (機器か動作範囲が変わるたびに作り直す)
#[derive(Component)]
pub struct SafetyVisual;

#[derive(Resource)]
pub struct SafetyMaterials {
    curtain: Handle<StandardMaterial>,
    area: Handle<StandardMaterial>,
    fence: Handle<StandardMaterial>,
    pillar: Handle<StandardMaterial>,
    button: Handle<StandardMaterial>,
    envelope: Handle<StandardMaterial>,
}

fn translucent(color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        unlit: true,
        ..default()
    }
}

pub fn setup(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut gizmo_config: ResMut<GizmoConfigStore>,
) {
    commands.insert_resource(SafetyMaterials {
        curtain: materials.add(translucent(Color::srgba(0.9, 0.2, 0.2, 0.12))),
        area: materials.add(translucent(Color::srgba(0.95, 0.85, 0.15, 0.10))),
        fence: materials.add(translucent(Color::srgba(0.85, 0.75, 0.2, 0.35))),
        pillar: materials.add(StandardMaterial {
            base_color: Color::srgb(0.95, 0.8, 0.1),
            ..default()
        }),
        button: materials.add(StandardMaterial {
            base_color: Color::srgb(0.85, 0.05, 0.05),
            ..default()
        }),
        envelope: materials.add(translucent(Color::srgba(1.0, 0.5, 0.1, 0.16))),
    });
    let (config, _) = gizmo_config.config_mut::<SafetyGizmos>();
    config.line.width = 2.0;
}

pub fn sync_meshes(
    mut commands: Commands,
    layout: Res<SafetyLayoutRes>,
    state: Res<SafetyState>,
    visuals: Query<Entity, With<SafetyVisual>>,
    materials: Res<SafetyMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut shown: Local<Option<(u64, bool)>>,
) {
    // 検知の状態は毎フレーム変わるので、形に関わるものが変わったときだけ作り直す
    let wanted = (state.envelope_revision, state.show_envelopes);
    if !layout.is_changed() && *shown == Some(wanted) {
        return;
    }
    *shown = Some(wanted);
    for entity in &visuals {
        commands.entity(entity).despawn();
    }
    for device in &layout.devices {
        spawn_device(&mut commands, &mut meshes, &materials, device);
    }
    if state.show_envelopes {
        for envelope in &state.envelopes {
            let b = envelope.bounds;
            commands.spawn((
                Mesh3d(meshes.add(Cuboid::from_size(b.size().as_vec3()))),
                MeshMaterial3d(materials.envelope.clone()),
                Transform::from_translation(b.center().as_vec3()),
                SafetyVisual,
            ));
        }
    }
}

fn spawn_device(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &SafetyMaterials,
    device: &SafetyDevice,
) {
    let [w, h, d] = device.size.map(|v| v as f32);
    let [x, y, z] = device.position.map(|v| v as f32);
    let rotation = Quat::from_rotation_y(device.yaw_deg.to_radians() as f32);
    match device.kind {
        DeviceKind::LightCurtain | DeviceKind::AreaSensor | DeviceKind::Fence => {
            let material = match device.kind {
                DeviceKind::LightCurtain => materials.curtain.clone(),
                DeviceKind::AreaSensor => materials.area.clone(),
                _ => materials.fence.clone(),
            };
            commands.spawn((
                Mesh3d(meshes.add(Cuboid::new(w, h, d))),
                MeshMaterial3d(material),
                Transform::from_xyz(x, y + h / 2.0, z).with_rotation(rotation),
                SafetyVisual,
            ));
        }
        DeviceKind::EmergencyStop => {
            let radius = w.max(d) / 2.0;
            commands.spawn((
                Mesh3d(meshes.add(Cylinder::new(radius * 0.5, h))),
                MeshMaterial3d(materials.pillar.clone()),
                Transform::from_xyz(x, y + h / 2.0, z),
                SafetyVisual,
            ));
            commands.spawn((
                Mesh3d(meshes.add(Sphere::new(radius))),
                MeshMaterial3d(materials.button.clone()),
                Transform::from_xyz(x, y + h, z),
                SafetyVisual,
            ));
        }
    }
}

pub fn draw(
    mut gizmos: Gizmos<SafetyGizmos>,
    layout: Res<SafetyLayoutRes>,
    state: Res<SafetyState>,
) {
    for device in &layout.devices {
        let alert = state.is_detecting(device.id);
        let selected = state.selected == Some(device.id);
        let color = if alert {
            ALERT_COLOR
        } else if device.kind == DeviceKind::Fence {
            FENCE_COLOR
        } else {
            OK_COLOR
        };
        match device.kind {
            DeviceKind::LightCurtain => draw_light_curtain(&mut gizmos, device, color),
            DeviceKind::AreaSensor | DeviceKind::Fence => {
                draw_box(&mut gizmos, device, color);
            }
            DeviceKind::EmergencyStop => {
                let p = device.button_position().as_vec3();
                let r = (device.size[0].max(device.size[2]) * 0.7) as f32;
                gizmos.circle(
                    Isometry3d::new(p, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                    r,
                    color,
                );
            }
        }
        if selected {
            draw_box(&mut gizmos, device, SELECTED_COLOR);
        }
    }

    // 距離が足りないところ: 機器から近い動作範囲へ赤い線
    for clearance in state.clearances.iter().filter(|c| !c.is_ok()) {
        let (Some(device), Some(envelope)) = (
            layout.get(clearance.device),
            state
                .envelopes
                .iter()
                .find(|e| e.actuator == clearance.actuator),
        ) else {
            continue;
        };
        let from = device.footprint().center;
        let to = Rect2::from_aabb(&envelope.bounds).center;
        let y = (device.position[1] + 10.0) as f32;
        gizmos.line(
            Vec3::new(from.x as f32, y, from.y as f32),
            Vec3::new(to.x as f32, y, to.y as f32),
            ALERT_COLOR,
        );
    }
}

/// 機器の形を床の長方形 × 高さの箱として線で描く
fn draw_box(gizmos: &mut Gizmos<SafetyGizmos>, device: &SafetyDevice, color: Color) {
    let zone = device.zone();
    let corners = zone.footprint.corners();
    for y0 in [zone.y_min, zone.y_max] {
        for i in 0..4 {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            gizmos.line(
                Vec3::new(a.x as f32, y0 as f32, a.y as f32),
                Vec3::new(b.x as f32, y0 as f32, b.y as f32),
                color,
            );
        }
    }
    for c in corners {
        gizmos.line(
            Vec3::new(c.x as f32, zone.y_min as f32, c.y as f32),
            Vec3::new(c.x as f32, zone.y_max as f32, c.y as f32),
            color,
        );
    }
}

/// 両端の柱と光軸を描く
fn draw_light_curtain(gizmos: &mut Gizmos<SafetyGizmos>, device: &SafetyDevice, color: Color) {
    let footprint = device.footprint();
    let [ax, _] = footprint.axes();
    let left = footprint.center - ax * footprint.half.x;
    let right = footprint.center + ax * footprint.half.x;
    let (y0, h) = (device.position[1], device.size[1]);
    for p in [left, right] {
        gizmos.line(
            Vec3::new(p.x as f32, y0 as f32, p.y as f32),
            Vec3::new(p.x as f32, (y0 + h) as f32, p.y as f32),
            color,
        );
    }
    // 光軸は多すぎると見づらいので、最大 40 本に間引く
    let pitch = device.resolution.max(h / 40.0).max(1.0);
    let mut y = y0;
    while y <= y0 + h + 1e-6 {
        gizmos.line(
            Vec3::new(left.x as f32, y as f32, left.y as f32),
            Vec3::new(right.x as f32, y as f32, right.y as f32),
            color.with_alpha(0.6),
        );
        y += pitch;
    }
}
