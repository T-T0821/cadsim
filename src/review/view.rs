//! ほかの参加者を 3D ビューに描く
//!
//! - アバター: カメラの位置に小さな球と、見ている向きの四角すい
//! - 指さし: アバターからマウスの指している点へ線を引き、点に印を付ける
//! - 「ここを見て」の印: 広がって消えていく輪
//! - 選んだ部品: その人の色の枠
//! - 名前とチャットの吹き出し: egui で 3D の位置に重ねる

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use plantwright_core::review::{PeerId, peer_color};

use super::Review;
use crate::view::camera::{OrbitCamera, ViewportRect};
use plantwright_core::model::Model;

/// チャットの吹き出しを出しておく秒数
const BUBBLE_SECONDS: f64 = 6.0;

fn color(rgb: [u8; 3]) -> Color {
    Color::srgb_u8(rgb[0], rgb[1], rgb[2])
}

fn egui_color(rgb: [u8; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

fn color_of(review: &Review, id: PeerId) -> [u8; 3] {
    review
        .session
        .peer(id)
        .map_or_else(|| peer_color(id), |p| p.info.color)
}

pub fn draw(
    time: Res<Time>,
    review: Res<Review>,
    orbit: Res<OrbitCamera>,
    model: Res<Model>,
    mut gizmos: Gizmos,
) {
    if !review.session.joined() {
        return;
    }
    let my_eye = orbit.eye();
    // 自分のカメラからの距離に比例した大きさにして、遠くの人も近くの人も同じくらいに見せる
    let size_at = |p: Vec3| (my_eye.distance(p) * 0.02).max(1e-4);
    for peer in review.session.peers() {
        let c = color(peer.info.color);
        if let Some(pose) = peer.view {
            let eye = pose.eye();
            let s = size_at(eye);
            let dir = pose.direction();
            let right = dir.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
            let up = right.cross(dir);
            gizmos.sphere(Isometry3d::from_translation(eye), s * 0.6, c);
            let center = eye + dir * s * 2.2;
            let corners = [
                center + right * s * 1.2 + up * s * 0.8,
                center - right * s * 1.2 + up * s * 0.8,
                center - right * s * 1.2 - up * s * 0.8,
                center + right * s * 1.2 - up * s * 0.8,
            ];
            for i in 0..4 {
                gizmos.line(eye, corners[i], c);
                gizmos.line(corners[i], corners[(i + 1) % 4], c);
            }
            if let Some(point) = peer.pointer {
                gizmos.line(eye, Vec3::from(point), c.with_alpha(0.5));
            }
        }
        if let Some(point) = peer.pointer {
            let p = Vec3::from(point);
            let s = size_at(p) * 0.4;
            gizmos.sphere(Isometry3d::from_translation(p), s, c);
            for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                gizmos.line(p - axis * s * 2.0, p + axis * s * 2.0, c);
            }
        }
        if let Some(bounds) = peer
            .selected
            .as_deref()
            .and_then(|label| review.labels.find(label))
            .and_then(|id| model.world_bounds(id))
        {
            let size = bounds.size().as_vec3().max(Vec3::splat(1e-4)) * 1.02;
            gizmos.cube(
                Transform::from_translation(bounds.center().as_vec3()).with_scale(size),
                c,
            );
        }
    }
    let now = time.elapsed_secs_f64();
    for ping in review.session.pings() {
        let p = Vec3::from(ping.point);
        let age = ping.age(now);
        let c = color(color_of(&review, ping.from)).with_alpha(1.0 - age);
        let s = size_at(p);
        let facing = (my_eye - p).try_normalize().unwrap_or(Vec3::Y);
        let rotation = Quat::from_rotation_arc(Vec3::Z, facing);
        for ring in 0..3 {
            let grow = (age * 3.0 + ring as f32 * 0.5).fract();
            gizmos.circle(Isometry3d::new(p, rotation), s * (0.5 + grow * 2.5), c);
        }
        gizmos.line(p, p + Vec3::Y * s * 4.0, c);
    }
}

/// 名前・チャットの吹き出し・「ここを見て」の名札を 3D の位置に重ねる
pub fn labels(
    mut contexts: EguiContexts,
    time: Res<Time>,
    review: Res<Review>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    viewport: Res<ViewportRect>,
) -> Result {
    if !review.session.joined() {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let (camera, camera_transform) = *camera;
    let view = viewport.rect.map(|r| {
        egui::Rect::from_min_max(egui::pos2(r.min.x, r.min.y), egui::pos2(r.max.x, r.max.y))
    });
    let to_screen = |p: Vec3| {
        camera
            .world_to_viewport(camera_transform, p)
            .ok()
            .map(|v| egui::pos2(v.x, v.y))
            .filter(|p| view.is_none_or(|r| r.contains(*p)))
    };
    let mut painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("review_labels"),
    ));
    if let Some(view) = view {
        painter.set_clip_rect(view);
    }
    let now = time.elapsed_secs_f64();
    let font = egui::FontId::proportional(13.0);
    let tag = |painter: &egui::Painter, at: egui::Pos2, text: &str, rgb: [u8; 3]| {
        let galley = painter.layout_no_wrap(text.to_string(), font.clone(), egui::Color32::BLACK);
        let rect = egui::Rect::from_min_size(
            at + egui::vec2(-galley.size().x / 2.0 - 6.0, -galley.size().y - 18.0),
            galley.size() + egui::vec2(12.0, 6.0),
        );
        painter.rect_filled(rect, 4.0, egui_color(rgb));
        painter.galley(
            rect.min + egui::vec2(6.0, 3.0),
            galley,
            egui::Color32::BLACK,
        );
        rect
    };
    for peer in review.session.peers() {
        let Some(at) = peer.view.and_then(|v| to_screen(v.eye())) else {
            continue;
        };
        let rect = tag(&painter, at, &peer.info.name, peer.info.color);
        if let Some((text, said_at)) = &peer.last_chat
            && now - said_at < BUBBLE_SECONDS
        {
            let galley = painter.layout(
                text.clone(),
                font.clone(),
                egui::Color32::from_gray(20),
                240.0,
            );
            let bubble = egui::Rect::from_min_size(
                egui::pos2(
                    rect.center().x - galley.size().x / 2.0 - 8.0,
                    rect.min.y - galley.size().y - 16.0,
                ),
                galley.size() + egui::vec2(16.0, 10.0),
            );
            painter.rect_filled(bubble, 8.0, egui::Color32::from_gray(245));
            painter.rect_stroke(
                bubble,
                8.0,
                egui::Stroke::new(2.0, egui_color(peer.info.color)),
                egui::StrokeKind::Inside,
            );
            painter.galley(
                bubble.min + egui::vec2(8.0, 5.0),
                galley,
                egui::Color32::from_gray(20),
            );
        }
    }
    for ping in review.session.pings() {
        let Some(at) = to_screen(Vec3::from(ping.point)) else {
            continue;
        };
        let name = if Some(ping.from) == review.session.me {
            "ここを見て".to_string()
        } else {
            review.session.peer(ping.from).map_or_else(
                || "ここを見て".into(),
                |p| format!("{}: ここを見て", p.info.name),
            )
        };
        tag(
            &painter,
            at + egui::vec2(0.0, -24.0),
            &name,
            color_of(&review, ping.from),
        );
    }
    Ok(())
}
