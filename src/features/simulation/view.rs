//! 判定結果の3D表示
//!
//! - 干渉部分の立体: 濃い赤のメッシュ (干渉している部品は main 側で半透明の赤にする)
//! - 交線: 赤い太線
//! - 接触・近接: 最も近い2点を結ぶ線 (接触 = 橙, 近接 = 黄)
//!
//! 線は部品の奥にあっても見えるよう、手前に描く設定のギズモで描く。
//!
//! 干渉の組が数千あっても軽いよう、どちらも判定結果が変わったときだけ作り直して GPU に置いておく。
//! 干渉部分の立体は全部の組を1つのメッシュにまとめ (1回の描画で済む)、線は保持型のギズモ
//! (`Gizmo` + `GizmoAsset`) にして、毎フレーム CPU で線を積み直さない。

use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology, prelude::*};

use super::InterferenceCheck;
use plantwright_core::simulation::geometry::{Triangle, triangle_normal};
use plantwright_core::simulation::interference::PairStatus;

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

/// 交線・最近点の線を入れておくギズモ (判定結果が変わったときだけ中身を作り直す)
#[derive(Resource)]
pub struct ResultLines(Handle<GizmoAsset>);

/// 線の太さ (ピクセル)
const LINE_WIDTH: f32 = 3.0;

pub fn setup(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
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
    config.line.width = LINE_WIDTH;

    let lines = gizmo_assets.add(GizmoAsset::default());
    commands.spawn((
        Gizmo {
            handle: lines.clone(),
            line_config: GizmoLineConfig {
                width: LINE_WIDTH,
                ..default()
            },
            depth_bias: -1.0,
        },
        Name::new("干渉チェックの線"),
    ));
    commands.insert_resource(ResultLines(lines));
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
    // 全部の組の干渉部分を1つのメッシュにまとめる (組ごとに分けると組の数だけ描画が要る)
    let triangles: Vec<Triangle> = check
        .report
        .pairs
        .iter()
        .filter_map(|pair| match &pair.status {
            PairStatus::Interfering {
                overlap: Some(overlap),
            } => Some(overlap.triangles.iter().copied()),
            _ => None,
        })
        .flatten()
        .collect();
    if !triangles.is_empty() {
        commands.spawn((
            Mesh3d(meshes.add(triangles_to_mesh(&triangles))),
            MeshMaterial3d(material.0.clone()),
            OverlapVisual,
        ));
    }
}

/// 判定結果が変わったら、交線と最近点の線を作り直す
pub fn sync_result_lines(
    check: Res<InterferenceCheck>,
    lines: Res<ResultLines>,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    mut shown_revision: Local<Option<u64>>,
) {
    if *shown_revision == Some(check.report.revision) {
        return;
    }
    *shown_revision = Some(check.report.revision);
    if let Some(mut asset) = gizmo_assets.get_mut(&lines.0) {
        *asset = result_lines(&check);
    }
}

fn result_lines(check: &InterferenceCheck) -> GizmoAsset {
    let mut gizmos = GizmoAsset::default();
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
    gizmos
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
