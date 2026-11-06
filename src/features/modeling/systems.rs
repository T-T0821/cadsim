//! 穴あけ道具のマウスとキーの読み方
//!
//! - 部品の面にカーソルを乗せると面が光り、クリックするとそこが穴の入口になる
//! - 置いた穴の「直径の点」「底の中心」をドラッグすると直径・深さが変わる (Ctrl で刻みにそろえる)
//! - Enter で穴をあけ、Esc で取り消す
//!
//! 寸法の計算は `plantwright_core::modeling::hole_tool`。ここはカーソルの位置をレイにして渡すだけ。

use bevy::{math::DVec3, prelude::*, window::PrimaryWindow};

use super::confirm_hole;
use crate::document::history::History;
use crate::document::import::ImportJobs;
use crate::document::selection::Selection;
use crate::viewport::camera::{ViewportRect, alt_pressed};
use crate::viewport::gizmo::GizmoTool;
use crate::viewport::picking::PartUnderPointer;
use plantwright_core::model::Model;
use plantwright_core::modeling::hole_tool::{Handle, HoleDraft, HoleTool};

/// つまみをつかめる範囲 (画面上のピクセル)
const HANDLE_PIXELS: f32 = 14.0;

/// Enter で穴をあけ、Esc で置きかけの穴を取り消す (もう一度で道具をやめる)
pub fn hole_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut tool: ResMut<HoleTool>,
    mut model: ResMut<Model>,
    mut history: ResMut<History>,
    mut jobs: ResMut<ImportJobs>,
) {
    if !tool.active {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        tool.cancel();
    }
    if keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter]) {
        confirm_hole(&mut tool, &mut model, &mut history, &mut jobs);
    }
}

/// 面のホバーとクリック、つまみのドラッグ
#[allow(clippy::too_many_arguments)]
pub fn hole_input(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    viewport: Res<ViewportRect>,
    under_pointer: PartUnderPointer,
    model: Res<Model>,
    gizmo: Res<GizmoTool>,
    mut tool: ResMut<HoleTool>,
    mut selection: ResMut<Selection>,
) {
    if !tool.active {
        return;
    }
    // 穴をあける部品は選択中の部品。選択が外れたら道具もやめる
    let Some(target) = selection.node.filter(|&id| model.get(id).is_some()) else {
        tool.stop();
        return;
    };
    let (camera, camera_transform) = *camera;
    let cursor = window.cursor_position();
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let step = (gizmo.snap.always || ctrl).then_some(gizmo.snap.size_step);
    let cursor_ray = |cursor: Vec2| {
        camera
            .viewport_to_world(camera_transform, cursor)
            .ok()
            .map(|ray| (ray.origin.as_dvec3(), ray.direction.as_dvec3()))
    };

    // ドラッグ中: カーソルのレイを入口の面か軸に落として寸法にする
    if let Some(handle) = tool.dragging {
        if !buttons.pressed(MouseButton::Left) {
            tool.dragging = None;
            return;
        }
        if let (Some(cursor), Some(draft)) = (cursor, tool.draft.as_mut())
            && let Some((origin, dir)) = cursor_ray(cursor)
        {
            match handle {
                Handle::Diameter => {
                    if let Some(p) = ray_plane(origin, dir, draft.center, draft.dir()) {
                        draft.set_diameter_from_point(p, step);
                    }
                }
                Handle::Depth => {
                    if let Some(p) = closest_on_line(origin, dir, draft.center, draft.dir()) {
                        draft.set_depth_from_point(p, step);
                    }
                }
            }
        }
        return;
    }

    // つまみのホバー
    let on_view = cursor.is_some() && !viewport.pointer_on_ui;
    tool.hovered = match (&tool.draft, cursor) {
        (Some(draft), Some(cursor)) if on_view => [Handle::Diameter, Handle::Depth]
            .into_iter()
            .filter_map(|handle| {
                let screen = camera
                    .world_to_viewport(camera_transform, draft.handle_position(handle).as_vec3())
                    .ok()?;
                let distance = screen.distance(cursor);
                (distance <= HANDLE_PIXELS).then_some((distance, handle))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, handle)| handle),
        _ => None,
    };

    // 面のホバー: 対象の部品の面だけ光らせる (選択中の面の強調をそのまま使う)
    let hit = (on_view && tool.hovered.is_none())
        .then(|| under_pointer.hit())
        .flatten()
        .filter(|hit| hit.node == target);
    let face = hit.map(|hit| hit.face);
    if selection.face != face {
        selection.face = face;
    }

    // Alt + 左ドラッグはカメラのオービット
    if !buttons.just_pressed(MouseButton::Left) || alt_pressed(&keys) || !on_view {
        return;
    }
    if let Some(handle) = tool.hovered {
        tool.dragging = Some(handle);
        return;
    }
    if let Some(hit) = hit
        && let Some(bounds) = model.world_bounds(hit.node)
    {
        let diameter = tool.default_diameter(&bounds);
        tool.draft = Some(HoleDraft::from_face(
            hit.node,
            hit.point.as_dvec3(),
            hit.normal.as_dvec3(),
            &bounds,
            diameter,
        ));
        tool.error = None;
    }
}

/// レイと平面 (点 `p`、法線 `n`) の交点。平行なら None
fn ray_plane(origin: DVec3, dir: DVec3, p: DVec3, n: DVec3) -> Option<DVec3> {
    let denom = dir.dot(n);
    if denom.abs() < 1e-12 {
        return None;
    }
    let t = (p - origin).dot(n) / denom;
    Some(origin + dir * t)
}

/// レイにいちばん近い、直線 (点 `p`、向き `d`) の上の点。平行なら None
fn closest_on_line(origin: DVec3, dir: DVec3, p: DVec3, d: DVec3) -> Option<DVec3> {
    // 2直線の最近点 (s は直線側のパラメータ)
    let w = p - origin;
    let a = dir.dot(dir);
    let b = dir.dot(d);
    let c = d.dot(d);
    let denom = a * c - b * b;
    if denom.abs() < 1e-12 {
        return None;
    }
    let e = w.dot(dir);
    let f = w.dot(d);
    let s = (b * e - a * f) / denom;
    Some(p + d * s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_hits_plane() {
        let p = ray_plane(
            DVec3::new(1.0, 10.0, 2.0),
            DVec3::new(0.0, -1.0, 0.0),
            DVec3::new(0.0, 3.0, 0.0),
            DVec3::Y,
        )
        .unwrap();
        assert_eq!(p, DVec3::new(1.0, 3.0, 2.0));
        assert_eq!(
            ray_plane(DVec3::ZERO, DVec3::X, DVec3::new(0.0, 3.0, 0.0), DVec3::Y),
            None
        );
    }

    #[test]
    fn closest_point_on_axis() {
        // 横から見たレイを、Y 軸に落とす
        let p = closest_on_line(
            DVec3::new(10.0, 4.0, 0.0),
            DVec3::new(-1.0, 0.0, 0.0),
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::Y,
        )
        .unwrap();
        assert!((p - DVec3::new(0.0, 4.0, 0.0)).length() < 1e-9);
        assert_eq!(
            closest_on_line(DVec3::ZERO, DVec3::Y, DVec3::X, DVec3::Y),
            None
        );
    }
}
