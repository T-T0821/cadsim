//! 2点間の距離計測ツール
//!
//! M キー (またはパネルのボタン) で計測モードに入り、部品の表面を2回クリックすると
//! その2点の距離と X / Y / Z 方向の差を表示する。頂点の近くをクリックすると頂点に吸着する。
//! Esc で計測中の1点目を取り消し、もう一度押すと計測モードを抜ける。
//!
//! 点はクリック位置 (f32 の表示メッシュ) を f64 の形状の表面に載せ直してから使う。

use bevy::{math::DVec3, prelude::*, window::PrimaryWindow};
use bevy_egui::{EguiContexts, egui};

use super::{SimBody, view::SimulationGizmos};
use crate::viewport::PartPicker;

/// 頂点に吸着する範囲 (画面上のピクセル)
const SNAP_PIXELS: f32 = 12.0;
const MEASURE_COLOR: Color = Color::srgb(0.15, 0.75, 0.95);

#[derive(Resource, Default)]
pub struct MeasureTool {
    pub active: bool,
    /// 1点目だけ決まっている計測
    pending: Option<DVec3>,
    /// カーソルの下の点 (吸着後)
    hover: Option<DVec3>,
    pub measurements: Vec<Measurement>,
}

#[derive(Clone, Copy)]
pub struct Measurement {
    pub a: DVec3,
    pub b: DVec3,
}

impl Measurement {
    pub fn distance(&self) -> f64 {
        self.a.distance(self.b)
    }

    pub fn delta(&self) -> DVec3 {
        self.b - self.a
    }
}

impl MeasureTool {
    pub fn set_active(&mut self, active: bool) {
        self.active = active;
        self.pending = None;
        self.hover = None;
    }
}

/// 計測モード中か (面の選択を止めるのに main 側でも使う)
pub fn is_measuring(tool: Res<MeasureTool>) -> bool {
    tool.active
}

pub fn not_measuring(tool: Res<MeasureTool>) -> bool {
    !tool.active
}

pub fn keyboard(keys: Res<ButtonInput<KeyCode>>, mut tool: ResMut<MeasureTool>) {
    if keys.just_pressed(KeyCode::KeyM) {
        let active = !tool.active;
        tool.set_active(active);
    }
    if keys.just_pressed(KeyCode::Escape) && tool.active {
        if tool.pending.is_some() {
            tool.pending = None;
        } else {
            tool.set_active(false);
        }
    }
}

/// カーソルの下の点を求め、クリックされたら計測点として確定する。
#[allow(clippy::too_many_arguments)]
pub fn pick_point(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    picker: PartPicker,
    bodies: Query<&SimBody>,
    mut tool: ResMut<MeasureTool>,
) {
    let (camera, camera_transform) = *camera;
    tool.hover = window.cursor_position().and_then(|cursor| {
        let ray = camera.viewport_to_world(camera_transform, cursor).ok()?;
        // 当たった部品のエンティティが SimBody を持っている
        let hit = picker.cast(ray)?;
        let (body, point) = (bodies.get(hit.entity).ok()?, hit.point);
        Some(snap(body, point, cursor, camera, camera_transform))
    });

    // Alt + 左ドラッグはカメラのオービットなので計測しない
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    if !buttons.just_pressed(MouseButton::Left) || alt {
        return;
    }
    let Some(point) = tool.hover else {
        return;
    };
    match tool.pending.take() {
        None => tool.pending = Some(point),
        Some(first) => tool.measurements.push(Measurement { a: first, b: point }),
    }
}

/// 表示メッシュ上の点を f64 の形状に載せ直し、近くに頂点があればそこへ吸着させる。
fn snap(
    body: &SimBody,
    hit: Vec3,
    cursor: Vec2,
    camera: &Camera,
    camera_transform: &GlobalTransform,
) -> DVec3 {
    let to_local = body.pose.inverse();
    let local_hit = to_local.apply(hit.as_dvec3());
    // 吸着の候補は、クリック位置の周りの三角形の頂点だけ
    let radius = camera_transform.translation().distance(hit) as f64 * 0.05;
    let region = super::geometry::Aabb::from_points([local_hit]).expanded(radius);
    let mut best: Option<(f32, DVec3)> = None;
    body.mesh.for_each_in(&region, |triangle| {
        for &vertex in triangle {
            let world = body.pose.apply(vertex);
            let Ok(screen) = camera.world_to_viewport(camera_transform, world.as_vec3()) else {
                continue;
            };
            let pixels = screen.distance(cursor);
            if pixels <= SNAP_PIXELS && best.is_none_or(|(d, _)| pixels < d) {
                best = Some((pixels, world));
            }
        }
    });
    if let Some((_, vertex)) = best {
        return vertex;
    }
    body.mesh
        .closest_point(local_hit)
        .map_or(hit.as_dvec3(), |(q, _)| body.pose.apply(q))
}

pub fn draw(mut gizmos: Gizmos<SimulationGizmos>, tool: Res<MeasureTool>) {
    let marker = |gizmos: &mut Gizmos<SimulationGizmos>, p: DVec3| {
        gizmos.sphere(
            Isometry3d::from_translation(p.as_vec3()),
            0.025,
            MEASURE_COLOR,
        );
    };
    for m in &tool.measurements {
        gizmos.line(m.a.as_vec3(), m.b.as_vec3(), MEASURE_COLOR);
        marker(&mut gizmos, m.a);
        marker(&mut gizmos, m.b);
    }
    if !tool.active {
        return;
    }
    if let Some(p) = tool.hover {
        marker(&mut gizmos, p);
    }
    if let Some(first) = tool.pending {
        marker(&mut gizmos, first);
        if let Some(p) = tool.hover {
            gizmos.line(first.as_vec3(), p.as_vec3(), MEASURE_COLOR.with_alpha(0.6));
        }
    }
}

/// 計測線の中点に、距離と各軸方向の差をラベルで出す。
pub fn draw_labels(
    mut contexts: EguiContexts,
    tool: Res<MeasureTool>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let painter = ctx.layer_painter(egui::LayerId::background());
    let (camera, camera_transform) = *camera;
    let mut labels: Vec<(DVec3, String)> = tool
        .measurements
        .iter()
        .enumerate()
        .map(|(i, m)| ((m.a + m.b) * 0.5, label_text(i + 1, m)))
        .collect();
    // 計測中は、1点目からカーソルまでの距離も出す
    if tool.active
        && let (Some(a), Some(b)) = (tool.pending, tool.hover)
    {
        labels.push(((a + b) * 0.5, format!("{:.4}", a.distance(b))));
    }
    for (anchor, text) in labels {
        let Ok(screen) = camera.world_to_viewport(camera_transform, anchor.as_vec3()) else {
            continue;
        };
        let galley =
            painter.layout_no_wrap(text, egui::FontId::monospace(13.0), egui::Color32::WHITE);
        let rect = egui::Rect::from_min_size(
            egui::pos2(screen.x + 8.0, screen.y - galley.size().y - 4.0),
            galley.size(),
        )
        .expand(4.0);
        painter.rect_filled(
            rect,
            3.0,
            egui::Color32::from_rgba_unmultiplied(20, 60, 80, 220),
        );
        painter.galley(
            rect.min + egui::vec2(4.0, 4.0),
            galley,
            egui::Color32::WHITE,
        );
    }
    Ok(())
}

pub fn label_text(number: usize, m: &Measurement) -> String {
    let d = m.delta();
    format!(
        "#{number}  L = {:.4}\ndX {:+.4}  dY {:+.4}  dZ {:+.4}",
        m.distance(),
        d.x,
        d.y,
        d.z
    )
}
