//! 役割の道具のマウスとキーの読み方
//!
//! - 矢印の先や点にカーソルを乗せて左ドラッグ: 値を変える (Ctrl でギズモのスナップの刻みにそろえる)
//! - 部品の面をクリック: シリンダ・直動軸・回転テーブルの向き、センサーの取り付け位置と向き
//! - 投入口の点をコンベアの部品までドラッグして離す: そのコンベアへ出す
//! - Enter で確定、Esc で取り消し
//!
//! 値の計算は `plantwright_core::part_role::tool`。ここはカーソルの位置をレイにして、
//! 道具が決めた線や面に落として渡すだけ。

use bevy::{prelude::*, window::PrimaryWindow};

use crate::document::selection::Selection;
use crate::features::conveyor::ConveyorConfigRes;
use crate::viewport::camera::{ViewportRect, alt_pressed};
use crate::viewport::gizmo::GizmoTool;
use crate::viewport::picking::PartUnderPointer;
use plantwright_core::math::{closest_on_line, ray_plane};
use plantwright_core::model::Model;
use plantwright_core::modeling::extrude_tool::ExtrudeTool;
use plantwright_core::modeling::hole_tool::HoleTool;
use plantwright_core::motion::chart::TimingChart;
use plantwright_core::part_role::assign;
use plantwright_core::part_role::{Constraint, Handle, RoleBody, RoleTool};
use plantwright_core::pointer::PointerLock;

/// `PointerLock` に名乗る名前
const LOCK_NAME: &str = "part_role";

/// つまみをつかめる範囲 (画面上のピクセル)
const HANDLE_PIXELS: f32 = 14.0;

/// Enter で設定に書き込み、Esc で下書きを捨てる
pub fn role_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut tool: ResMut<RoleTool>,
    mut chart: ResMut<TimingChart>,
    mut conveyors: ResMut<ConveyorConfigRes>,
) {
    if !tool.active() {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        tool.stop();
        return;
    }
    if keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter])
        && let Some(draft) = tool.draft.take()
    {
        assign::apply(&draft, &mut chart, &mut conveyors);
        tool.stop();
    }
}

/// 道具を使っている間は、クリックを道具のものにする (選択・ギズモ・右クリックメニューを止める)
pub fn hold_pointer(tool: Res<RoleTool>, mut lock: ResMut<PointerLock>) {
    if tool.active() {
        if lock.held_by() != Some(LOCK_NAME) {
            lock.hold(LOCK_NAME);
        }
    } else if lock.held_by() == Some(LOCK_NAME) {
        lock.release(LOCK_NAME);
    }
}

/// つまみのホバーとドラッグ、面のクリック
#[allow(clippy::too_many_arguments)]
pub fn role_input(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    viewport: Res<ViewportRect>,
    under_pointer: PartUnderPointer,
    model: Res<Model>,
    gizmo: Res<GizmoTool>,
    hole: Res<HoleTool>,
    extrude: Res<ExtrudeTool>,
    conveyors: Res<ConveyorConfigRes>,
    mut tool: ResMut<RoleTool>,
    mut selection: ResMut<Selection>,
) {
    let Some(part) = tool.draft.as_ref().map(|d| d.part) else {
        return;
    };
    // 穴あけ・押し出しを始めた・部品が消えた・ほかの部品を選んだら、下書きを捨ててやめる
    if hole.active || extrude.active || model.get(part).is_none() {
        tool.stop();
        return;
    }
    // 道具の間はクリックでの選択が止まっている (PointerLock)。ツリーで選び直したらやめる
    if selection.node != Some(part) {
        tool.stop();
        return;
    }

    let (camera, camera_transform) = *camera;
    let cursor = window.cursor_position();
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let step = (gizmo.snap.always || ctrl).then_some(gizmo.snap.size_step);
    let ray = cursor.and_then(|c| {
        camera
            .viewport_to_world(camera_transform, c)
            .ok()
            .map(|ray| (ray.origin.as_dvec3(), ray.direction.as_dvec3()))
    });
    let view_dir = camera_transform.forward().as_dvec3();

    // ドラッグ中: カーソルを道具が決めた線か面に落とす
    if let Some(handle) = tool.dragging {
        if !buttons.pressed(MouseButton::Left) {
            tool.dragging = None;
            if handle == Handle::Link {
                // 離した位置の部品がコンベアならつなぐ
                let target = under_pointer.hit().and_then(|hit| {
                    conveyors
                        .conveyors
                        .iter()
                        .find(|c| c.part == Some(hit.node))
                        .map(|c| c.id)
                });
                tool.link_point = None;
                if let Some(draft) = tool.draft.as_mut() {
                    draft.link_to(target);
                }
            }
            return;
        }
        let Some((origin, dir)) = ray else {
            return;
        };
        let Some(draft) = tool.draft.as_ref() else {
            return;
        };
        let point = match draft.constraint(handle) {
            Constraint::Line { origin: p, dir: d } => closest_on_line(origin, dir, p, d),
            Constraint::Plane { origin: p, normal } => ray_plane(origin, dir, p, normal),
            Constraint::ViewPlane { origin: p } => ray_plane(origin, dir, p, view_dir),
        };
        if let Some(point) = point {
            if handle == Handle::Link {
                tool.link_point = Some(point);
            } else if let Some(draft) = tool.draft.as_mut() {
                draft.drag(handle, point, step);
            }
        }
        return;
    }

    // つまみのホバー
    let on_view = cursor.is_some() && !viewport.pointer_on_ui;
    tool.hovered = match (&tool.draft, cursor) {
        (Some(draft), Some(cursor)) if on_view => draft
            .handles()
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

    // 面のホバー: 向きを面で決める役割のときだけ、対象の部品の面を光らせる
    let uses_faces = tool
        .draft
        .as_ref()
        .is_some_and(|d| matches!(d.body, RoleBody::Actuator(_) | RoleBody::Sensor(_)));
    let hit = (on_view && uses_faces && tool.hovered.is_none())
        .then(|| under_pointer.hit())
        .flatten()
        .filter(|hit| hit.node == part);
    let face = hit.map(|hit| hit.face);
    if uses_faces && selection.face != face {
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
    if let (Some(hit), Some(draft)) = (hit, tool.draft.as_mut()) {
        draft.click_face(hit.point.as_dvec3(), hit.normal.as_dvec3());
    }
}
