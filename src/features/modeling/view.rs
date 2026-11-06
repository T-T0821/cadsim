//! 穴あけ道具の 3D の線と、画面に出す寸法・案内の文字
//!
//! 置きかけの穴は、入口と底の円・それをつなぐ線・軸の線で描く (青)。
//! 直径の点と底の中心は小さな球で、カーソルが乗ると明るくなる。
//! 部品の中に隠れないよう、線も点も形状より手前に描く ([`HoleGizmos`])。
//!
//! 断面の道具は、平面の枠 (部品に隠れる)・切り口の線 (手前に太く、[`SectionGizmos`])・
//! 平面から出る矢印で描く。部品を選んでいる間は、重心に小さな球を出す。

use bevy::{math::DVec3, prelude::*};
use bevy_egui::{EguiContexts, egui};

use crate::document::selection::Selection;
use crate::viewport::camera::ViewportRect;
use plantwright_core::modeling::extrude_tool::{self, ExtrudeTool};
use plantwright_core::modeling::hole_tool::{Handle, HoleTool};
use plantwright_core::modeling::mass::MassCache;
use plantwright_core::modeling::section_tool::SectionTool;

const HOLE_COLOR: Color = Color::srgb(0.25, 0.65, 1.0);
const HANDLE_COLOR: Color = Color::srgb(1.0, 0.85, 0.25);
const HANDLE_HOVER_COLOR: Color = Color::srgb(1.0, 1.0, 0.6);
const CIRCLE_SEGMENTS: usize = 48;

/// 穴あけ道具のギズモ。部品の中の線 (底の円・軸) やつまみが形状に隠れないよう手前に描く
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct HoleGizmos;

/// 断面の切り口の線と重心の球。部品に隠れないよう手前に、太めに描く
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct SectionGizmos;

pub fn configure_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<HoleGizmos>();
    config.depth_bias = -1.0;
    config.line.width = 2.0;
    let (config, _) = store.config_mut::<SectionGizmos>();
    config.depth_bias = -1.0;
    config.line.width = 3.0;
}

pub fn draw_preview(
    mut gizmos: Gizmos<HoleGizmos>,
    tool: Res<HoleTool>,
    camera: Single<&GlobalTransform, With<Camera3d>>,
) {
    let Some(draft) = tool.draft.as_ref().filter(|_| tool.active) else {
        return;
    };
    let polyline = |gizmos: &mut Gizmos<HoleGizmos>, points: &[DVec3], color: Color| {
        let mut pts: Vec<Vec3> = points.iter().map(|p| p.as_vec3()).collect();
        if let Some(first) = pts.first().copied() {
            pts.push(first);
        }
        gizmos.linestrip(pts, color);
    };
    let entrance = draft.circle(draft.center, CIRCLE_SEGMENTS);
    let bottom = draft.circle(draft.bottom(), CIRCLE_SEGMENTS);
    polyline(&mut gizmos, &entrance, HOLE_COLOR);
    polyline(&mut gizmos, &bottom, HOLE_COLOR.with_alpha(0.7));
    for i in (0..CIRCLE_SEGMENTS).step_by(CIRCLE_SEGMENTS / 4) {
        gizmos.line(
            entrance[i].as_vec3(),
            bottom[i].as_vec3(),
            HOLE_COLOR.with_alpha(0.5),
        );
    }
    // 軸 (貫通なら部品の反対側まで)
    gizmos.line(
        draft.center.as_vec3(),
        draft.bottom().as_vec3(),
        HOLE_COLOR.with_alpha(0.8),
    );

    // つまみ。見かけの大きさがそろうよう、カメラからの距離で半径を決める
    for handle in [Handle::Diameter, Handle::Depth] {
        let p = draft.handle_position(handle).as_vec3();
        let radius = camera.translation().distance(p) * 0.008;
        let color = if tool.hovered == Some(handle) || tool.dragging == Some(handle) {
            HANDLE_HOVER_COLOR
        } else {
            HANDLE_COLOR
        };
        gizmos.sphere(Isometry3d::from_translation(p), radius, color);
    }
}

/// 押し出し: 床に描いた形 (閉じる前はカーソルまでの線も)、閉じたら上の面と縦の線、つまみ
pub fn draw_extrude_preview(
    mut gizmos: Gizmos<HoleGizmos>,
    tool: Res<ExtrudeTool>,
    camera: Single<&GlobalTransform, With<Camera3d>>,
) {
    if !tool.active {
        return;
    }
    let base: Vec<Vec3> = tool.points.iter().map(|p| p.as_vec3()).collect();
    if tool.closed {
        let top: Vec<Vec3> = tool.top_points().iter().map(|p| p.as_vec3()).collect();
        let mut ring = base.clone();
        ring.extend(base.first().copied());
        gizmos.linestrip(ring, HOLE_COLOR);
        let mut ring = top.clone();
        ring.extend(top.first().copied());
        gizmos.linestrip(ring, HOLE_COLOR.with_alpha(0.8));
        for (a, b) in base.iter().zip(&top) {
            gizmos.line(*a, *b, HOLE_COLOR.with_alpha(0.5));
        }
        // 高さの軸
        gizmos.line(
            tool.center().as_vec3(),
            tool.handle_position(extrude_tool::Handle::Height).as_vec3(),
            HOLE_COLOR.with_alpha(0.8),
        );
    } else {
        if base.len() >= 2 {
            gizmos.linestrip(base.clone(), HOLE_COLOR);
        }
        if let (Some(last), Some(hover)) = (base.last(), tool.hover_point) {
            gizmos.line(*last, hover.as_vec3(), HOLE_COLOR.with_alpha(0.6));
        }
    }

    // つまみ: 閉じたら高さの点と頂点、閉じる前は頂点 (最初の点は大きめ: クリックで閉じる)
    let handles: Vec<extrude_tool::Handle> = if tool.closed {
        std::iter::once(extrude_tool::Handle::Height)
            .chain((0..base.len()).map(extrude_tool::Handle::Vertex))
            .collect()
    } else {
        (0..base.len()).map(extrude_tool::Handle::Vertex).collect()
    };
    for handle in handles {
        let p = tool.handle_position(handle).as_vec3();
        let closing = !tool.closed && handle == extrude_tool::Handle::Vertex(0) && base.len() >= 3;
        let radius = camera.translation().distance(p) * if closing { 0.011 } else { 0.007 };
        let color = if tool.hovered == Some(handle) || tool.dragging == Some(handle) {
            HANDLE_HOVER_COLOR
        } else {
            HANDLE_COLOR
        };
        gizmos.sphere(Isometry3d::from_translation(p), radius, color);
    }
    if let Some(hover) = tool.hover_point.filter(|_| tool.hovered.is_none()) {
        let p = hover.as_vec3();
        let radius = camera.translation().distance(p) * 0.005;
        gizmos.sphere(
            Isometry3d::from_translation(p),
            radius,
            HANDLE_COLOR.with_alpha(0.6),
        );
    }
}

/// つまみの横に寸法を出す (穴あけ・押し出し)
pub fn draw_labels(
    mut contexts: EguiContexts,
    tool: Res<HoleTool>,
    extrude: Res<ExtrudeTool>,
    viewport: Res<ViewportRect>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
) -> Result {
    if !tool.active && !extrude.active {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let Some(view) = viewport.rect.map(|r| {
        egui::Rect::from_min_max(egui::pos2(r.min.x, r.min.y), egui::pos2(r.max.x, r.max.y))
    }) else {
        return Ok(());
    };
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("hole_tool_labels"),
        ))
        .with_clip_rect(view);
    let (camera, camera_transform) = *camera;

    if let Some(draft) = &tool.draft {
        let labels = [
            (Handle::Diameter, format!("直径 {}", round3(draft.diameter))),
            (
                Handle::Depth,
                match draft.depth {
                    None => "貫通".to_owned(),
                    Some(d) => format!("深さ {}", round3(d)),
                },
            ),
        ];
        for (handle, text) in labels {
            let Ok(screen) =
                camera.world_to_viewport(camera_transform, draft.handle_position(handle).as_vec3())
            else {
                continue;
            };
            let strong = tool.hovered == Some(handle) || tool.dragging == Some(handle);
            label(
                &painter,
                egui::pos2(screen.x + 12.0, screen.y - 10.0),
                &text,
                if strong {
                    egui::Color32::from_rgb(60, 60, 20)
                } else {
                    egui::Color32::from_rgba_unmultiplied(20, 50, 90, 220)
                },
            );
        }
    }

    // 押し出し: 高さと、描いている途中の辺の長さ
    if extrude.active {
        let mut labels: Vec<(DVec3, String, bool)> = Vec::new();
        if extrude.closed {
            let strong = matches!(
                extrude.hovered.or(extrude.dragging),
                Some(extrude_tool::Handle::Height)
            );
            labels.push((
                extrude.handle_position(extrude_tool::Handle::Height),
                format!("高さ {}", round3(extrude.height)),
                strong,
            ));
        } else if let (Some(length), Some(hover)) = (extrude.pending_length(), extrude.hover_point)
        {
            labels.push((hover, format!("{}", round3(length)), false));
        }
        for (at, text, strong) in labels {
            let Ok(screen) = camera.world_to_viewport(camera_transform, at.as_vec3()) else {
                continue;
            };
            label(
                &painter,
                egui::pos2(screen.x + 12.0, screen.y - 10.0),
                &text,
                if strong {
                    egui::Color32::from_rgb(60, 60, 20)
                } else {
                    egui::Color32::from_rgba_unmultiplied(20, 50, 90, 220)
                },
            );
        }
    }
    // 案内はビュー共通の案内バー (ui/view_hints.rs) に出す
    Ok(())
}

fn label(painter: &egui::Painter, at: egui::Pos2, text: &str, background: egui::Color32) {
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        egui::FontId::monospace(13.0),
        egui::Color32::WHITE,
    );
    let rect = egui::Rect::from_min_size(at, galley.size()).expand(4.0);
    painter.rect_filled(rect, 3.0, background);
    painter.galley(
        rect.min + egui::vec2(4.0, 4.0),
        galley,
        egui::Color32::WHITE,
    );
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

// ---------------------------------------------------------------------------
// 丸める・面取り・肉抜きの道具
//
// 選んだ辺はオレンジ、カーソルの下の辺は黄色で、形状より手前に描く。面をクリックで周りの辺を
// まとめて選べるので、カーソルの下の面の周りの辺も薄い黄色で出す。矢印は根元から先の点まで。
// 結果のプレビューは半透明の青いメッシュで、出している間は元の部品の面を隠す (稜線は残す)。
// ---------------------------------------------------------------------------

use plantwright_core::model::Model;
use plantwright_core::modeling::shape_tool::{self, Hover, PartGeometry, ShapeTool};

const PICKED_COLOR: Color = Color::srgb(1.0, 0.55, 0.1);
const HOVER_COLOR: Color = Color::srgb(1.0, 0.95, 0.3);
const ARROW_COLOR: Color = Color::srgb(0.25, 0.65, 1.0);
const ERROR_COLOR: Color = Color::srgb(0.95, 0.3, 0.3);
/// ドラッグ中にプレビューを作り直す間隔 (秒)。形状カーネルの計算が毎フレームだと重いため
const PREVIEW_INTERVAL: f32 = 0.12;

/// 結果のプレビューの表示
#[derive(Resource, Default)]
pub struct ShapePreview {
    /// 今のプレビューを作ったときの (部品, 道具, 選んだもの, 値)
    key: Option<(u32, shape_tool::ShapeOp, Vec<usize>, u64)>,
    entity: Option<Entity>,
    material: Option<bevy::asset::Handle<StandardMaterial>>,
    /// 前にプレビューを作った時刻
    built_at: f32,
    /// プレビューを出している部品 (元の部品の面を隠す)
    pub hidden_node: Option<u32>,
}

/// 選んだ辺・面と値が変わったら、結果の形を作り直して半透明で出す。作れなければ理由を案内に出す
#[allow(clippy::too_many_arguments)]
pub fn update_shape_preview(
    mut commands: Commands,
    mut tool: ResMut<ShapeTool>,
    model: Res<Model>,
    time: Res<Time>,
    mut preview: ResMut<ShapePreview>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let key = tool
        .node
        .filter(|_| tool.ready())
        .map(|node| (node, tool.op, tool.picks.clone(), tool.value.to_bits()));
    let clear = |preview: &mut ShapePreview, commands: &mut Commands| {
        if let Some(entity) = preview.entity.take() {
            commands.entity(entity).despawn();
        }
        preview.hidden_node = None;
    };
    let Some(key) = key else {
        clear(&mut preview, &mut commands);
        preview.key = None;
        return;
    };
    let now = time.elapsed_secs();
    if preview.key.as_ref() == Some(&key) || now - preview.built_at < PREVIEW_INTERVAL {
        // プレビューの位置は部品に合わせ続ける
        if let Some(entity) = preview.entity {
            commands
                .entity(entity)
                .insert(world_transform(&model, key.0));
        }
        return;
    }
    preview.key = Some(key.clone());
    preview.built_at = now;
    let solid = shape_tool::preview_solid(
        &model,
        &plantwright_core::modeling::record::JsonSolids,
        &tool,
    );
    clear(&mut preview, &mut commands);
    match solid {
        Ok(solid) => {
            tool.error = None;
            let shape = plantwright_core::modeling::shape_from_solid(&solid);
            let mesh = Mesh::new(
                bevy::mesh::PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, shape.mesh.positions.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, shape.mesh.normals.clone());
            let material = preview
                .material
                .get_or_insert_with(|| {
                    materials.add(StandardMaterial {
                        base_color: Color::srgba(0.35, 0.7, 1.0, 0.55),
                        alpha_mode: AlphaMode::Blend,
                        double_sided: true,
                        cull_mode: None,
                        ..default()
                    })
                })
                .clone();
            preview.entity = Some(
                commands
                    .spawn((
                        Mesh3d(meshes.add(mesh)),
                        MeshMaterial3d(material),
                        world_transform(&model, key.0),
                    ))
                    .id(),
            );
            preview.hidden_node = Some(key.0);
        }
        Err(e) => tool.error = Some(e),
    }
}

fn world_transform(model: &Model, node: u32) -> Transform {
    Transform::from_matrix(model.world_matrix(node).as_mat4())
}

/// プレビューを出している間は、元の部品の面を描かない (稜線は残るので元の形と見比べられる)
pub fn hide_previewed_part(
    preview: Res<ShapePreview>,
    mut parts: Query<(&crate::features::simulation::SimBody, &mut Visibility)>,
) {
    let Some(node) = preview.hidden_node else {
        return;
    };
    for (body, mut visibility) in &mut parts {
        if body.id == node {
            visibility.set_if_neq(Visibility::Hidden);
        }
    }
}

/// 選んだ辺・カーソルの下の辺と、値を決める矢印
pub fn draw_shape_tool(
    mut gizmos: Gizmos<HoleGizmos>,
    tool: Res<ShapeTool>,
    geometry: Res<PartGeometry>,
    camera: Single<&GlobalTransform, With<Camera3d>>,
) {
    if !tool.active || tool.node != Some(geometry.node) {
        return;
    }
    let edge = |gizmos: &mut Gizmos<HoleGizmos>, e: usize, color: Color| {
        if let Some(points) = geometry.edges.get(e) {
            gizmos.linestrip(points.iter().map(|p| p.as_vec3()), color);
        }
    };
    // 選んだもの (肉抜きは面の周りの辺で見せる)
    for &pick in &tool.picks {
        if tool.op.picks_faces() {
            for e in geometry.face_edges(pick) {
                edge(&mut gizmos, e, PICKED_COLOR);
            }
        } else {
            edge(&mut gizmos, pick, PICKED_COLOR);
        }
    }
    match tool.hovered {
        Some(Hover::Edge(e)) => edge(&mut gizmos, e, HOVER_COLOR),
        Some(Hover::Face(f)) => {
            for e in geometry.face_edges(f) {
                edge(&mut gizmos, e, HOVER_COLOR.with_alpha(0.6));
            }
        }
        None => {}
    }
    // 矢印
    let Some((base, dir)) = tool.arrow(&geometry) else {
        return;
    };
    if tool.op.picks_faces() || !tool.picks.is_empty() {
        let tip = (base + dir * tool.value).as_vec3();
        let color = if tool.error.is_some() {
            ERROR_COLOR
        } else {
            ARROW_COLOR
        };
        gizmos.line(base.as_vec3(), tip, color);
        let radius = camera.translation().distance(tip) * 0.008;
        let handle = if tool.error.is_some() {
            ERROR_COLOR
        } else if tool.handle_hovered || tool.dragging {
            HANDLE_HOVER_COLOR
        } else {
            HANDLE_COLOR
        };
        gizmos.sphere(Isometry3d::from_translation(tip), radius, handle);
    }
}

/// 矢印の先に値を出す (案内はビューの下の共通の案内バー。`ui::view_hints`)
pub fn draw_shape_labels(
    mut contexts: EguiContexts,
    tool: Res<ShapeTool>,
    geometry: Res<PartGeometry>,
    viewport: Res<ViewportRect>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
) -> Result {
    if !tool.active {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let Some(view) = viewport.rect.map(|r| {
        egui::Rect::from_min_max(egui::pos2(r.min.x, r.min.y), egui::pos2(r.max.x, r.max.y))
    }) else {
        return Ok(());
    };
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("shape_tool_labels"),
        ))
        .with_clip_rect(view);
    let (camera, camera_transform) = *camera;
    if (tool.op.picks_faces() || !tool.picks.is_empty())
        && tool.node == Some(geometry.node)
        && let Some(tip) = tool.handle_position(&geometry)
        && let Ok(screen) = camera.world_to_viewport(camera_transform, tip.as_vec3())
    {
        label(
            &painter,
            egui::pos2(screen.x + 12.0, screen.y - 10.0),
            &format!("{} {}", tool.op.value_label(), round3(tool.value)),
            if tool.handle_hovered || tool.dragging {
                egui::Color32::from_rgb(60, 60, 20)
            } else {
                egui::Color32::from_rgba_unmultiplied(20, 50, 90, 220)
            },
        );
    }
    Ok(())
}

const SECTION_PLANE_COLOR: Color = Color::srgba(0.95, 0.45, 0.2, 0.8);
const SECTION_CUT_COLOR: Color = Color::srgb(1.0, 0.3, 0.15);
const CENTROID_COLOR: Color = Color::srgb(0.9, 0.2, 0.75);

/// 断面の平面・切り口・矢印
pub fn draw_section(
    mut gizmos: Gizmos,
    mut front: Gizmos<SectionGizmos>,
    mut handles: Gizmos<HoleGizmos>,
    tool: Res<SectionTool>,
    camera: Single<&GlobalTransform, With<Camera3d>>,
) {
    if !tool.active {
        return;
    }
    // 平面の枠と、中の細かい格子 (薄く)
    let c = tool.plane_corners();
    let corners: Vec<Vec3> = c.iter().map(|p| p.as_vec3()).collect();
    gizmos.linestrip(
        corners.iter().copied().chain([corners[0]]),
        SECTION_PLANE_COLOR,
    );
    for i in 1..8 {
        let t = i as f64 / 8.0;
        gizmos.line(
            c[0].lerp(c[1], t).as_vec3(),
            c[3].lerp(c[2], t).as_vec3(),
            SECTION_PLANE_COLOR.with_alpha(0.25),
        );
        gizmos.line(
            c[0].lerp(c[3], t).as_vec3(),
            c[1].lerp(c[2], t).as_vec3(),
            SECTION_PLANE_COLOR.with_alpha(0.25),
        );
    }
    // 切り口
    for line in &tool.lines {
        front.linestrip(line.iter().map(|p| p.as_vec3()), SECTION_CUT_COLOR);
    }
    // 平面から出る矢印と、その先のつまみ
    let tip = tool.handle_position().as_vec3();
    handles.arrow(tool.point().as_vec3(), tip, SECTION_PLANE_COLOR);
    let radius = camera.translation().distance(tip) * 0.008;
    let color = if tool.hovered || tool.dragging {
        HANDLE_HOVER_COLOR
    } else {
        HANDLE_COLOR
    };
    handles.sphere(Isometry3d::from_translation(tip), radius, color);
}

/// 選んでいる部品の重心に小さな球と十字を出す (プロパティの質量の欄が計算したもの)
pub fn draw_centroid(
    mut gizmos: Gizmos<SectionGizmos>,
    mass: Res<MassCache>,
    selection: Res<Selection>,
    camera: Single<&GlobalTransform, With<Camera3d>>,
) {
    let Some((node, centroid)) = mass.centroid else {
        return;
    };
    if selection.node != Some(node) {
        return;
    }
    let p = centroid.as_vec3();
    let r = camera.translation().distance(p) * 0.01;
    gizmos.sphere(Isometry3d::from_translation(p), r, CENTROID_COLOR);
    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
        gizmos.line(p - axis * r * 2.0, p + axis * r * 2.0, CENTROID_COLOR);
    }
}

/// 断面の位置の文字 (矢印の先の横)
pub fn draw_section_labels(
    mut contexts: EguiContexts,
    tool: Res<SectionTool>,
    viewport: Res<ViewportRect>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
) -> Result {
    if !tool.active {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let Some(view) = viewport.rect.map(|r| {
        egui::Rect::from_min_max(egui::pos2(r.min.x, r.min.y), egui::pos2(r.max.x, r.max.y))
    }) else {
        return Ok(());
    };
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("section_tool_labels"),
        ))
        .with_clip_rect(view);
    let (camera, camera_transform) = *camera;
    if let Ok(screen) = camera.world_to_viewport(camera_transform, tool.handle_position().as_vec3())
    {
        let p = tool.point();
        let n = tool.normal;
        // 軸にそろっていればその座標、斜めなら中心からの距離
        let text = match [n.x, n.y, n.z].iter().position(|v| v.abs() > 1.0 - 1e-9) {
            Some(i) => format!("{} = {}", ["X", "Y", "Z"][i], round3(p[i])),
            None => format!("中心から {}", round3(tool.offset)),
        };
        let strong = tool.hovered || tool.dragging;
        label(
            &painter,
            egui::pos2(screen.x + 12.0, screen.y - 10.0),
            &text,
            if strong {
                egui::Color32::from_rgb(60, 60, 20)
            } else {
                egui::Color32::from_rgba_unmultiplied(110, 50, 20, 220)
            },
        );
    }
    // 案内はビュー共通の案内バー (ui/view_hints.rs) に出す
    Ok(())
}
