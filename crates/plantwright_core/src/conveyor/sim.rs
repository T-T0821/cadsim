//! ワーク搬送の計算
//!
//! - 投入口が決まった間隔でワークを出す (コンベアの始端か、部品の上)。
//! - コンベアは部品 (ベルト) の上面に沿ってワークを流す。前のワークやストッパに当たったら止まる。
//! - グリッパはチャックが閉じた瞬間に、TCP の近くにあるワークを掴む (別のグリッパが
//!   持っているワークでも掴み直せるので、グリッパ同士で受け渡しできる)。
//! - チャックが開いた瞬間にワークを離す。真下がコンベアならそこで流れ、アクチュエータで動く
//!   部品 (回転テーブルなど) の上ならその部品に載り替わって一緒に動く。
//! - 排出口の部品に触れたワークと、排出端から出たワークは消して数える。
//!
//! 計算は時刻 0 から一定刻みで進めるだけなので、同じ入力からは必ず同じ結果になる
//! (シーク位置を戻したら、0 から計算し直せばよい)。
//!
//! ライン連携では、工程シミュレーションから送り出されたワークを投入口から流せる
//! ([`ConveyorSim::drive_sources`] と [`ConveyorSim::feed_external`])。そのワークには荷札 (`tag`) が付き、
//! 投入口から出たときと、排出されたときに [`ConveyorEvent`] を出す。外から入れるワークは
//! 時刻 0 からの計算し直しでは再現できないので、そのときは [`ConveyorSim::seek`] を使わず、
//! [`ConveyorSim::advance_to`] で前へだけ進める。

use std::collections::{HashMap, HashSet, VecDeque};

use glam::{DQuat, DVec3};

use super::config::{Conveyor, ConveyorConfig, ConveyorEnd, Gripper, SourceOutput, WorkKind};

/// 計算の刻み (秒)。再生速度 x2 でもワークが1刻みでベルトを飛び越えない程度に細かくする。
pub const TIME_STEP: f64 = 1.0 / 240.0;
/// コンベア上で詰まったときのワーク同士のすき間 (ワークの長さに対する割合)。
/// 寸法の単位 (mm など) によらないよう、絶対値ではなく割合で持つ。
const QUEUE_GAP_RATIO: f64 = 0.05;
/// ワークの下面が面の上面から「ワークの高さ x この割合」以内で離されたら、その面に載ったとみなす
const PLACE_TOLERANCE_RATIO: f64 = 0.5;
/// 排出口に「触れた」とみなすすき間 (ワークの一番短い辺に対する割合)
const TOUCH_RATIO: f64 = 0.05;
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

/// 軸に沿った箱 (部品の外接箱など)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub center: DVec3,
    pub size: DVec3,
}

impl Bounds {
    pub fn from_min_max(min: DVec3, max: DVec3) -> Self {
        Self {
            center: (min + max) / 2.0,
            size: max - min,
        }
    }

    pub fn min(&self) -> DVec3 {
        self.center - self.size / 2.0
    }

    pub fn max(&self) -> DVec3 {
        self.center + self.size / 2.0
    }

    pub fn top(&self) -> f64 {
        self.center.y + self.size.y / 2.0
    }

    /// 真上から見て点 `p` が箱の中にある
    pub fn contains_xz(&self, p: DVec3) -> bool {
        let half = self.size / 2.0;
        (p.x - self.center.x).abs() <= half.x && (p.z - self.center.z).abs() <= half.z
    }

    /// `margin` だけ広げた箱どうしが重なる
    pub fn overlaps(&self, other: &Bounds, margin: f64) -> bool {
        let (a_min, a_max) = (self.min(), self.max());
        let (b_min, b_max) = (other.min(), other.max());
        (0..3).all(|i| a_min[i] - margin <= b_max[i] && b_min[i] - margin <= a_max[i])
    }

    /// 剛体変換したあとの外接箱
    pub fn transformed(&self, pose: Pose) -> Bounds {
        let (min, max) = corners(self.center, self.size / 2.0, pose).fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(min, max), p| (min.min(p), max.max(p)),
        );
        Bounds::from_min_max(min, max)
    }
}

/// 中心 `center`、半寸法 `half` の箱の 8 隅を `pose` で動かしたもの
fn corners(center: DVec3, half: DVec3, pose: Pose) -> impl Iterator<Item = DVec3> {
    (0..8).map(move |corner| {
        let local = DVec3::new(
            if corner & 1 == 0 { -half.x } else { half.x },
            if corner & 2 == 0 { -half.y } else { half.y },
            if corner & 4 == 0 { -half.z } else { half.z },
        );
        pose.transform_point(center + local)
    })
}

/// ある時刻でのグリッパの状態
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GripperState {
    /// チャックが閉じている
    pub closed: bool,
    /// グリッパを運ぶ変換 (設計位置からの動き)。TCP と掴んだワークはこれで動く。
    pub carrier: Pose,
}

/// ワークを載せて動ける台 (アクチュエータで動く部品)。ある時刻での位置。
#[derive(Clone, Debug, PartialEq)]
pub struct Carrier {
    pub id: u32,
    /// 設計位置からの動き。載せたワークはこれで動く。
    pub pose: Pose,
    /// ワークを置ける面 (動いた後の部品の外接箱)
    pub surfaces: Vec<Bounds>,
}

/// 装置の動き (グリッパの開閉と、ワークを載せて動く台) を教えるもの。
/// アプリではタイミングチャートから求める。テストでは時刻ごとの台本を返す。
pub trait MachineMotion {
    /// 経過時刻 `time` でのグリッパの状態。チャックが未設定などで動かせなければ None。
    fn gripper_state(&self, gripper: &Gripper, time: f64) -> Option<GripperState>;

    /// 経過時刻 `time` での、ワークを載せて動ける台。
    fn carriers(&self, _time: f64) -> Vec<Carrier> {
        Vec::new()
    }
}

/// 1つのワーク
#[derive(Clone, Debug, PartialEq)]
pub struct Work {
    pub id: u32,
    /// ワーク種類の id
    pub kind: u32,
    /// 幅 / 高さ / 奥行
    pub size: DVec3,
    /// ワーク中心のワールド座標での位置・姿勢
    pub pose: Pose,
    pub place: Place,
    /// 工程シミュレーションから送り込んだワークの荷札。投入口が間隔で出したワークは None。
    pub tag: Option<u64>,
}

/// 工程シミュレーションから送り込んだワークの出来事
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConveyorEvent {
    /// 投入口から出た
    Fed { tag: u64 },
    /// 排出口か排出端のコンベアから出た (置き場があふれて消したときも、ワークを失わないようこれを出す)
    Discharged { tag: u64 },
}

/// ワークがいまどこにあるか (何に「親付け」されているか)
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// コンベアの上。`s` はベルト始端からワーク後端までの距離。
    Conveyor { conveyor: u32, s: f64 },
    /// グリッパに掴まれている。`offset` はグリッパを運ぶ変換から見たワークの位置。
    Held { gripper: u32, offset: Pose },
    /// 動く台 ([`Carrier`]) の上。`offset` は台の変換から見たワークの位置。
    Carried { carrier: u32, offset: Pose },
    /// 動かない場所に置かれて止まっている
    Placed,
}

impl Work {
    /// ワールド座標での外接箱。センサーの検知などに使う。
    pub fn bounds(&self) -> Bounds {
        Bounds {
            center: DVec3::ZERO,
            size: self.size,
        }
        .transformed(self.pose)
    }

    /// ワールド座標での外接箱 (最小, 最大)
    pub fn aabb(&self) -> (DVec3, DVec3) {
        let b = self.bounds();
        (b.min(), b.max())
    }

    pub fn is_held(&self) -> bool {
        matches!(self.place, Place::Held { .. })
    }
}

/// ワークが消えた場所
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Exit {
    Sink(u32),
    /// 排出端のコンベア
    Conveyor(u32),
}

/// 投入数・排出数
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    /// 投入口 id → 出した数
    pub fed: HashMap<u32, u32>,
    /// 消えた場所 → 数
    pub discharged: HashMap<Exit, u32>,
    /// ワーク種類 id → (投入数, 排出数)
    pub by_kind: HashMap<u32, (u32, u32)>,
}

impl Counts {
    pub fn total_fed(&self) -> u32 {
        self.fed.values().sum()
    }

    pub fn total_discharged(&self) -> u32 {
        self.discharged.values().sum()
    }

    fn count_fed(&mut self, source: u32, kind: u32) {
        *self.fed.entry(source).or_default() += 1;
        self.by_kind.entry(kind).or_default().0 += 1;
    }

    fn count_discharged(&mut self, exit: Exit, kind: u32) {
        *self.discharged.entry(exit).or_default() += 1;
        self.by_kind.entry(kind).or_default().1 += 1;
    }
}

/// 計算に必要な入力をまとめたもの
pub struct ConveyorInputs<'a> {
    pub config: &'a ConveyorConfig,
    /// 部品 id → 設計位置での外接箱 (投入口・排出口・ベルトに使う部品)
    pub parts: &'a HashMap<u32, Bounds>,
    pub motion: &'a dyn MachineMotion,
}

impl ConveyorInputs<'_> {
    fn belt(&self, conveyor: &Conveyor) -> Option<&Bounds> {
        self.parts.get(&conveyor.part?)
    }
}

/// ある時刻でのワークの状態と、次の刻みへ進めるための内部状態
#[derive(Clone, Debug, Default)]
pub struct ConveyorSim {
    /// 経過時刻 (秒)
    pub time: f64,
    pub works: Vec<Work>,
    pub counts: Counts,
    next_work_id: u32,
    /// 投入口 id → 次にワークを出す時刻
    next_feed: HashMap<u32, f64>,
    /// グリッパ id → 前の刻みでチャックが閉じていたか
    was_closed: HashMap<u32, bool>,
    /// 間隔では出さず、[`ConveyorSim::feed_external`] で頼まれたときだけ出す投入口
    driven: HashSet<u32>,
    /// 頼まれて、まだ出していないワーク (投入口 id, 荷札)。頼まれた順に出す。
    pending: VecDeque<(u32, u64)>,
    events: Vec<ConveyorEvent>,
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

    /// 仕掛数 (投入されてまだ排出されていないワーク)
    pub fn in_process(&self) -> usize {
        self.works.len()
    }

    /// 工程から送り込む投入口を決める。これらは間隔ではワークを出さない。
    pub fn drive_sources(&mut self, sources: impl IntoIterator<Item = u32>) {
        self.driven = sources.into_iter().collect();
    }

    pub fn is_driven(&self, source: u32) -> bool {
        self.driven.contains(&source)
    }

    /// 投入口 `source` から荷札 `tag` のワークを流すよう頼む。出す場所が空いた刻みで出る。
    pub fn feed_external(&mut self, source: u32, tag: u64) {
        self.pending.push_back((source, tag));
    }

    /// 頼まれてまだ出していないワークの数
    pub fn pending_feeds(&self) -> usize {
        self.pending.len()
    }

    /// たまった出来事を取り出す
    pub fn take_events(&mut self) -> Vec<ConveyorEvent> {
        std::mem::take(&mut self.events)
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
        let carriers = inputs.motion.carriers(self.time);

        self.feed(inputs);
        self.convey(inputs, dt);
        self.grip_and_release(inputs, &grippers, &carriers);
        self.follow(&grippers, &carriers);
        self.discharge_into_sinks(inputs);
        self.forget_old_works();
    }

    /// 投入口からワークを出す。出す場所がふさがっていたら空くまで待つ。
    fn feed(&mut self, inputs: &ConveyorInputs) {
        let config = inputs.config;
        for source in &config.sources {
            if self.driven.contains(&source.id) {
                self.feed_requested(inputs, source);
                continue;
            }
            let fed = self.counts.fed.get(&source.id).copied().unwrap_or(0);
            if source.limit.is_some_and(|limit| fed >= limit) {
                continue;
            }
            let due = self.next_feed.get(&source.id).copied().unwrap_or(0.0);
            if self.time < due {
                continue;
            }
            let Some(kind) = config.kind_for(source, fed) else {
                continue;
            };
            let Some((pose, place)) = self.feed_position(inputs, source.output, source.part, kind)
            else {
                continue;
            };
            self.next_feed
                .insert(source.id, due + source.interval.max(TIME_STEP));
            self.push_work(source.id, kind, pose, place, None);
        }
    }

    /// 工程から頼まれたワークを、頼まれた順に1つ出す (出す場所がふさがっていれば次の刻みに)
    fn feed_requested(&mut self, inputs: &ConveyorInputs, source: &super::config::Source) {
        let Some(i) = self.pending.iter().position(|(s, _)| *s == source.id) else {
            return;
        };
        let fed = self.counts.fed.get(&source.id).copied().unwrap_or(0);
        let Some(kind) = inputs.config.kind_for(source, fed) else {
            return;
        };
        let Some((pose, place)) = self.feed_position(inputs, source.output, source.part, kind)
        else {
            return;
        };
        let (_, tag) = self.pending.remove(i).expect("位置は上で見つけた");
        self.push_work(source.id, kind, pose, place, Some(tag));
        self.events.push(ConveyorEvent::Fed { tag });
    }

    fn push_work(
        &mut self,
        source: u32,
        kind: &WorkKind,
        pose: Pose,
        place: Place,
        tag: Option<u64>,
    ) {
        self.next_work_id += 1;
        self.works.push(Work {
            id: self.next_work_id,
            kind: kind.id,
            size: DVec3::from_array(kind.size),
            pose,
            place,
            tag,
        });
        self.counts.count_fed(source, kind.id);
    }

    /// 新しいワークを置く位置。ふさがっていれば None。
    fn feed_position(
        &self,
        inputs: &ConveyorInputs,
        output: SourceOutput,
        part: Option<u32>,
        kind: &WorkKind,
    ) -> Option<(Pose, Place)> {
        let size = DVec3::from_array(kind.size);
        match output {
            SourceOutput::Conveyor(id) => {
                let conveyor = inputs.config.conveyors.iter().find(|c| c.id == id)?;
                let belt = inputs.belt(conveyor)?;
                let length = conveyor.along(size);
                let start_is_free = self.works.iter().all(|w| match w.place {
                    Place::Conveyor { conveyor: c, s } if c == id => {
                        s >= length * (1.0 + QUEUE_GAP_RATIO)
                    }
                    _ => true,
                });
                start_is_free.then(|| {
                    let pose = Pose::from_translation(belt_point(conveyor, belt, size, 0.0));
                    (
                        pose,
                        Place::Conveyor {
                            conveyor: id,
                            s: 0.0,
                        },
                    )
                })
            }
            SourceOutput::OnPart => {
                let bounds = inputs.parts.get(&part?)?;
                let center = DVec3::new(
                    bounds.center.x,
                    bounds.top() + size.y / 2.0,
                    bounds.center.z,
                );
                // 前に出したワークがまだ残っていれば待つ (少し縮めて、隣に接しているだけなら出す)
                let new = Bounds { center, size };
                let margin = -size.min_element() * TOUCH_RATIO;
                let free = self
                    .works
                    .iter()
                    .all(|w| !w.bounds().overlaps(&new, margin));
                free.then(|| (Pose::from_translation(center), Place::Placed))
            }
        }
    }

    /// コンベア上のワークを流す。前のワークやストッパに当たったらそこで止まる。
    fn convey(&mut self, inputs: &ConveyorInputs, dt: f64) {
        for conveyor in &inputs.config.conveyors {
            let Some(belt) = inputs.belt(conveyor) else {
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
            // 排出端から完全に出たワークは消して数える
            let (counts, events) = (&mut self.counts, &mut self.events);
            self.works.retain(|w| {
                let gone = on_conveyor(w, conveyor.id) && conveyor_s(w) >= belt_length;
                if gone {
                    counts.count_discharged(Exit::Conveyor(conveyor.id), w.kind);
                    discharged(events, w);
                }
                !gone
            });
        }
    }

    /// チャックが閉じた瞬間に掴み、開いた瞬間に離す
    fn grip_and_release(
        &mut self,
        inputs: &ConveyorInputs,
        grippers: &[(&Gripper, GripperState)],
        carriers: &[Carrier],
    ) {
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
                // 別のグリッパが持っているワークも掴める (受け渡し)
                let nearest = self
                    .works
                    .iter_mut()
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
                put_down(&mut self.works[i], inputs, carriers);
            }
        }
    }

    /// 掴まれている・台に載っているワークを、親と一緒に動かす
    fn follow(&mut self, grippers: &[(&Gripper, GripperState)], carriers: &[Carrier]) {
        for work in &mut self.works {
            let parent = match work.place {
                Place::Held { gripper, offset } => grippers
                    .iter()
                    .find(|(g, _)| g.id == gripper)
                    .map(|(_, state)| (state.carrier, offset)),
                Place::Carried { carrier, offset } => carriers
                    .iter()
                    .find(|c| c.id == carrier)
                    .map(|c| (c.pose, offset)),
                Place::Conveyor { .. } | Place::Placed => None,
            };
            if let Some((pose, offset)) = parent {
                work.pose = pose.then_apply_to(offset);
            }
        }
    }

    /// 排出口に触れたワーク (掴まれているものは除く) を消して数える
    fn discharge_into_sinks(&mut self, inputs: &ConveyorInputs) {
        let sinks: Vec<(u32, Bounds)> = inputs
            .config
            .sinks
            .iter()
            .filter_map(|s| Some((s.id, *inputs.parts.get(&s.part?)?)))
            .collect();
        if sinks.is_empty() {
            return;
        }
        let (counts, events) = (&mut self.counts, &mut self.events);
        self.works.retain(|w| {
            if w.is_held() {
                return true;
            }
            let bounds = w.bounds();
            let margin = w.size.min_element() * TOUCH_RATIO;
            match sinks.iter().find(|(_, s)| s.overlaps(&bounds, margin)) {
                Some((id, _)) => {
                    counts.count_discharged(Exit::Sink(*id), w.kind);
                    discharged(events, w);
                    false
                }
                None => true,
            }
        });
    }

    fn forget_old_works(&mut self) {
        while self.works.len() > MAX_WORKS {
            let work = match self.works.iter().position(|w| w.place == Place::Placed) {
                Some(i) => self.works.remove(i),
                None => self.works.remove(0),
            };
            discharged(&mut self.events, &work);
        }
    }
}

/// 工程から送り込んだワークなら、出ていったことを知らせる
fn discharged(events: &mut Vec<ConveyorEvent>, work: &Work) {
    if let Some(tag) = work.tag {
        events.push(ConveyorEvent::Discharged { tag });
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
fn belt_point(conveyor: &Conveyor, belt: &Bounds, work_size: DVec3, s: f64) -> DVec3 {
    let direction = conveyor.direction();
    let belt_start = belt.center - direction * conveyor.along(belt.size) / 2.0;
    let height = DVec3::Y * (belt.size.y + work_size.y) / 2.0;
    belt_start + direction * (s + conveyor.along(work_size) / 2.0) + height
}

/// ワークの下面が面 `surface` の上面にほぼ接していて、真上から見て中にある
fn rests_on(work: &Work, surface: &Bounds) -> bool {
    let center = work.pose.translation;
    let bottom = center.y - work.size.y / 2.0;
    surface.contains_xz(center)
        && (bottom - surface.top()).abs() <= work.size.y * PLACE_TOLERANCE_RATIO
}

/// 離したワークの行き先を決める (親の付け替え)。
/// 真下がコンベアならそのコンベア (ベルトの中心線に寄せる)、動く台ならその台、どちらでもなければその場に置く。
fn put_down(work: &mut Work, inputs: &ConveyorInputs, carriers: &[Carrier]) {
    for conveyor in &inputs.config.conveyors {
        let Some(belt) = inputs.belt(conveyor) else {
            continue;
        };
        if !rests_on(work, belt) {
            continue;
        }
        let direction = conveyor.direction();
        let belt_start = belt.center - direction * conveyor.along(belt.size) / 2.0;
        let max_s = (conveyor.along(belt.size) - conveyor.along(work.size)).max(0.0);
        let s = ((work.pose.translation - belt_start).dot(direction)
            - conveyor.along(work.size) / 2.0)
            .clamp(0.0, max_s);
        work.place = Place::Conveyor {
            conveyor: conveyor.id,
            s,
        };
        work.pose = Pose::from_translation(belt_point(conveyor, belt, work.size, s));
        return;
    }
    let carrier = carriers
        .iter()
        .find(|c| c.surfaces.iter().any(|s| rests_on(work, s)));
    work.place = match carrier {
        Some(c) => Place::Carried {
            carrier: c.id,
            offset: c.pose.inverse().then_apply_to(work.pose),
        },
        None => Place::Placed,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conveyor::config::{BeltAxis, Sink, Source};

    const BELT_PART: u32 = 10;
    const SINK_PART: u32 = 20;
    const KIND_A: u32 = 100;
    const KIND_B: u32 = 101;

    fn parts() -> HashMap<u32, Bounds> {
        HashMap::from([
            (
                BELT_PART,
                Bounds {
                    center: DVec3::new(0.0, 0.5, 0.0),
                    size: DVec3::new(2.0, 1.0, 0.6),
                },
            ),
            (
                SINK_PART,
                Bounds {
                    center: DVec3::new(5.0, 0.5, 0.0),
                    size: DVec3::new(1.0, 1.0, 1.0),
                },
            ),
        ])
    }

    fn kind(id: u32) -> WorkKind {
        WorkKind {
            id,
            name: format!("種類 {id}"),
            size: [0.4, 0.2, 0.4],
            color: [1.0; 3],
        }
    }

    /// 長さ 2.0 のベルト (x = -1 〜 1) の始端へ、0.5 秒おきにワークを出す設定
    fn line(end: ConveyorEnd) -> ConveyorConfig {
        ConveyorConfig {
            kinds: vec![kind(KIND_A), kind(KIND_B)],
            sources: vec![Source {
                id: 2,
                name: "投入口".into(),
                part: None,
                output: SourceOutput::Conveyor(1),
                interval: 0.5,
                kinds: vec![KIND_A],
                limit: None,
            }],
            conveyors: vec![Conveyor {
                id: 1,
                name: "コンベア".into(),
                part: Some(BELT_PART),
                axis: BeltAxis::X,
                reversed: false,
                speed: 1.0,
                end,
            }],
            ..Default::default()
        }
    }

    /// グリッパも台も動かさない
    struct NoMotion;

    impl MachineMotion for NoMotion {
        fn gripper_state(&self, _: &Gripper, _: f64) -> Option<GripperState> {
            None
        }
    }

    /// 時刻ごとの台本でグリッパ 1 を動かす
    struct Script {
        close_at: f64,
        open_at: f64,
        offset: fn(f64) -> DVec3,
        carriers: fn(f64) -> Vec<Carrier>,
    }

    impl Script {
        fn new(close_at: f64, open_at: f64, offset: fn(f64) -> DVec3) -> Self {
            Self {
                close_at,
                open_at,
                offset,
                carriers: |_| Vec::new(),
            }
        }
    }

    impl MachineMotion for Script {
        fn gripper_state(&self, gripper: &Gripper, t: f64) -> Option<GripperState> {
            (gripper.id == 1).then(|| GripperState {
                closed: t >= self.close_at && t < self.open_at,
                carrier: Pose::from_translation((self.offset)(t)),
            })
        }

        fn carriers(&self, t: f64) -> Vec<Carrier> {
            (self.carriers)(t)
        }
    }

    fn run(config: &ConveyorConfig, motion: &dyn MachineMotion, seconds: f64) -> ConveyorSim {
        let parts = parts();
        let inputs = ConveyorInputs {
            config,
            parts: &parts,
            motion,
        };
        let mut sim = ConveyorSim::default();
        sim.advance_to(&inputs, seconds);
        sim
    }

    fn gripper(id: u32, tcp: [f64; 3]) -> Gripper {
        Gripper {
            id,
            name: format!("チャック {id}"),
            chuck: Some(id),
            tcp,
            reach: 0.2,
        }
    }

    /// ワークを 1 個だけ流し、ストッパ (中心 x = 0.8) で止まったところをグリッパ 1 で掴む設定
    fn single_work_line() -> ConveyorConfig {
        let mut config = line(ConveyorEnd::Stopper);
        config.sources[0].limit = Some(1);
        config.grippers = vec![gripper(1, [0.8, 1.1, 0.0])];
        config
    }

    #[test]
    fn works_queue_up_at_the_stopper() {
        let sim = run(&line(ConveyorEnd::Stopper), &NoMotion, 10.0);
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
    fn discharge_end_removes_and_counts_works() {
        let sim = run(&line(ConveyorEnd::Discharge), &NoMotion, 10.0);
        // 2 秒でベルトを渡り切るので、0.5 秒おきの投入なら常に 4 〜 5 個しか載っていない
        assert!((4..=5).contains(&sim.works.len()), "{}", sim.works.len());
        let counts = &sim.counts;
        assert_eq!(counts.total_fed(), 20); // 0, 0.5, …, 9.5 秒
        assert_eq!(
            counts.total_discharged() as usize + sim.in_process(),
            counts.total_fed() as usize
        );
        assert_eq!(
            counts.discharged.get(&Exit::Conveyor(1)).copied(),
            Some(counts.total_discharged())
        );
        assert_eq!(counts.by_kind[&KIND_A].0, 20);
    }

    #[test]
    fn source_alternates_kinds_and_stops_at_its_limit() {
        let mut config = line(ConveyorEnd::Discharge);
        config.sources[0].kinds = vec![KIND_A, KIND_B];
        config.sources[0].limit = Some(3);
        let sim = run(&config, &NoMotion, 1.4);
        let kinds: Vec<u32> = sim.works.iter().map(|w| w.kind).collect();
        assert_eq!(kinds, vec![KIND_A, KIND_B, KIND_A]);
        let later = run(&config, &NoMotion, 20.0);
        assert_eq!(later.counts.total_fed(), 3);
        assert!(later.works.is_empty());
    }

    #[test]
    fn source_on_a_part_waits_until_the_spot_is_free() {
        let mut config = line(ConveyorEnd::Stopper);
        config.sources[0].output = SourceOutput::OnPart;
        config.sources[0].part = Some(BELT_PART);
        let sim = run(&config, &NoMotion, 3.0);
        // 置いたワークは動かないので、2 個目以降は出せない
        assert_eq!(sim.works.len(), 1);
        let work = &sim.works[0];
        assert_eq!(work.place, Place::Placed);
        assert!((work.pose.translation - DVec3::new(0.0, 1.1, 0.0)).length() < 1e-9);
    }

    #[test]
    fn sink_removes_and_counts_works_that_touch_it() {
        let mut config = line(ConveyorEnd::Stopper);
        config.sources[0].output = SourceOutput::OnPart;
        config.sources[0].part = Some(SINK_PART);
        config.sinks = vec![Sink {
            id: 3,
            name: "排出口".into(),
            part: Some(SINK_PART),
        }];
        // 排出口の上に出したワークはすぐ消えるので、0.5 秒おきに出し続けられる
        let sim = run(&config, &NoMotion, 2.2);
        assert!(sim.works.is_empty());
        assert_eq!(sim.counts.total_fed(), 5);
        assert_eq!(sim.counts.discharged[&Exit::Sink(3)], 5);
    }

    #[test]
    fn gripper_carries_work_and_releases_it() {
        // 5 秒で閉じ、5〜6 秒で 1.0 上げ、6〜7 秒で Z に 2.0 動かし、7 秒で開く
        let motion = Script::new(5.0, 7.0, |t| {
            DVec3::new(
                0.0,
                (t - 5.0).clamp(0.0, 1.0),
                2.0 * (t - 6.0).clamp(0.0, 1.0),
            )
        });
        let config = single_work_line();

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
        let motion = Script::new(5.0, 6.0, |t| {
            DVec3::new(0.0, 0.0, 0.1 * (t - 5.0).clamp(0.0, 1.0))
        });
        let sim = run(&single_work_line(), &motion, 7.0);
        let work = &sim.works[0];
        assert!(matches!(work.place, Place::Conveyor { conveyor: 1, .. }));
        // コンベアに載ると、ベルトの中心線 (z = 0) に寄せられる
        assert!(work.pose.translation.z.abs() < 1e-9, "{work:?}");
    }

    #[test]
    fn released_work_rides_on_a_moving_table() {
        // テーブル (上面 y = 1.0、x = 2.5 〜 3.5) は 8 秒から Z に動く
        fn table(t: f64) -> Vec<Carrier> {
            let pose = Pose::from_translation(DVec3::new(0.0, 0.0, (t - 8.0).max(0.0)));
            let design = Bounds {
                center: DVec3::new(3.0, 0.5, 0.0),
                size: DVec3::ONE,
            };
            vec![Carrier {
                id: 7,
                pose,
                surfaces: vec![design.transformed(pose)],
            }]
        }
        // 5 秒で掴み、5〜6 秒で X に 2.2 動かして (x = 3.0)、7 秒で離す
        let mut motion = Script::new(5.0, 7.0, |t| {
            DVec3::new(2.2 * (t - 5.0).clamp(0.0, 1.0), 0.0, 0.0)
        });
        motion.carriers = table;
        let sim = run(&single_work_line(), &motion, 9.5);
        let work = &sim.works[0];
        assert!(matches!(work.place, Place::Carried { carrier: 7, .. }));
        assert!((work.pose.translation - DVec3::new(3.0, 1.1, 1.5)).length() < 1e-6);
    }

    #[test]
    fn grippers_hand_a_work_over() {
        /// グリッパ 1 は 5 秒で掴んで X に 1.0 運び、8 秒で開く。グリッパ 2 (固定) は 7 秒で閉じる。
        struct HandOver;
        impl MachineMotion for HandOver {
            fn gripper_state(&self, gripper: &Gripper, t: f64) -> Option<GripperState> {
                Some(match gripper.id {
                    1 => GripperState {
                        closed: (5.0..8.0).contains(&t),
                        carrier: Pose::from_translation(DVec3::X * (t - 5.0).clamp(0.0, 1.0)),
                    },
                    _ => GripperState {
                        closed: t >= 7.0,
                        carrier: Pose::IDENTITY,
                    },
                })
            }
        }
        let mut config = single_work_line();
        config.grippers.push(gripper(2, [1.8, 1.1, 0.0]));
        let sim = run(&config, &HandOver, 9.0);
        let work = &sim.works[0];
        assert!(matches!(work.place, Place::Held { gripper: 2, .. }));
        assert!((work.pose.translation - DVec3::new(1.8, 1.1, 0.0)).length() < 1e-6);
    }

    #[test]
    fn nothing_is_gripped_out_of_reach() {
        // 0.5 秒ではワークはまだ始端にいて、TCP (x = 0.8) から遠い
        let motion = Script::new(0.5, 100.0, |_| DVec3::ZERO);
        let sim = run(&single_work_line(), &motion, 3.0);
        assert!(!sim.works[0].is_held());
    }

    #[test]
    fn seeking_back_recomputes_from_the_start() {
        let config = line(ConveyorEnd::Stopper);
        let parts = parts();
        let inputs = ConveyorInputs {
            config: &config,
            parts: &parts,
            motion: &NoMotion,
        };
        let mut sim = ConveyorSim::default();
        sim.seek(&inputs, 5.0);
        sim.seek(&inputs, 0.25);
        assert_eq!(sim.works.len(), 1);
        assert_eq!(sim.counts.total_fed(), 1);
        assert!((conveyor_s(&sim.works[0]) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn driven_source_feeds_only_requested_works_and_reports_them() {
        let config = line(ConveyorEnd::Discharge);
        let parts = parts();
        let inputs = ConveyorInputs {
            config: &config,
            parts: &parts,
            motion: &NoMotion,
        };
        let mut sim = ConveyorSim::default();
        sim.drive_sources([2]);
        // 頼まれるまで、間隔 (0.5 秒) が来ても出さない
        sim.advance_to(&inputs, 1.0);
        assert!(sim.works.is_empty());
        assert!(sim.take_events().is_empty());

        // 2 個続けて頼むと、始端が空いた順に出る
        sim.feed_external(2, 7);
        sim.feed_external(2, 8);
        sim.advance_to(&inputs, 1.1);
        assert_eq!(sim.take_events(), vec![ConveyorEvent::Fed { tag: 7 }]);
        assert_eq!(sim.pending_feeds(), 1);
        assert_eq!(sim.works[0].tag, Some(7));
        sim.advance_to(&inputs, 2.0);
        assert_eq!(sim.take_events(), vec![ConveyorEvent::Fed { tag: 8 }]);

        // 長さ 2.0 のベルトを速さ 1.0 で流れて排出端から出る
        sim.advance_to(&inputs, 4.0);
        assert_eq!(
            sim.take_events(),
            vec![
                ConveyorEvent::Discharged { tag: 7 },
                ConveyorEvent::Discharged { tag: 8 }
            ]
        );
        assert_eq!(sim.counts.total_discharged(), 2);
    }
}
