//! 干渉結果の CSV: タイミングチャートの1サイクルを時刻ごとに動かして干渉チェックし、
//! 「どの部品の組が、いつからいつまで、どのステップで、どれだけ」を区間にまとめる。
//!
//! 画面の干渉チェックはいま表示している時刻だけを判定するので、ここでは別に
//! サイクル全体を一定間隔で調べ直す。区間の開始・終了の精度はサンプリング間隔と同じ。

use std::{collections::HashMap, sync::Arc};

use bevy::math::DMat4;

use crate::model::NodeId;
use crate::motion::{RigidPose, Schedule, TimingChart, part_poses};
use crate::simulation::{
    Pose, TriMesh,
    interference::{Body, CheckSettings, PairStatus, check_all},
};

use super::csv::{Csv, seconds};

/// 調べる部品1つ
pub struct SweepBody {
    pub id: NodeId,
    pub name: String,
    /// 部品ローカル座標の形状
    pub mesh: Arc<TriMesh>,
    /// 設計上の配置 (ローカル → ワールド)
    pub placement: DMat4,
    /// ルートから自分までのノード。アクチュエータにはアセンブリも割り当てられるので、
    /// 自分にいちばん近いものの動きを使う (`SimulationPose::pose_of` と同じ決め方)。
    pub ancestors: Vec<NodeId>,
}

impl SweepBody {
    /// 動き (ノードごとのワールド座標での変換) を重ねた姿勢
    fn pose(&self, motion: &HashMap<NodeId, RigidPose>) -> Pose {
        let moved = self.ancestors.iter().rev().find_map(|id| motion.get(id));
        match moved {
            Some(m) => Pose::from_matrix(m.to_dmat4() * self.placement),
            None => Pose::from_matrix(self.placement),
        }
    }
}

/// 部品の組の状態のうち、区間として書き出すもの
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Interfering,
    Contact,
    Near,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Interfering => "干渉",
            Kind::Contact => "接触",
            Kind::Near => "近接",
        }
    }
}

/// 同じ部品の組が同じ状態だった時間帯
#[derive(Clone, Debug, PartialEq)]
struct Interval {
    pub a: u32,
    pub b: u32,
    pub kind: Kind,
    /// その状態だった最初と最後のサンプル時刻 (秒)
    pub start: f64,
    pub end: f64,
    /// 区間中で最大の干渉体積 (干渉で、体積を計算した場合のみ)
    pub max_volume: Option<f64>,
    /// 区間中で最小のクリアランス (近接のみ)
    pub min_clearance: Option<f64>,
}

/// サイクルを `interval` 秒ごとに区切った時刻。ステップの切り替わりとサイクルの終わりも必ず含める。
fn sample_times(schedule: &Schedule, interval: f64) -> Vec<f64> {
    let cycle = schedule.cycle_time;
    let mut times = vec![0.0];
    if cycle > 0.0 && interval > 0.0 {
        let count = (cycle / interval).ceil() as usize;
        times.extend((1..count).map(|i| i as f64 * interval));
        times.extend(schedule.steps.iter().map(|s| s.start));
        times.push(cycle);
    }
    times.sort_by(f64::total_cmp);
    // ほぼ同じ時刻は1つにまとめる
    times.dedup_by(|b, a| (*b - *a).abs() < 1e-9);
    times
}

/// サイクル全体を調べて、状態の区間を時刻順に返す
fn sweep(
    bodies: &[SweepBody],
    chart: &TimingChart,
    schedule: &Schedule,
    settings: &CheckSettings,
    interval: f64,
) -> Vec<Interval> {
    let mut finished = Vec::new();
    // 組 (a, b) ごとに、いま続いている区間
    let mut open: HashMap<(u32, u32), Interval> = HashMap::new();

    for t in sample_times(schedule, interval) {
        let poses = part_poses(chart, schedule, t);
        let inputs: Vec<Body> = bodies
            .iter()
            .map(|body| Body {
                id: body.id,
                mesh: &body.mesh,
                pose: body.pose(&poses),
            })
            .collect();

        let mut seen = HashMap::new();
        for pair in check_all(&inputs, settings) {
            let (kind, volume, clearance) = match pair.status {
                PairStatus::Interfering { overlap } => {
                    (Kind::Interfering, overlap.map(|o| o.volume), None)
                }
                PairStatus::Contact => (Kind::Contact, None, None),
                PairStatus::Near { clearance } => (Kind::Near, None, Some(clearance)),
            };
            seen.insert((pair.a, pair.b), (kind, volume, clearance));
        }

        // 状態が変わった・なくなった組の区間を閉じる
        open.retain(|key, interval| {
            let continues = seen
                .get(key)
                .is_some_and(|(kind, ..)| *kind == interval.kind);
            if !continues {
                finished.push(interval.clone());
            }
            continues
        });

        for (key, (kind, volume, clearance)) in seen {
            let interval = open.entry(key).or_insert_with(|| Interval {
                a: key.0,
                b: key.1,
                kind,
                start: t,
                end: t,
                max_volume: None,
                min_clearance: None,
            });
            interval.end = t;
            interval.max_volume = max_option(interval.max_volume, volume, f64::max);
            interval.min_clearance = max_option(interval.min_clearance, clearance, f64::min);
        }
    }

    finished.extend(open.into_values());
    finished.sort_by(|x, y| {
        x.start
            .total_cmp(&y.start)
            .then(x.a.cmp(&y.a))
            .then(x.b.cmp(&y.b))
    });
    finished
}

/// 片方だけ値があればそれを、両方あれば `pick` で選んだ方を返す
fn max_option(a: Option<f64>, b: Option<f64>, pick: fn(f64, f64) -> f64) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(pick(a, b)),
        (a, b) => a.or(b),
    }
}

const HEADER: [&str; 13] = [
    "判定",
    "部品A ID",
    "部品A",
    "部品B ID",
    "部品B",
    "開始[s]",
    "終了[s]",
    "継続時間[s]",
    "開始ステップ",
    "開始ステップのコメント",
    "終了ステップ",
    "最大干渉体積",
    "最小クリアランス",
];

/// 1サイクル分を `interval` 秒ごとに調べ、干渉・接触・近接の時間帯を CSV にする
pub fn interference_csv(
    bodies: &[SweepBody],
    chart: &TimingChart,
    schedule: &Schedule,
    settings: &CheckSettings,
    interval: f64,
) -> String {
    intervals_csv(
        &sweep(bodies, chart, schedule, settings, interval),
        bodies,
        chart,
        schedule,
    )
}

/// 区間の一覧を CSV にする。ステップ番号は1始まり (チャートの表示と同じ)。
fn intervals_csv(
    intervals: &[Interval],
    bodies: &[SweepBody],
    chart: &TimingChart,
    schedule: &Schedule,
) -> String {
    let name = |id: u32| {
        bodies
            .iter()
            .find(|b| b.id == id)
            .map_or_else(|| format!("#{id}"), |b| b.name.clone())
    };
    let step_number = |t: f64| schedule.step_at(t).map(|i| i + 1);
    let mut csv = Csv::with_header(&HEADER);
    for interval in intervals {
        let start_step = step_number(interval.start);
        csv.row([
            interval.kind.label().to_string(),
            interval.a.to_string(),
            name(interval.a),
            interval.b.to_string(),
            name(interval.b),
            seconds(interval.start),
            seconds(interval.end),
            seconds(interval.end - interval.start),
            start_step.map_or_else(String::new, |n| n.to_string()),
            start_step
                .and_then(|n| chart.steps.get(n - 1))
                .map_or_else(String::new, |s| s.comment.clone()),
            step_number(interval.end).map_or_else(String::new, |n| n.to_string()),
            interval
                .max_volume
                .map_or_else(String::new, |v| format!("{v:.6}")),
            interval
                .min_clearance
                .map_or_else(String::new, |c| format!("{c:.6}")),
        ]);
    }
    csv.into_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::chart::{Action, ActuatorKind, Axis, Easing, Step};
    use bevy::math::DVec3;

    /// テスト用: 辺の長さ `size` の立方体を `offset` だけずらしたメッシュ
    fn cube_mesh(offset: DVec3, size: f64) -> TriMesh {
        let corner =
            |x: usize, y: usize, z: usize| offset + DVec3::new(x as f64, y as f64, z as f64) * size;
        // 面ごとに外向き (反時計回り) の4頂点
        let faces = [
            [(0, 0, 0), (0, 1, 0), (1, 1, 0), (1, 0, 0)],
            [(0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)],
            [(0, 0, 0), (1, 0, 0), (1, 0, 1), (0, 0, 1)],
            [(0, 1, 0), (0, 1, 1), (1, 1, 1), (1, 1, 0)],
            [(0, 0, 0), (0, 0, 1), (0, 1, 1), (0, 1, 0)],
            [(1, 0, 0), (1, 1, 0), (1, 1, 1), (1, 0, 1)],
        ];
        let mut triangles = Vec::new();
        for face in faces {
            let [a, b, c, d] = face.map(|(x, y, z)| corner(x, y, z));
            triangles.push([a, b, c]);
            triangles.push([a, c, d]);
        }
        TriMesh::new(triangles)
    }

    /// 部品1 (アセンブリ 10 ごと) を X 方向へ2秒かけて 2 動かし、2秒かけて戻す。部品2は X=1.5 に置いてある。
    fn pusher() -> (Vec<SweepBody>, TimingChart) {
        let bodies = vec![
            SweepBody {
                id: 1,
                name: "プッシャー".into(),
                mesh: Arc::new(cube_mesh(DVec3::ZERO, 1.0)),
                placement: DMat4::IDENTITY,
                // アセンブリ 10 の中の部品。アクチュエータはアセンブリに割り当てる
                ancestors: vec![10, 1],
            },
            SweepBody {
                id: 2,
                name: "ストッパー".into(),
                // ローカル原点に作った箱を、配置で X = 1.5 へ置く
                mesh: Arc::new(cube_mesh(DVec3::ZERO, 1.0)),
                placement: DMat4::from_translation(DVec3::new(1.5, 0.0, 0.0)),
                ancestors: vec![2],
            },
        ];
        let mut chart = TimingChart::default();
        let axis = chart.add_actuator(ActuatorKind::LinearAxis);
        let actuator = chart.actuator_mut(axis).unwrap();
        actuator.axis = Axis::X;
        actuator.easing = Easing::Linear;
        actuator.parts = vec![10];
        for (comment, target) in [("前進", 2.0), ("後退", 0.0)] {
            chart.steps.push(Step {
                comment: comment.into(),
                actions: vec![Action {
                    actuator: axis,
                    target,
                    duration: 2.0,
                }],
                dwell: 0.0,
            });
        }
        (bodies, chart)
    }

    #[test]
    fn sample_times_include_step_boundaries() {
        let (_, chart) = pusher();
        let times = sample_times(&chart.schedule(), 1.5);
        assert_eq!(times, vec![0.0, 1.5, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn pusher_interferes_while_it_is_past_the_stopper() {
        let (bodies, chart) = pusher();
        let schedule = chart.schedule();
        let settings = CheckSettings {
            clearance_warning: 0.0,
            ..Default::default()
        };
        let intervals = sweep(&bodies, &chart, &schedule, &settings, 0.1);
        let interfering: Vec<&Interval> = intervals
            .iter()
            .filter(|i| i.kind == Kind::Interfering)
            .collect();
        assert_eq!(interfering.len(), 1);
        // プッシャーの先端 (x = 1 + 移動量) がストッパー (x = 1.5) を越えている間: 0.5 < t < 3.5
        let hit = interfering[0];
        assert!((hit.start - 0.6).abs() < 1e-9, "start = {}", hit.start);
        assert!((hit.end - 3.4).abs() < 1e-9, "end = {}", hit.end);
        // 最も食い込むのは t = 2 (重なり 1 x 1 x 1)
        let volume = hit.max_volume.expect("体積を計算している");
        assert!((volume - 1.0).abs() < 0.05, "volume = {volume}");

        let text = intervals_csv(&intervals, &bodies, &chart, &schedule);
        let row = text
            .lines()
            .find(|l| l.starts_with("干渉"))
            .expect("干渉の行がある");
        assert!(
            row.starts_with("干渉,1,プッシャー,2,ストッパー,0.600,3.400,2.800,1,前進,2,"),
            "{row}"
        );
    }

    #[test]
    fn static_layout_is_checked_once_without_a_chart() {
        let (bodies, _) = pusher();
        let chart = TimingChart::default();
        let settings = CheckSettings::default();
        let intervals = sweep(&bodies, &chart, &chart.schedule(), &settings, 0.1);
        // 0.5 離れているので、警告値 0.1 では何も出ない
        assert!(intervals.is_empty());
    }
}
