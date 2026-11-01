//! 作業者と AGV の動きを「アニメーションのクリップ」と「混ぜ方 (重み)」に分けて決める
//!
//! 表示側 (Bevy) はクリップを 1回だけ作り、毎フレーム「どのクリップを、どの時刻で、どの重みで」
//! 再生するかだけをここに聞く。重みは時刻だけから決まるので、巻き戻しても同じ姿勢になる。
//!
//! - 作業者: 歩く・作業する・待つ の 3本。区間が切り替わると、前の動きから次の動きへ
//!   [`BLEND_TIME`] 秒かけて重みを移す (急に姿勢が変わらない)。
//! - AGV: 車輪が回る (走った距離で進める) と、荷が上下する (積み込み・荷降ろしの進み具合で進める) の 2本。
//!
//! 形の並びは [`super::figure`] と同じ。クリップの `parts[i]` は形 `i` の動き。
//! Bevy に依存しない純粋な計算だけを置く。

use std::f64::consts::TAU;

use glam::{DQuat, DVec2, DVec3};

use super::agv_sim::Phase;
use super::figure::{
    AgvPose, AgvStatus, FigurePart, PersonPose, STRIDE, WHEEL_RADIUS, agv_local_parts,
    person_local_parts,
};
use super::operator_plan::{Activity, OperatorPlan};

/// 動きの切り替えにかける時間 (秒)
pub const BLEND_TIME: f64 = 0.35;
/// 作業中の手元の動き 1回ぶんの時間 (秒)
pub const WORK_PERIOD: f64 = 1.0 / 1.6;
/// 歩くクリップの長さ。クリップの時刻 = 歩幅の位相 (0〜1)
pub const WALK_CLIP_LENGTH: f64 = 1.0;
/// 荷を積み込むときに、どれだけ上から降ろすか (mm)
pub const CARGO_LIFT: f64 = 260.0;

/// 1つの形の、キーごとの置き方
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartKeys {
    pub translations: Vec<DVec3>,
    pub rotations: Vec<DQuat>,
    pub scales: Vec<DVec3>,
}

/// 形ごとの動きをキーで並べたもの。`times` はクリップの中の時刻 (0〜`duration`)
#[derive(Clone, Debug, PartialEq)]
pub struct FigureClip {
    pub duration: f64,
    pub times: Vec<f64>,
    /// 形の並びと同じ。動かない形は None (そのクリップでは置き方を変えない)
    pub parts: Vec<Option<PartKeys>>,
}

impl FigureClip {
    /// 時刻を `[0, duration)` に折り返す (クリップは繰り返す)
    pub fn wrap(&self, t: f64) -> f64 {
        if self.duration <= 0.0 {
            0.0
        } else {
            t.rem_euclid(self.duration)
        }
    }

    /// `t` から `duration` までを `samples` 等分して形を並べる。`keep_still` が false なら、
    /// どのキーでも置き方が同じ形は None にする
    fn sample(
        duration: f64,
        samples: usize,
        keep_still: bool,
        parts_at: impl Fn(f64) -> Vec<FigurePart>,
    ) -> Self {
        let samples = samples.max(1);
        let times: Vec<f64> = (0..=samples)
            .map(|i| duration * i as f64 / samples as f64)
            .collect();
        let frames: Vec<Vec<FigurePart>> = times.iter().map(|&t| parts_at(t)).collect();
        let count = frames[0].len();
        let parts = (0..count)
            .map(|i| {
                let first = &frames[0][i];
                let still = frames.iter().all(|f| {
                    let p = &f[i];
                    p.translation.abs_diff_eq(first.translation, 1e-9)
                        && p.rotation.abs_diff_eq(first.rotation, 1e-9)
                        && p.scale.abs_diff_eq(first.scale, 1e-9)
                });
                if still && !keep_still {
                    return None;
                }
                let mut keys = PartKeys::default();
                let mut previous: Option<DQuat> = None;
                for f in &frames {
                    let p = &f[i];
                    keys.translations.push(p.translation);
                    // 隣のキーと同じ半球にそろえ、補間が遠回りしないようにする
                    let mut q = p.rotation;
                    if previous.is_some_and(|prev| prev.dot(q) < 0.0) {
                        q = -q;
                    }
                    previous = Some(q);
                    keys.rotations.push(q);
                    keys.scales.push(p.scale);
                }
                Some(keys)
            })
            .collect();
        Self {
            duration,
            times,
            parts,
        }
    }
}

// ---------------------------------------------------------------------------
// 作業者

/// 作業者のクリップの順番 (表示側はこの順で並べる)
pub const PERSON_ACTIVITIES: [Activity; 3] = [Activity::Walk, Activity::Work, Activity::Wait];

fn person_at(activity: Activity, walked: f64, time: f64) -> Vec<FigurePart> {
    person_local_parts(&PersonPose {
        position: DVec2::ZERO,
        heading: DVec2::X,
        activity,
        walked,
        time,
        selected: false,
    })
}

/// 作業者の 1つの動きのクリップ。混ぜるので、動かない形も含めてすべての形を持つ
pub fn person_clip(activity: Activity) -> FigureClip {
    match activity {
        Activity::Walk => FigureClip::sample(WALK_CLIP_LENGTH, 32, true, |t| {
            person_at(Activity::Walk, t / WALK_CLIP_LENGTH * STRIDE, 0.0)
        }),
        Activity::Work => {
            FigureClip::sample(WORK_PERIOD, 16, true, |t| person_at(Activity::Work, 0.0, t))
        }
        Activity::Wait => FigureClip::sample(1.0, 1, true, |_| person_at(Activity::Wait, 0.0, 0.0)),
    }
}

/// 1つの動きの、いまの重みとクリップの中の時刻
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipWeight {
    pub activity: Activity,
    pub weight: f64,
    pub time: f64,
}

/// クリップの中の時刻。歩くのは歩いた距離で、作業は時刻で進める
pub fn person_clip_time(activity: Activity, walked: f64, time: f64) -> f64 {
    match activity {
        Activity::Walk => (walked / STRIDE).rem_euclid(1.0) * WALK_CLIP_LENGTH,
        Activity::Work => time.rem_euclid(WORK_PERIOD),
        Activity::Wait => 0.0,
    }
}

fn smoothstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// 時刻 `t` での 3つの動きの重み ([`PERSON_ACTIVITIES`] の順、合計 1)。
/// 区間の始まりから [`BLEND_TIME`] 秒は、前の区間の動きと混ぜる。計画がなければ待つ。
pub fn person_weights(plan: Option<&OperatorPlan>, t: f64) -> [ClipWeight; 3] {
    let walked = plan.map_or(0.0, |p| p.walked_at(t));
    let mut weights = PERSON_ACTIVITIES.map(|activity| ClipWeight {
        activity,
        weight: 0.0,
        time: person_clip_time(activity, walked, t),
    });
    let mut add = |activity: Activity, w: f64| {
        if let Some(c) = weights.iter_mut().find(|c| c.activity == activity) {
            c.weight += w;
        }
    };
    let Some((index, segment)) = plan.and_then(|p| {
        let ct = p.cycle_time_of(t);
        p.segments.iter().enumerate().find(|(_, s)| ct < s.end).or(p
            .segments
            .iter()
            .enumerate()
            .next_back())
    }) else {
        add(Activity::Wait, 1.0);
        return weights;
    };
    let plan = plan.expect("区間があれば計画もある");
    let elapsed = plan.cycle_time_of(t) - segment.start;
    // 前の区間 (先頭ならサイクルの最後の区間。最初のサイクルの先頭は混ぜない)
    let previous = if index > 0 {
        plan.segments.get(index - 1)
    } else if t >= plan.cycle_time && plan.cycle_time > 0.0 {
        plan.segments.last()
    } else {
        None
    };
    match previous {
        Some(prev) if prev.activity != segment.activity && elapsed < BLEND_TIME => {
            let w = smoothstep(elapsed / BLEND_TIME);
            add(segment.activity, w);
            add(prev.activity, 1.0 - w);
        }
        _ => add(segment.activity, 1.0),
    }
    weights
}

// ---------------------------------------------------------------------------
// AGV

/// AGV のクリップを作り直すかどうかを決める寸法 (mm)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgvDims {
    pub length: f64,
    pub width: f64,
    pub height: f64,
}

/// 姿勢のうち、クリップの外で決まるもの (状態・荷・時刻) を入れた AGV の姿勢
pub fn agv_rest_pose(dims: AgvDims) -> AgvPose {
    AgvPose {
        position: DVec2::ZERO,
        heading: DVec2::X,
        length: dims.length,
        width: dims.width,
        height: dims.height,
        status: AgvStatus::Idle,
        loaded: true,
        cargo_lift: 0.0,
        distance: 0.0,
        time: 0.0,
    }
}

/// 車輪 (ハブ) が 1回転するクリップ。クリップの時刻 0〜1 が 1回転
pub fn agv_roll_clip(dims: AgvDims) -> FigureClip {
    FigureClip::sample(1.0, 16, false, |u| {
        agv_local_parts(&AgvPose {
            distance: u * TAU * WHEEL_RADIUS,
            ..agv_rest_pose(dims)
        })
    })
}

/// 荷が上から天板に降りるクリップ。クリップの時刻 0 で [`CARGO_LIFT`] だけ上、1 で天板の上
pub fn agv_cargo_clip(dims: AgvDims) -> FigureClip {
    FigureClip::sample(1.0, 4, false, |u| {
        agv_local_parts(&AgvPose {
            cargo_lift: (1.0 - u) * CARGO_LIFT,
            ..agv_rest_pose(dims)
        })
    })
}

/// 車輪のクリップの時刻 (走った距離から)
pub fn agv_roll_time(distance: f64) -> f64 {
    (distance / (TAU * WHEEL_RADIUS)).rem_euclid(1.0)
}

/// 荷のクリップの時刻 = 荷がどれだけ載っているか (0: なし、1: 天板の上)。
/// 積み込み中は 0 → 1、荷降ろし中は 1 → 0 へ進む。`remaining` は作業の残り時間、`total` は作業の長さ
pub fn agv_cargo_amount(phase: Phase, remaining: f64, total: f64) -> f64 {
    let done = if total > 0.0 {
        (1.0 - remaining / total).clamp(0.0, 1.0)
    } else {
        1.0
    };
    match phase {
        Phase::Idle | Phase::ToPickup => 0.0,
        Phase::ToDrop => 1.0,
        // 前半は待って、後半で降ろす (載せる) と、荷を扱っているように見える
        Phase::Loading => smoothstep(done * 2.0 - 1.0),
        Phase::Unloading => 1.0 - smoothstep(done * 2.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::floor::operator_plan::PlanSegment;

    fn assert_parts_match(keys: &PartKeys, k: usize, parts: &[FigurePart], i: usize) {
        let p = &parts[i];
        assert!(keys.translations[k].abs_diff_eq(p.translation, 1e-6));
        assert!(
            keys.rotations[k].abs_diff_eq(p.rotation, 1e-6)
                || keys.rotations[k].abs_diff_eq(-p.rotation, 1e-6)
        );
        assert!(keys.scales[k].abs_diff_eq(p.scale, 1e-6));
    }

    #[test]
    fn walk_clip_follows_the_figure_and_loops() {
        let clip = person_clip(Activity::Walk);
        let rest = person_at(Activity::Walk, 0.0, 0.0);
        assert_eq!(clip.parts.len(), rest.len());
        assert!(clip.parts.iter().all(Option::is_some));
        // 4分の1周のキーは、その位相の形と同じ
        let k = clip.times.len() / 4;
        let at = person_at(Activity::Walk, clip.times[k] * STRIDE, 0.0);
        for (i, keys) in clip.parts.iter().enumerate() {
            let keys = keys.as_ref().unwrap();
            assert_parts_match(keys, k, &at, i);
            // 最後のキーは最初のキーと同じ (繰り返してもつながる)
            let last = keys.translations.len() - 1;
            assert!(keys.translations[0].abs_diff_eq(keys.translations[last], 1e-6));
        }
    }

    #[test]
    fn rotations_stay_on_one_hemisphere_between_keys() {
        for activity in PERSON_ACTIVITIES {
            let clip = person_clip(activity);
            for keys in clip.parts.iter().flatten() {
                for pair in keys.rotations.windows(2) {
                    assert!(pair[0].dot(pair[1]) >= 0.0);
                }
            }
        }
    }

    #[test]
    fn clip_time_follows_walked_distance_and_time() {
        assert!((person_clip_time(Activity::Walk, STRIDE * 2.25, 9.0) - 0.25).abs() < 1e-9);
        assert!(
            (person_clip_time(Activity::Work, 0.0, WORK_PERIOD * 3.0 + 0.1) - 0.1).abs() < 1e-9
        );
        assert_eq!(person_clip_time(Activity::Wait, 500.0, 3.0), 0.0);
    }

    fn plan() -> OperatorPlan {
        let seg = |activity, start, end| PlanSegment {
            task: Some(0),
            activity,
            start,
            end,
            from: DVec2::ZERO,
            to: DVec2::ZERO,
        };
        OperatorPlan {
            segments: vec![
                seg(Activity::Walk, 0.0, 2.0),
                seg(Activity::Work, 2.0, 5.0),
                seg(Activity::Work, 5.0, 6.0),
                seg(Activity::Wait, 6.0, 8.0),
            ],
            cycle_time: 8.0,
            ..Default::default()
        }
    }

    fn weight(w: &[ClipWeight; 3], activity: Activity) -> f64 {
        w.iter().find(|c| c.activity == activity).unwrap().weight
    }

    #[test]
    fn weights_blend_from_the_previous_activity() {
        let plan = plan();
        let total = |w: &[ClipWeight; 3]| w.iter().map(|c| c.weight).sum::<f64>();
        // 区間の中ほどは、その動きだけ
        let w = person_weights(Some(&plan), 1.0);
        assert_eq!(weight(&w, Activity::Walk), 1.0);
        // 歩き → 作業 の切り替え直後は混ざり、作業の重みが増えていく
        let a = person_weights(Some(&plan), 2.0 + BLEND_TIME * 0.25);
        let b = person_weights(Some(&plan), 2.0 + BLEND_TIME * 0.75);
        assert!((total(&a) - 1.0).abs() < 1e-9);
        assert!(weight(&a, Activity::Walk) > 0.0 && weight(&a, Activity::Work) > 0.0);
        assert!(weight(&b, Activity::Work) > weight(&a, Activity::Work));
        // 同じ動きが続く区間の境目では混ぜない
        let same = person_weights(Some(&plan), 5.0 + 0.01);
        assert_eq!(weight(&same, Activity::Work), 1.0);
        // 2周目の先頭は、前の周の最後 (待ち) から混ぜる。最初の周の先頭は混ぜない
        let first = person_weights(Some(&plan), 0.01);
        assert_eq!(weight(&first, Activity::Walk), 1.0);
        let second = person_weights(Some(&plan), 8.0 + 0.01);
        assert!(weight(&second, Activity::Wait) > 0.9);
        assert!(weight(&second, Activity::Walk) > 0.0);
    }

    #[test]
    fn no_plan_means_waiting() {
        let w = person_weights(None, 3.0);
        assert_eq!(weight(&w, Activity::Wait), 1.0);
    }

    const DIMS: AgvDims = AgvDims {
        length: 1200.0,
        width: 800.0,
        height: 400.0,
    };

    #[test]
    fn roll_clip_moves_only_the_hubs() {
        let clip = agv_roll_clip(DIMS);
        let moving = clip.parts.iter().filter(|p| p.is_some()).count();
        assert_eq!(moving, 2, "左右のハブだけが回る");
        let quarter = agv_local_parts(&AgvPose {
            distance: 0.25 * TAU * WHEEL_RADIUS,
            ..agv_rest_pose(DIMS)
        });
        let k = clip.times.len() / 4;
        for (i, keys) in clip.parts.iter().enumerate() {
            if let Some(keys) = keys {
                assert_parts_match(keys, k, &quarter, i);
            }
        }
    }

    #[test]
    fn cargo_clip_lowers_the_cargo_onto_the_deck() {
        let clip = agv_cargo_clip(DIMS);
        let (index, keys) = clip
            .parts
            .iter()
            .enumerate()
            .find_map(|(i, k)| k.as_ref().map(|k| (i, k)))
            .expect("荷の形が動く");
        assert_eq!(clip.parts.iter().flatten().count(), 1);
        assert_eq!(index, agv_local_parts(&agv_rest_pose(DIMS)).len() - 1);
        let first = keys.translations[0].y;
        let last = *keys.translations.last().unwrap();
        assert!((first - last.y - CARGO_LIFT).abs() < 1e-6);
    }

    #[test]
    fn cargo_amount_follows_the_station_work() {
        assert_eq!(agv_cargo_amount(Phase::ToPickup, 0.0, 10.0), 0.0);
        assert_eq!(agv_cargo_amount(Phase::ToDrop, 0.0, 10.0), 1.0);
        assert_eq!(agv_cargo_amount(Phase::Loading, 10.0, 10.0), 0.0);
        assert_eq!(agv_cargo_amount(Phase::Loading, 0.0, 10.0), 1.0);
        assert_eq!(agv_cargo_amount(Phase::Unloading, 10.0, 10.0), 1.0);
        assert_eq!(agv_cargo_amount(Phase::Unloading, 0.0, 10.0), 0.0);
        let mid = agv_cargo_amount(Phase::Loading, 2.5, 10.0);
        assert!(mid > 0.0 && mid < 1.0);
        assert!((agv_roll_time(TAU * WHEEL_RADIUS * 3.5) - 0.5).abs() < 1e-9);
    }
}
