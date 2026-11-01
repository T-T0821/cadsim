//! 部品パレット (ドラッグで配置)、標準部品のプロパティ、I/O 一覧、光軸と搬送方向の表示

use crate::ui::dock::{self, Dock};
use bevy::{
    math::DVec3,
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
    window::PrimaryWindow,
};
use bevy_egui::{EguiContexts, egui};
use plantwright_core::workspace::Tool;

use super::{Jog, add_standard, dir_to_world, io_capacity, io_table, local_to_world};
use super::{rebuild_bodies, standard_nodes};
use crate::document::history::History;
use crate::document::import::ImportJobs;
use crate::document::selection::Selection;
use crate::features::custom_part::{CustomDrag, CustomLibrary};
use crate::features::floor::figures::FloorFigure;
use crate::ui::UiState;
use crate::viewport::camera::{OrbitCamera, ViewportRect};
use crate::viewport::factory_hall::HallMesh;
use plantwright_core::library::catalog::{Category, IoDir, Motion, PartKind, units_per_mm};
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::model::NodeKind;

/// ドロップ位置を丸める刻み (mm)
const DROP_SNAP_MM: f64 = 10.0;

#[derive(Resource)]
pub struct LibraryUiState {
    pub show_io: bool,
    /// センサーの光軸やコンベアの搬送方向を表示する
    pub show_overlays: bool,
}

impl Default for LibraryUiState {
    fn default() -> Self {
        Self {
            show_io: false,
            show_overlays: true,
        }
    }
}

/// 部品パレット (ツールレールの「部品」で開く)
#[allow(clippy::too_many_arguments)]
pub fn palette_window(
    mut contexts: EguiContexts,
    mut state: ResMut<LibraryUiState>,
    mut model: ResMut<Model>,
    mut selection: ResMut<Selection>,
    mut history: ResMut<History>,
    orbit: Res<OrbitCamera>,
    viewport: Res<ViewportRect>,
    mut custom: ResMut<CustomLibrary>,
    mut jobs: ResMut<ImportJobs>,
    mut dock: ResMut<Dock>,
) -> Result {
    let Some(window) = dock::panel(Tool::Library, &dock, viewport.rect) else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let mut open = true;
    window.open(&mut open).show(ctx, |ui| {
        ui.weak("ビューへドラッグして配置 (+ は注視点に配置)");
        for category in Category::ALL {
            // 縦に長くならないよう、分類は畳んでおく
            egui::CollapsingHeader::new(category.label())
                .default_open(category == Category::Actuator)
                .show(ui, |ui| {
                    let kinds = PartKind::ALL
                        .into_iter()
                        .filter(|k| k.category() == category);
                    for kind in kinds {
                        ui.horizontal(|ui| {
                            if ui.small_button("+").clicked() {
                                let f = orbit.focus.as_dvec3();
                                let at = DVec3::new(f.x, 0.0, f.z);
                                place(&mut model, &mut selection, &mut history, kind, at);
                            }
                            let id = egui::Id::new(("library_palette", kind.key()));
                            ui.dnd_drag_source(id, kind, |ui| {
                                ui.add(
                                    egui::Button::new(kind.label())
                                        .min_size(egui::vec2(150.0, 0.0))
                                        .sense(egui::Sense::hover()),
                                );
                            });
                        });
                    }
                });
        }
        crate::features::custom_part::palette_section(
            ui,
            &mut custom,
            &mut jobs,
            orbit.focus.as_dvec3(),
        );
        ui.separator();
        ui.checkbox(&mut state.show_io, "I/O 一覧を表示");
        ui.checkbox(&mut state.show_overlays, "光軸・搬送方向を表示");
    });
    if !open {
        dock.set_open(Tool::Library, false);
    }
    Ok(())
}

/// 「作成」メニューの標準部品。選ぶと、いま置いてあるものの右 (+X 側) の床に置く。
pub fn add_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("標準部品", |ui| {
        for category in Category::ALL {
            ui.menu_button(category.label(), |ui| {
                for kind in PartKind::ALL
                    .into_iter()
                    .filter(|k| k.category() == category)
                {
                    if ui.button(kind.label()).clicked() {
                        let at = DVec3::from(state.model.free_spot([0.0; 3]));
                        place(
                            &mut state.model,
                            &mut state.selection,
                            &mut state.history,
                            kind,
                            at,
                        );
                    }
                }
            });
        }
    });
}

/// 部品を置き、履歴に積んで選択する
fn place(
    model: &mut Model,
    selection: &mut Selection,
    history: &mut History,
    kind: PartKind,
    at: DVec3,
) {
    let step = DROP_SNAP_MM * units_per_mm(&model.unit);
    let at = (at / step).round() * step + DVec3::ZERO;
    let id = add_standard(model, kind, at);
    let label = format!("{} を追加", model.path_label(id));
    history.record_added(model, id, label);
    selection.select(id);
    selection.reveal_in_tree = true;
}

/// パレットからドラッグ中の部品を、カーソル下の面 (なければ床) に置く
/// 部品を落とす先にしない 3D のもの (作業者・AGV の体と、背景の工場の建屋)
type NotDropTarget = Or<(With<FloorFigure>, With<HallMesh>)>;

#[allow(clippy::too_many_arguments)]
pub fn drop_on_viewport(
    mut contexts: EguiContexts,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    viewport: Res<ViewportRect>,
    mut ray_cast: MeshRayCast,
    figures: Query<(), NotDropTarget>,
    mut gizmos: Gizmos,
    mut model: ResMut<Model>,
    mut selection: ResMut<Selection>,
    mut history: ResMut<History>,
    custom: Res<CustomLibrary>,
    mut jobs: ResMut<ImportJobs>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let kind = egui::DragAndDrop::payload::<PartKind>(ctx);
    let custom_part = egui::DragAndDrop::payload::<CustomDrag>(ctx);
    if kind.is_none() && custom_part.is_none() {
        return Ok(());
    }
    let Some(cursor) = window.cursor_position().filter(|&c| viewport.contains(c)) else {
        return Ok(());
    };
    // パレットなどのウィンドウの上では置かない (ビュー自体は背景のレイヤー)
    let over_window = ctx
        .layer_id_at(egui::pos2(cursor.x, cursor.y))
        .is_some_and(|l| !matches!(l.order, egui::Order::Background | egui::Order::Tooltip));
    if over_window {
        return Ok(());
    }
    let (camera, camera_transform) = *camera;
    let Ok(ray) = camera.viewport_to_world(camera_transform, cursor) else {
        return Ok(());
    };
    // 作業者・AGV の体と、背景の工場の壁・柱・床の上には置かない (床は下の on_floor で拾う)
    let not_figure = |e: Entity| !figures.contains(e);
    let on_part = ray_cast
        .cast_ray(
            ray,
            &MeshRayCastSettings::default().with_filter(&not_figure),
        )
        .first()
        .map(|(_, hit)| hit.point);
    let on_floor = || {
        ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
            .map(|d| ray.get_point(d))
    };
    let Some(point) = on_part.or_else(on_floor) else {
        return Ok(());
    };

    // 置く位置の目印
    let size = (40.0 * units_per_mm(&model.unit)) as f32;
    let mark = Color::srgb(1.0, 0.6, 0.2);
    let flat = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    gizmos.circle(Isometry3d::new(point, flat), size, mark);
    gizmos.line(point, point + Vec3::Y * size * 3.0, mark);

    if ctx.input(|i| i.pointer.any_released()) {
        egui::DragAndDrop::clear_payload(ctx);
        if let Some(kind) = kind {
            place(
                &mut model,
                &mut selection,
                &mut history,
                *kind,
                point.as_dvec3(),
            );
        }
        if let Some(CustomDrag(entry)) = custom_part.as_deref() {
            custom.request_place(entry, &mut jobs, point.as_dvec3());
        }
    }
    Ok(())
}

/// プロパティパネルの「標準部品」欄: 寸法パラメータ、動作テスト、I/O 点
pub fn properties_section(ui: &mut egui::Ui, state: &mut UiState, id: NodeId) {
    let Some(NodeKind::Standard(spec)) = state.model.get(id).map(|n| n.kind.clone()) else {
        return;
    };
    let mut edited = spec.clone();
    let mut changed = false;
    egui::Grid::new("standard_params")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            for (value, p) in edited.params.0.iter_mut().zip(spec.kind.param_specs()) {
                ui.weak(p.label);
                let drag = egui::DragValue::new(value)
                    .speed((p.max - p.min) / 500.0)
                    .range(p.min..=p.max)
                    .max_decimals(1);
                changed |= ui.add(drag).changed();
                ui.end_row();
            }
        });
    if changed {
        state.history.begin_edit(&state.model, id);
        if let Some(node) = state.model.get_mut(id) {
            node.kind = NodeKind::Standard(edited);
        }
        rebuild_bodies(&mut state.model, id);
    }

    let Some(NodeKind::Standard(spec)) = state.model.get(id).map(|n| n.kind.clone()) else {
        return;
    };
    let actuators = spec.actuators();
    if !actuators.is_empty() {
        ui.add_space(4.0);
        ui.label("動作テスト (0 = 原位置, 1 = 動作端)");
        let values = state.jog.0.entry(id).or_default();
        values.resize(actuators.len(), 0.0);
        for (act, value) in actuators.iter().zip(values.iter_mut()) {
            ui.horizontal(|ui| {
                ui.weak(format!("{} [{}]", act.name, act.drive));
                ui.add(egui::Slider::new(value, 0.0..=1.0).max_decimals(2));
            });
            if let Motion::Belt { speed, .. } = act.motion {
                ui.weak(format!("搬送速度 {speed:.0} mm/s"));
            }
        }
        ui.weak("表示だけ動かします (設計位置は変わりません)");
    }

    ui.add_space(4.0);
    egui::CollapsingHeader::new(format!("I/O 点 ({} 点)", spec.io_points().len()))
        .id_salt("standard_io")
        .default_open(false)
        .show(ui, |ui| {
            egui::Grid::new("standard_io_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    for point in spec.io_points() {
                        ui.weak(point.dir.label());
                        ui.label(point.name);
                        ui.end_row();
                    }
                });
        });
}

/// 置いた機器の I/O 点の一覧と、シーケンサー・リモート I/O の点数が足りているか
pub fn io_list_window(
    mut contexts: EguiContexts,
    mut state: ResMut<LibraryUiState>,
    model: Res<Model>,
) -> Result {
    if !state.show_io {
        return Ok(());
    }
    let rows = io_table(&model);
    let (cap_in, cap_out) = io_capacity(&model);
    let mut open = true;
    egui::Window::new("I/O 一覧")
        .open(&mut open)
        .default_pos([420.0, 120.0])
        .show(contexts.ctx_mut()?, |ui| {
            for (dir, cap) in [(IoDir::Input, cap_in), (IoDir::Output, cap_out)] {
                let n = rows.iter().filter(|r| r.dir == dir).count();
                let text = format!(
                    "{}: {n} 点 / シーケンサー・リモート I/O {cap} 点",
                    dir.label()
                );
                if n > cap {
                    ui.colored_label(egui::Color32::from_rgb(230, 90, 60), text + "  (不足)");
                } else {
                    ui.label(text);
                }
            }
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    egui::Grid::new("io_table").striped(true).show(ui, |ui| {
                        ui.strong("機器");
                        ui.strong("I/O");
                        ui.strong("向き");
                        ui.end_row();
                        for row in &rows {
                            ui.label(&row.device);
                            ui.label(&row.point);
                            ui.label(row.dir.label());
                            ui.end_row();
                        }
                    });
                });
        });
    state.show_io = open;
    Ok(())
}

/// センサーの光軸とコンベアの搬送方向を描く
pub fn draw_overlays(
    mut gizmos: Gizmos,
    state: Res<LibraryUiState>,
    model: Res<Model>,
    jog: Res<Jog>,
) {
    if !state.show_overlays {
        return;
    }
    let scale = units_per_mm(&model.unit);
    for id in standard_nodes(&model) {
        if !model.is_shown(id) {
            continue;
        }
        let Some(NodeKind::Standard(spec)) = model.get(id).map(|n| &n.kind) else {
            continue;
        };
        if let Some(s) = spec.sensing() {
            let from = local_to_world(&model, id, s.origin);
            let to = from + dir_to_world(&model, id, s.direction) * s.range * scale;
            let red = Color::srgb(1.0, 0.15, 0.1);
            gizmos.line(from.as_vec3(), to.as_vec3(), red);
            gizmos.sphere(
                Isometry3d::from_translation(to.as_vec3()),
                (3.0 * scale) as f32,
                red,
            );
        }
        if let Some(surface) = spec.conveying_surface() {
            // 動作テストの値に合わせて矢印を流す
            let phase = jog
                .0
                .get(&id)
                .and_then(|v| v.first())
                .copied()
                .unwrap_or(0.0);
            let half = surface.length / 2.0;
            let step = 200.0;
            let y = surface.center[1] + 2.0;
            let yellow = Color::srgb(0.95, 0.95, 0.3);
            let mut x = -half + phase * step + step / 2.0;
            while x < half {
                let p = |x: f64, z: f64| local_to_world(&model, id, [x, y, z]).as_vec3();
                gizmos.line(p(x, -40.0), p(x + 40.0, 0.0), yellow);
                gizmos.line(p(x, 40.0), p(x + 40.0, 0.0), yellow);
                x += step;
            }
        }
    }
}
