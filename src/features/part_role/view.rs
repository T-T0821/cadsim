//! 役割の道具の 3D の矢印・点・半透明の動いた先と、画面に出す値・「!」の印
//!
//! - シリンダ・直動軸: 面から出る橙の矢印と、動いた先の部品の外接箱 (行き来する箱と、出端の薄い箱)
//! - 回転テーブル: 部品の中心を通る回転軸
//! - センサー: 取り付け位置からの光の線
//! - コンベア: 上面の緑の矢印 (長さが 1 秒に進む距離)
//! - グリッパ: 掴む位置の青い球と、掴める距離の円
//! - 投入口: 上の点と、つないだコンベア (またはドラッグ中のカーソル) までの線
//!
//! 線も点も部品に隠れないよう手前に描く ([`RoleGizmos`])。

use bevy::{math::DVec3, prelude::*};
use bevy_egui::{EguiContexts, egui};

use crate::document::selection::Selection;
use crate::features::conveyor::ConveyorConfigRes;
use crate::viewport::camera::ViewportRect;
use plantwright_core::conveyor::SourceOutput;
use plantwright_core::math::Aabb;
use plantwright_core::model::Model;
use plantwright_core::motion::chart::TimingChart;
use plantwright_core::part_role::assign;
use plantwright_core::part_role::{Handle, RoleBody, RoleTool};

const ACTUATOR_COLOR: Color = Color::srgb(1.0, 0.6, 0.2);
const SENSOR_COLOR: Color = Color::srgb(1.0, 0.3, 0.3);
const CONVEYOR_COLOR: Color = Color::srgb(0.3, 0.9, 0.4);
const GRIPPER_COLOR: Color = Color::srgb(0.3, 0.55, 1.0);
const LINK_COLOR: Color = Color::srgb(0.75, 0.5, 1.0);
const HANDLE_COLOR: Color = Color::srgb(1.0, 0.85, 0.25);
const HANDLE_HOVER_COLOR: Color = Color::srgb(1.0, 1.0, 0.6);

/// 役割の道具のギズモ。部品の中を通る線やつまみが形状に隠れないよう手前に描く
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct RoleGizmos;

pub fn configure_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<RoleGizmos>();
    config.depth_bias = -1.0;
    config.line.width = 2.5;
}

pub fn draw_preview(
    mut gizmos: Gizmos<RoleGizmos>,
    tool: Res<RoleTool>,
    model: Res<Model>,
    conveyors: Res<ConveyorConfigRes>,
    time: Res<Time>,
    camera: Single<&GlobalTransform, With<Camera3d>>,
) {
    let Some(draft) = &tool.draft else {
        return;
    };
    let color = match &draft.body {
        RoleBody::Actuator(_) => ACTUATOR_COLOR,
        RoleBody::Sensor(_) => SENSOR_COLOR,
        RoleBody::Conveyor(_) => CONVEYOR_COLOR,
        RoleBody::Gripper(_) => GRIPPER_COLOR,
        RoleBody::Source(_) | RoleBody::Sink(_) => LINK_COLOR,
    };
    // 役割を付ける部品の外接箱
    cuboid(&mut gizmos, &draft.bounds, color.with_alpha(0.35));

    if let Some((base, tip)) = draft.arrow() {
        match &draft.body {
            RoleBody::Sensor(_) => {
                gizmos.line(base.as_vec3(), tip.as_vec3(), color);
            }
            _ if base.distance(tip) > 1e-9 => {
                gizmos
                    .arrow(base.as_vec3(), tip.as_vec3(), color)
                    .with_tip_length((base.distance(tip) * 0.15) as f32);
            }
            _ => {}
        }
    }
    // シリンダ・直動軸は出端の箱 (薄く) と、行き来する箱 (2 秒で 1 往復)
    if let Some(end) = draft.moved_bounds(1.0) {
        cuboid(&mut gizmos, &end, color.with_alpha(0.3));
        let phase = (time.elapsed_secs_f64() * std::f64::consts::PI).sin() * 0.5 + 0.5;
        if let Some(moving) = draft.moved_bounds(phase) {
            cuboid(&mut gizmos, &moving, color.with_alpha(0.8));
        }
    }
    match &draft.body {
        RoleBody::Gripper(g) => {
            let tcp = DVec3::from_array(g.tcp).as_vec3();
            gizmos.circle(
                Isometry3d::new(tcp, Quat::from_rotation_arc(Vec3::Z, Vec3::Y)),
                g.reach as f32,
                color,
            );
        }
        RoleBody::Source(s) => {
            let from = draft.handle_position(Handle::Link).as_vec3();
            let to = tool
                .link_point
                .map(|p| p.as_vec3())
                .or_else(|| match s.output {
                    SourceOutput::Conveyor(id) => conveyors
                        .conveyors
                        .iter()
                        .find(|c| c.id == id)
                        .and_then(|c| model.world_bounds(c.part?))
                        .map(|b| top_center(&b).as_vec3()),
                    SourceOutput::OnPart => None,
                });
            if let Some(to) = to {
                gizmos.arrow(from, to, color);
            }
        }
        _ => {}
    }

    // つまみ。見かけの大きさがそろうよう、カメラからの距離で半径を決める
    for handle in draft.handles() {
        let p = draft.handle_position(handle).as_vec3();
        let radius = camera.translation().distance(p) * 0.009;
        let color = if tool.hovered == Some(handle) || tool.dragging == Some(handle) {
            HANDLE_HOVER_COLOR
        } else if handle == Handle::Grip {
            GRIPPER_COLOR
        } else {
            HANDLE_COLOR
        };
        gizmos.sphere(Isometry3d::from_translation(p), radius, color);
    }
}

fn cuboid(gizmos: &mut Gizmos<RoleGizmos>, b: &Aabb, color: Color) {
    let size = b.size().as_vec3();
    gizmos.cube(
        Transform::from_translation(b.center().as_vec3()).with_scale(size),
        color,
    );
}

fn top_center(b: &Aabb) -> DVec3 {
    let c = b.center();
    DVec3::new(c.x, b.max.y, c.z)
}

fn view_rect(viewport: &ViewportRect) -> Option<egui::Rect> {
    viewport.rect.map(|r| {
        egui::Rect::from_min_max(egui::pos2(r.min.x, r.min.y), egui::pos2(r.max.x, r.max.y))
    })
}

/// つまみの横に値を出す (案内はビューの下の共通の案内バー)
pub fn draw_labels(
    mut contexts: EguiContexts,
    tool: Res<RoleTool>,
    model: Res<Model>,
    viewport: Res<ViewportRect>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
) -> Result {
    let Some(draft) = &tool.draft else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let Some(view) = view_rect(&viewport) else {
        return Ok(());
    };
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("part_role_labels"),
        ))
        .with_clip_rect(view);
    let (camera, camera_transform) = *camera;
    for handle in draft.handles() {
        let Ok(screen) =
            camera.world_to_viewport(camera_transform, draft.handle_position(handle).as_vec3())
        else {
            continue;
        };
        let strong = tool.hovered == Some(handle) || tool.dragging == Some(handle);
        label(
            &painter,
            egui::pos2(screen.x + 12.0, screen.y - 10.0),
            &draft.handle_label(handle, &model.unit),
            if strong {
                egui::Color32::from_rgb(60, 60, 20)
            } else {
                egui::Color32::from_rgba_unmultiplied(20, 50, 90, 220)
            },
        );
    }

    Ok(())
}

/// 設定が足りない役割の部品の上に黄色い「!」を出す。押すとその部品を選ぶ (プロパティに役割が出る)
pub fn draw_issue_marks(
    mut contexts: EguiContexts,
    chart: Res<TimingChart>,
    conveyors: Res<ConveyorConfigRes>,
    model: Res<Model>,
    viewport: Res<ViewportRect>,
    mut selection: ResMut<Selection>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
) -> Result {
    let Some(view) = view_rect(&viewport) else {
        return Ok(());
    };
    let marks = assign::parts_with_issues(&chart, &conveyors);
    if marks.is_empty() {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let (camera, camera_transform) = *camera;
    for (part, reasons) in marks {
        let Some(bounds) = model.world_bounds(part) else {
            continue;
        };
        let Ok(screen) = camera.world_to_viewport(camera_transform, top_center(&bounds).as_vec3())
        else {
            continue;
        };
        let at = egui::pos2(screen.x, screen.y - 14.0);
        if !view.contains(at) {
            continue;
        }
        let name = model.get(part).map_or("", |n| n.name.as_str());
        let response = egui::Area::new(egui::Id::new(("part_role_issue", part)))
            .order(egui::Order::Background)
            .fixed_pos(at - egui::vec2(10.0, 10.0))
            .show(ctx, |ui| {
                ui.add(
                    egui::Button::new(
                        egui::RichText::new("!")
                            .strong()
                            .color(egui::Color32::BLACK),
                    )
                    .fill(egui::Color32::from_rgb(250, 200, 40))
                    .corner_radius(10.0)
                    .min_size(egui::vec2(20.0, 20.0)),
                )
                .on_hover_text(format!("{name}\n{}", reasons.join("\n")))
            })
            .inner;
        if response.clicked() {
            selection.select(part);
            selection.reveal_in_tree = true;
        }
    }
    Ok(())
}

fn label(painter: &egui::Painter, at: egui::Pos2, text: &str, background: egui::Color32) {
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        egui::FontId::monospace(13.0),
        egui::Color32::WHITE,
    );
    let rect = egui::Rect::from_min_size(at, galley.size()).expand(4.0);
    painter.rect_filled(rect, 3.0, background);
    painter.galley(
        rect.min + egui::vec2(4.0, 4.0),
        galley,
        egui::Color32::WHITE,
    );
}
