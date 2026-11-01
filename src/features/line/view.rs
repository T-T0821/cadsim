//! ライン連携中のワークの 3D 表示
//!
//! 位置の計算は `plantwright_core::line::workpieces`。ここではギズモの立方体で描くだけ。
//! 毎フレーム数も位置も変わるので、メッシュは作らない。

use std::collections::HashMap;

use bevy::math::DVec3;
use bevy::prelude::*;

use plantwright_core::line::workpieces::{arc, element_anchors, stack};

use super::LineState;
use crate::features::floor::{AGV_HEIGHT, AgvLayoutRes, FloorState};
use crate::features::flow::{FlowRes, FlowRun};
use plantwright_core::floor::agv_sim::AgvSim;
use plantwright_core::model::Model;

/// ワークの一辺 (mm)
const WORK_SIZE: f64 = 150.0;
/// 設備から設備へ渡るのにかける見た目の時間 (ラインの秒)
const TRANSIT_TIME: f64 = 1.5;
/// 渡るときの弧の高さ (mm)
const TRANSIT_LIFT: f64 = 400.0;
const WORK_COLOR: Color = Color::srgb(1.0, 0.55, 0.15);

fn to_vec3(p: DVec3) -> Vec3 {
    p.as_vec3()
}

fn cube(gizmos: &mut Gizmos, center: DVec3) {
    gizmos.cube(
        Transform::from_translation(to_vec3(center)).with_scale(Vec3::splat(WORK_SIZE as f32)),
        WORK_COLOR,
    );
}

pub fn draw(
    mut gizmos: Gizmos,
    mut line: ResMut<LineState>,
    flow: Res<FlowRes>,
    run: Res<FlowRun>,
    model: Res<Model>,
    floor: Res<FloorState>,
    layout: Res<AgvLayoutRes>,
) {
    if !line.started {
        return;
    }
    // 表示のための読み出しなので、変更を知らせない
    let line = line.bypass_change_detection();

    if let Some(sim) = run.sim.as_ref().filter(|s| s.is_linked()) {
        let anchors = element_anchors(
            &flow,
            |part| {
                let b = model.world_bounds(part)?;
                let c = b.center();
                Some(DVec3::new(c.x, b.max.y, c.z))
            },
            |station| {
                let n = layout.graph.node(station)?;
                Some(DVec3::new(n.pos[0], 0.0, n.pos[1]))
            },
        );
        // 渡っている途中のワークは、届く先にはまだ描かない
        let mut arriving: HashMap<u32, usize> = HashMap::new();
        for (t, progress) in line.coupler.transits(sim.now(), TRANSIT_TIME) {
            let (Some(&a), Some(&b)) = (anchors.get(&t.from), anchors.get(&t.to)) else {
                continue;
            };
            *arriving.entry(t.to).or_default() += 1;
            let lift = DVec3::Y * (WORK_SIZE / 2.0);
            cube(&mut gizmos, arc(a + lift, b + lift, progress, TRANSIT_LIFT));
        }
        for (&id, &top) in &anchors {
            let content = sim.content_of(id).unwrap_or(0);
            let count = content.saturating_sub(arriving.get(&id).copied().unwrap_or(0));
            for p in stack(top, count, WORK_SIZE) {
                cube(&mut gizmos, p);
            }
        }
    }

    // AGV に積んでいるワーク
    if let Some(agv) = floor.agv_sim() {
        for v in &agv.vehicles {
            let carrying = v
                .job
                .as_ref()
                .and_then(|j| j.tag)
                .is_some_and(|tag| line.coupler.is_loaded(tag));
            if !(carrying && AgvSim::is_loaded(v)) {
                continue;
            }
            let (p, _) = v.pose(agv.graph());
            cube(
                &mut gizmos,
                DVec3::new(p.x, AGV_HEIGHT + WORK_SIZE / 2.0, p.y),
            );
        }
    }
}
