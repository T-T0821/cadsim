//! 移動・回転・拡大縮小のギズモ (Unity / Unreal と同じく W / E / R で切り替え)
//!
//! ギズモの計算は transform-gizmo (f64) に任せ、描画は egui の背景レイヤーに描く。
//! ドラッグ結果は Bevy の Transform ではなく BoxPart の f64 パラメータに書き戻し、
//! truck の形状を作り直す。
//!
//! Ctrl を押しながらドラッグすると一定量ずつ動く (スナップ)。刻み幅はパネルで変えられる。
//! - 移動: ドラッグ開始位置から「移動の刻み」ずつ動く
//! - 回転: ドラッグ開始の向きから「回転の刻み (度)」ずつ回る
//! - 拡大縮小: 寸法そのものが「寸法の刻み」の倍数になる

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
            Tool::Translate => "Move (W)",
            Tool::Rotate => "Rotate (E)",
            Tool::Scale => "Scale (R)",
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

/// スナップの設定。パネルから編集する。
pub struct SnapSettings {
    /// true なら Ctrl を押さなくても常にスナップする
    pub always: bool,
    /// 移動の刻み (モデル単位)
    pub translate_step: f64,
    /// 回転の刻み (度)
    pub rotate_step_deg: f64,
    /// 寸法の刻み (モデル単位)
    pub size_step: f64,
}

impl Default for SnapSettings {
    fn default() -> Self {
        Self {
            always: false,
            translate_step: 0.1,
            rotate_step_deg: 15.0,
            size_step: 0.1,
        }
    }
}

#[derive(Resource)]
pub struct GizmoTool {
    pub tool: Tool,
    pub snap: SnapSettings,
    gizmo: Gizmo,
    /// 直前のフレームでカーソルがギズモに乗っていたか、ドラッグ中か。
    /// true の間は面の選択とカメラ操作を止める。
    pub focused: bool,
    /// このフレームでスナップが効いているか (パネルの表示用)
    pub snapping: bool,
}

impl Default for GizmoTool {
    fn default() -> Self {
        Self {
            tool: Tool::Translate,
            snap: SnapSettings::default(),
            gizmo: Gizmo::default(),
            focused: false,
            snapping: false,
        }
    }
}

pub fn gizmo_not_focused(tool: Res<GizmoTool>) -> bool {
    !tool.focused
}

fn ctrl_pressed(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight])
}

/// 拡大縮小のスナップ: 変化した軸の寸法だけを step の倍数に丸める。
/// 動かしていない軸まで丸めると、触っていない寸法が勝手に変わってしまうため。
fn snap_size(old: [f64; 3], new: [f64; 3], step: f64) -> [f64; 3] {
    std::array::from_fn(|i| {
        if new[i] == old[i] {
            new[i]
        } else {
            ((new[i] / step).round() * step).max(step)
        }
    })
}

/// パネルに出すスナップ設定の欄
pub fn snap_settings_ui(ui: &mut egui::Ui, tool: &mut GizmoTool) {
    let state = if tool.snapping { "on" } else { "hold Ctrl" };
    ui.label(format!("Snap ({state})"));
    let snap = &mut tool.snap;
    egui::Grid::new("snap_settings").show(ui, |ui| {
        let step = |ui: &mut egui::Ui, label: &str, value: &mut f64, speed: f64| {
            ui.label(label);
            ui.add(
                egui::DragValue::new(value)
                    .speed(speed)
                    .range(0.001..=1000.0)
                    .max_decimals(3),
            );
            ui.end_row();
        };
        step(ui, "Move step", &mut snap.translate_step, 0.01);
        step(ui, "Rotate step (deg)", &mut snap.rotate_step_deg, 1.0);
        step(ui, "Size step", &mut snap.size_step, 0.01);
    });
    ui.checkbox(&mut snap.always, "Always snap");
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
    tool.snapping = tool.snap.always || ctrl_pressed(&keys);
    let tool_kind = tool.tool.kind();
    // 移動と回転は transform-gizmo のスナップ (ドラッグ開始からの変化量を丸める) を使う。
    // 拡大縮小は倍率ではなく寸法を丸めたいので、下で自前で丸める。
    let gizmo_snapping = tool.snapping && tool.tool != Tool::Scale;
    let snap_distance = tool.snap.translate_step as f32;
    let snap_angle = tool.snap.rotate_step_deg.to_radians() as f32;
    let config = *tool.gizmo.config();
    tool.gizmo.update_config(GizmoConfig {
        view_matrix: to_row_matrix(view),
        projection_matrix: to_row_matrix(proj),
        viewport,
        modes: GizmoMode::all_from_kind(tool_kind),
        orientation: GizmoOrientation::Local,
        pixels_per_point: ctx.pixels_per_point(),
        snapping: gizmo_snapping,
        snap_distance,
        snap_angle,
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
        let mut size = scale.to_array();
        if tool.snapping && tool.tool == Tool::Scale {
            size = snap_size(part.size, size, tool.snap.size_step);
        }
        edit.position = translation.to_array();
        edit.rotation_deg = rotation_to_deg(rotation);
        edit.size = size.map(|s| s.max(MIN_SIZE));
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
    fn snap_size_rounds_only_changed_axes() {
        let old = [2.0, 1.03, 1.0];
        let new = [2.27, 1.03, 0.94];
        let snapped = snap_size(old, new, 0.1);
        assert!((snapped[0] - 2.3).abs() < 1e-9);
        assert_eq!(snapped[1], 1.03, "動かしていない軸はそのまま");
        assert!((snapped[2] - 0.9).abs() < 1e-9);
    }

    #[test]
    fn snap_size_never_reaches_zero() {
        assert_eq!(snap_size([1.0; 3], [0.02, 1.0, 1.0], 0.5)[0], 0.5);
    }

    #[test]
    fn y_only_rotation_past_90_stays_on_y() {
        let back = rotation_to_deg(DQuat::from_rotation_y(95f64.to_radians()));
        assert_eq!(back, [0.0, 95.0, 0.0]);
    }
}
