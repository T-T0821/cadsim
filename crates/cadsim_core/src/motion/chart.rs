//! タイミングチャートのデータ (ステップベース / PLC 風)
//!
//! - アクチュエータ (シリンダ・直動軸・回転テーブル) が部品を動かす。
//! - シーケンスはステップの並び。1つのステップには「どのアクチュエータを、どこまで、何秒で」
//!   動かすかの動作 (Action) を複数置ける。同じステップの動作は同時に始まる。
//! - ステップ内の動作がすべて完了し、さらにタイマー (dwell) が経過したら次のステップへ進む。
//! - ステップには開始条件 (インターロック: 「センサー X が ON」など) を付けられる。
//!   条件が成立するまでそのステップの動作は始まらず、[`INTERLOCK_TIMEOUT`] 秒待っても
//!   成立しなければ異常停止としてサイクルを止める。
//!
//! 再生は [`TimingChart::schedule`] で「何秒から何秒まで、どの値へ動くか」の時刻表に
//! 変換してから行う。時刻表は純粋な計算なので、Bevy に依存せずテストできる。

use std::collections::HashMap;

pub use super::sensor::{Sensor, SensorKind};

use super::profile;
pub use super::profile::CamTable;
use super::robot_path::{self, PathSample};

/// アクチュエータの種類
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActuatorKind {
    /// エアシリンダ。値は 0 = 戻り端 / 1 = 出端。移動量は stroke。
    Cylinder,
    /// 直動軸 (サーボ・電動スライダ)。値は原点からの移動量 (モデル単位)。
    LinearAxis,
    /// 回転テーブル・インデックス。値は原点からの角度 (度)。
    RotaryTable,
}

impl ActuatorKind {
    pub const ALL: [ActuatorKind; 3] = [
        ActuatorKind::Cylinder,
        ActuatorKind::LinearAxis,
        ActuatorKind::RotaryTable,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ActuatorKind::Cylinder => "シリンダ",
            ActuatorKind::LinearAxis => "直動軸",
            ActuatorKind::RotaryTable => "回転テーブル",
        }
    }

    /// 値の単位 (UI 表示用)
    pub fn unit(self) -> &'static str {
        match self {
            ActuatorKind::Cylinder => "",
            ActuatorKind::LinearAxis => "",
            ActuatorKind::RotaryTable => "°",
        }
    }
}

/// 動く方向 (直動は移動方向、回転は回転軸)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    pub fn label(self) -> &'static str {
        match self {
            Axis::X => "X",
            Axis::Y => "Y",
            Axis::Z => "Z",
        }
    }

    pub fn unit_vector(self) -> [f64; 3] {
        match self {
            Axis::X => [1.0, 0.0, 0.0],
            Axis::Y => [0.0, 1.0, 0.0],
            Axis::Z => [0.0, 0.0, 1.0],
        }
    }
}

/// 動き始めと止まり際の速度の付け方
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Easing {
    /// 一定速度
    Linear,
    /// 加減速あり (S字、3 次式)
    Smooth,
    /// 台形加減速。`accel` は加速 (と減速) にかける時間の割合 (0〜0.5)。
    Trapezoid { accel: f64 },
    /// サイクロイド (加速度が 0 から始まり 0 で終わる)
    Cycloid,
    /// 電子カム表 (チャートの `cams` の id)
    Cam(u32),
}

impl Easing {
    /// 選べる種類 (カムは表ごとに別に並べる)
    pub const BASIC: [Easing; 4] = [
        Easing::Smooth,
        Easing::Trapezoid { accel: 0.25 },
        Easing::Cycloid,
        Easing::Linear,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Easing::Linear => "等速",
            Easing::Smooth => "S字加減速",
            Easing::Trapezoid { .. } => "台形加減速",
            Easing::Cycloid => "サイクロイド",
            Easing::Cam(_) => "電子カム",
        }
    }

    /// 種類が同じか (台形の割合やカムの表の違いは見ない)
    pub fn same_kind(self, other: Easing) -> bool {
        std::mem::discriminant(&self) == std::mem::discriminant(&other)
    }

    /// 0..1 の進み具合を、速度の付け方に合わせた 0..1 に変換する。
    /// 電子カムは表がないと分からないので等速として扱う ([`Easing::apply_with`] を使う)。
    pub fn apply(self, x: f64) -> f64 {
        self.apply_with(x, &[])
    }

    /// 電子カムの表 `cams` も使って変換する
    pub fn apply_with(self, x: f64, cams: &[CamTable]) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Easing::Linear => x,
            Easing::Smooth => x * x * (3.0 - 2.0 * x),
            Easing::Trapezoid { accel } => profile::trapezoid(x, accel),
            Easing::Cycloid => profile::cycloid(x),
            Easing::Cam(id) => cams.iter().find(|c| c.id == id).map_or(x, |c| c.eval(x)),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Actuator {
    pub id: u32,
    pub name: String,
    pub kind: ActuatorKind,
    pub axis: Axis,
    /// true なら軸のマイナス方向に動く (回転なら逆回り)
    pub reversed: bool,
    /// シリンダのストローク (モデル単位)。シリンダ以外では使わない。
    pub stroke: f64,
    /// 回転の中心 (モデル座標)。回転テーブル以外では使わない。
    pub pivot: [f64; 3],
    /// この装置が載っている親アクチュエータ。親が動くと、この装置ごと一緒に動く。
    pub mounted_on: Option<u32>,
    /// このアクチュエータが動かす部品の id
    pub parts: Vec<u32>,
    pub easing: Easing,
    /// 新しく動作を置いたときの動作時間 (秒)
    pub default_duration: f64,
    /// 可動範囲 [下限, 上限] (直動はモデル単位、回転は度)。None なら制限なし。
    pub limits: Option<[f64; 2]>,
}

impl Actuator {
    pub fn new(id: u32, kind: ActuatorKind) -> Self {
        let (axis, stroke, default_duration) = match kind {
            ActuatorKind::Cylinder => (Axis::Y, 0.5, 0.4),
            ActuatorKind::LinearAxis => (Axis::X, 0.0, 1.0),
            ActuatorKind::RotaryTable => (Axis::Y, 0.0, 0.8),
        };
        Self {
            id,
            name: format!("{} {id}", kind.label()),
            kind,
            axis,
            reversed: false,
            stroke,
            pivot: [0.0; 3],
            mounted_on: None,
            parts: Vec::new(),
            easing: Easing::Smooth,
            default_duration,
            limits: None,
        }
    }

    /// 値を可動範囲に収める
    pub fn clamp(&self, value: f64) -> f64 {
        match self.limits {
            Some([lo, hi]) if lo <= hi => value.clamp(lo, hi),
            _ => value,
        }
    }

    /// ロボットの関節として逆運動学で動かせるか (シリンダは出 / 戻の2値なので対象外)
    pub fn is_joint(&self) -> bool {
        self.kind != ActuatorKind::Cylinder
    }

    /// チャート上の値を表示用の文字にする (シリンダは出端 / 戻り端)
    pub fn format_value(&self, value: f64) -> String {
        match self.kind {
            ActuatorKind::Cylinder if value >= 0.5 => "出".into(),
            ActuatorKind::Cylinder => "戻".into(),
            ActuatorKind::LinearAxis => format!("{value:.2}"),
            ActuatorKind::RotaryTable => format!("{value:.0}°"),
        }
    }
}

/// 1つのステップで1つのアクチュエータに出す指令
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub actuator: u32,
    /// 動かした先の値 (シリンダは 0 / 1、直動は移動量、回転は角度)
    pub target: f64,
    /// 動作にかかる時間 (秒)
    pub duration: f64,
}

/// インターロックの条件が成立しないまま、この秒数待ったら異常停止とする
pub const INTERLOCK_TIMEOUT: f64 = 5.0;

/// 条件待ちのあいだ、センサーを見直す間隔 (秒)
const INTERLOCK_POLL: f64 = 0.02;

/// ステップの開始条件: 「センサー `sensor` が `on` (ON / OFF) になっていること」
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interlock {
    pub sensor: u32,
    pub on: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    /// ステップの説明 ("ワーク把持" など)
    pub comment: String,
    pub actions: Vec<Action>,
    /// 動作完了後に待つ時間 (秒)。タイマー待ちや確認時間に使う。
    pub dwell: f64,
    /// 開始条件。すべて成立するまで、このステップの動作を始めない。
    pub interlocks: Vec<Interlock>,
    /// ロボットの教示点への移動 (各関節の動作に展開される)
    pub moves: Vec<RobotMove>,
}

impl Step {
    pub fn action(&self, actuator: u32) -> Option<&Action> {
        self.actions.iter().find(|a| a.actuator == actuator)
    }

    pub fn action_mut(&mut self, actuator: u32) -> Option<&mut Action> {
        self.actions.iter_mut().find(|a| a.actuator == actuator)
    }

    pub fn interlock(&self, sensor: u32) -> Option<&Interlock> {
        self.interlocks.iter().find(|i| i.sensor == sensor)
    }

    /// センサー `sensor` の条件を付け替える (`None` なら外す)
    pub fn set_interlock(&mut self, sensor: u32, on: Option<bool>) {
        self.interlocks.retain(|i| i.sensor != sensor);
        if let Some(on) = on {
            self.interlocks.push(Interlock { sensor, on });
        }
    }

    pub fn robot_move(&self, robot: u32) -> Option<&RobotMove> {
        self.moves.iter().find(|m| m.robot == robot)
    }

    pub fn robot_move_mut(&mut self, robot: u32) -> Option<&mut RobotMove> {
        self.moves.iter_mut().find(|m| m.robot == robot)
    }

    /// 全動作が完了するまでの時間 + タイマー
    pub fn duration(&self) -> f64 {
        let motion = self
            .actions
            .iter()
            .map(|a| a.duration)
            .chain(self.moves.iter().map(|m| m.duration))
            .map(|d| d.max(0.0))
            .fold(0.0, f64::max);
        motion + self.dwell.max(0.0)
    }
}

/// ロボットの教示点 (TCP を持っていく位置と、工具の向き)
#[derive(Clone, Debug, PartialEq)]
pub struct TeachPoint {
    pub id: u32,
    pub name: String,
    /// TCP の目標位置 (モデル座標)
    pub position: [f64; 3],
    /// 工具軸を向けたい方向 (モデル座標)。None なら位置だけ合わせる。
    pub approach: Option<[f64; 3]>,
}

/// 関節 (回転テーブル / 直動軸) を載せ先でつないだ直列リンクを、1台のロボットとして扱う。
///
/// 関節は先端の関節 `tip` から載せ先をたどった全アクチュエータ。
/// 教示点への移動は逆運動学で各関節の目標値に直してからチャートに展開する。
#[derive(Clone, Debug, PartialEq)]
pub struct Robot {
    pub id: u32,
    pub name: String,
    /// 先端 (手首) の関節。TCP はこの関節と一緒に動く。
    pub tip: u32,
    /// 設計位置 (全関節 0) での TCP の位置 (モデル座標)
    pub tcp: [f64; 3],
    /// 設計位置での工具軸の向き (モデル座標)
    pub tool_axis: [f64; 3],
    pub points: Vec<TeachPoint>,
    pub next_point_id: u32,
    /// 速度の上限 (サイクルタイムの見積もりとロボット言語の書き出しに使う)
    pub speed: RobotSpeed,
}

/// ロボットの速度の上限
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RobotSpeed {
    /// 回転関節の最高速度 (度/秒)
    pub joint: f64,
    /// TCP と直動関節の最高速度 (モデル単位/秒)。None ならロボットの大きさの 2 倍/秒。
    pub linear: Option<f64>,
    /// 最高速度まで加速する (減速する) のにかかる時間 (秒)
    pub accel_time: f64,
}

impl Default for RobotSpeed {
    fn default() -> Self {
        Self {
            joint: 180.0,
            linear: None,
            accel_time: 0.2,
        }
    }
}

impl Robot {
    pub fn point(&self, id: u32) -> Option<&TeachPoint> {
        self.points.iter().find(|p| p.id == id)
    }

    pub fn add_point(
        &mut self,
        name: String,
        position: [f64; 3],
        approach: Option<[f64; 3]>,
    ) -> u32 {
        self.next_point_id = self.next_point_id.max(1);
        let id = self.next_point_id;
        self.next_point_id += 1;
        self.points.push(TeachPoint {
            id,
            name,
            position,
            approach,
        });
        id
    }
}

/// ロボットの移動の補間のしかた
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MoveKind {
    /// 関節補間 (MoveJ): 各関節を同時に動かす。TCP の通り道は曲線になる。
    #[default]
    Joint,
    /// 直線補間 (MoveL): TCP を教示点まで直線で動かす
    Linear,
}

impl MoveKind {
    pub fn label(self) -> &'static str {
        match self {
            MoveKind::Joint => "関節補間 (MoveJ)",
            MoveKind::Linear => "直線補間 (MoveL)",
        }
    }
}

/// 1つのステップでロボットに出す「教示点へ移動」の指令
#[derive(Clone, Debug, PartialEq)]
pub struct RobotMove {
    pub robot: u32,
    pub point: u32,
    pub duration: f64,
    pub kind: MoveKind,
}

/// タイミングチャート全体 (アクチュエータ・センサー一覧 + ステップの並び)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimingChart {
    pub actuators: Vec<Actuator>,
    pub sensors: Vec<Sensor>,
    pub steps: Vec<Step>,
    pub next_actuator_id: u32,
    pub next_sensor_id: u32,
    pub robots: Vec<Robot>,
    pub next_robot_id: u32,
    /// 電子カム表 (アクチュエータの加減速で選ぶ)
    pub cams: Vec<CamTable>,
    pub next_cam_id: u32,
}

impl TimingChart {
    pub fn cam_mut(&mut self, id: u32) -> Option<&mut CamTable> {
        self.cams.iter_mut().find(|c| c.id == id)
    }

    /// 電子カム表を追加する。最初は「前半で加速、後半で減速」の点を入れておく。
    pub fn add_cam(&mut self) -> u32 {
        self.next_cam_id = self.next_cam_id.max(1);
        let id = self.next_cam_id;
        self.next_cam_id += 1;
        self.cams.push(CamTable {
            id,
            name: format!("カム {id}"),
            points: vec![[0.25, 0.1], [0.5, 0.5], [0.75, 0.9]],
        });
        id
    }

    /// 電子カム表を消す。その表を使っていたアクチュエータは S字加減速に戻す。
    pub fn remove_cam(&mut self, id: u32) {
        self.cams.retain(|c| c.id != id);
        for actuator in &mut self.actuators {
            if actuator.easing == Easing::Cam(id) {
                actuator.easing = Easing::Smooth;
            }
        }
    }

    pub fn add_actuator(&mut self, kind: ActuatorKind) -> u32 {
        self.next_actuator_id = self.next_actuator_id.max(1);
        let id = self.next_actuator_id;
        self.next_actuator_id += 1;
        self.actuators.push(Actuator::new(id, kind));
        id
    }

    pub fn actuator(&self, id: u32) -> Option<&Actuator> {
        self.actuators.iter().find(|a| a.id == id)
    }

    pub fn actuator_mut(&mut self, id: u32) -> Option<&mut Actuator> {
        self.actuators.iter_mut().find(|a| a.id == id)
    }

    pub fn add_sensor(&mut self, kind: SensorKind) -> u32 {
        self.next_sensor_id = self.next_sensor_id.max(1);
        let id = self.next_sensor_id;
        self.next_sensor_id += 1;
        self.sensors.push(Sensor::new(id, kind));
        id
    }

    pub fn sensor(&self, id: u32) -> Option<&Sensor> {
        self.sensors.iter().find(|s| s.id == id)
    }

    pub fn sensor_mut(&mut self, id: u32) -> Option<&mut Sensor> {
        self.sensors.iter_mut().find(|s| s.id == id)
    }

    /// センサーを消す。そのセンサーを使ったインターロックも外す。
    pub fn remove_sensor(&mut self, id: u32) {
        self.sensors.retain(|s| s.id != id);
        for step in &mut self.steps {
            step.interlocks.retain(|i| i.sensor != id);
        }
    }

    pub fn robot(&self, id: u32) -> Option<&Robot> {
        self.robots.iter().find(|r| r.id == id)
    }

    pub fn robot_mut(&mut self, id: u32) -> Option<&mut Robot> {
        self.robots.iter_mut().find(|r| r.id == id)
    }

    /// 先端の関節 `tip` のロボットを追加する。TCP は設計位置の原点に置くので、あとで合わせる。
    pub fn add_robot(&mut self, tip: u32) -> u32 {
        self.next_robot_id = self.next_robot_id.max(1);
        let id = self.next_robot_id;
        self.next_robot_id += 1;
        self.robots.push(Robot {
            id,
            name: format!("ロボット {id}"),
            tip,
            tcp: [0.0; 3],
            tool_axis: [0.0, 1.0, 0.0],
            points: Vec::new(),
            next_point_id: 1,
            speed: RobotSpeed::default(),
        });
        id
    }

    /// ロボットと、そのロボットへの移動指令を消す
    pub fn remove_robot(&mut self, id: u32) {
        self.robots.retain(|r| r.id != id);
        for step in &mut self.steps {
            step.moves.retain(|m| m.robot != id);
        }
    }

    /// 教示点を消す。その点への移動指令も消す。
    pub fn remove_point(&mut self, robot: u32, point: u32) {
        if let Some(r) = self.robot_mut(robot) {
            r.points.retain(|p| p.id != point);
        }
        for step in &mut self.steps {
            step.moves
                .retain(|m| !(m.robot == robot && m.point == point));
        }
    }

    /// アクチュエータを消す。そのアクチュエータへの動作と、載せていた子・センサーの親設定も外す。
    pub fn remove_actuator(&mut self, id: u32) {
        self.actuators.retain(|a| a.id != id);
        for actuator in &mut self.actuators {
            if actuator.mounted_on == Some(id) {
                actuator.mounted_on = None;
            }
        }
        for sensor in &mut self.sensors {
            if sensor.mounted_on == Some(id) {
                sensor.mounted_on = None;
            }
        }
        for step in &mut self.steps {
            step.actions.retain(|a| a.actuator != id);
        }
        // 先端の関節を消したロボットは関節がなくなるので一緒に消す
        let orphaned: Vec<u32> = self
            .robots
            .iter()
            .filter(|r| r.tip == id)
            .map(|r| r.id)
            .collect();
        for robot in orphaned {
            self.remove_robot(robot);
        }
    }

    /// ロボットに載って動くアクチュエータ (関節の根元と、そこに載ったすべて。グリッパなども含む)
    pub fn robot_actuators(&self, robot: &Robot) -> Vec<u32> {
        let mut root = robot.tip;
        for _ in 0..self.actuators.len() {
            match self.actuator(root).and_then(|a| a.mounted_on) {
                Some(parent) => root = parent,
                None => break,
            }
        }
        self.actuators
            .iter()
            .map(|a| a.id)
            .filter(|&id| id == root || self.is_mounted_under(id, root))
            .collect()
    }

    /// `id` の親をたどった先に `ancestor` があるか (載せ替えで循環しないための確認)
    pub fn is_mounted_under(&self, id: u32, ancestor: u32) -> bool {
        let mut current = self.actuator(id).and_then(|a| a.mounted_on);
        // 壊れたデータで無限ループしないよう、たどる回数に上限を付ける
        for _ in 0..self.actuators.len() {
            match current {
                Some(parent) if parent == ancestor => return true,
                Some(parent) => current = self.actuator(parent).and_then(|a| a.mounted_on),
                None => return false,
            }
        }
        false
    }

    /// ステップの並びを時刻表に変換する (インターロックはすべて成立しているものとする)
    pub fn schedule(&self) -> Schedule {
        self.schedule_with(|_, _, _| Vec::new())
    }

    /// ステップの並びを時刻表に変換する。
    ///
    /// `unmet(途中までの時刻表, t, ステップ)` は、時刻 t にそのステップの開始条件のうち
    /// 成立していないものを返す。条件付きのステップでは、前のステップが終わった時刻から
    /// 少しずつ時刻を進めて、すべて成立した時刻に動作を始める。
    /// 途中までの時刻表には t より後の動作が入っていないが、ステップの境目では
    /// すべての動作が終わっているので、部品の位置は正しく求まる。
    pub fn schedule_with(
        &self,
        mut unmet: impl FnMut(&Schedule, f64, &Step) -> Vec<Interlock>,
    ) -> Schedule {
        let mut schedule = Schedule {
            cams: self.cams.clone(),
            ..Default::default()
        };
        // 各アクチュエータの、直前までの動作で決まった値 (サイクル開始時はすべて 0)
        let mut values: HashMap<u32, f64> = HashMap::new();
        let mut time = 0.0;
        for (index, step) in self.steps.iter().enumerate() {
            let start = time;
            let Some(action_start) = wait_for_interlocks(&schedule, start, step, &mut unmet) else {
                // 時間切れ: このステップで止まる。待った時間までを時刻表に残す。
                let end = start + INTERLOCK_TIMEOUT;
                schedule.stop = Some(InterlockStop {
                    step: index,
                    time: end,
                    unmet: unmet(&schedule, start, step),
                });
                schedule.steps.push(StepSpan {
                    start,
                    action_start: end,
                    end,
                });
                // 実行されないステップは長さ 0 として並べておく (チャートの列はそのまま描く)
                for _ in index + 1..self.steps.len() {
                    schedule.steps.push(StepSpan {
                        start: end,
                        action_start: end,
                        end,
                    });
                }
                schedule.cycle_time = end;
                return schedule;
            };
            // アクチュエータごとの、このステップでの動き (区切りごとの動作) と、動いた後の値
            let mut motions: Vec<(u32, Vec<Segment>, f64)> = step
                .actions
                .iter()
                .filter_map(|a| {
                    let actuator = self.actuator(a.actuator)?;
                    let target = actuator.clamp(a.target);
                    let segment = Segment {
                        start: action_start,
                        duration: a.duration.max(0.0),
                        target,
                        easing: None,
                    };
                    Some((a.actuator, vec![segment], target))
                })
                .collect();
            for robot_move in &step.moves {
                let Some(robot) = self.robot(robot_move.robot) else {
                    continue;
                };
                let Some(point) = robot.point(robot_move.point) else {
                    continue;
                };
                let plan = robot_path::plan_move(self, robot, point, robot_move.kind, &values);
                // 同じステップで関節に直接置いた動作より、ロボットの移動を優先する
                motions.retain(|(id, _, _)| !plan.joints.iter().any(|j| j.actuator == *id));
                let duration = robot_move.duration.max(0.0);
                for joint in &plan.joints {
                    let mut previous = 0.0;
                    let segments = joint
                        .waypoints
                        .iter()
                        .map(|&(end, target)| {
                            let segment = Segment {
                                start: action_start + previous * duration,
                                duration: (end - previous) * duration,
                                target,
                                easing: joint.easing,
                            };
                            previous = end;
                            segment
                        })
                        .collect();
                    let end_value = plan.end_values.get(&joint.actuator).copied().unwrap_or(0.0);
                    motions.push((joint.actuator, segments, end_value));
                }
                schedule.moves.push(SolvedMove {
                    step: index,
                    robot: robot.id,
                    point: point.id,
                    kind: robot_move.kind,
                    start: action_start,
                    duration,
                    position_error: plan.position_error,
                    angle_error_deg: plan.angle_error.to_degrees(),
                    reached: plan.reached,
                    path_reached: plan.path_reached,
                    min_dexterity: plan.min_dexterity,
                    singular: plan.singular,
                    joint_travel: plan
                        .joints
                        .iter()
                        .map(|j| {
                            let from = values.get(&j.actuator).copied().unwrap_or(0.0);
                            let to = plan.end_values.get(&j.actuator).copied().unwrap_or(from);
                            (j.actuator, (to - from).abs())
                        })
                        .collect(),
                    path: plan.samples,
                });
            }
            motions.sort_by_key(|(id, _, _)| *id);
            for (id, segments, end_value) in motions {
                schedule.segments.entry(id).or_default().extend(segments);
                values.insert(id, end_value);
            }
            let end = action_start + step.duration();
            schedule.steps.push(StepSpan {
                start,
                action_start,
                end,
            });
            time = end;
        }
        schedule.cycle_time = time;
        schedule
    }
}

/// `start` から条件が成立するまで待ち、動作を始める時刻を返す。時間切れなら `None`。
fn wait_for_interlocks(
    schedule: &Schedule,
    start: f64,
    step: &Step,
    unmet: &mut impl FnMut(&Schedule, f64, &Step) -> Vec<Interlock>,
) -> Option<f64> {
    if step.interlocks.is_empty() {
        return Some(start);
    }
    let polls = (INTERLOCK_TIMEOUT / INTERLOCK_POLL).round() as usize;
    (0..=polls)
        .map(|k| start + k as f64 * INTERLOCK_POLL)
        .find(|&t| unmet(schedule, t, step).is_empty())
}

/// 1つの動作を時刻で表したもの
#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub start: f64,
    pub duration: f64,
    pub target: f64,
    /// 速度の付け方。None ならアクチュエータの設定に従う (直線補間の区切りは等速)。
    pub easing: Option<Easing>,
}

#[derive(Clone, Copy, Debug)]
pub struct StepSpan {
    /// 前のステップが終わった時刻 (ここから開始条件を待つ)
    pub start: f64,
    /// 開始条件が成立し、動作を始めた時刻 (条件がなければ `start` と同じ)
    pub action_start: f64,
    pub end: f64,
}

impl StepSpan {
    /// 開始条件を待った時間
    pub fn wait(&self) -> f64 {
        self.action_start - self.start
    }
}

/// インターロックが成立せず、サイクルが止まったところ
#[derive(Clone, Debug, PartialEq)]
pub struct InterlockStop {
    /// 止まったステップの番号 (0 始まり)
    pub step: usize,
    /// 止まった (時間切れになった) 時刻
    pub time: f64,
    /// 成立しなかった条件
    pub unmet: Vec<Interlock>,
}

/// ステップを時刻に展開した結果
#[derive(Clone, Debug, Default)]
pub struct Schedule {
    /// アクチュエータごとの動作 (開始時刻の早い順)
    pub segments: HashMap<u32, Vec<Segment>>,
    /// 各ステップの開始・終了時刻
    pub steps: Vec<StepSpan>,
    /// 1サイクルの時間 (秒)。異常停止したときは止まった時刻。
    pub cycle_time: f64,
    /// インターロックで止まったときの情報
    pub stop: Option<InterlockStop>,
    /// ロボットの移動ごとの逆運動学の結果 (届かない教示点の警告に使う)
    pub moves: Vec<SolvedMove>,
    /// 電子カム表 (チャートから写したもの。値を読むときに使う)
    pub cams: Vec<CamTable>,
}

/// ロボットの移動1つ分の逆運動学と経路の結果
#[derive(Clone, Debug)]
pub struct SolvedMove {
    pub step: usize,
    pub robot: u32,
    pub point: u32,
    pub kind: MoveKind,
    /// 動き始めた時刻と動作時間 (秒)
    pub start: f64,
    pub duration: f64,
    /// 届かなかった距離 (モデル単位)
    pub position_error: f64,
    /// 工具の向きのずれ (度)
    pub angle_error_deg: f64,
    /// 教示点に届いたか
    pub reached: bool,
    /// 経路の途中もすべて届いたか (直線補間で意味がある)
    pub path_reached: bool,
    /// 経路の上で最小の動かしやすさ (0〜1)
    pub min_dexterity: f64,
    /// 特異点の近くを通るか
    pub singular: bool,
    /// 各関節が動いた量 (回転は度、直動はモデル単位。最短時間の見積もりに使う)
    pub joint_travel: Vec<(u32, f64)>,
    /// TCP の通り道を区切った点 (3D ビューの表示用)
    pub path: Vec<PathSample>,
}

impl SolvedMove {
    /// 教示点にも途中にも問題がないか
    pub fn ok(&self) -> bool {
        self.reached && self.path_reached && !self.singular
    }
}

impl Schedule {
    /// 時刻 t でのアクチュエータの値。サイクル開始時は全アクチュエータ 0 (原点) とする。
    pub fn value_at(&self, actuator: &Actuator, t: f64) -> f64 {
        let Some(segments) = self.segments.get(&actuator.id) else {
            return 0.0;
        };
        let mut value = 0.0;
        for segment in segments {
            if t < segment.start {
                break;
            }
            let progress = if segment.duration <= 0.0 {
                1.0
            } else {
                (t - segment.start) / segment.duration
            };
            let easing = segment.easing.unwrap_or(actuator.easing);
            value += (segment.target - value) * easing.apply_with(progress, &self.cams);
        }
        actuator.clamp(value)
    }

    /// 時刻 t でのアクチュエータの速度 (値の単位/秒。シリンダは 0〜1 の値の速さ)
    pub fn velocity_at(&self, actuator: &Actuator, t: f64) -> f64 {
        let h = 1e-3;
        (self.value_at(actuator, t + h) - self.value_at(actuator, (t - h).max(0.0)))
            / (t + h - (t - h).max(0.0))
    }

    /// 時刻 t での全アクチュエータの値
    pub fn values_at(&self, chart: &TimingChart, t: f64) -> HashMap<u32, f64> {
        chart
            .actuators
            .iter()
            .map(|a| (a.id, self.value_at(a, t)))
            .collect()
    }

    /// あるステップのロボット移動の逆運動学の結果
    pub fn solved_move(&self, step: usize, robot: u32) -> Option<&SolvedMove> {
        self.moves
            .iter()
            .find(|m| m.step == step && m.robot == robot)
    }

    /// サイクル終了時の値 (原点に戻っていないアクチュエータを見つけるのに使う)
    pub fn final_value(&self, actuator: &Actuator) -> f64 {
        self.segments
            .get(&actuator.id)
            .and_then(|s| s.last())
            .map_or(0.0, |s| s.target)
    }

    /// 実際に実行される最後のステップ (異常停止したら止まったステップ)
    pub fn last_step(&self) -> Option<usize> {
        match &self.stop {
            Some(stop) => Some(stop.step),
            None => self.steps.len().checked_sub(1),
        }
    }

    /// 時刻 t が含まれるステップの番号
    pub fn step_at(&self, t: f64) -> Option<usize> {
        self.steps
            .iter()
            .position(|s| t >= s.start && t < s.end)
            .or_else(|| self.last_step().filter(|_| t >= self.cycle_time))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chart_with_cylinder() -> (TimingChart, u32) {
        let mut chart = TimingChart::default();
        let id = chart.add_actuator(ActuatorKind::Cylinder);
        chart.actuator_mut(id).unwrap().easing = Easing::Linear;
        (chart, id)
    }

    #[test]
    fn next_step_waits_for_the_slowest_action() {
        let mut chart = TimingChart::default();
        let a = chart.add_actuator(ActuatorKind::Cylinder);
        let b = chart.add_actuator(ActuatorKind::LinearAxis);
        chart.steps.push(Step {
            actions: vec![
                Action {
                    actuator: a,
                    target: 1.0,
                    duration: 0.4,
                },
                Action {
                    actuator: b,
                    target: 2.0,
                    duration: 1.0,
                },
            ],
            dwell: 0.5,
            ..Default::default()
        });
        chart.steps.push(Step {
            actions: vec![Action {
                actuator: a,
                target: 0.0,
                duration: 0.4,
            }],
            ..Default::default()
        });
        let schedule = chart.schedule();
        assert_eq!(schedule.steps[1].start, 1.5);
        assert_eq!(schedule.cycle_time, 1.9);
    }

    #[test]
    fn value_ramps_between_steps() {
        let (mut chart, id) = chart_with_cylinder();
        for target in [1.0, 0.0] {
            chart.steps.push(Step {
                actions: vec![Action {
                    actuator: id,
                    target,
                    duration: 1.0,
                }],
                ..Default::default()
            });
        }
        let schedule = chart.schedule();
        let actuator = chart.actuator(id).unwrap();
        let at = |t| schedule.value_at(actuator, t);
        assert_eq!(at(0.0), 0.0);
        assert!((at(0.5) - 0.5).abs() < 1e-12);
        assert_eq!(at(1.0), 1.0);
        assert!((at(1.25) - 0.75).abs() < 1e-12);
        assert_eq!(at(5.0), 0.0);
    }

    /// S1 でシリンダを出し、S2 は「センサー 1 が ON」を待ってから戻す
    fn chart_with_interlock() -> TimingChart {
        let (mut chart, id) = chart_with_cylinder();
        let act = |target| Action {
            actuator: id,
            target,
            duration: 1.0,
        };
        chart.steps.push(Step {
            actions: vec![act(1.0)],
            ..Default::default()
        });
        chart.steps.push(Step {
            actions: vec![act(0.0)],
            interlocks: vec![Interlock {
                sensor: 1,
                on: true,
            }],
            ..Default::default()
        });
        chart
    }

    #[test]
    fn step_waits_until_interlock_holds() {
        let chart = chart_with_interlock();
        // センサーは t = 1.5 で ON になる
        let schedule = chart.schedule_with(|_, t, step| {
            step.interlocks
                .iter()
                .copied()
                .filter(|i| (t >= 1.5) != i.on)
                .collect()
        });
        let span = schedule.steps[1];
        assert_eq!(span.start, 1.0);
        assert!((span.action_start - 1.5).abs() < 1e-9);
        assert!((schedule.cycle_time - 2.5).abs() < 1e-9);
        assert!(schedule.stop.is_none());
        // 待っているあいだシリンダは出たまま
        let actuator = &chart.actuators[0];
        assert_eq!(schedule.value_at(actuator, 1.4), 1.0);
    }

    #[test]
    fn unmet_interlock_stops_the_cycle() {
        let mut chart = chart_with_interlock();
        chart.steps.push(Step::default());
        let schedule = chart.schedule_with(|_, _, step| step.interlocks.clone());
        let stop = schedule.stop.as_ref().expect("止まるはず");
        assert_eq!(stop.step, 1);
        assert_eq!(stop.unmet, chart.steps[1].interlocks);
        assert_eq!(schedule.cycle_time, 1.0 + INTERLOCK_TIMEOUT);
        // 止まった後のステップも列としては残る
        assert_eq!(schedule.steps.len(), 3);
        assert_eq!(schedule.step_at(100.0), Some(1));
        // 戻しの動作は行われない
        assert_eq!(schedule.final_value(&chart.actuators[0]), 1.0);
    }

    #[test]
    fn removing_a_sensor_removes_its_interlocks() {
        let mut chart = chart_with_interlock();
        let id = chart.add_sensor(SensorKind::Proximity);
        assert_eq!(id, 1);
        chart.remove_sensor(id);
        assert!(chart.steps[1].interlocks.is_empty());
    }

    #[test]
    fn mounting_cycles_are_detected() {
        let mut chart = TimingChart::default();
        let a = chart.add_actuator(ActuatorKind::LinearAxis);
        let b = chart.add_actuator(ActuatorKind::Cylinder);
        chart.actuator_mut(b).unwrap().mounted_on = Some(a);
        assert!(chart.is_mounted_under(b, a));
        assert!(!chart.is_mounted_under(a, b));
    }
}
#[test]
fn cam_table_shapes_the_motion_and_is_reset_when_removed() {
    let mut chart = TimingChart::default();
    let id = chart.add_actuator(ActuatorKind::LinearAxis);
    let cam = chart.add_cam();
    chart.cam_mut(cam).unwrap().points = vec![[0.5, 0.9]];
    chart.actuator_mut(id).unwrap().easing = Easing::Cam(cam);
    chart.steps = vec![Step {
        actions: vec![Action {
            actuator: id,
            target: 100.0,
            duration: 2.0,
        }],
        ..Default::default()
    }];
    let schedule = chart.schedule();
    let actuator = chart.actuator(id).unwrap();
    // 動作時間の半分で、移動量の 90% まで進む
    assert!((schedule.value_at(actuator, 1.0) - 90.0).abs() < 1e-9);
    assert!((schedule.value_at(actuator, 2.0) - 100.0).abs() < 1e-9);
    assert!(schedule.velocity_at(actuator, 0.5) > schedule.velocity_at(actuator, 1.5));

    chart.remove_cam(cam);
    assert_eq!(chart.actuator(id).unwrap().easing, Easing::Smooth);
}
