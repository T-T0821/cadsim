//! 3D ビューにセンサーを描く
//!
//! - 光電センサー: 投光器 (小さな箱) と光軸。光軸を遮って ON のときは緑、OFF のときは赤。
//! - 近接センサー: 検出面 (小さな箱) と、検出距離を半径にした球。ON で緑、OFF で灰色。
//!
//! チャートで選んでいるセンサーは橙色の枠で囲む。

use bevy::prelude::*;

use super::chart::TimingChart;
use super::pose::actuator_poses;
use super::sensor::SensorKind;
use super::ui::ChartUiState;
use super::{ChartSchedule, SensorStates, SimulationPose};

const ON_COLOR: Color = Color::srgb(0.2, 0.9, 0.3);
const BEAM_OFF_COLOR: Color = Color::srgb(0.95, 0.25, 0.2);
const PROXIMITY_OFF_COLOR: Color = Color::srgba(0.5, 0.5, 0.55, 0.8);
const SELECT_COLOR: Color = Color::srgb(1.0, 0.65, 0.2);
/// センサー本体 (投光器・検出面) を表す箱の大きさ
const BODY_SIZE: f32 = 0.08;

pub fn draw_sensors(
    mut gizmos: Gizmos,
    chart: Res<TimingChart>,
    schedule: Res<ChartSchedule>,
    pose: Res<SimulationPose>,
    states: Res<SensorStates>,
    ui_state: Res<ChartUiState>,
) {
    if chart.sensors.is_empty() {
        return;
    }
    let poses = actuator_poses(&chart, &schedule, pose.time);
    for sensor in &chart.sensors {
        let mount = sensor
            .mounted_on
            .and_then(|id| poses.get(&id).copied())
            .unwrap_or_default();
        let (origin, direction) = sensor.placement(mount);
        let origin = origin.as_vec3();
        let direction = direction.as_vec3();
        let on = states.is_on(sensor.id);
        let range = sensor.range as f32;

        let color = match (sensor.kind, on) {
            (_, true) => ON_COLOR,
            (SensorKind::Photoelectric, false) => BEAM_OFF_COLOR,
            (SensorKind::Proximity, false) => PROXIMITY_OFF_COLOR,
        };
        let body = Transform::from_translation(origin)
            .with_rotation(mount.rotation.as_quat())
            .with_scale(Vec3::splat(BODY_SIZE));
        gizmos.cube(body, color);

        match sensor.kind {
            SensorKind::Photoelectric => {
                let end = origin + direction * range;
                gizmos.line(origin, end, color);
                // 受光器 (光軸の先)
                gizmos.cube(
                    body.with_translation(end)
                        .with_scale(Vec3::splat(BODY_SIZE * 0.6)),
                    color,
                );
            }
            SensorKind::Proximity => {
                gizmos.sphere(Isometry3d::from_translation(origin), range, color);
            }
        }

        if ui_state.selected_sensor() == Some(sensor.id) {
            gizmos.cube(body.with_scale(Vec3::splat(BODY_SIZE * 2.0)), SELECT_COLOR);
        }
    }
}
