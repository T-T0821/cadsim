//! キーボードショートカット (カメラのフライ移動は camera.rs)

use bevy::prelude::*;

use crate::camera::{CameraRequest, StandardView, alt_pressed, keyboard_free};
use crate::file_io::ImportJobs;
use crate::gizmo::{GizmoTool, Tool};
use crate::history::{History, Request};
use crate::model::{Model, Selection};
use crate::project::{ProjectAction, ProjectState};

pub struct ShortcutsPlugin;

impl Plugin for ShortcutsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (keyboard_shortcuts, project_shortcuts).run_if(keyboard_free),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn keyboard_shortcuts(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut model: ResMut<Model>,
    mut selection: ResMut<Selection>,
    mut gizmo_tool: ResMut<GizmoTool>,
    mut jobs: ResMut<ImportJobs>,
    mut camera: MessageWriter<CameraRequest>,
    mut history: ResMut<History>,
) {
    // 右ボタンを押している間は WASD / QE がフライ移動になる
    if buttons.pressed(MouseButton::Right) {
        return;
    }
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);

    if ctrl {
        if keys.just_pressed(KeyCode::KeyO) {
            jobs.request_dialog();
        }
        let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        if keys.just_pressed(KeyCode::KeyZ) {
            history.request = Some(if shift { Request::Redo } else { Request::Undo });
        }
        if keys.just_pressed(KeyCode::KeyY) {
            history.request = Some(Request::Redo);
        }
    } else {
        // ギズモの切り替え (Unity / Unreal と同じ W / E / R)
        for (key, tool) in [
            (KeyCode::KeyW, Tool::Translate),
            (KeyCode::KeyE, Tool::Rotate),
            (KeyCode::KeyR, Tool::Scale),
        ] {
            if keys.just_pressed(key) {
                gizmo_tool.tool = tool;
            }
        }
    }

    // 標準ビュー (Ctrl を押すと反対側から)
    for (keys_for_view, view, opposite) in [
        (
            [KeyCode::Digit1, KeyCode::Numpad1],
            StandardView::Front,
            StandardView::Back,
        ),
        (
            [KeyCode::Digit3, KeyCode::Numpad3],
            StandardView::Right,
            StandardView::Left,
        ),
        (
            [KeyCode::Digit7, KeyCode::Numpad7],
            StandardView::Top,
            StandardView::Bottom,
        ),
        (
            [KeyCode::Digit0, KeyCode::Numpad0],
            StandardView::Isometric,
            StandardView::Isometric,
        ),
    ] {
        if keys.any_just_pressed(keys_for_view) {
            camera.write(CameraRequest::View(if ctrl { opposite } else { view }));
        }
    }
    // 透視投影 / 平行投影の切り替え (Blender のテンキー 5 と同じ)
    if keys.any_just_pressed([KeyCode::Digit5, KeyCode::Numpad5]) {
        camera.write(CameraRequest::ToggleProjection);
    }
    if keys.just_pressed(KeyCode::Home) {
        camera.write(CameraRequest::FitAll);
    }
    if keys.just_pressed(KeyCode::KeyF) {
        camera.write(CameraRequest::FitSelection);
    }

    if keys.just_pressed(KeyCode::KeyH) {
        if alt_pressed(&keys) {
            model.show_all();
        } else if let Some(node) = selection.node.and_then(|id| model.get_mut(id)) {
            node.visible = false;
        }
    }
    if keys.just_pressed(KeyCode::Escape) {
        selection.clear();
    }
    if keys.just_pressed(KeyCode::Delete)
        && let Some(id) = selection.node
    {
        history.remove(&mut model, id);
        selection.clear();
    }
}

/// プロジェクトの新規作成・保存 (Ctrl+O の「開く」は keyboard_shortcuts で STEP と共通)
fn project_shortcuts(keys: Res<ButtonInput<KeyCode>>, mut project: ResMut<ProjectState>) {
    if !keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]) {
        return;
    }
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if keys.just_pressed(KeyCode::KeyN) {
        project.request(ProjectAction::New);
    }
    if keys.just_pressed(KeyCode::KeyS) {
        project.request(if shift {
            ProjectAction::SaveAs
        } else {
            ProjectAction::Save
        });
    }
}
