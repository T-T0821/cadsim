//! 穴あけ・押し出し・断面の道具のマウスとキーの読み方
//!
//! 穴あけ:
//! - 部品の面にカーソルを乗せると面が光り、クリックするとそこが穴の入口になる
//! - 置いた穴の「直径の点」「底の中心」をドラッグすると直径・深さが変わる (Ctrl で刻みにそろえる)
//! - Enter で穴をあけ、Esc で取り消す
//!
//! 押し出し:
//! - 床をクリックして頂点を置き、最初の点をクリック (か Enter) で形を閉じる
//! - 上の点をドラッグで高さ、頂点をドラッグで形を直す。Enter で押し出す
//!
//! 寸法の計算は `plantwright_core::modeling::{hole_tool, extrude_tool}`。
//! ここはカーソルの位置をレイにして渡すだけ。
//!
//! 断面の道具 (`section_*`) も同じ作り: 矢印の先をドラッグで平面を動かし、面のクリックで向きをそろえる。
//! 平面の計算と切り口の線は `plantwright_core::modeling::section_tool`。

use bevy::{
    ecs::change_detection::DetectChangesMut, math::DVec3, prelude::*, window::PrimaryWindow,
};

use super::{confirm_extrude, confirm_hole};
use crate::document::history::History;
use crate::document::import::ImportJobs;
use crate::document::selection::Selection;
use crate::viewport::camera::{ViewportRect, alt_pressed};
use crate::viewport::gizmo::GizmoTool;
use crate::viewport::picking::PartUnderPointer;
use plantwright_core::model::Model;
use plantwright_core::modeling::extrude_tool::{self, ExtrudeTool};
use plantwright_core::modeling::hole_tool::{Handle, HoleDraft, HoleTool};
use plantwright_core::modeling::record::JsonSolids;
use plantwright_core::modeling::repair::RepairNotice;
use plantwright_core::modeling::section_tool::SectionTool;
use plantwright_core::pointer::PointerLock;

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

/// Enter で形を閉じる (閉じていれば押し出す)、Esc で1つ戻る
pub fn extrude_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut tool: ResMut<ExtrudeTool>,
    mut model: ResMut<Model>,
    mut history: ResMut<History>,
    mut selection: ResMut<Selection>,
    mut jobs: ResMut<ImportJobs>,
) {
    if !tool.active {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        tool.cancel();
    }
    if keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter]) {
        if tool.closed {
            confirm_extrude(
                &mut tool,
                &mut model,
                &mut history,
                &mut selection,
                &mut jobs,
            );
        } else {
            tool.close();
        }
    }
}

/// 床のクリックで頂点を置く、つまみのドラッグ
pub fn extrude_input(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    viewport: Res<ViewportRect>,
    gizmo: Res<GizmoTool>,
    mut tool: ResMut<ExtrudeTool>,
) {
    if !tool.active {
        return;
    }
    let (camera, camera_transform) = *camera;
    let cursor = window.cursor_position();
    let on_view = cursor.is_some() && !viewport.pointer_on_ui;
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let step = (gizmo.snap.always || ctrl).then_some(gizmo.snap.translate_step);
    let cursor_ray = |cursor: Vec2| {
        camera
            .viewport_to_world(camera_transform, cursor)
            .ok()
            .map(|ray| (ray.origin.as_dvec3(), ray.direction.as_dvec3()))
    };
    let floor = DVec3::new(0.0, tool.floor_y, 0.0);
    let to_screen = |p: DVec3| camera.world_to_viewport(camera_transform, p.as_vec3()).ok();

    // ドラッグ中
    if let Some(handle) = tool.dragging {
        if !buttons.pressed(MouseButton::Left) {
            tool.dragging = None;
            return;
        }
        if let Some((origin, dir)) = cursor.and_then(cursor_ray) {
            match handle {
                extrude_tool::Handle::Height => {
                    let center = tool.center();
                    if let Some(p) = closest_on_line(origin, dir, center, DVec3::Y) {
                        tool.set_height_from_point(p, step);
                    }
                }
                extrude_tool::Handle::Vertex(i) => {
                    if let Some(p) = ray_plane(origin, dir, floor, DVec3::Y) {
                        tool.move_point(i, p, step);
                    }
                }
            }
        }
        return;
    }

    // カーソルの下の床の点 (閉じるまでは次の頂点の候補)
    let floor_point = cursor
        .filter(|_| on_view)
        .and_then(cursor_ray)
        .and_then(|(o, d)| ray_plane(o, d, floor, DVec3::Y))
        // 床を裏から見ているときは置けない
        .filter(|p| p.y.is_finite());
    tool.hover_point = (!tool.closed).then_some(floor_point).flatten();

    // つまみのホバー: 閉じたら高さの点と頂点、閉じる前は最初の点 (クリックで閉じる)
    let handles: Vec<extrude_tool::Handle> = if tool.closed {
        std::iter::once(extrude_tool::Handle::Height)
            .chain((0..tool.points.len()).map(extrude_tool::Handle::Vertex))
            .collect()
    } else if tool.points.len() >= 3 {
        vec![extrude_tool::Handle::Vertex(0)]
    } else {
        Vec::new()
    };
    tool.hovered = match cursor {
        Some(cursor) if on_view => handles
            .into_iter()
            .filter_map(|handle| {
                let screen = to_screen(tool.handle_position(handle))?;
                let distance = screen.distance(cursor);
                (distance <= HANDLE_PIXELS).then_some((distance, handle))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, handle)| handle),
        _ => None,
    };

    // Alt + 左ドラッグはカメラのオービット
    if !buttons.just_pressed(MouseButton::Left) || alt_pressed(&keys) || !on_view {
        return;
    }
    match tool.hovered {
        // 閉じる前に最初の点をクリック → 閉じる
        Some(extrude_tool::Handle::Vertex(0)) if !tool.closed => {
            tool.close();
        }
        Some(handle) => tool.dragging = Some(handle),
        None => {
            if let Some(p) = floor_point {
                tool.add_point(p, step);
                tool.error = None;
            }
        }
    }
}

/// 断面の道具: Esc か Enter で閉じる
pub fn section_keys(keys: Res<ButtonInput<KeyCode>>, mut tool: ResMut<SectionTool>) {
    if tool.active && keys.any_just_pressed([KeyCode::Escape, KeyCode::Enter, KeyCode::NumpadEnter])
    {
        tool.stop();
    }
}

/// 断面の道具: 矢印の先のホバーとドラッグ、面のクリックで向きをそろえる
#[allow(clippy::too_many_arguments)]
pub fn section_input(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    viewport: Res<ViewportRect>,
    under_pointer: PartUnderPointer,
    model: Res<Model>,
    gizmo: Res<GizmoTool>,
    mut tool: ResMut<SectionTool>,
    mut lock: ResMut<PointerLock>,
) {
    // 使っている間はクリックを道具のものにする (クリック選択・ギズモ・右クリックメニューを止める)
    if !tool.active {
        if lock.held_by() == Some("section") {
            lock.release("section");
        }
        return;
    }
    if lock.held_by() != Some("section") {
        lock.hold("section");
    }
    // 切っていた部品が消えたら道具もやめる
    if tool.target.is_some_and(|id| model.get(id).is_none()) {
        tool.stop();
        return;
    }
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

    if tool.dragging {
        if !buttons.pressed(MouseButton::Left) {
            tool.dragging = false;
            return;
        }
        if let Some(cursor) = cursor
            && let Some((origin, dir)) = cursor_ray(cursor)
            && let Some(p) = closest_on_line(origin, dir, tool.point(), tool.normal)
        {
            tool.set_offset_from_point(p, step);
        }
        return;
    }

    let on_view = cursor.is_some() && !viewport.pointer_on_ui;
    let hovered = match cursor {
        Some(cursor) if on_view => camera
            .world_to_viewport(camera_transform, tool.handle_position().as_vec3())
            .is_ok_and(|screen| screen.distance(cursor) <= HANDLE_PIXELS),
        _ => false,
    };
    if tool.hovered != hovered {
        tool.hovered = hovered;
    }

    if !buttons.just_pressed(MouseButton::Left) || alt_pressed(&keys) || !on_view {
        return;
    }
    if hovered {
        tool.dragging = true;
        return;
    }
    // 切っている部品の面をクリックすると、その面に平行な断面にする
    if let Some(hit) = under_pointer.hit().filter(|hit| {
        tool.target
            .is_none_or(|t| model.ancestors(hit.node).contains(&t))
    }) {
        tool.align_to_face(hit.point.as_dvec3(), hit.normal.as_dvec3());
    }
}

/// 断面の平面か部品が変わったら、切り口の線を作り直す
pub fn section_refresh(model: Res<Model>, mut tool: ResMut<SectionTool>) {
    if tool.active {
        tool.bypass_change_detection().refresh(&model, &JsonSolids);
    }
}

/// 読み込んだばかりの木で形状を直した部品があれば、ステータスバーに案内を出す
pub fn repair_notice(
    mut jobs: ResMut<ImportJobs>,
    model: Res<Model>,
    mut notice: ResMut<RepairNotice>,
) {
    if jobs.imported.is_empty() {
        return;
    }
    for root in std::mem::take(&mut jobs.imported) {
        notice.show(&model, root);
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
