//! 人と AGV の動線のヒートマップの表示と書き出し (UI 層)
//!
//! 数え方と色付けは `plantwright_core::floor::heatmap` が決める。ここでは毎フレーム、
//! 作業者と AGV の位置を集めて渡し、結果の画像を床に敷いた板のテクスチャに写すだけにする。
//! PNG への書き出しは `plantwright_core::floor::heatmap_png`。

use bevy::asset::RenderAssetUsages;
use bevy::ecs::system::NonSendMarker;
use bevy::image::ImageSampler;
use bevy::math::DVec2;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use super::{AgvLayoutRes, FloorClock, FloorState, OperatorsRes, figures::FloorFigure};
use crate::file_io::ImportJobs;
use plantwright_core::floor::heatmap::{
    HeatImage, HeatMetric, MoverFilter, MoverId, TrafficHeatmap,
};
use plantwright_core::floor::heatmap_png;

/// 床からの高さ (mm)。経路の線 (3〜5 mm) より下に敷く
const OVERLAY_Y: f32 = 1.0;
/// 表示を描き直す間隔 (秒)。毎フレーム作り直すほど細かく変わらない
const REFRESH_SECS: f32 = 0.25;
/// 書き出す画像の長い辺 (画素)
const EXPORT_LONG_SIDE: u32 = 1600;

/// ヒートマップと、その表示の設定
#[derive(Resource, Default)]
pub struct HeatmapRes {
    pub map: TrafficHeatmap,
    /// 床に色を敷くか
    pub show: bool,
    pub filter: MoverFilter,
    pub metric: HeatMetric,
    /// 「画像で書き出す…」が押された
    pub export_requested: bool,
    /// 表示を描き直すまでの残り (秒)
    refresh_in: f32,
    /// 前に描いたときから記録・設定が変わったか
    dirty: bool,
}

impl HeatmapRes {
    /// 記録を消す
    pub fn clear(&mut self) {
        self.map.clear();
        self.dirty = true;
    }

    /// マス目の大きさを変える (記録は消える)
    pub fn set_cell(&mut self, cell: f64) {
        self.map.set_cell(cell);
        self.dirty = true;
    }

    /// 表示の設定が変わったので描き直す
    pub fn redraw(&mut self) {
        self.dirty = true;
        self.refresh_in = 0.0;
    }
}

/// 床に敷く板
#[derive(Component)]
pub struct HeatmapOverlay;

/// 作業者と AGV の位置を記録する。止めている間は時刻が進まないので数えない
pub fn record(
    clock: Res<FloorClock>,
    operators: Res<OperatorsRes>,
    state: Res<FloorState>,
    mut heat: ResMut<HeatmapRes>,
) {
    let mut positions: Vec<(MoverId, DVec2)> = operators
        .operators
        .iter()
        .map(|op| {
            (
                MoverId::person(op.id),
                super::operator_position(&state, op, clock.time),
            )
        })
        .collect();
    if let Some(sim) = state.agv_sim() {
        positions.extend(
            sim.vehicles
                .iter()
                .map(|v| (MoverId::agv(v.id as u32), v.pose(sim.graph()).0)),
        );
    }
    // 変更検知は表示のためだけに使うので、記録では知らせない
    let heat = heat.bypass_change_detection();
    let before = heat.map.recorded_seconds();
    let was_empty = heat.map.is_empty();
    heat.map.observe(clock.time, &positions);
    if heat.map.recorded_seconds() != before || heat.map.is_empty() != was_empty {
        heat.dirty = true;
    }
}

/// 板を 1枚作っておく (最初は隠す)
pub fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Name::new("動線ヒートマップ"),
        HeatmapOverlay,
        // 部品をドロップするときの当たり判定から外す
        FloorFigure,
        Mesh3d(meshes.add(unit_quad())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            alpha_mode: AlphaMode::Blend,
            double_sided: true,
            cull_mode: None,
            unlit: true,
            ..default()
        })),
        Transform::default(),
        Visibility::Hidden,
    ));
}

/// 1 × 1 の床の板。UV の (0, 0) が x・z の小さい角 (画像の左上 = 床の min)
fn unit_quad() -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [-0.5, 0.0, -0.5],
            [0.5, 0.0, -0.5],
            [0.5, 0.0, 0.5],
            [-0.5, 0.0, 0.5],
        ],
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 4])
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    )
    .with_inserted_indices(Indices::U32(vec![0, 3, 2, 0, 2, 1]))
}

fn texture_of(img: &HeatImage) -> Image {
    let mut texture = Image::new(
        Extent3d {
            width: img.width,
            height: img.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        img.rgba.clone(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    // なめらかに拡大して、マス目の境目をぼかす
    texture.sampler = ImageSampler::linear();
    texture
}

/// 記録や設定が変わったら、画像を作り直して板に貼る
pub fn sync_overlay(
    time: Res<Time>,
    mut heat: ResMut<HeatmapRes>,
    overlay: Single<
        (
            &mut Transform,
            &mut Visibility,
            &MeshMaterial3d<StandardMaterial>,
        ),
        With<HeatmapOverlay>,
    >,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let (mut transform, mut visibility, material) = overlay.into_inner();
    let heat = heat.bypass_change_detection();
    if !heat.show {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    }
    heat.refresh_in -= time.delta_secs();
    if !heat.dirty || heat.refresh_in > 0.0 {
        return;
    }
    heat.dirty = false;
    heat.refresh_in = REFRESH_SECS;
    let Some(img) = heat.map.render(heat.filter, heat.metric) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    let Some(mut material) = materials.get_mut(&material.0) else {
        return;
    };
    // 前の画像は、参照がなくなれば消える
    material.base_color_texture = Some(images.add(texture_of(&img)));
    let center = (img.min + img.max) / 2.0;
    let size = img.max - img.min;
    *transform = Transform::from_xyz(center.x as f32, OVERLAY_Y, center.y as f32)
        .with_scale(Vec3::new(size.x as f32, 1.0, size.y as f32));
    visibility.set_if_neq(Visibility::Visible);
}

/// 「画像で書き出す…」: 白地に AGV の通路と作業者の歩行経路を描き、色を重ねた PNG を保存する
pub fn export_if_requested(
    mut heat: ResMut<HeatmapRes>,
    operators: Res<OperatorsRes>,
    layout: Res<AgvLayoutRes>,
    state: Res<FloorState>,
    mut jobs: ResMut<ImportJobs>,
    _main_thread: NonSendMarker,
) {
    if !heat.export_requested {
        return;
    }
    heat.export_requested = false;
    let lines = guide_lines(&operators, &layout, &state);
    let Some(img) = heat
        .map
        .render_export(heat.filter, heat.metric, &lines, EXPORT_LONG_SIDE)
    else {
        jobs.messages
            .push("動線ヒートマップ: まだ記録がありません。再生してから書き出してください".into());
        return;
    };
    let Some(path) = rfd::FileDialog::new()
        .set_title("動線ヒートマップを書き出し")
        .add_filter("PNG 画像", &["png"])
        .set_file_name("動線ヒートマップ.png")
        .save_file()
    else {
        return;
    };
    let path = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
    {
        path
    } else {
        path.with_extension("png")
    };
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    match heatmap_png::write(&path, &img) {
        Ok(()) => jobs.messages.push(format!("{name} に書き出しました")),
        Err(e) => {
            error!("{e}");
            jobs.messages
                .push(format!("{name} に書き出せませんでした: {e}"));
        }
    }
}

/// 書き出す画像に描く線: AGV の通路と、作業者の歩行経路
fn guide_lines(
    operators: &OperatorsRes,
    layout: &AgvLayoutRes,
    state: &FloorState,
) -> Vec<(DVec2, DVec2)> {
    let graph = &layout.graph;
    let mut lines: Vec<_> = graph
        .edges
        .iter()
        .map(|&(a, b)| (graph.pos(a), graph.pos(b)))
        .collect();
    for op in &operators.operators {
        if let Some(plan) = state.plans.get(&op.id) {
            let path = plan.path(DVec2::from_array(op.start));
            lines.extend(path.windows(2).map(|w| (w[0], w[1])));
        }
    }
    lines
}
