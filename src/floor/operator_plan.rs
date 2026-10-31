//! 作業者の作業手順と、その時間・歩行距離の計算
//!
//! 作業者は床の上を直線で歩き、決められた場所でピックやボタン操作をする。
//! 手順 (歩く・ピック・ボタン・待つ) の並びから、各作業の開始・終了時刻、
//! 1サイクルの作業時間の内訳 (歩行・作業・待ち) と歩行距離を求める。
//!
//! Bevy に依存しない純粋なデータと計算だけを置く。長さは mm、時間は秒。

use bevy::math::DVec2;

/// 作業者の手順 1つ
#[derive(Clone, Debug, PartialEq)]
pub enum OperatorTask {
    /// 床の点 (x, z) まで歩く
    Walk { to: [f64; 2] },
    /// その場でワークを取る・置く
    Pick { label: String, duration: f64 },
    /// その場でボタンを押す
    Button { label: String, duration: f64 },
    /// その場で待つ
    Wait { duration: f64 },
}

impl OperatorTask {
    pub fn label(&self) -> String {
        match self {
            OperatorTask::Walk { to } => format!("歩く → ({:.0}, {:.0})", to[0], to[1]),
            OperatorTask::Pick { label, .. } => format!("ピック: {label}"),
            OperatorTask::Button { label, .. } => format!("ボタン: {label}"),
            OperatorTask::Wait { .. } => "待つ".into(),
        }
    }
}

/// 作業者 1人
#[derive(Clone, Debug, PartialEq)]
pub struct Operator {
    pub id: u32,
    pub name: String,
    /// サイクルの開始位置 (x, z)
    pub start: [f64; 2],
    /// 歩く速さ (mm/s)
    pub walk_speed: f64,
    pub tasks: Vec<OperatorTask>,
}

impl Operator {
    pub fn new(id: u32, start: [f64; 2]) -> Self {
        Self {
            id,
            name: format!("作業者 {id}"),
            start,
            // 工場の通路を歩く一般的な速さ (およそ 1 m/s)
            walk_speed: 1000.0,
            tasks: Vec::new(),
        }
    }
}

/// 作業の種類 (時間の内訳の分類)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Walk,
    Work,
    Wait,
}

/// 手順を時刻に展開した 1区間
#[derive(Clone, Debug, PartialEq)]
pub struct PlanSegment {
    /// 手順の番号 (0 始まり)。サイクルの最後に開始位置へ戻る歩行は None。
    pub task: Option<usize>,
    pub activity: Activity,
    pub start: f64,
    pub end: f64,
    pub from: DVec2,
    pub to: DVec2,
}

/// 1サイクル分の計画と集計
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OperatorPlan {
    pub segments: Vec<PlanSegment>,
    pub cycle_time: f64,
    pub walk_time: f64,
    pub work_time: f64,
    pub wait_time: f64,
    pub walk_distance: f64,
}

impl OperatorPlan {
    /// 手順を時刻に展開する。最後に開始位置まで歩いて戻り、そこで1サイクルとする。
    pub fn new(operator: &Operator) -> Self {
        let speed = operator.walk_speed.max(1.0);
        let start = DVec2::from_array(operator.start);
        let mut plan = OperatorPlan::default();
        let mut time = 0.0;
        let mut at = start;
        let mut push = |plan: &mut OperatorPlan,
                        task: Option<usize>,
                        activity: Activity,
                        duration: f64,
                        from: DVec2,
                        to: DVec2| {
            let duration = duration.max(0.0);
            plan.segments.push(PlanSegment {
                task,
                activity,
                start: time,
                end: time + duration,
                from,
                to,
            });
            match activity {
                Activity::Walk => {
                    plan.walk_time += duration;
                    plan.walk_distance += from.distance(to);
                }
                Activity::Work => plan.work_time += duration,
                Activity::Wait => plan.wait_time += duration,
            }
            time += duration;
        };
        for (index, task) in operator.tasks.iter().enumerate() {
            match task {
                OperatorTask::Walk { to } => {
                    let to = DVec2::from_array(*to);
                    push(
                        &mut plan,
                        Some(index),
                        Activity::Walk,
                        at.distance(to) / speed,
                        at,
                        to,
                    );
                    at = to;
                }
                OperatorTask::Pick { duration, .. } | OperatorTask::Button { duration, .. } => {
                    push(&mut plan, Some(index), Activity::Work, *duration, at, at);
                }
                OperatorTask::Wait { duration } => {
                    push(&mut plan, Some(index), Activity::Wait, *duration, at, at);
                }
            }
        }
        if at.distance(start) > 1e-9 {
            push(
                &mut plan,
                None,
                Activity::Walk,
                at.distance(start) / speed,
                at,
                start,
            );
        }
        plan.cycle_time = time;
        plan
    }

    /// サイクルの中の時刻 (繰り返す) に直す
    pub fn cycle_time_of(&self, t: f64) -> f64 {
        if self.cycle_time <= 0.0 {
            0.0
        } else {
            t.rem_euclid(self.cycle_time)
        }
    }

    /// 時刻 t (サイクルを繰り返す) で実行中の区間
    pub fn segment_at(&self, t: f64) -> Option<&PlanSegment> {
        let t = self.cycle_time_of(t);
        self.segments
            .iter()
            .find(|s| t < s.end)
            .or(self.segments.last())
    }

    /// 時刻 t (サイクルを繰り返す) での床の位置
    pub fn position_at(&self, t: f64, start: DVec2) -> DVec2 {
        let local = self.cycle_time_of(t);
        match self.segment_at(t) {
            Some(s) if s.end > s.start => {
                let k = ((local - s.start) / (s.end - s.start)).clamp(0.0, 1.0);
                s.from.lerp(s.to, k)
            }
            Some(s) => s.to,
            None => start,
        }
    }

    /// 歩いた経路 (開始位置から順に、向きが変わる点を並べたもの)
    pub fn path(&self, start: DVec2) -> Vec<DVec2> {
        let mut points = vec![start];
        for s in self
            .segments
            .iter()
            .filter(|s| s.activity == Activity::Walk)
        {
            if points.last() != Some(&s.from) {
                points.push(s.from);
            }
            points.push(s.to);
        }
        points
    }

    /// 歩行・作業・待ちの割合 (%)
    pub fn ratios(&self) -> [f64; 3] {
        if self.cycle_time <= 0.0 {
            return [0.0; 3];
        }
        [self.walk_time, self.work_time, self.wait_time].map(|t| t / self.cycle_time * 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operator() -> Operator {
        let mut op = Operator::new(1, [0.0, 0.0]);
        op.walk_speed = 1000.0;
        op.tasks = vec![
            OperatorTask::Walk { to: [3000.0, 0.0] },
            OperatorTask::Pick {
                label: "部品箱".into(),
                duration: 2.0,
            },
            OperatorTask::Walk {
                to: [3000.0, 4000.0],
            },
            OperatorTask::Button {
                label: "起動".into(),
                duration: 1.0,
            },
            OperatorTask::Wait { duration: 0.5 },
        ];
        op
    }

    #[test]
    fn plan_sums_walk_work_and_wait() {
        let plan = OperatorPlan::new(&operator());
        // 3 m + 4 m + 戻り 5 m = 12 m を 1 m/s で
        assert!((plan.walk_distance - 12000.0).abs() < 1e-9);
        assert!((plan.walk_time - 12.0).abs() < 1e-9);
        assert!((plan.work_time - 3.0).abs() < 1e-9);
        assert!((plan.wait_time - 0.5).abs() < 1e-9);
        assert!((plan.cycle_time - 15.5).abs() < 1e-9);
        let [walk, work, wait] = plan.ratios();
        assert!((walk + work + wait - 100.0).abs() < 1e-9);
    }

    #[test]
    fn position_follows_walk_and_repeats() {
        let op = operator();
        let plan = OperatorPlan::new(&op);
        let start = DVec2::from_array(op.start);
        assert!((plan.position_at(1.5, start) - DVec2::new(1500.0, 0.0)).length() < 1e-9);
        // ピック中は止まっている
        assert!((plan.position_at(4.0, start) - DVec2::new(3000.0, 0.0)).length() < 1e-9);
        // 2サイクル目も同じ位置
        let again = plan.position_at(1.5 + plan.cycle_time, start);
        assert!((again - DVec2::new(1500.0, 0.0)).length() < 1e-9);
    }

    #[test]
    fn path_lists_corners_and_returns_home() {
        let op = operator();
        let plan = OperatorPlan::new(&op);
        let path = plan.path(DVec2::from_array(op.start));
        assert_eq!(
            path,
            vec![
                DVec2::new(0.0, 0.0),
                DVec2::new(3000.0, 0.0),
                DVec2::new(3000.0, 4000.0),
                DVec2::new(0.0, 0.0),
            ]
        );
    }

    #[test]
    fn segment_at_names_current_task() {
        let plan = OperatorPlan::new(&operator());
        assert_eq!(plan.segment_at(4.0).unwrap().task, Some(1));
        assert_eq!(plan.segment_at(15.0).unwrap().task, None);
    }
}
