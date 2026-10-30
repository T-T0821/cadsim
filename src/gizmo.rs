//! 移動・回転・拡大縮小のギズモ (Unity / Unreal と同じく W / E / R で切り替え)
//!
//! ギズモの計算は transform-gizmo (f64) に任せ、描画は egui の背景レイヤーに描く。
//! ドラッグ結果は Bevy の Transform ではなく BoxPart の f64 パラメータに書き戻し、
//! truck の形状を作り直す。

use std::f64::consts::{FRAC_PI_2, PI};

use bevy::{prelude::*, window::PrimaryWindow};
use bevy_egui::{EguiContexts, egui, input::EguiWantsInput};
// transform-gizmo は Bevy と別バージョンの glam を使うので、その型で計算する
use transform_gizmo::{
    Gizmo, GizmoConfig, GizmoInteraction, GizmoMode, GizmoOrientation,
    config::GizmoModeKind,
    math::{DMat3, DMat4, DQuat, DVec3, Transform as GizmoTransform},
    mint,
};

use crate::{Model, Selection, alt_pressed};

/// 箱の寸法の下限 (スケールで潰れないようにする)
const MIN_SIZE: f64 = 0.01;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Translate,
    Rotate,
    Scale,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Translate => "移動 (W)",
            Tool::Rotate => "回転 (E)",
            Tool::Scale => "拡大縮小 (R)",
        }
    }

    fn kind(self) -> GizmoModeKind {
        match self {
            Tool::Translate => GizmoModeKind::Translate,
            Tool::Rotate => GizmoModeKind::Rotate,
            Tool::Scale => GizmoModeKind::Scale,
        }
    }
}

#[derive(Resource)]
pub struct GizmoTool {
    pub tool: Tool,
    gizmo: Gizmo,
    /// 直前のフレームでカーソルがギズモに乗っていたか、ドラッグ中か。
    /// true の間は面の選択とカメラ操作を止める。
    pub focused: bool,
}

impl Default for GizmoTool {
    fn default() -> Self {
        Self {
            tool: Tool::Translate,
            gizmo: Gizmo::default(),
            focused: false,
        }
    }
}

pub fn gizmo_not_focused(tool: Res<GizmoTool>) -> bool {
    !tool.focused
}

fn to_row_matrix(m: DMat4) -> mint::RowMatrix4<f64> {
    let row = |i| {
        let r = m.row(i);
        mint::Vector4 {
            x: r.x,
            y: r.y,
            z: r.z,
            w: r.w,
        }
    };
    mint::RowMatrix4 {
        x: row(0),
        y: row(1),
        z: row(2),
        w: row(3),
    }
}

/// BoxPart の回転 (X → Y → Z の順に回す角度) をクォータニオンにする
fn part_rotation(deg: [f64; 3]) -> DQuat {
    let [x, y, z] = deg.map(f64::to_radians);
    DQuat::from_rotation_z(z) * DQuat::from_rotation_y(y) * DQuat::from_rotation_x(x)
}

/// part_rotation の逆。R = Rz * Ry * Rx から X → Y → Z の角度 (度) を取り出す。
fn rotation_to_deg(q: DQuat) -> [f64; 3] {
    let m = DMat3::from_quat(q);
    let y = (-m.x_axis.z).clamp(-1.0, 1.0).asin();
    let x = m.y_axis.z.atan2(m.z_axis.z);
    let z = m.x_axis.y.atan2(m.x_axis.x);
    // (x, y, z) と (x ± 180°, 180° - y, z ± 180°) は同じ回転。Y だけ回したときに
    // (180, 85, 180) のような表示にならないよう、X と Z が小さい方を選ぶ。
    let (x, y, z) = if x.abs() > FRAC_PI_2 && z.abs() > FRAC_PI_2 {
        (x - PI.copysign(x), PI.copysign(y) - y, z - PI.copysign(z))
    } else {
        (x, y, z)
    };
    // 表示上の -0 や 1e-15 のような誤差を消す
    [x, y, z].map(|r| (r.to_degrees() * 1e9).round() / 1e9 + 0.0)
}

/// 右手系・深度 0..1 の標準的な透視投影行列
fn perspective(fov_y: f64, aspect: f64, near: f64, far: f64) -> DMat4 {
    let f = 1.0 / (fov_y / 2.0).tan();
    let r = far / (near - far);
    DMat4::from_cols_array(&[
        f / aspect,
        0.0,
        0.0,
        0.0, //
        0.0,
        f,
        0.0,
        0.0, //
        0.0,
        0.0,
        r,
        -1.0, //
        0.0,
        0.0,
        r * near,
        0.0,
    ])
}

#[allow(clippy::too_many_arguments)]
pub fn transform_gizmo_system(
    mut contexts: EguiContexts,
    mut tool: ResMut<GizmoTool>,
    mut model: ResMut<Model>,
    selection: Res<Selection>,
    egui_wants: Res<EguiWantsInput>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&GlobalTransform, &Projection), With<Camera3d>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let Some(id) = selection.part else {
        tool.focused = false;
        return Ok(());
    };
    let Some(part) = model.parts.iter().find(|p| p.id == id).cloned() else {
        tool.focused = false;
        return Ok(());
    };

    // 標準的な透視投影行列を作り直して渡す (Bevy は reverse-Z の無限遠投影なので)
    let (camera_transform, projection) = *camera;
    let fov = match projection {
        Projection::Perspective(p) => p.fov,
        _ => std::f32::consts::FRAC_PI_4,
    };
    let size = window.size();
    let aspect = (size.x / size.y.max(1.0)) as f64;
    let view = Mat4::from(camera_transform.affine()).inverse().as_dmat4();
    let view = DMat4::from_cols_array(&view.to_cols_array());
    let proj = perspective(fov as f64, aspect, 0.05, 1000.0);

    let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.x, size.y));
    let config = *tool.gizmo.config();
    let tool_kind = tool.tool.kind();
    tool.gizmo.update_config(GizmoConfig {
        view_matrix: to_row_matrix(view),
        projection_matrix: to_row_matrix(proj),
        viewport,
        modes: GizmoMode::all_from_kind(tool_kind),
        orientation: GizmoOrientation::Local,
        pixels_per_point: ctx.pixels_per_point(),
        ..config
    });

    let target = GizmoTransform {
        scale: DVec3::from(part.size).into(),
        rotation: part_rotation(part.rotation_deg).into(),
        translation: DVec3::from(part.position).into(),
    };

    let cursor = window.cursor_position();
    let over_ui = egui_wants.wants_any_pointer_input();
    let alt = alt_pressed(&keys);
    let interaction = GizmoInteraction {
        cursor_pos: cursor.map(|c| (c.x, c.y)).unwrap_or_default(),
        hovered: cursor.is_some() && !over_ui,
        drag_started: !alt && buttons.just_pressed(MouseButton::Left),
        dragging: !alt && buttons.pressed(MouseButton::Left),
    };

    if let Some((_, transforms)) = tool.gizmo.update(interaction, &[target])
        && let Some(new) = transforms.first()
        && let Some(edit) = model.part_mut(id)
    {
        let translation: DVec3 = new.translation.into();
        let rotation: DQuat = new.rotation.into();
        let scale: DVec3 = new.scale.into();
        edit.position = translation.to_array();
        edit.rotation_deg = rotation_to_deg(rotation);
        edit.size = scale.to_array().map(|s| s.max(MIN_SIZE));
        model.dirty.insert(id);
    }
    tool.focused = tool.gizmo.is_focused();

    let draw = tool.gizmo.draw();
    let mesh = egui::Mesh {
        indices: draw.indices,
        vertices: draw
            .vertices
            .into_iter()
            .zip(draw.colors)
            .map(|(pos, [r, g, b, a])| egui::epaint::Vertex {
                pos: pos.into(),
                uv: egui::Pos2::ZERO,
                color: egui::Rgba::from_rgba_premultiplied(r, g, b, a).into(),
            })
            .collect(),
        ..default()
    };
    ctx.layer_painter(egui::LayerId::background()).add(mesh);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_round_trip() {
        for deg in [[0.0, 0.0, 0.0], [15.0, 30.0, 45.0], [-70.0, 10.0, 120.0]] {
            let back = rotation_to_deg(part_rotation(deg));
            for (a, b) in deg.iter().zip(back) {
                assert!((a - b).abs() < 1e-9, "{deg:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn y_only_rotation_past_90_stays_on_y() {
        let back = rotation_to_deg(DQuat::from_rotation_y(95f64.to_radians()));
        assert_eq!(back, [0.0, 95.0, 0.0]);
    }
}
