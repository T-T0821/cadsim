//! 穴あけ道具の 3D の線と、画面に出す寸法・案内の文字
//!
//! 置きかけの穴は、入口と底の円・それをつなぐ線・軸の線で描く (青)。
//! 直径の点と底の中心は小さな球で、カーソルが乗ると明るくなる。
//! 部品の中に隠れないよう、線も点も形状より手前に描く ([`HoleGizmos`])。

use bevy::{math::DVec3, prelude::*};
use bevy_egui::{EguiContexts, egui};

use crate::viewport::camera::ViewportRect;
use plantwright_core::modeling::hole_tool::{Handle, HoleTool};

const HOLE_COLOR: Color = Color::srgb(0.25, 0.65, 1.0);
const HANDLE_COLOR: Color = Color::srgb(1.0, 0.85, 0.25);
const HANDLE_HOVER_COLOR: Color = Color::srgb(1.0, 1.0, 0.6);
const CIRCLE_SEGMENTS: usize = 48;

/// 穴あけ道具のギズモ。部品の中の線 (底の円・軸) やつまみが形状に隠れないよう手前に描く
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct HoleGizmos;

pub fn configure_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<HoleGizmos>();
    config.depth_bias = -1.0;
    config.line.width = 2.0;
}

pub fn draw_preview(
    mut gizmos: Gizmos<HoleGizmos>,
    tool: Res<HoleTool>,
    camera: Single<&GlobalTransform, With<Camera3d>>,
) {
    let Some(draft) = tool.draft.as_ref().filter(|_| tool.active) else {
        return;
    };
    let polyline = |gizmos: &mut Gizmos<HoleGizmos>, points: &[DVec3], color: Color| {
        let mut pts: Vec<Vec3> = points.iter().map(|p| p.as_vec3()).collect();
        if let Some(first) = pts.first().copied() {
            pts.push(first);
        }
        gizmos.linestrip(pts, color);
    };
    let entrance = draft.circle(draft.center, CIRCLE_SEGMENTS);
    let bottom = draft.circle(draft.bottom(), CIRCLE_SEGMENTS);
    polyline(&mut gizmos, &entrance, HOLE_COLOR);
    polyline(&mut gizmos, &bottom, HOLE_COLOR.with_alpha(0.7));
    for i in (0..CIRCLE_SEGMENTS).step_by(CIRCLE_SEGMENTS / 4) {
        gizmos.line(
            entrance[i].as_vec3(),
            bottom[i].as_vec3(),
            HOLE_COLOR.with_alpha(0.5),
        );
    }
    // 軸 (貫通なら部品の反対側まで)
    gizmos.line(
        draft.center.as_vec3(),
        draft.bottom().as_vec3(),
        HOLE_COLOR.with_alpha(0.8),
    );

    // つまみ。見かけの大きさがそろうよう、カメラからの距離で半径を決める
    for handle in [Handle::Diameter, Handle::Depth] {
        let p = draft.handle_position(handle).as_vec3();
        let radius = camera.translation().distance(p) * 0.008;
        let color = if tool.hovered == Some(handle) || tool.dragging == Some(handle) {
            HANDLE_HOVER_COLOR
        } else {
            HANDLE_COLOR
        };
        gizmos.sphere(Isometry3d::from_translation(p), radius, color);
    }
}

/// つまみの横に寸法、ビューの下に案内を出す
pub fn draw_labels(
    mut contexts: EguiContexts,
    tool: Res<HoleTool>,
    viewport: Res<ViewportRect>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
) -> Result {
    if !tool.active {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let Some(view) = viewport.rect.map(|r| {
        egui::Rect::from_min_max(egui::pos2(r.min.x, r.min.y), egui::pos2(r.max.x, r.max.y))
    }) else {
        return Ok(());
    };
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("hole_tool_labels"),
        ))
        .with_clip_rect(view);
    let (camera, camera_transform) = *camera;

    if let Some(draft) = &tool.draft {
        let labels = [
            (Handle::Diameter, format!("直径 {}", round3(draft.diameter))),
            (
                Handle::Depth,
                match draft.depth {
                    None => "貫通".to_owned(),
                    Some(d) => format!("深さ {}", round3(d)),
                },
            ),
        ];
        for (handle, text) in labels {
            let Ok(screen) =
                camera.world_to_viewport(camera_transform, draft.handle_position(handle).as_vec3())
            else {
                continue;
            };
            let strong = tool.hovered == Some(handle) || tool.dragging == Some(handle);
            label(
                &painter,
                egui::pos2(screen.x + 12.0, screen.y - 10.0),
                &text,
                if strong {
                    egui::Color32::from_rgb(60, 60, 20)
                } else {
                    egui::Color32::from_rgba_unmultiplied(20, 50, 90, 220)
                },
            );
        }
    }

    // 案内 (エラーがあればそれを赤く)
    let (text, background) = match &tool.error {
        Some(e) => (
            e.clone(),
            egui::Color32::from_rgba_unmultiplied(120, 30, 30, 230),
        ),
        None => (
            tool.hint().to_owned(),
            egui::Color32::from_rgba_unmultiplied(20, 50, 90, 220),
        ),
    };
    let galley =
        painter.layout_no_wrap(text, egui::FontId::proportional(14.0), egui::Color32::WHITE);
    let size = galley.size() + egui::vec2(16.0, 10.0);
    let rect = egui::Rect::from_center_size(egui::pos2(view.center().x, view.max.y - 28.0), size);
    painter.rect_filled(rect, 6.0, background);
    painter.galley(
        rect.min + egui::vec2(8.0, 5.0),
        galley,
        egui::Color32::WHITE,
    );
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

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}
