//! 判定結果の3D表示
//!
//! - 干渉部分の立体: 濃い赤のメッシュ (干渉している部品は main 側で半透明の赤にする)
//! - 交線: 赤い太線
//! - 接触・近接: 最も近い2点を結ぶ線 (接触 = 橙, 近接 = 黄)
//!
//! 線は部品の奥にあっても見えるよう、手前に描く設定のギズモ群で描く。

use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology, prelude::*};

use super::{
    InterferenceCheck,
    geometry::{Triangle, triangle_normal},
    interference::PairStatus,
};

pub const INTERFERENCE_COLOR: Color = Color::srgb(0.95, 0.15, 0.15);
pub const CONTACT_COLOR: Color = Color::srgb(1.0, 0.55, 0.1);
pub const NEAR_COLOR: Color = Color::srgb(0.95, 0.85, 0.15);

/// シミュレーション表示用の線 (常に手前に描く)
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct SimulationGizmos;

/// 干渉部分の立体メッシュ (判定のたびに作り直す)
#[derive(Component)]
pub struct OverlapVisual;

#[derive(Resource)]
pub struct OverlapMaterial(Handle<StandardMaterial>);

pub fn setup(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut gizmo_config: ResMut<GizmoConfigStore>,
) {
    commands.insert_resource(OverlapMaterial(materials.add(StandardMaterial {
        base_color: Color::srgb(0.75, 0.05, 0.05),
        emissive: LinearRgba::rgb(0.35, 0.0, 0.0),
        double_sided: true,
        cull_mode: None,
        // 部品の面と重なる所でちらつかないよう、少しだけ手前に描く
        depth_bias: 10.0,
        ..default()
    })));
    let (config, _) = gizmo_config.config_mut::<SimulationGizmos>();
    config.depth_bias = -1.0;
    config.line.width = 3.0;
}

/// 判定結果が変わったら、干渉部分の立体メッシュを作り直す。
pub fn sync_overlap_meshes(
    mut commands: Commands,
    check: Res<InterferenceCheck>,
    visuals: Query<Entity, With<OverlapVisual>>,
    material: Res<OverlapMaterial>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut shown_revision: Local<u64>,
) {
    if check.report.revision == *shown_revision {
        return;
    }
    *shown_revision = check.report.revision;
    for entity in &visuals {
        commands.entity(entity).despawn();
    }
    for pair in &check.report.pairs {
        if let PairStatus::Interfering {
            overlap: Some(overlap),
        } = &pair.status
            && !overlap.triangles.is_empty()
        {
            commands.spawn((
                Mesh3d(meshes.add(triangles_to_mesh(&overlap.triangles))),
                MeshMaterial3d(material.0.clone()),
                OverlapVisual,
            ));
        }
    }
}

pub fn draw_results(mut gizmos: Gizmos<SimulationGizmos>, check: Res<InterferenceCheck>) {
    for pair in &check.report.pairs {
        for [a, b] in &pair.curves {
            gizmos.line(a.as_vec3(), b.as_vec3(), INTERFERENCE_COLOR);
        }
        let color = match pair.status {
            PairStatus::Interfering { .. } => continue,
            PairStatus::Contact => CONTACT_COLOR,
            PairStatus::Near { .. } => NEAR_COLOR,
        };
        if let Some([a, b]) = pair.closest {
            let (a, b) = (a.as_vec3(), b.as_vec3());
            gizmos.line(a, b, color);
            gizmos.sphere(Isometry3d::from_translation(a), 0.03, color);
            gizmos.sphere(Isometry3d::from_translation(b), 0.03, color);
        }
    }
}

fn triangles_to_mesh(triangles: &[Triangle]) -> Mesh {
    let positions = super::overlap::to_f32_positions(triangles);
    let normals: Vec<[f32; 3]> = triangles
        .iter()
        .flat_map(|t| [triangle_normal(t).as_vec3().to_array(); 3])
        .collect();
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
}
