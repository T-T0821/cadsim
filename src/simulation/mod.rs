//! シミュレーション機能: 部品どうしの干渉チェックと距離計測
//!
//! 設計の詳細は `docs/simulation.md` を参照。ここでは全体の配線だけを行う。
//!
//! | ファイル | 役割 |
//! | --- | --- |
//! | `geometry.rs` | f64 の幾何計算 (姿勢・外接箱・三角形の交差と距離) |
//! | `mesh.rs` | 干渉チェック用メッシュ [`TriMesh`] と検索木 (BVH) |
//! | `interference.rs` | 部品の組ごとの判定 (干渉 / 接触 / 近接) |
//! | `overlap.rs` | 干渉部分の立体 (体積・表示用メッシュ) |
//! | `view.rs` | 判定結果の3D表示 |
//! | `measure.rs` | 2点間の距離計測ツール |
//! | `panel.rs` | 右側の「Simulation」パネル |
//!
//! 他の機能とのつなぎ目は [`SimBody`] コンポーネントだけ。部品のルートエンティティに
//! 付けておけば、このモジュールが勝手に拾って判定する。

use std::{sync::Arc, time::Instant};

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;
use truck_meshalgo::prelude::*;

pub mod geometry;
pub mod interference;
pub mod measure;
mod mesh;
mod overlap;
mod panel;
mod view;

pub use geometry::Pose;
pub use mesh::TriMesh;

use interference::{Body, CheckSettings, PairResult, PairStatus, check_all};

/// 干渉チェックの対象になる部品。部品のルートエンティティに付ける。
///
/// `mesh` は部品ローカル座標の形状、`pose` はローカル → ワールドの姿勢。
/// タイミングチャートで部品を動かすときは `pose` だけを書き換えれば、
/// メッシュや検索木を作り直さずに毎フレーム判定できる。
#[derive(Component, Clone)]
pub struct SimBody {
    pub id: u32,
    pub mesh: Arc<TriMesh>,
    pub pose: Pose,
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
    /// 判定するたびに1増える (表示側が作り直すかの目印)
    pub revision: u64,
}

impl InterferenceCheck {
    pub fn request(&mut self) {
        self.requested = true;
    }

    /// 部品が干渉に関わっているか (表示の色分けに使う)
    pub fn is_interfering(&self, id: u32) -> bool {
        self.report.pairs.iter().any(|pair| {
            matches!(pair.status, PairStatus::Interfering { .. }) && (pair.a == id || pair.b == id)
        })
    }
}

pub struct SimulationPlugin;

impl Plugin for SimulationPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(InterferenceCheck {
            auto: true,
            ..default()
        })
        .init_resource::<measure::MeasureTool>()
        .init_gizmo_group::<view::SimulationGizmos>()
        .add_systems(Startup, view::setup)
        .add_systems(
            Update,
            (
                measure::keyboard.run_if(not(bevy_egui::input::egui_wants_any_keyboard_input)),
                measure::pick_point
                    .run_if(not(bevy_egui::input::egui_wants_any_pointer_input))
                    .run_if(measure::is_measuring),
                run_interference_check,
                view::sync_overlap_meshes,
                view::draw_results,
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

/// 部品が増えた・減った・動いた・設定が変わったときに判定し直す。
fn run_interference_check(
    mut check: ResMut<InterferenceCheck>,
    bodies: Query<&SimBody>,
    changed: Query<(), Changed<SimBody>>,
    mut removed: RemovedComponents<SimBody>,
) {
    let geometry_changed = !changed.is_empty() || removed.read().count() > 0;
    if !(check.requested || (check.auto && geometry_changed)) {
        return;
    }
    let started = Instant::now();
    let inputs: Vec<Body> = bodies
        .iter()
        .map(|body| Body {
            id: body.id,
            mesh: &body.mesh,
            pose: body.pose,
        })
        .collect();
    let pairs = check_all(&inputs, &check.settings);
    let revision = check.report.revision + 1;
    check.report = Report {
        revision,
        pairs,
        body_count: inputs.len(),
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
    };
    check.requested = false;
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
