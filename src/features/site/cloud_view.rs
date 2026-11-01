//! 点群の 3D 表示 (LOD) と、設備との干渉チェックの実行・表示
//!
//! 点群1つにつき親のエンティティを1つ作り、区画 (タイル) ごと・LOD の段ごとに点のメッシュを持つ。
//! 毎フレーム、カメラからの距離で区画ごとに1段だけを表示する。
//! 干渉チェックは部品が動いたり点群を動かしたりしたときに別スレッドで行う。

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use bevy::{
    asset::RenderAssetUsages,
    mesh::PrimitiveTopology,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, block_on, poll_once},
};

use crate::features::simulation::view::SimulationGizmos;
use crate::features::simulation::{Pose, SimBody, TriMesh};
use plantwright_core::model::Model;
use plantwright_core::site::pointcloud::{
    CloudHit, LOD_LEVELS, PlacedCloud, PointCloud, check_body, lod_fraction, lod_level,
};

/// 点群1つの表示
#[derive(Component)]
pub struct CloudView {
    index: usize,
}

/// 区画1つの、LOD の1段
#[derive(Component)]
pub struct CloudLod {
    tile: usize,
    level: usize,
}

/// 点群の表示の設定 (パネルで変える)
#[derive(Resource)]
pub struct CloudDisplay {
    /// 大きいほど遠くまで細かく描く
    pub detail: f32,
    material: Option<Handle<StandardMaterial>>,
}

impl Default for CloudDisplay {
    fn default() -> Self {
        Self {
            detail: 1.0,
            material: None,
        }
    }
}

fn cloud_transform(placed: &PlacedCloud) -> Transform {
    crate::viewport::convert::to_transform(placed.to_world())
}

/// 点群の増減に合わせて表示を作り直し、配置と LOD を毎フレーム写す
#[allow(clippy::too_many_arguments)]
pub fn sync_clouds(
    mut commands: Commands,
    model: Res<Model>,
    mut display: ResMut<CloudDisplay>,
    mut views: Query<(Entity, &CloudView, &mut Transform, &mut Visibility), Without<CloudLod>>,
    mut lods: Query<(&CloudLod, &ChildOf, &mut Visibility), Without<CloudView>>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut shown: Local<Vec<usize>>,
) {
    let clouds = &model.site.clouds;
    let wanted: Vec<usize> = clouds
        .iter()
        .map(|c| Arc::as_ptr(&c.cloud) as usize)
        .collect();
    if *shown != wanted {
        for (entity, ..) in &views {
            commands.entity(entity).despawn();
        }
        let material = display
            .material
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: Color::WHITE,
                    unlit: true,
                    ..default()
                })
            })
            .clone();
        for (index, placed) in clouds.iter().enumerate() {
            spawn_cloud(&mut commands, index, placed, &material, &mut meshes);
        }
        *shown = wanted;
        return;
    }

    let camera = camera.iter().next().map(|t| t.translation());
    let mut inverse = std::collections::HashMap::new();
    for (entity, view, mut transform, mut visibility) in &mut views {
        let Some(placed) = clouds.get(view.index) else {
            continue;
        };
        *transform = cloud_transform(placed);
        *visibility = if placed.visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        inverse.insert(entity, (view.index, transform.compute_affine().inverse()));
    }
    for (lod, parent, mut visibility) in &mut lods {
        let Some((index, to_local)) = inverse.get(&parent.parent()) else {
            continue;
        };
        let Some(tile) = clouds.get(*index).and_then(|c| c.cloud.tiles.get(lod.tile)) else {
            continue;
        };
        let level = camera.map_or(LOD_LEVELS - 1, |eye| {
            lod_level(
                tile.min,
                tile.max,
                to_local.transform_point3(eye),
                display.detail,
            )
        });
        let want = if level == lod.level {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != want {
            *visibility = want;
        }
    }
}

fn spawn_cloud(
    commands: &mut Commands,
    index: usize,
    placed: &PlacedCloud,
    material: &Handle<StandardMaterial>,
    meshes: &mut Assets<Mesh>,
) {
    let cloud = &placed.cloud;
    let (min, max) = cloud.bounds();
    let mut children = Vec::new();
    for (tile_index, tile) in cloud.tiles.iter().enumerate() {
        for level in 0..LOD_LEVELS {
            let count = ((tile.indices.len() as f32 * lod_fraction(level)).ceil() as usize)
                .clamp(1, tile.indices.len());
            let mesh = points_mesh(cloud, &tile.indices[..count], min.y, max.y);
            children.push((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material.clone()),
                CloudLod {
                    tile: tile_index,
                    level,
                },
                Visibility::Hidden,
            ));
        }
    }
    commands
        .spawn((
            Name::new(format!("点群 {}", placed.name)),
            cloud_transform(placed),
            Visibility::default(),
            CloudView { index },
        ))
        .with_children(|parent| {
            for child in children {
                parent.spawn(child);
            }
        });
}

/// 点のメッシュ。色のない点群は高さで色分けする (低い = 青、高い = 赤)
fn points_mesh(cloud: &PointCloud, indices: &[u32], low: f32, high: f32) -> Mesh {
    let positions: Vec<[f32; 3]> = indices
        .iter()
        .map(|&i| cloud.points[i as usize].to_array())
        .collect();
    let colors: Vec<[f32; 4]> = match &cloud.colors {
        Some(colors) => indices
            .iter()
            .map(|&i| {
                let [r, g, b] = colors[i as usize];
                Color::srgb_u8(r, g, b).to_linear().to_f32_array()
            })
            .collect(),
        None => indices
            .iter()
            .map(|&i| {
                let t = (cloud.points[i as usize].y - low) / (high - low).max(1e-6);
                Color::hsl(240.0 * (1.0 - t.clamp(0.0, 1.0)), 0.8, 0.55)
                    .to_linear()
                    .to_f32_array()
            })
            .collect(),
    };
    Mesh::new(PrimitiveTopology::PointList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
}

// ---------------------------------------------------------------------------
// 干渉チェック
// ---------------------------------------------------------------------------

/// 点群と部品の判定結果
#[derive(Resource)]
pub struct CloudCheck {
    pub auto: bool,
    /// この距離より近い点を「近接」として出す (モデルの単位)
    pub warning: f64,
    /// (点群の番号, 結果)
    pub hits: Vec<(usize, CloudHit)>,
    pub running: bool,
    task: Option<Task<Vec<(usize, CloudHit)>>>,
    signature: u64,
    requested: bool,
}

impl Default for CloudCheck {
    fn default() -> Self {
        Self {
            auto: true,
            warning: 100.0,
            hits: Vec::new(),
            running: false,
            task: None,
            signature: 0,
            requested: false,
        }
    }
}

impl CloudCheck {
    pub fn request(&mut self) {
        self.requested = true;
    }
}

type ChangedBody = (With<SimBody>, Or<(Changed<SimBody>, Changed<Visibility>)>);

/// 点群の置き方と警告距離の目印 (変わったら判定し直す)
fn signature(model: &Model, warning: f64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    model.site.revision.hash(&mut h);
    warning.to_bits().hash(&mut h);
    for c in &model.site.clouds {
        c.offset.map(f64::to_bits).hash(&mut h);
        c.rotation_deg.to_bits().hash(&mut h);
        c.visible.hash(&mut h);
    }
    h.finish()
}

pub fn run_cloud_check(
    model: Res<Model>,
    mut check: ResMut<CloudCheck>,
    bodies: Query<(&SimBody, &Visibility)>,
    changed: Query<(), ChangedBody>,
    mut removed: RemovedComponents<SimBody>,
) {
    let check = check.bypass_change_detection();
    if let Some(task) = &mut check.task
        && let Some(hits) = block_on(poll_once(task))
    {
        check.hits = hits;
        check.task = None;
        check.running = false;
    }
    let sig = signature(&model, check.warning);
    let moved = !changed.is_empty() || removed.read().count() > 0;
    if sig != check.signature || (check.auto && moved) {
        check.requested = true;
    }
    if !check.requested || check.task.is_some() {
        return;
    }
    check.requested = false;
    check.signature = sig;
    let clouds: Vec<(usize, PlacedCloud)> = model
        .site
        .clouds
        .iter()
        .cloned()
        .enumerate()
        .filter(|(_, c)| c.visible)
        .collect();
    if clouds.is_empty() {
        check.hits.clear();
        return;
    }
    let inputs: Vec<(u32, Arc<TriMesh>, Pose)> = bodies
        .iter()
        .filter(|(_, v)| **v != Visibility::Hidden)
        .map(|(b, _)| (b.id, b.mesh.clone(), b.pose))
        .collect();
    let warning = check.warning;
    check.running = true;
    check.task = Some(AsyncComputeTaskPool::get().spawn(async move {
        let mut hits = Vec::new();
        for (index, cloud) in &clouds {
            for (id, mesh, pose) in &inputs {
                if let Some(hit) = check_body(cloud, *id, mesh, pose, warning) {
                    hits.push((*index, hit));
                }
            }
        }
        // 干渉している部品を先に、次に近い順
        hits.sort_by(|a, b| {
            b.1.inside.cmp(&a.1.inside).then(
                a.1.distance()
                    .unwrap_or(f64::MAX)
                    .total_cmp(&b.1.distance().unwrap_or(f64::MAX)),
            )
        });
        hits
    }));
}

/// 干渉している点 (赤) と、いちばん近い点までの線 (黄) を描く
pub fn draw_cloud_hits(
    mut gizmos: Gizmos<SimulationGizmos>,
    check: Res<CloudCheck>,
    model: Res<Model>,
) {
    let size = (check.warning * 0.05).max(1e-6) as f32;
    for (index, hit) in &check.hits {
        if !model.site.clouds.get(*index).is_some_and(|c| c.visible) {
            continue;
        }
        for p in &hit.inside_samples {
            let p = p.as_vec3();
            gizmos.line(
                p - Vec3::X * size,
                p + Vec3::X * size,
                Color::srgb(1.0, 0.15, 0.15),
            );
            gizmos.line(
                p - Vec3::Z * size,
                p + Vec3::Z * size,
                Color::srgb(1.0, 0.15, 0.15),
            );
        }
        if hit.inside == 0
            && let Some((a, b)) = hit.closest
        {
            let color = Color::srgb(0.95, 0.85, 0.15);
            gizmos.line(a.as_vec3(), b.as_vec3(), color);
            gizmos.sphere(Isometry3d::from_translation(a.as_vec3()), size, color);
        }
    }
}
