//! 下敷きの 3D 表示
//!
//! 平面図1枚につきエンティティを1つ作り、線は LineList のメッシュ、画像は板に貼る。
//! メッシュは図面の座標の中心を原点にして作り、縮尺・回転・位置は Transform で付ける
//! (縮尺合わせのたびにメッシュを作り直さなくて済む)。

use std::sync::Arc;

use bevy::{
    asset::RenderAssetUsages,
    math::DVec2,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

use super::PlanTool;
use crate::features::simulation::view::SimulationGizmos;
use plantwright_core::model::Model;
use plantwright_core::site::underlay::{PlanContent, Underlay};

/// GPU に載せられる画像の一辺の上限
const MAX_TEXTURE_SIZE: u32 = 8192;
const LINE_COLOR: Color = Color::srgb(0.35, 0.75, 1.0);

/// 下敷き1枚の表示
#[derive(Component)]
pub struct UnderlayView {
    /// 下敷きの番号
    index: usize,
    /// 表示している中身 (Arc のアドレス)。変わったら作り直す
    content: usize,
    /// メッシュの原点にした図面の座標
    center: DVec2,
    materials: Vec<Handle<StandardMaterial>>,
}

/// 下敷きの増減に合わせてエンティティを作り直し、配置・不透明度・表示を毎フレーム写す
pub fn sync_underlays(
    mut commands: Commands,
    model: Res<Model>,
    mut views: Query<(Entity, &UnderlayView, &mut Transform, &mut Visibility)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut shown: Local<Vec<usize>>,
) {
    let underlays = &model.site.underlays;
    let wanted: Vec<usize> = underlays
        .iter()
        .map(|u| Arc::as_ptr(&u.content) as usize)
        .collect();
    if *shown != wanted {
        for (entity, ..) in &views {
            commands.entity(entity).despawn();
        }
        for (index, underlay) in underlays.iter().enumerate() {
            spawn_underlay(
                &mut commands,
                index,
                underlay,
                &mut meshes,
                &mut materials,
                &mut images,
            );
        }
        *shown = wanted;
        return;
    }
    for (_, view, mut transform, mut visibility) in &mut views {
        let Some(underlay) = underlays.get(view.index) else {
            continue;
        };
        if view.content != Arc::as_ptr(&underlay.content) as usize {
            continue;
        }
        *transform = underlay_transform(underlay, view.center);
        *visibility = if underlay.visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        for handle in &view.materials {
            if let Some(mut material) = materials.get_mut(handle) {
                let alpha = underlay.opacity.clamp(0.05, 1.0);
                if (material.base_color.alpha() - alpha).abs() > 1e-3 {
                    material.base_color.set_alpha(alpha);
                }
            }
        }
    }
}

fn underlay_transform(underlay: &Underlay, center: DVec2) -> Transform {
    Transform {
        translation: underlay.to_world(center).as_vec3(),
        rotation: Quat::from_rotation_y(underlay.rotation_deg.to_radians() as f32),
        scale: Vec3::splat(underlay.scale as f32),
    }
}

fn spawn_underlay(
    commands: &mut Commands,
    index: usize,
    underlay: &Underlay,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) {
    let content = &underlay.content;
    let center = content
        .bounds()
        .map_or(DVec2::ZERO, |(min, max)| (min + max) / 2.0);
    let alpha = underlay.opacity.clamp(0.05, 1.0);
    let mut handles = Vec::new();
    let mut children = Vec::new();

    if let Some(mesh) = line_mesh(content, center) {
        let material = materials.add(StandardMaterial {
            base_color: LINE_COLOR.with_alpha(alpha),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..default()
        });
        handles.push(material.clone());
        children.push((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material)));
    }
    if let Some(image) = &content.image {
        let texture = images.add(to_texture(image.width, image.height, &image.rgba));
        let material = materials.add(StandardMaterial {
            base_color: Color::WHITE.with_alpha(alpha),
            base_color_texture: Some(texture),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            double_sided: true,
            cull_mode: None,
            // 線より少し奥に描く
            depth_bias: -1.0,
            ..default()
        });
        handles.push(material.clone());
        let quad = image_quad(image.min - center, image.max - center);
        children.push((Mesh3d(meshes.add(quad)), MeshMaterial3d(material)));
    }

    let mut entity = commands.spawn((
        Name::new(format!("平面図 {}", underlay.name)),
        underlay_transform(underlay, center),
        Visibility::default(),
        UnderlayView {
            index,
            content: Arc::as_ptr(&underlay.content) as usize,
            center,
            materials: handles,
        },
    ));
    entity.with_children(|parent| {
        for child in children {
            parent.spawn(child);
        }
    });
}

/// 図面の (u, v) を床の (u, 0, -v) に置いた線のメッシュ
fn line_mesh(content: &PlanContent, center: DVec2) -> Option<Mesh> {
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(content.segment_count() * 2);
    for line in &content.polylines {
        for pair in line.windows(2) {
            for p in pair {
                let p = *p - center;
                positions.push([p.x as f32, 0.0, -p.y as f32]);
            }
        }
    }
    if positions.is_empty() {
        return None;
    }
    let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
    Some(
        Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals),
    )
}

/// 画像を貼る床の板。画像の上の行が図面の +v (= ワールドの -Z) 側に来る
fn image_quad(min: DVec2, max: DVec2) -> Mesh {
    let (x0, x1) = (min.x as f32, max.x as f32);
    let (z_bottom, z_top) = (-min.y as f32, -max.y as f32);
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [x0, 0.0, z_top],
            [x1, 0.0, z_top],
            [x1, 0.0, z_bottom],
            [x0, 0.0, z_bottom],
        ],
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 4])
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    )
    .with_inserted_indices(Indices::U32(vec![0, 2, 1, 0, 3, 2]))
}

fn to_texture(width: u32, height: u32, rgba: &[u8]) -> Image {
    let (width, height, data) = if width > MAX_TEXTURE_SIZE || height > MAX_TEXTURE_SIZE {
        let ratio = MAX_TEXTURE_SIZE as f64 / width.max(height) as f64;
        let (w, h) = (
            ((width as f64 * ratio) as u32).max(1),
            ((height as f64 * ratio) as u32).max(1),
        );
        let source = image::RgbaImage::from_raw(width, height, rgba.to_vec())
            .unwrap_or_else(|| image::RgbaImage::new(width, height));
        let resized = image::imageops::resize(&source, w, h, image::imageops::FilterType::Triangle);
        (w, h, resized.into_raw())
    } else {
        (width, height, rgba.to_vec())
    };
    Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

/// 縮尺合わせの点と、カーソルの下の点を描く
pub fn draw_plan_tool(
    mut gizmos: Gizmos<SimulationGizmos>,
    tool: Res<PlanTool>,
    model: Res<Model>,
) {
    if tool.calibrating.is_none() {
        return;
    }
    let size = model
        .site
        .underlays
        .get(tool.calibrating.unwrap_or(0))
        .and_then(|u| u.world_bounds())
        .map_or(10.0, |(min, max)| ((max - min).length() * 0.004) as f32);
    let color = Color::srgb(1.0, 0.3, 0.8);
    for p in &tool.points {
        gizmos.sphere(Isometry3d::from_translation(p.as_vec3()), size, color);
    }
    if let [a, b] = tool.points.as_slice() {
        gizmos.line(a.as_vec3(), b.as_vec3(), color);
    } else if let (Some(a), Some(hover)) = (tool.points.first(), tool.hover) {
        gizmos.line(a.as_vec3(), hover.as_vec3(), color.with_alpha(0.6));
    }
    if let Some(hover) = tool.hover {
        gizmos.sphere(
            Isometry3d::from_translation(hover.as_vec3()),
            size * 0.7,
            Color::srgb(0.3, 1.0, 0.4),
        );
    }
}
