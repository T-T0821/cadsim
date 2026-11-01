//! 作業者の歩行経路・AGV の通路の表示と、床をクリックして点を置く操作
//!
//! 経路はすべて線 (ギズモ) で描く。作業者と AGV の体は `figures.rs` が 3D モデルで出す。

use bevy::math::DVec2;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use super::{AgvLayoutRes, FloorState, FloorTool, OperatorsRes};
use crate::viewport::camera::ViewportRect;
use plantwright_core::floor::operator_plan::{Activity, OperatorTask};

const PATH_COLOR: Color = Color::srgb(0.3, 0.75, 1.0);
const EDGE_COLOR: Color = Color::srgb(0.55, 0.55, 0.6);
const STATION_COLOR: Color = Color::srgb(0.3, 0.85, 0.5);
const RESERVED_COLOR: Color = Color::srgb(1.0, 0.55, 0.1);
const SELECTED_COLOR: Color = Color::srgb(0.25, 0.55, 1.0);
/// 経路の点をクリックで拾う距離 (mm)
const PICK_RADIUS: f64 = 400.0;

fn floor_point(p: DVec2, y: f64) -> Vec3 {
    Vec3::new(p.x as f32, y as f32, p.y as f32)
}

pub fn draw(
    mut gizmos: Gizmos,
    operators: Res<OperatorsRes>,
    layout: Res<AgvLayoutRes>,
    state: Res<FloorState>,
) {
    // 作業者の歩行経路・作業場所
    for op in &operators.operators {
        let start = DVec2::from_array(op.start);
        let selected = state.selected_operator == Some(op.id);
        if (state.show_paths || selected)
            && let Some(plan) = state.plans.get(&op.id)
        {
            {
                let path = plan.path(start);
                gizmos.linestrip(path.iter().map(|&p| floor_point(p, 5.0)), PATH_COLOR);
                for s in plan
                    .segments
                    .iter()
                    .filter(|s| s.activity != Activity::Walk)
                {
                    let color = match s.task.and_then(|i| op.tasks.get(i)) {
                        Some(OperatorTask::Button { .. }) => Color::srgb(0.95, 0.3, 0.3),
                        Some(OperatorTask::Pick { .. }) => STATION_COLOR,
                        _ => Color::srgb(0.7, 0.7, 0.7),
                    };
                    gizmos.circle(
                        Isometry3d::new(
                            floor_point(s.from, 5.0),
                            Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
                        ),
                        200.0,
                        color,
                    );
                }
            }
        }
    }

    // AGV の経路: 通路・点・ステーション・予約中の交差点
    let graph = &layout.graph;
    for &(a, b) in &graph.edges {
        gizmos.line(
            floor_point(graph.pos(a), 3.0),
            floor_point(graph.pos(b), 3.0),
            EDGE_COLOR,
        );
    }
    let sim = state.sim.as_ref();
    for node in &graph.nodes {
        let p = floor_point(DVec2::from_array(node.pos), 3.0);
        let reserved = sim.and_then(|s| s.holder(node.id)).is_some();
        let color = if state.selected_node == Some(node.id) || state.connect_from == Some(node.id) {
            SELECTED_COLOR
        } else if reserved {
            RESERVED_COLOR
        } else if node.station {
            STATION_COLOR
        } else {
            EDGE_COLOR
        };
        let flat = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        if node.station {
            gizmos.rect(Isometry3d::new(p, flat), Vec2::splat(600.0), color);
        } else {
            let size = if graph.is_intersection(node.id) {
                350.0
            } else {
                150.0
            };
            gizmos.circle(Isometry3d::new(p, flat), size, color);
        }
    }
}

/// 床 (y = 0) をクリックした位置
fn clicked_floor(
    window: &Window,
    viewport: &ViewportRect,
    camera: &Camera,
    camera_transform: &GlobalTransform,
) -> Option<DVec2> {
    let cursor = window.cursor_position().filter(|&c| viewport.contains(c))?;
    let ray = camera.viewport_to_world(camera_transform, cursor).ok()?;
    let distance = ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))?;
    let p = ray.get_point(distance);
    Some(DVec2::new(p.x as f64, p.z as f64))
}

/// 床をクリックして、経路の点・ステーション・通路・作業者の歩く先を置く
pub fn floor_click(
    buttons: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    viewport: Res<ViewportRect>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut layout: ResMut<AgvLayoutRes>,
    mut operators: ResMut<OperatorsRes>,
    mut state: ResMut<FloorState>,
) {
    if state.tool == FloorTool::None || !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let (camera, camera_transform) = *camera;
    let Some(p) = clicked_floor(&window, &viewport, camera, camera_transform) else {
        return;
    };
    // 10 mm 単位に丸める
    let snapped = [(p.x / 10.0).round() * 10.0, (p.y / 10.0).round() * 10.0];
    let near = layout
        .graph
        .nodes
        .iter()
        .map(|n| (n.id, DVec2::from_array(n.pos).distance(p)))
        .filter(|&(_, d)| d < PICK_RADIUS)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id);
    match state.tool {
        FloorTool::None => {}
        FloorTool::AddNode | FloorTool::AddStation => {
            let station = state.tool == FloorTool::AddStation;
            let id = match near {
                // 既存の点の上なら、ステーションかどうかを切り替える
                Some(id) => {
                    if let Some(node) = layout.graph.node_mut(id) {
                        node.station = station;
                    }
                    id
                }
                None => {
                    let id = layout.graph.add_node(snapped, station);
                    // 直前に選んだ点と通路でつなぐ
                    if let Some(prev) = state.selected_node {
                        layout.graph.toggle_edge(prev, id);
                    }
                    id
                }
            };
            state.selected_node = Some(id);
        }
        FloorTool::Connect => {
            let Some(id) = near else {
                return;
            };
            match state.connect_from.take() {
                Some(first) if first != id => layout.graph.toggle_edge(first, id),
                Some(_) => {}
                None => state.connect_from = Some(id),
            }
            state.selected_node = Some(id);
        }
        FloorTool::AddWalk(op) => {
            if let Some(op) = operators.operators.iter_mut().find(|o| o.id == op) {
                op.tasks.push(OperatorTask::Walk { to: snapped });
            }
        }
    }
}
