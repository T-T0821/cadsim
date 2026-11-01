//! シミュレーション機能: 部品どうしの干渉チェックと距離計測
//!
//! 設計の詳細は `docs/simulation.md` を参照。ここでは全体の配線だけを行う。
//!
//! | ファイル | 役割 |
//! | --- | --- |
//! | `cadsim_core::simulation::geometry` | f64 の幾何計算 (姿勢・外接箱・三角形の交差と距離) |
//! | `cadsim_core::simulation::mesh` | 干渉チェック用メッシュ [`TriMesh`] と検索木 (BVH) |
//! | `cadsim_core::simulation::interference` | 部品の組ごとの判定 (干渉 / 接触 / 近接) |
//! | `cadsim_core::simulation::overlap` | 干渉部分の立体 (体積・表示用メッシュ) |
//! | `view.rs` | 判定結果の3D表示 |
//! | `measure.rs` | 2点間の距離計測ツール |
//! | `panel.rs` | 右側の「シミュレーション」パネル |
//!
//! 他の機能とのつなぎ目は [`SimBody`] コンポーネントだけ。部品のルートエンティティに
//! 付けておけば、このモジュールが勝手に拾って判定する。

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, block_on, poll_once},
};
use bevy_egui::EguiPrimaryContextPass;

use crate::view::camera::{keyboard_free, pointer_in_3d_view};
use truck_meshalgo::prelude::*;

pub use cadsim_core::simulation::{geometry, interference};
use cadsim_core::simulation::{mesh, overlap};
pub mod measure;
mod panel;
pub mod view;

pub use geometry::Pose;
pub use mesh::TriMesh;

use interference::{Body, CheckSettings, PairResult, PairStatus, check_all};
use overlap::{OverlapRegion, overlap_region};

/// 干渉チェックの対象になる部品。部品のルートエンティティに付ける。
///
/// `mesh` は部品ローカル座標の形状、`pose` はローカル → ワールドの姿勢。
/// タイミングチャートで部品を動かすときは `pose` だけを書き換えれば、
/// メッシュや検索木を作り直さずに毎フレーム判定できる。
#[derive(Component, Clone)]
pub struct SimBody {
    pub id: u32,
    /// 同じ値の部品どうしは調べない。標準部品の本体 (ロッドとチューブなど) は
    /// 組み付いた状態で重なっているので、部品全体の id をここに入れる。
    /// 「中の部品どうしの干渉を調べない」アセンブリの中ならそのアセンブリ。ふつうは `id` と同じ
    /// (`Model::interference_group`)。
    pub group: u32,
    pub mesh: Arc<TriMesh>,
    pub pose: Pose,
    /// モデル上で表示中か (非表示にした部品は調べない)。表示の都合で描かないだけの
    /// 部品 (画面上で小さい部品など) は表示中のままにする。
    pub shown: bool,
}

/// 干渉チェックの設定と、最新の結果
#[derive(Resource, Default)]
pub struct InterferenceCheck {
    pub settings: CheckSettings,
    /// 部品が変わるたびに自動で判定する (タイミングチャート再生中は毎フレーム)
    pub auto: bool,
    pub report: Report,
    /// 次のフレームで判定し直す
    requested: bool,
}

#[derive(Default)]
pub struct Report {
    pub pairs: Vec<PairResult>,
    pub body_count: usize,
    /// 判定にかかった時間 (ミリ秒)
    pub elapsed_ms: f64,
    /// 結果が変わるたびに1増える (表示側が作り直すかの目印)
    pub revision: u64,
    /// 干渉部分の立体を別スレッドで計算中
    pub overlap_pending: bool,
}

impl InterferenceCheck {
    pub fn request(&mut self) {
        self.requested = true;
    }

    /// 干渉に関わっている部品の id (表示の色分けに使う)
    pub fn interfering_parts(&self) -> HashSet<u32> {
        self.report
            .pairs
            .iter()
            .filter(|pair| matches!(pair.status, PairStatus::Interfering { .. }))
            .flat_map(|pair| [pair.a, pair.b])
            .collect()
    }
}

pub struct SimulationPlugin;

impl Plugin for SimulationPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(InterferenceCheck {
            auto: true,
            ..default()
        })
        .init_resource::<OverlapJob>()
        .init_resource::<measure::MeasureTool>()
        .init_gizmo_group::<view::SimulationGizmos>()
        .add_systems(Startup, view::setup)
        .add_systems(
            Update,
            (
                measure::keyboard.run_if(keyboard_free),
                measure::pick_point
                    .run_if(pointer_in_3d_view)
                    .run_if(measure::is_measuring),
                run_interference_check,
                receive_overlaps,
                view::sync_overlap_meshes,
                view::sync_result_lines,
                measure::draw,
            )
                .chain(),
        )
        .add_systems(
            EguiPrimaryContextPass,
            (panel::simulation_panel, measure::draw_labels),
        );
    }
}

/// 動いた・表示が切り替わった部品
type ChangedBody = Changed<SimBody>;

/// 部品が増えた・減った・動いた・表示が切り替わった・設定が変わったときに判定し直す。
///
/// 干渉の有無・交線・すき間はこのフレームで求める (毎フレームでも間に合う軽さ)。
/// 干渉部分の立体と体積は重いので別スレッドに回し、終わったら [`receive_overlaps`] が結果に書き込む。
fn run_interference_check(
    mut check: ResMut<InterferenceCheck>,
    mut job: ResMut<OverlapJob>,
    bodies: Query<&SimBody>,
    changed: Query<(), ChangedBody>,
    mut removed: RemovedComponents<SimBody>,
) {
    let geometry_changed = !changed.is_empty() || removed.read().count() > 0;
    if !(check.requested || (check.auto && geometry_changed)) {
        return;
    }
    let started = Instant::now();
    // 非表示にした部品は調べない
    let mut inputs: Vec<Body> = bodies
        .iter()
        .filter(|body| body.shown)
        .map(|body| Body {
            id: body.id,
            mesh: &body.mesh,
            pose: body.pose,
        })
        .collect();
    // 毎回同じ並びにして、一覧の順番と体積の近似を安定させる
    inputs.sort_by_key(|body| body.id);
    let settings = CheckSettings {
        compute_overlap: false,
        ..check.settings
    };
    let groups: HashMap<u32, u32> = bodies.iter().map(|b| (b.id, b.group)).collect();
    let mut pairs = check_all(&inputs, &settings);
    pairs.retain(|p| groups.get(&p.a) != groups.get(&p.b));
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

    // 前の計算が残っていれば捨てる (Task を落とすと止まる)
    job.0 = None;
    let interfering: Vec<(usize, u32, u32)> = pairs
        .iter()
        .enumerate()
        .filter(|(_, p)| matches!(p.status, PairStatus::Interfering { .. }))
        .map(|(index, p)| (index, p.a, p.b))
        .collect();
    let revision = check.report.revision + 1;
    let overlap_pending = check.settings.compute_overlap && !interfering.is_empty();
    if overlap_pending {
        let shapes: HashMap<u32, (Arc<TriMesh>, Pose)> = bodies
            .iter()
            .map(|b| (b.id, (b.mesh.clone(), b.pose)))
            .collect();
        let tolerance = check.settings.tolerance;
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let body = |id: u32| {
                let (mesh, pose) = &shapes[&id];
                Body {
                    id,
                    mesh,
                    pose: *pose,
                }
            };
            interfering
                .into_iter()
                .map(|(index, a, b)| (index, overlap_region(&body(a), &body(b), tolerance)))
                .collect()
        });
        job.0 = Some((revision, task));
    }
    check.report = Report {
        revision,
        pairs,
        body_count: inputs.len(),
        elapsed_ms,
        overlap_pending,
    };
    check.requested = false;
}

/// 別スレッドで計算中の干渉部分 (どの判定結果に対するものか, 計算)
#[derive(Resource, Default)]
struct OverlapJob(Option<(u64, Task<Vec<IndexedOverlap>>)>);

/// 判定結果の何番目の組の干渉部分か
type IndexedOverlap = (usize, OverlapRegion);

/// 干渉部分の計算が終わっていたら、判定結果に書き込む。
fn receive_overlaps(mut check: ResMut<InterferenceCheck>, mut job: ResMut<OverlapJob>) {
    let Some((revision, task)) = &mut job.0 else {
        return;
    };
    let Some(regions) = block_on(poll_once(task)) else {
        return; // まだ計算中
    };
    let revision = *revision;
    job.0 = None;
    if revision != check.report.revision {
        return; // その間に判定し直していた
    }
    for (index, region) in regions {
        if let Some(pair) = check.report.pairs.get_mut(index) {
            pair.status = PairStatus::Interfering {
                overlap: Some(region),
            };
        }
    }
    check.report.overlap_pending = false;
    check.report.revision += 1;
}

/// truck のテッセレーション結果 (面ごとの PolygonMesh) から干渉チェック用メッシュを作る。
/// 面の向きはそろえてから渡すこと (外向きが表)。
pub fn trimesh_from_polygons<'a>(polygons: impl IntoIterator<Item = &'a PolygonMesh>) -> TriMesh {
    let mut triangles = Vec::new();
    for polygon in polygons {
        let positions = polygon.positions();
        for tri in polygon.faces().triangle_iter() {
            triangles.push(tri.map(|v| {
                let p = positions[v.pos];
                bevy::math::DVec3::new(p.x, p.y, p.z)
            }));
        }
    }
    TriMesh::new(triangles)
}
