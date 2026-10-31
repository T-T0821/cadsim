//! ワーク搬送の計算
//!
//! - コンベアは部品 (ベルト) の上面に沿ってワークを流す。前のワークやストッパに当たったら止まる。
//! - グリッパはチャックが閉じた瞬間に、TCP の近くにあるワークを掴む。
//!   掴んだワークは、グリッパを運ぶ動き ([`GripperState::carrier`]) と一緒に動く。
//! - チャックが開いた瞬間にワークを離す。コンベアの上で離せば、そのコンベアで流れる。
//!
//! 計算は時刻 0 から一定刻みで進めるだけなので、同じ入力からは必ず同じ結果になる
//! (シーク位置を戻したら、0 から計算し直せばよい)。

use std::collections::HashMap;

use glam::{DQuat, DVec3};

use super::config::{Conveyor, ConveyorConfig, ConveyorEnd, Gripper};

/// 計算の刻み (秒)。再生速度 x2 でもワークが1刻みでベルトを飛び越えない程度に細かくする。
pub const TIME_STEP: f64 = 1.0 / 240.0;
/// コンベア上で詰まったときのワーク同士のすき間 (ワークの長さに対する割合)。
/// 寸法の単位 (mm など) によらないよう、絶対値ではなく割合で持つ。
const QUEUE_GAP_RATIO: f64 = 0.05;
/// ワークの下面がベルト上面から「ワークの高さ x この割合」以内で離されたら、コンベアに載ったとみなす
const PLACE_TOLERANCE_RATIO: f64 = 0.5;
/// 置きっぱなしのワークがこれより増えたら、古いものから消す (メモリと描画の上限)
const MAX_WORKS: usize = 200;

/// 回転してから平行移動する剛体変換
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub rotation: DQuat,
    pub translation: DVec3,
}

impl Default for Pose {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Pose {
    pub const IDENTITY: Self = Self {
        rotation: DQuat::IDENTITY,
        translation: DVec3::ZERO,
    };

    pub fn from_translation(translation: DVec3) -> Self {
        Self {
            rotation: DQuat::IDENTITY,
            translation,
        }
    }

    /// 先に `other` を適用し、次に `self` を適用する変換
    pub fn then_apply_to(self, other: Pose) -> Pose {
        Pose {
            rotation: self.rotation * other.rotation,
            translation: self.rotation * other.translation + self.translation,
        }
    }

    pub fn inverse(self) -> Pose {
        let rotation = self.rotation.inverse();
        Pose {
            rotation,
            translation: -(rotation * self.translation),
        }
    }

    pub fn transform_point(&self, p: DVec3) -> DVec3 {
        self.rotation * p + self.translation
    }
}

/// ある時刻でのグリッパの状態
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GripperState {
    /// チャックが閉じている
    pub closed: bool,
    /// グリッパを運ぶ変換 (設計位置からの動き)。TCP と掴んだワークはこれで動く。
    pub carrier: Pose,
}

/// グリッパがいつ閉じ、どう動くかを教えるもの。
/// アプリではタイミングチャートから求める。テストでは時刻ごとの台本を返す。
pub trait GripperMotion {
    /// 経過時刻 `time` でのグリッパの状態。チャックが未設定などで動かせなければ None。
    fn gripper_state(&self, gripper: &Gripper, time: f64) -> Option<GripperState>;
}

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
    pub pose: Pose,
    pub place: Place,
}

/// ワークがいまどこにあるか
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// コンベアの上。`s` はベルト始端からワーク後端までの距離。
    Conveyor { conveyor: u32, s: f64 },
    /// グリッパに掴まれている。`offset` はグリッパを運ぶ変換から見たワークの位置。
    Held { gripper: u32, offset: Pose },
    /// コンベア以外の場所に置かれて止まっている
    Placed,
}

impl Work {
    /// ワールド座標での外接箱 (最小, 最大)。センサーの検知などに使う。
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

    pub fn is_held(&self) -> bool {
        matches!(self.place, Place::Held { .. })
    }
}

/// 計算に必要な入力をまとめたもの
pub struct ConveyorInputs<'a> {
    pub config: &'a ConveyorConfig,
    /// コンベア id → ベルトの形 (ベルトの部品が見つからないコンベアは入れない)
    pub belts: &'a HashMap<u32, Belt>,
    pub motion: &'a dyn GripperMotion,
}

/// ある時刻でのワークの状態と、次の刻みへ進めるための内部状態
#[derive(Clone, Debug, Default)]
pub struct ConveyorSim {
    /// 経過時刻 (秒)
    pub time: f64,
    pub works: Vec<Work>,
    next_work_id: u32,
    /// コンベア id → 次にワークを供給する時刻
    next_feed: HashMap<u32, f64>,
    /// グリッパ id → 前の刻みでチャックが閉じていたか
    was_closed: HashMap<u32, bool>,
}

impl ConveyorSim {
    /// `target` の時刻の状態にする。前の時刻へ戻すときは 0 から計算し直す。
    pub fn seek(&mut self, inputs: &ConveyorInputs, target: f64) {
        if target < self.time {
            *self = ConveyorSim::default();
        }
        self.advance_to(inputs, target);
    }

    /// `target` の時刻まで刻みを進める
    pub fn advance_to(&mut self, inputs: &ConveyorInputs, target: f64) {
        while self.time + 1e-9 < target {
            let dt = TIME_STEP.min(target - self.time);
            self.step(inputs, dt);
        }
    }

    /// 1刻み進める
    pub fn step(&mut self, inputs: &ConveyorInputs, dt: f64) {
        self.time += dt;
        let grippers: Vec<(&Gripper, GripperState)> = inputs
            .config
            .grippers
            .iter()
            .filter_map(|g| Some((g, inputs.motion.gripper_state(g, self.time)?)))
            .collect();

        self.feed(inputs);
        self.convey(inputs, dt);
        self.grip_and_release(inputs, &grippers);
        self.follow_grippers(&grippers);
        self.forget_old_works();
    }

    /// 供給ありのコンベアの始端にワークを置く。始端が詰まっていたら空くまで待つ。
    fn feed(&mut self, inputs: &ConveyorInputs) {
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
                pose: Pose::from_translation(belt_point(conveyor, belt, size, 0.0)),
                place: Place::Conveyor {
                    conveyor: conveyor.id,
                    s: 0.0,
                },
            });
        }
    }

    /// コンベア上のワークを流す。前のワークやストッパに当たったらそこで止まる。
    fn convey(&mut self, inputs: &ConveyorInputs, dt: f64) {
        for conveyor in &inputs.config.conveyors {
            let Some(belt) = inputs.belts.get(&conveyor.id) else {
                continue;
            };
            let belt_length = conveyor.along(belt.size);
            // 先頭 (s が大きい) から順に動かし、後ろのワークは前のワークの後端で止める
            let mut on_belt: Vec<usize> = (0..self.works.len())
                .filter(|&i| on_conveyor(&self.works[i], conveyor.id))
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
            self.works
                .retain(|w| !on_conveyor(w, conveyor.id) || conveyor_s(w) < belt_length);
        }
    }

    /// チャックが閉じた瞬間に掴み、開いた瞬間に離す
    fn grip_and_release(&mut self, inputs: &ConveyorInputs, grippers: &[(&Gripper, GripperState)]) {
        for (gripper, state) in grippers {
            let was_closed = self
                .was_closed
                .insert(gripper.id, state.closed)
                .unwrap_or(false);
            let holding = self
                .works
                .iter()
                .position(|w| matches!(w.place, Place::Held { gripper: g, .. } if g == gripper.id));

            if state.closed && !was_closed && holding.is_none() {
                let tcp = state
                    .carrier
                    .transform_point(DVec3::from_array(gripper.tcp));
                let nearest = self
                    .works
                    .iter_mut()
                    .filter(|w| !w.is_held())
                    .map(|w| (w.pose.translation.distance(tcp), w))
                    .filter(|(d, _)| *d <= gripper.reach)
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                if let Some((_, work)) = nearest {
                    work.place = Place::Held {
                        gripper: gripper.id,
                        offset: state.carrier.inverse().then_apply_to(work.pose),
                    };
                }
            } else if !state.closed
                && was_closed
                && let Some(i) = holding
            {
                let work = &mut self.works[i];
                work.place = Place::Placed;
                place_on_conveyor(work, inputs);
            }
        }
    }

    /// 掴まれているワークを、グリッパと一緒に動かす
    fn follow_grippers(&mut self, grippers: &[(&Gripper, GripperState)]) {
        for work in &mut self.works {
            let Place::Held { gripper, offset } = work.place else {
                continue;
            };
            if let Some((_, state)) = grippers.iter().find(|(g, _)| g.id == gripper) {
                work.pose = state.carrier.then_apply_to(offset);
            }
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

fn on_conveyor(work: &Work, conveyor: u32) -> bool {
    matches!(work.place, Place::Conveyor { conveyor: c, .. } if c == conveyor)
}

fn conveyor_s(work: &Work) -> f64 {
    match work.place {
        Place::Conveyor { s, .. } => s,
        _ => 0.0,
    }
}

/// ベルト上で、後端が始端から `s` の位置にあるワークの中心
fn belt_point(conveyor: &Conveyor, belt: &Belt, work_size: DVec3, s: f64) -> DVec3 {
    let direction = conveyor.direction();
    let belt_start = belt.center - direction * conveyor.along(belt.size) / 2.0;
    let height = DVec3::Y * (belt.size.y + work_size.y) / 2.0;
    belt_start + direction * (s + conveyor.along(work_size) / 2.0) + height
}

/// 離したワークの真下にコンベアのベルトがあれば、そのコンベアに載せる (ベルトの中心線に寄せる)
fn place_on_conveyor(work: &mut Work, inputs: &ConveyorInputs) {
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
        work.pose = Pose::from_translation(belt_point(conveyor, belt, work.size, s));
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conveyor::config::BeltAxis;

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
            axis: BeltAxis::X,
            reversed: false,
            speed: 1.0,
            feed_interval: Some(0.5),
            end,
            work_size: [0.4, 0.2, 0.4],
        }
    }

    /// グリッパを動かさない
    struct NoMotion;

    impl GripperMotion for NoMotion {
        fn gripper_state(&self, _: &Gripper, _: f64) -> Option<GripperState> {
            None
        }
    }

    /// 時刻ごとの台本でグリッパを動かす。`(閉じる時刻, 開く時刻)` と、時刻 → 移動量。
    struct Script {
        close_at: f64,
        open_at: f64,
        offset: fn(f64) -> DVec3,
    }

    impl GripperMotion for Script {
        fn gripper_state(&self, _: &Gripper, t: f64) -> Option<GripperState> {
            Some(GripperState {
                closed: t >= self.close_at && t < self.open_at,
                carrier: Pose::from_translation((self.offset)(t)),
            })
        }
    }

    fn run(config: &ConveyorConfig, motion: &dyn GripperMotion, seconds: f64) -> ConveyorSim {
        let belts = HashMap::from([(1, belt_along_x())]);
        let inputs = ConveyorInputs {
            config,
            belts: &belts,
            motion,
        };
        let mut sim = ConveyorSim::default();
        sim.advance_to(&inputs, seconds);
        sim
    }

    fn gripper() -> Gripper {
        Gripper {
            id: 1,
            name: "チャック".into(),
            chuck: Some(1),
            tcp: [0.8, 1.1, 0.0],
            reach: 0.2,
        }
    }

    fn single_work_config() -> ConveyorConfig {
        let mut conveyor = feeder(ConveyorEnd::Stopper);
        conveyor.feed_interval = Some(100.0);
        ConveyorConfig {
            conveyors: vec![conveyor],
            grippers: vec![gripper()],
            ..Default::default()
        }
    }

    #[test]
    fn works_queue_up_at_the_stopper() {
        let config = ConveyorConfig {
            conveyors: vec![feeder(ConveyorEnd::Stopper)],
            ..Default::default()
        };
        let sim = run(&config, &NoMotion, 10.0);
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
        let config = ConveyorConfig {
            conveyors: vec![feeder(ConveyorEnd::Discharge)],
            ..Default::default()
        };
        let sim = run(&config, &NoMotion, 10.0);
        // 2 秒でベルトを渡り切るので、0.5 秒おきの供給なら常に 4 〜 5 個しか載っていない
        assert!((4..=5).contains(&sim.works.len()), "{}", sim.works.len());
        assert!(sim.works.iter().all(|w| conveyor_s(w) < 2.0));
    }

    #[test]
    fn gripper_carries_work_and_releases_it() {
        // 5 秒で閉じ、5〜6 秒で 1.0 上げ、6〜7 秒で Z に 2.0 動かし、7 秒で開く
        let motion = Script {
            close_at: 5.0,
            open_at: 7.0,
            offset: |t| {
                DVec3::new(
                    0.0,
                    (t - 5.0).clamp(0.0, 1.0),
                    2.0 * (t - 6.0).clamp(0.0, 1.0),
                )
            },
        };
        let config = single_work_config();

        let held = run(&config, &motion, 6.5);
        let work = &held.works[0];
        assert!(work.is_held());
        assert!((work.pose.translation - DVec3::new(0.8, 2.1, 1.0)).length() < 1e-6);

        let released = run(&config, &motion, 8.0);
        let work = &released.works[0];
        assert_eq!(work.place, Place::Placed);
        assert!((work.pose.translation - DVec3::new(0.8, 2.1, 2.0)).length() < 1e-6);
    }

    #[test]
    fn released_work_lands_on_the_conveyor_below() {
        // 持ち上げずに Z へ 0.1 (ベルトの幅の中) だけずらして離す
        let motion = Script {
            close_at: 5.0,
            open_at: 6.0,
            offset: |t| DVec3::new(0.0, 0.0, 0.1 * (t - 5.0).clamp(0.0, 1.0)),
        };
        let sim = run(&single_work_config(), &motion, 7.0);
        let work = &sim.works[0];
        assert!(matches!(work.place, Place::Conveyor { conveyor: 1, .. }));
        // コンベアに載ると、ベルトの中心線 (z = 0) に寄せられる
        assert!(work.pose.translation.z.abs() < 1e-9, "{work:?}");
    }

    #[test]
    fn nothing_is_gripped_out_of_reach() {
        let motion = Script {
            close_at: 0.5,
            open_at: 100.0,
            offset: |_| DVec3::ZERO,
        };
        // 0.5 秒ではワークはまだ始端にいて、TCP (x = 0.8) から遠い
        let sim = run(&single_work_config(), &motion, 3.0);
        assert!(!sim.works[0].is_held());
    }

    #[test]
    fn seeking_back_recomputes_from_the_start() {
        let config = ConveyorConfig {
            conveyors: vec![feeder(ConveyorEnd::Stopper)],
            ..Default::default()
        };
        let belts = HashMap::from([(1, belt_along_x())]);
        let inputs = ConveyorInputs {
            config: &config,
            belts: &belts,
            motion: &NoMotion,
        };
        let mut sim = ConveyorSim::default();
        sim.seek(&inputs, 5.0);
        sim.seek(&inputs, 0.25);
        assert_eq!(sim.works.len(), 1);
        assert!((conveyor_s(&sim.works[0]) - 0.25).abs() < 1e-9);
    }
}
