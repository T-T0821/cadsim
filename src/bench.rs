//! 描画の速さを測る: `cargo run --release -- --stress 5000 --bench 10`
//!
//! 読み込みが終わってから少し待ち、カメラを一定の速さで回しながらフレーム時間を集める。
//! 指定の秒数が経ったら平均 FPS などを標準出力に1行で出してアプリを終了する。
//! 最適化の前後を同じ条件で比べるためのもの。

use bevy::prelude::*;

use crate::file_io::{CommandLine, ImportJobs};
use crate::model::Model;
use crate::view::camera::OrbitCamera;

/// 読み込み後、カメラの「全体を表示」アニメーションが落ち着くまで待つ時間 (秒)
const WARM_UP_SECS: f32 = 2.0;

pub struct BenchPlugin;

impl Plugin for BenchPlugin {
    fn build(&self, app: &mut App) {
        if let Some(secs) = CommandLine::parse().bench_secs {
            app.insert_resource(Bench {
                duration: secs,
                ..default()
            })
            .add_systems(Update, run_bench);
        }
    }
}

#[derive(Resource, Default)]
struct Bench {
    duration: f32,
    /// 読み込みが終わってからの時間
    elapsed: f32,
    frame_times: Vec<f32>,
}

fn run_bench(
    time: Res<Time<Real>>,
    jobs: Res<ImportJobs>,
    model: Res<Model>,
    mut bench: ResMut<Bench>,
    mut orbit: ResMut<OrbitCamera>,
    mut exit: MessageWriter<AppExit>,
) {
    if !jobs.loading.is_empty() || model.is_empty() {
        return;
    }
    let dt = time.delta_secs();
    bench.elapsed += dt;
    if bench.elapsed < WARM_UP_SECS {
        return;
    }
    // 1周 約 10 秒の速さで回す
    orbit.yaw += 0.6 * dt;
    bench.frame_times.push(dt);
    if bench.elapsed < WARM_UP_SECS + bench.duration {
        return;
    }

    let mut times = bench.frame_times.clone();
    times.sort_by(f32::total_cmp);
    let frames = times.len();
    let total: f32 = times.iter().sum();
    let p95 = times[(frames * 95 / 100).min(frames - 1)];
    let parts = model
        .roots
        .iter()
        .map(|&r| model.measure(r).part_count)
        .sum::<usize>();
    println!(
        "BENCH parts={parts} frames={frames} avg_fps={:.1} avg_ms={:.2} p95_ms={:.2} max_ms={:.2}",
        frames as f32 / total,
        total / frames as f32 * 1000.0,
        p95 * 1000.0,
        times[frames - 1] * 1000.0,
    );
    exit.write(AppExit::Success);
}
