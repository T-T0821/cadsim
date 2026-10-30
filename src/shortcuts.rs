//! キーボードショートカット (カメラのフライ移動は camera.rs)

use bevy::prelude::*;

use crate::camera::{CameraRequest, StandardView, alt_pressed, keyboard_free};
use crate::file_io::ImportJobs;
use crate::gizmo::{GizmoTool, Tool};
use crate::model::{Model, Selection};

pub struct ShortcutsPlugin;

impl Plugin for ShortcutsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, keyboard_shortcuts.run_if(keyboard_free));
    }
}

fn keyboard_shortcuts(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut model: ResMut<Model>,
    mut selection: ResMut<Selection>,
    mut gizmo_tool: ResMut<GizmoTool>,
    mut jobs: ResMut<ImportJobs>,
    mut camera: MessageWriter<CameraRequest>,
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
        model.remove(id);
        selection.clear();
    }
}
