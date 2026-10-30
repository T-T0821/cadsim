//! 3D ビュー右上のビューキューブと左下の座標軸
//!
//! キューブはカメラと同じ向きで回り、面をクリックするとその方向から見たビューになる。

use bevy::prelude::*;
use bevy_egui::egui;

use super::UiState;
use crate::camera::{CameraRequest, StandardView};

const CUBE_SIZE: f32 = 46.0;
const MARGIN: f32 = 16.0;

/// キューブの面 (外向きの法線, ラベル, クリックしたときのビュー)
const FACES: [(Vec3, &str, StandardView); 6] = [
    (Vec3::Z, "正面", StandardView::Front),
    (Vec3::NEG_Z, "背面", StandardView::Back),
    (Vec3::X, "右", StandardView::Right),
    (Vec3::NEG_X, "左", StandardView::Left),
    (Vec3::Y, "上", StandardView::Top),
    (Vec3::NEG_Y, "下", StandardView::Bottom),
];

pub fn show(ctx: &egui::Context, view: egui::Rect, state: &mut UiState) {
    let rotation = state.orbit.rotation();
    let right = rotation * Vec3::X;
    let up = rotation * Vec3::Y;
    let toward_camera = rotation * Vec3::Z;

    let center = egui::pos2(
        view.max.x - MARGIN - CUBE_SIZE * 1.6,
        view.min.y + MARGIN + CUBE_SIZE * 1.6,
    );
    // 正射影でキューブ上の点を画面に写す
    let project = |p: Vec3| center + egui::vec2(p.dot(right), -p.dot(up)) * CUBE_SIZE;

    egui::Area::new("view_cube".into())
        .fixed_pos(view.right_top() + egui::vec2(-(CUBE_SIZE * 3.2 + MARGIN), MARGIN))
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(CUBE_SIZE * 3.2, CUBE_SIZE * 3.0),
                egui::Sense::click(),
            );
            let painter = ui.painter_at(rect);
            let pointer = response.hover_pos();

            // 見えている面を奥から順に描く
            let mut faces: Vec<_> = FACES
                .iter()
                .filter(|(normal, ..)| normal.dot(toward_camera) > 0.05)
                .collect();
            faces.sort_by(|a, b| a.0.dot(toward_camera).total_cmp(&b.0.dot(toward_camera)));

            let mut clicked_view = None;
            for &(normal, label, view) in faces {
                let corners = face_corners(normal).map(project);
                let hovered = pointer.is_some_and(|p| inside_convex(&corners, p));
                let shade = 0.55 + 0.45 * normal.dot(toward_camera);
                let base = if hovered {
                    egui::Color32::from_rgb(70, 140, 255)
                } else {
                    let v = (shade * 200.0) as u8;
                    egui::Color32::from_rgb(v, v, (v as f32 * 1.05).min(255.0) as u8)
                };
                painter.add(egui::Shape::convex_polygon(
                    corners.to_vec(),
                    base,
                    egui::Stroke::new(1.0, egui::Color32::from_gray(60)),
                ));
                // 面がある程度こちらを向いているときだけ文字を出す
                if normal.dot(toward_camera) > 0.35 {
                    painter.text(
                        project(normal * 0.5),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(11.0),
                        egui::Color32::from_gray(20),
                    );
                }
                if hovered && response.clicked() {
                    clicked_view = Some(view);
                }
            }
            if let Some(view) = clicked_view {
                state.camera.write(CameraRequest::View(view));
            }

            // キューブの下に等角ビューと全体表示のボタン
            let buttons = egui::Rect::from_center_size(
                egui::pos2(rect.center().x, rect.max.y - 10.0),
                egui::vec2(rect.width(), 20.0),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(buttons), |ui| {
                ui.horizontal_centered(|ui| {
                    if ui.small_button("等角").clicked() {
                        state
                            .camera
                            .write(CameraRequest::View(StandardView::Isometric));
                    }
                    if ui.small_button("全体").clicked() {
                        state.camera.write(CameraRequest::FitAll);
                    }
                });
            });
        });

    axis_triad(ctx, view, right, up, toward_camera);
}

/// 法線 `n` の面の4つの角 (一辺 1 のキューブ, 中心が原点)
fn face_corners(n: Vec3) -> [Vec3; 4] {
    let u = if n.y.abs() > 0.5 { Vec3::X } else { Vec3::Y };
    let v = n.cross(u);
    let c = n * 0.5;
    [
        c + (u + v) * 0.5,
        c + (u - v) * 0.5,
        c + (-u - v) * 0.5,
        c + (-u + v) * 0.5,
    ]
}

/// 凸多角形の内側か (辺の外積の符号がすべて同じ)
fn inside_convex(points: &[egui::Pos2], p: egui::Pos2) -> bool {
    let signs: Vec<f32> = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(a, b)| (*b - *a).x * (p - *a).y - (*b - *a).y * (p - *a).x)
        .collect();
    signs.iter().all(|&s| s >= 0.0) || signs.iter().all(|&s| s <= 0.0)
}

/// 左下の X/Y/Z 軸 (どちらが上かをいつでも分かるように)
fn axis_triad(ctx: &egui::Context, view: egui::Rect, right: Vec3, up: Vec3, toward_camera: Vec3) {
    let length = 28.0;
    let origin = view.left_bottom() + egui::vec2(MARGIN + length, -(MARGIN + length));
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        "axis_triad".into(),
    ));
    let mut axes = [
        (Vec3::X, "X", egui::Color32::from_rgb(230, 70, 70)),
        (Vec3::Y, "Y", egui::Color32::from_rgb(80, 200, 80)),
        (Vec3::Z, "Z", egui::Color32::from_rgb(80, 130, 240)),
    ];
    // 奥の軸から描く
    axes.sort_by(|a, b| a.0.dot(toward_camera).total_cmp(&b.0.dot(toward_camera)));
    for (axis, label, color) in axes {
        let tip = origin + egui::vec2(axis.dot(right), -axis.dot(up)) * length;
        painter.line_segment([origin, tip], egui::Stroke::new(2.0, color));
        painter.text(
            tip + (tip - origin).normalized() * 8.0,
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            color,
        );
    }
}
