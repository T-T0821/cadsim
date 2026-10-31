//! ワーク搬送の計算 (Bevy の ECS に依存しない)
//!
//! - コンベアは部品 (ベルトにする箱) の上面に沿ってワークを流す。先のワークに詰まったら止まる。
//! - グリッパはチャック (シリンダ) が閉じ切った瞬間に、TCP の近くにあるワークを掴む。
//!   掴んだワークは、チャックが載っているアクチュエータと一緒に動く。
//! - チャックが開き始めた瞬間にワークを離す。コンベアの上で離せば、そのコンベアで流れる。
//!
//! 計算は時刻 0 から一定刻みで進めるだけなので、同じ入力からは必ず同じ結果になる
//! (シーク位置を戻したら、0 から計算し直せばよい)。

use std::collections::HashMap;

use glam::DVec3;

use super::config::{Conveyor, ConveyorEnd, FlowConfig};
use crate::motion::{ActuatorKind, RigidPose, Schedule, TimingChart, actuator_poses};

/// 計算の刻み (秒)。再生速度 x2 でもワークが1刻みでベルトを飛び越えない程度に細かくする。
pub const TIME_STEP: f64 = 1.0 / 240.0;
/// コンベア上で詰まったときのワーク同士のすき間 (ワークの長さに対する割合)。
/// 寸法の単位 (mm など) によらないよう、絶対値ではなく割合で持つ。
const QUEUE_GAP_RATIO: f64 = 0.05;
/// ワークの下面がベルト上面から「ワークの高さ x この割合」以内で離されたら、コンベアに載ったとみなす
const PLACE_TOLERANCE_RATIO: f64 = 0.5;
/// チャックの値がこれ以上なら「閉じている」(シリンダは 1 = 出端 = 閉)
const CLOSED_THRESHOLD: f64 = 1.0 - 1e-6;
/// 置きっぱなしのワークがこれより増えたら、古いものから消す (メモリと描画の上限)
const MAX_WORKS: usize = 200;

/// コンベアのベルト (箱) の、時刻によらない形
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Belt {
    pub center: DVec3,
    pub size: DVec3,
}

/// 1つのワーク
#[derive(Clone, Debug, PartialEq)]
pub struct Work {
    pub id: u32,
    /// 幅 / 高さ / 奥行
    pub size: DVec3,
    /// ワーク中心のワールド座標での位置・姿勢
    pub pose: RigidPose,
    pub place: Place,
}

/// ワークがいまどこにあるか
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// コンベアの上。`s` はベルト始端からワーク後端までの距離。
    Conveyor { conveyor: u32, s: f64 },
    /// グリッパに掴まれている。`offset` はチャックの載り先から見たワークの位置。
    Held { gripper: u32, offset: RigidPose },
    /// コンベア以外の場所に置かれて止まっている
    Placed,
}

impl Work {
    /// ワールド座標での外接箱 (最小, 最大)。センサーの検知などに使う。
    #[allow(dead_code)]
    pub fn aabb(&self) -> (DVec3, DVec3) {
        let half = self.size / 2.0;
        let mut min = DVec3::splat(f64::INFINITY);
        let mut max = DVec3::splat(f64::NEG_INFINITY);
        for corner in 0..8 {
            let local = DVec3::new(
                if corner & 1 == 0 { -half.x } else { half.x },
                if corner & 2 == 0 { -half.y } else { half.y },
                if corner & 4 == 0 { -half.z } else { half.z },
            );
            let p = self.pose.transform_point(local);
            min = min.min(p);
            max = max.max(p);
        }
        (min, max)
    }
}

/// 計算に必要な入力をまとめたもの
pub struct FlowInputs<'a> {
    pub config: &'a FlowConfig,
    /// コンベア id → ベルトの形 (ベルトの部品が見つからないコンベアは入れない)
    pub belts: &'a HashMap<u32, Belt>,
    pub chart: &'a TimingChart,
    pub schedule: &'a Schedule,
    /// true ならサイクルの最後まで行ったらチャートの先頭に戻って繰り返す (ワークは残る)。
    /// false ならチャート時刻 = 経過時刻のまま (作りかけの時刻表でも使える)。
    pub repeat: bool,
}

impl FlowInputs<'_> {
    /// 経過時刻をチャート上の時刻に直す
    fn chart_time(&self, time: f64) -> f64 {
        let cycle = self.schedule.cycle_time;
        if self.repeat && cycle > 0.0 {
            time % cycle
        } else {
            time
        }
    }
}

/// ある時刻でのワークの状態と、次の刻みへ進めるための内部状態
#[derive(Clone, Debug, Default)]
pub struct FlowSim {
    /// サイクルをまたいで増え続ける時刻 (秒)
    pub time: f64,
    pub works: Vec<Work>,
    next_work_id: u32,
    /// コンベア id → 次にワークを供給する時刻
    next_feed: HashMap<u32, f64>,
    /// グリッパ id → 前の刻みでチャックが閉じていたか
    was_closed: HashMap<u32, bool>,
}

impl FlowSim {
    /// `target` の時刻の状態にする。前の時刻へ戻すときは 0 から計算し直す。
    pub fn seek(&mut self, inputs: &FlowInputs, target: f64) {
        if target < self.time {
            *self = FlowSim::default();
        }
        self.advance_to(inputs, target);
    }

    /// `target` の時刻まで刻みを進める
    pub fn advance_to(&mut self, inputs: &FlowInputs, target: f64) {
        while self.time + 1e-9 < target {
            let dt = TIME_STEP.min(target - self.time);
            self.step(inputs, dt);
        }
    }

    /// 1刻み進める
    pub fn step(&mut self, inputs: &FlowInputs, dt: f64) {
        self.time += dt;
        let chart_time = inputs.chart_time(self.time);
        let poses = actuator_poses(inputs.chart, inputs.schedule, chart_time);

        self.feed(inputs);
        self.convey(inputs, dt);
        self.grip_and_release(inputs, &poses, chart_time);
        self.follow_grippers(inputs, &poses);
        self.forget_old_works();
    }

    /// 供給ありのコンベアの始端にワークを置く。始端が詰まっていたら空くまで待つ。
    fn feed(&mut self, inputs: &FlowInputs) {
        for conveyor in &inputs.config.conveyors {
            let Some(interval) = conveyor.feed_interval else {
                continue;
            };
            let Some(belt) = inputs.belts.get(&conveyor.id) else {
                continue;
            };
            let due = self.next_feed.entry(conveyor.id).or_insert(0.0);
            if self.time < *due {
                continue;
            }
            let size = DVec3::from_array(conveyor.work_size);
            let length = conveyor.along(size);
            let start_is_free = self.works.iter().all(|w| match w.place {
                Place::Conveyor { conveyor: c, s } if c == conveyor.id => {
                    s >= length * (1.0 + QUEUE_GAP_RATIO)
                }
                _ => true,
            });
            if !start_is_free {
                continue;
            }
            *due += interval.max(TIME_STEP);
            self.next_work_id += 1;
            self.works.push(Work {
                id: self.next_work_id,
                size,
                pose: RigidPose::from_translation(belt_point(conveyor, belt, size, 0.0)),
                place: Place::Conveyor {
                    conveyor: conveyor.id,
                    s: 0.0,
                },
            });
        }
    }

    /// コンベア上のワークを流す。前のワークやストッパに当たったらそこで止まる。
    fn convey(&mut self, inputs: &FlowInputs, dt: f64) {
        for conveyor in &inputs.config.conveyors {
            let Some(belt) = inputs.belts.get(&conveyor.id) else {
                continue;
            };
            let belt_length = conveyor.along(belt.size);
            // 先頭 (s が大きい) から順に動かし、後ろのワークは前のワークの後端で止める
            let mut on_belt: Vec<usize> = (0..self.works.len())
                .filter(|&i| {
                    matches!(self.works[i].place, Place::Conveyor { conveyor: c, .. } if c == conveyor.id)
                })
                .collect();
            on_belt.sort_by(|&a, &b| {
                conveyor_s(&self.works[b]).total_cmp(&conveyor_s(&self.works[a]))
            });

            let mut front_rear: Option<f64> = None;
            for i in on_belt {
                let work = &mut self.works[i];
                let length = conveyor.along(work.size);
                let limit = match (front_rear, conveyor.end) {
                    (Some(rear), _) => rear - length * (1.0 + QUEUE_GAP_RATIO),
                    (None, ConveyorEnd::Stopper) => belt_length - length,
                    (None, ConveyorEnd::Discharge) => f64::INFINITY,
                };
                let s = conveyor_s(work);
                let moved = (s + conveyor.speed.max(0.0) * dt).min(limit).max(s);
                work.place = Place::Conveyor {
                    conveyor: conveyor.id,
                    s: moved,
                };
                work.pose.translation = belt_point(conveyor, belt, work.size, moved);
                front_rear = Some(moved);
            }
            // 排出端から完全に出たワークは消す
            self.works.retain(|w| match w.place {
                Place::Conveyor { conveyor: c, s } if c == conveyor.id => s < belt_length,
                _ => true,
            });
        }
    }

    /// チャックが閉じた瞬間に掴み、開いた瞬間に離す
    fn grip_and_release(
        &mut self,
        inputs: &FlowInputs,
        poses: &HashMap<u32, RigidPose>,
        chart_time: f64,
    ) {
        for gripper in &inputs.config.grippers {
            let Some(chuck) = gripper
                .chuck
                .and_then(|id| inputs.chart.actuator(id))
                .filter(|a| a.kind == ActuatorKind::Cylinder)
            else {
                continue;
            };
            let closed = inputs.schedule.value_at(chuck, chart_time) >= CLOSED_THRESHOLD;
            let was_closed = self.was_closed.insert(gripper.id, closed).unwrap_or(false);
            let carrier = carrier_pose(inputs.chart, chuck.mounted_on, poses);
            let holding = self
                .works
                .iter()
                .position(|w| matches!(w.place, Place::Held { gripper: g, .. } if g == gripper.id));

            if closed && !was_closed && holding.is_none() {
                let tcp = carrier.transform_point(DVec3::from_array(gripper.tcp));
                let nearest = self
                    .works
                    .iter_mut()
                    .filter(|w| !matches!(w.place, Place::Held { .. }))
                    .map(|w| (w.pose.translation.distance(tcp), w))
                    .filter(|(d, _)| *d <= gripper.reach)
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                if let Some((_, work)) = nearest {
                    work.place = Place::Held {
                        gripper: gripper.id,
                        offset: carrier.inverse().then_apply_to(work.pose),
                    };
                }
            } else if !closed
                && was_closed
                && let Some(i) = holding
            {
                let work = &mut self.works[i];
                work.place = Place::Placed;
                place_on_conveyor(work, inputs);
            }
        }
    }

    /// 掴まれているワークを、チャックの載り先と一緒に動かす
    fn follow_grippers(&mut self, inputs: &FlowInputs, poses: &HashMap<u32, RigidPose>) {
        for work in &mut self.works {
            let Place::Held { gripper, offset } = work.place else {
                continue;
            };
            let mount = inputs
                .config
                .grippers
                .iter()
                .find(|g| g.id == gripper)
                .and_then(|g| g.chuck)
                .and_then(|id| inputs.chart.actuator(id))
                .and_then(|a| a.mounted_on);
            work.pose = carrier_pose(inputs.chart, mount, poses).then_apply_to(offset);
        }
    }

    fn forget_old_works(&mut self) {
        while self.works.len() > MAX_WORKS {
            match self.works.iter().position(|w| w.place == Place::Placed) {
                Some(i) => self.works.remove(i),
                None => self.works.remove(0),
            };
        }
    }
}

fn conveyor_s(work: &Work) -> f64 {
    match work.place {
        Place::Conveyor { s, .. } => s,
        _ => 0.0,
    }
}

/// チャックが載っているアクチュエータの、親の動きも含めた変換 (載っていなければ動かない)
fn carrier_pose(
    chart: &TimingChart,
    mount: Option<u32>,
    poses: &HashMap<u32, RigidPose>,
) -> RigidPose {
    mount
        .filter(|id| chart.actuator(*id).is_some())
        .and_then(|id| poses.get(&id).copied())
        .unwrap_or_default()
}

/// ベルト上で、後端が始端から `s` の位置にあるワークの中心
fn belt_point(conveyor: &Conveyor, belt: &Belt, work_size: DVec3, s: f64) -> DVec3 {
    let direction = conveyor.direction();
    let belt_start = belt.center - direction * conveyor.along(belt.size) / 2.0;
    let height = DVec3::Y * (belt.size.y + work_size.y) / 2.0;
    belt_start + direction * (s + conveyor.along(work_size) / 2.0) + height
}

/// 離したワークの真下にコンベアのベルトがあれば、そのコンベアに載せる
fn place_on_conveyor(work: &mut Work, inputs: &FlowInputs) {
    let center = work.pose.translation;
    for conveyor in &inputs.config.conveyors {
        let Some(belt) = inputs.belts.get(&conveyor.id) else {
            continue;
        };
        let half = belt.size / 2.0;
        let inside = (center.x - belt.center.x).abs() <= half.x
            && (center.z - belt.center.z).abs() <= half.z;
        let bottom = center.y - work.size.y / 2.0;
        let belt_top = belt.center.y + half.y;
        if !inside || (bottom - belt_top).abs() > work.size.y * PLACE_TOLERANCE_RATIO {
            continue;
        }
        let direction = conveyor.direction();
        let belt_start = belt.center - direction * conveyor.along(belt.size) / 2.0;
        let max_s = (conveyor.along(belt.size) - conveyor.along(work.size)).max(0.0);
        let s = ((center - belt_start).dot(direction) - conveyor.along(work.size) / 2.0)
            .clamp(0.0, max_s);
        work.place = Place::Conveyor {
            conveyor: conveyor.id,
            s,
        };
        work.pose = RigidPose::from_translation(belt_point(conveyor, belt, work.size, s));
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::domain::Gripper;
    use crate::motion::{Action, Axis, Easing, Step};

    fn belt_along_x() -> Belt {
        Belt {
            center: DVec3::new(0.0, 0.5, 0.0),
            size: DVec3::new(2.0, 1.0, 0.6),
        }
    }

    fn feeder(end: ConveyorEnd) -> Conveyor {
        Conveyor {
            id: 1,
            name: "コンベア".into(),
            part: Some(10),
            axis: Axis::X,
            reversed: false,
            speed: 1.0,
            feed_interval: Some(0.5),
            end,
            work_size: [0.4, 0.2, 0.4],
        }
    }

    fn run(config: &FlowConfig, chart: &TimingChart, seconds: f64) -> FlowSim {
        let belts = HashMap::from([(1, belt_along_x())]);
        let schedule = chart.schedule();
        let inputs = FlowInputs {
            config,
            belts: &belts,
            chart,
            schedule: &schedule,
            repeat: false,
        };
        let mut sim = FlowSim::default();
        sim.advance_to(&inputs, seconds);
        sim
    }

    #[test]
    fn works_queue_up_at_the_stopper() {
        let config = FlowConfig {
            conveyors: vec![feeder(ConveyorEnd::Stopper)],
            ..Default::default()
        };
        let sim = run(&config, &TimingChart::default(), 10.0);
        // 長さ 2.0 のベルトに 0.4 + すき間 0.02 のワークは 4 個まで並ぶ
        let mut s: Vec<f64> = sim.works.iter().map(conveyor_s).collect();
        s.sort_by(f64::total_cmp);
        assert_eq!(s.len(), 4);
        assert!((s[3] - 1.6).abs() < 1e-9);
        assert!((s[2] - (1.6 - 0.42)).abs() < 1e-9);
        // 先頭のワークはベルト終端 (x = 1.0) に後ろから接して、ベルトの上に載っている
        let front = sim.works.iter().find(|w| conveyor_s(w) > 1.5).unwrap();
        let (min, max) = front.aabb();
        assert!((max.x - 1.0).abs() < 1e-9);
        assert!((min.y - 1.0).abs() < 1e-9);
    }

    #[test]
    fn discharge_end_removes_works() {
        let config = FlowConfig {
            conveyors: vec![feeder(ConveyorEnd::Discharge)],
            ..Default::default()
        };
        let sim = run(&config, &TimingChart::default(), 10.0);
        // 2 秒でベルトを渡り切るので、0.5 秒おきの供給なら常に 4 〜 5 個しか載っていない
        assert!((4..=5).contains(&sim.works.len()), "{}", sim.works.len());
        assert!(sim.works.iter().all(|w| conveyor_s(w) < 2.0));
    }

    /// 上下シリンダにチャックを載せ、コンベア終端のワークを掴んで持ち上げるチャート
    fn pick_chart() -> (TimingChart, u32) {
        let mut chart = TimingChart::default();
        let lifter = chart.add_actuator(ActuatorKind::Cylinder);
        let chuck = chart.add_actuator(ActuatorKind::Cylinder);
        let axis = chart.add_actuator(ActuatorKind::LinearAxis);
        for a in &mut chart.actuators {
            a.easing = Easing::Linear;
        }
        chart.actuator_mut(lifter).unwrap().stroke = 1.0;
        chart.actuator_mut(lifter).unwrap().mounted_on = Some(axis);
        chart.actuator_mut(axis).unwrap().axis = Axis::Z;
        chart.actuator_mut(chuck).unwrap().mounted_on = Some(lifter);
        let act = |actuator, target| Action {
            actuator,
            target,
            duration: 1.0,
        };
        let step = |actions| Step {
            actions,
            ..Default::default()
        };
        chart.steps = vec![
            // 0〜4 秒: ワークがストッパまで流れてくるのを待つ
            Step {
                dwell: 4.0,
                ..Default::default()
            },
            step(vec![act(chuck, 1.0)]),  // 4〜5 秒: 閉じる (5 秒で掴む)
            step(vec![act(lifter, 1.0)]), // 5〜6 秒: 1.0 上げる
            step(vec![act(axis, 2.0)]),   // 6〜7 秒: Z に 2.0 動かす
            step(vec![act(chuck, 0.0)]),  // 7〜8 秒: 開く (7 秒で離す)
        ];
        (chart, chuck)
    }

    #[test]
    fn gripper_carries_work_and_releases_it() {
        let (chart, chuck) = pick_chart();
        let mut conveyor = feeder(ConveyorEnd::Stopper);
        conveyor.feed_interval = Some(100.0);
        let config = FlowConfig {
            conveyors: vec![conveyor],
            grippers: vec![Gripper {
                id: 1,
                name: "チャック".into(),
                chuck: Some(chuck),
                tcp: [0.8, 1.1, 0.0],
                reach: 0.2,
            }],
            ..Default::default()
        };

        let held = run(&config, &chart, 6.5);
        let work = &held.works[0];
        assert!(matches!(work.place, Place::Held { .. }));
        assert!((work.pose.translation - DVec3::new(0.8, 2.1, 1.0)).length() < 1e-6);

        let released = run(&config, &chart, 8.0);
        let work = &released.works[0];
        assert_eq!(work.place, Place::Placed);
        assert!((work.pose.translation - DVec3::new(0.8, 2.1, 2.0)).length() < 1e-6);
    }

    #[test]
    fn released_work_lands_on_the_conveyor_below() {
        let (mut chart, chuck) = pick_chart();
        // 持ち上げずに Z だけ少し動かして (ベルトの幅の中で) 離す
        chart.steps.remove(2);
        let axis = chart.actuators[2].id;
        chart.steps[2].actions[0] = Action {
            actuator: axis,
            target: 0.1,
            duration: 1.0,
        };
        let mut conveyor = feeder(ConveyorEnd::Stopper);
        conveyor.feed_interval = Some(100.0);
        let config = FlowConfig {
            conveyors: vec![conveyor],
            grippers: vec![Gripper {
                id: 1,
                name: "チャック".into(),
                chuck: Some(chuck),
                tcp: [0.8, 1.1, 0.0],
                reach: 0.2,
            }],
            ..Default::default()
        };
        let sim = run(&config, &chart, 7.5);
        let work = &sim.works[0];
        assert!(
            matches!(work.place, Place::Conveyor { conveyor: 1, .. }),
            "{work:?}"
        );
        // コンベアに載ると、ベルトの中心線 (z = 0) に寄せられる
        assert!(work.pose.translation.z.abs() < 1e-9, "{work:?}");
    }
}
