//! 移動・回転・拡大縮小のギズモ (Unity / Unreal と同じく W / E / R で切り替え)
//!
//! ギズモの計算は transform-gizmo (f64) に任せ、描画は egui の背景レイヤーに描く。
//! ドラッグ結果は Bevy の Transform ではなくモデルの f64 の値に書き戻す。
//! - 箱: 寸法・位置・回転のパラメータを書き換え、truck の形状を作り直す。
//! - 取り込んだ部品やアセンブリ: 親から見た配置を書き換える (拡大縮小はしない)。
//!
//! Ctrl を押しながらドラッグすると一定量ずつ動く (スナップ)。刻み幅はパネルで変えられる。
//! - 移動: ドラッグ開始位置から「移動の刻み」ずつ動く
//! - 回転: ドラッグ開始の向きから「回転の刻み (度)」ずつ回る
//! - 拡大縮小: 寸法そのものが「寸法の刻み」の倍数になる

use bevy::{prelude::*, window::PrimaryWindow};
use bevy_egui::{EguiContexts, egui};
// transform-gizmo は Bevy と別バージョンの glam を使うので、その型で計算する
use transform_gizmo::{
    Gizmo, GizmoConfig, GizmoInteraction, GizmoMode, GizmoOrientation,
    config::GizmoModeKind,
    math::{DMat4, DQuat, DVec3, Transform as GizmoTransform},
    mint,
};

use crate::camera::{ViewportRect, alt_pressed};
use crate::math::{euler_deg_to_quat, quat_to_euler_deg};
use crate::model::{Model, NodeKind, Selection};

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
    let state = if tool.snapping {
        "有効"
    } else {
        "Ctrl で有効"
    };
    ui.label(format!("スナップ ({state})"));
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
        step(ui, "移動の刻み", &mut snap.translate_step, 0.01);
        step(ui, "回転の刻み (度)", &mut snap.rotate_step_deg, 1.0);
        step(ui, "寸法の刻み", &mut snap.size_step, 0.01);
    });
    ui.checkbox(&mut snap.always, "常にスナップ");
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

/// Bevy の glam (f64) から transform-gizmo の glam (f64) に移す (別バージョンなので配列経由)
fn to_gizmo_mat(m: bevy::math::DMat4) -> DMat4 {
    DMat4::from_cols_array(&m.to_cols_array())
}

fn from_gizmo_mat(m: DMat4) -> bevy::math::DMat4 {
    bevy::math::DMat4::from_cols_array(&m.to_cols_array())
}

/// ギズモで動かす対象のワールド座標での姿勢
fn gizmo_target(model: &Model, id: u32) -> Option<GizmoTransform> {
    let node = model.get(id)?;
    Some(match &node.kind {
        // 箱は最上位にだけ置くので、パラメータがそのままワールド座標
        NodeKind::Box(part) => GizmoTransform {
            scale: DVec3::from(part.size).into(),
            rotation: DQuat::from_array(euler_deg_to_quat(part.rotation_deg).to_array()).into(),
            translation: DVec3::from(part.position).into(),
        },
        _ => {
            let (_, rotation, translation) =
                to_gizmo_mat(model.world_matrix(id)).to_scale_rotation_translation();
            GizmoTransform {
                scale: DVec3::ONE.into(),
                rotation: rotation.into(),
                translation: translation.into(),
            }
        }
    })
}

/// ドラッグ結果をモデルに書き戻す。`size_step` があれば箱の寸法をその倍数に丸める。
fn apply_gizmo_result(model: &mut Model, id: u32, new: &GizmoTransform, size_step: Option<f64>) {
    let translation: DVec3 = new.translation.into();
    let rotation: DQuat = new.rotation.into();
    let scale: DVec3 = new.scale.into();
    let parent_world = model
        .get(id)
        .and_then(|n| n.parent)
        .map(|p| model.world_matrix(p))
        .unwrap_or(bevy::math::DMat4::IDENTITY);
    let Some(node) = model.get_mut(id) else {
        return;
    };
    match &mut node.kind {
        NodeKind::Box(part) => {
            part.position = translation.to_array();
            part.rotation_deg =
                quat_to_euler_deg(bevy::math::DQuat::from_array(rotation.to_array()));
            let mut size = scale.to_array();
            if let Some(step) = size_step {
                size = snap_size(part.size, size, step);
            }
            part.size = size.map(|s| s.max(MIN_SIZE));
            model.dirty.insert(id);
        }
        _ => {
            // 新しいワールド姿勢を、親の座標系から見た配置に直す
            let world = from_gizmo_mat(DMat4::from_rotation_translation(rotation, translation));
            node.placement = parent_world.inverse() * world;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn transform_gizmo_system(
    mut contexts: EguiContexts,
    mut tool: ResMut<GizmoTool>,
    mut model: ResMut<Model>,
    selection: Res<Selection>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    viewport: Res<ViewportRect>,
    camera: Single<(&GlobalTransform, &Projection), With<Camera3d>>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let target = selection
        .node
        .and_then(|id| Some((id, gizmo_target(&model, id)?)));
    let is_box = selection
        .node
        .and_then(|id| model.get(id))
        .is_some_and(|n| matches!(n.kind, NodeKind::Box(_)));
    // 拡大縮小は寸法を持つ箱だけ。取り込んだ部品は形状を変えない。
    let Some((id, target)) = target.filter(|_| is_box || tool.tool != Tool::Scale) else {
        tool.focused = false;
        return Ok(());
    };

    // 標準的な透視投影行列を作り直して渡す (Bevy は reverse-Z の無限遠投影なので)
    let (camera_transform, projection) = *camera;
    let (fov, near) = match projection {
        Projection::Perspective(p) => (p.fov, p.near),
        _ => (std::f32::consts::FRAC_PI_4, 0.05),
    };
    let rect = viewport
        .rect
        .unwrap_or_else(|| Rect::from_corners(Vec2::ZERO, window.size()));
    let aspect = (rect.width() / rect.height().max(1.0)) as f64;
    let view = to_gizmo_mat(Mat4::from(camera_transform.affine()).inverse().as_dmat4());
    let proj = perspective(fov as f64, aspect, near as f64, 1e7);

    let egui_viewport = egui::Rect::from_min_max(
        egui::pos2(rect.min.x, rect.min.y),
        egui::pos2(rect.max.x, rect.max.y),
    );
    tool.snapping = tool.snap.always || ctrl_pressed(&keys);
    let tool_kind = tool.tool.kind();
    // 移動と回転は transform-gizmo のスナップ (ドラッグ開始からの変化量を丸める) を使う。
    // 拡大縮小は倍率ではなく寸法を丸めたいので、apply_gizmo_result で自前で丸める。
    let gizmo_snapping = tool.snapping && tool.tool != Tool::Scale;
    let snap_distance = tool.snap.translate_step as f32;
    let snap_angle = tool.snap.rotate_step_deg.to_radians() as f32;
    let config = *tool.gizmo.config();
    tool.gizmo.update_config(GizmoConfig {
        view_matrix: to_row_matrix(view),
        projection_matrix: to_row_matrix(proj),
        viewport: egui_viewport,
        modes: GizmoMode::all_from_kind(tool_kind),
        orientation: GizmoOrientation::Local,
        pixels_per_point: ctx.pixels_per_point(),
        snapping: gizmo_snapping,
        snap_distance,
        snap_angle,
        ..config
    });

    let cursor = window.cursor_position();
    let over_ui = viewport.pointer_on_ui;
    let alt = alt_pressed(&keys);
    let interaction = GizmoInteraction {
        cursor_pos: cursor.map(|c| (c.x, c.y)).unwrap_or_default(),
        hovered: cursor.is_some() && !over_ui,
        drag_started: !alt && buttons.just_pressed(MouseButton::Left),
        dragging: !alt && buttons.pressed(MouseButton::Left),
    };

    if let Some((_, transforms)) = tool.gizmo.update(interaction, &[target])
        && let Some(new) = transforms.first()
    {
        let size_step = (tool.snapping && tool.tool == Tool::Scale).then_some(tool.snap.size_step);
        apply_gizmo_result(&mut model, id, new, size_step);
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
    // パネルの上にはみ出さないよう 3D ビューの範囲で切る
    ctx.layer_painter(egui::LayerId::background())
        .with_clip_rect(egui_viewport)
        .add(mesh);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
