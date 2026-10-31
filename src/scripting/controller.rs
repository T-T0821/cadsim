//! 部品スクリプトで設備を動かす (PLC の代わり)
//!
//! スクリプトの付いた部品ごとに [`ScriptRunner`] を1つ動かし、PLC 連携の I/O を通して
//! アクチュエータを動かす。Bevy に依存しない。
//!
//! スクリプトから見える名前は次のとおり。
//!
//! | 関数 | 名前 | 中身 |
//! | --- | --- | --- |
//! | `get` / `wait_signal` | I/O の信号名 (例: `シリンダ1.出端`) | PLC 連携の I/O 一覧のビット |
//! | | センサー名 | センサーの ON / OFF |
//! | | それ以外 | 内部信号。どれかのスクリプトが `set` すると作られ、全スクリプトで共有する |
//! | `set` | I/O の出力ビット (例: `シリンダ1.出`) | 出力を書く (入力・センサーは書けない) |
//! | `joint` / `move_joint` | アクチュエータ名 | シリンダは 0 = 戻端 / 1 = 出端、直動は移動量、回転は角度 |
//!
//! 自作部品の中のスクリプトは、名前の頭 (例: `プッシャー 1/`) を省いて自分の部品の
//! I/O・センサー・関節を指せる。頭を付けた名前が見つからなければ、書いたとおりの名前で探す。

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use super::runner::{ScriptHost, ScriptRunner, ScriptState};
use crate::model::NodeId;
use crate::motion::{Actuator, ActuatorKind, TimingChart};
use crate::plc::{IoTable, IoValues, Point};

/// 位置が着いたとみなす許容差 (PLC 連携の設備モデルと同じ)
const TOLERANCE: f64 = 1e-6;

/// ログに残す行数の上限
const MAX_LOG_LINES: usize = 500;

/// スクリプト1つ分 (どの部品のものか、名前、実行の状態)
pub struct RunningScript {
    pub node: NodeId,
    pub name: String,
    /// 自作部品の中なら、その部品の名前の頭
    scope: String,
    /// 文法の誤りなどで開始できなかったときは Err
    runner: Result<ScriptRunner, String>,
}

impl RunningScript {
    pub fn state(&self) -> ScriptState {
        match &self.runner {
            Ok(runner) => runner.state().clone(),
            Err(e) => ScriptState::Failed(e.clone()),
        }
    }
}

/// 動かすスクリプト1つ分
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptSource {
    pub node: NodeId,
    /// 部品名 (ログに出す)
    pub name: String,
    /// 自作部品の中なら、その部品の名前の頭 (例: "プッシャー 1/")。なければ空。
    pub scope: String,
    pub source: String,
}

impl From<(NodeId, String, String)> for ScriptSource {
    fn from((node, name, source): (NodeId, String, String)) -> Self {
        Self {
            node,
            name,
            scope: String::new(),
            source,
        }
    }
}

/// 実行中の部品スクリプト一式
#[derive(Default)]
pub struct ScriptController {
    pub scripts: Vec<RunningScript>,
    /// 実行を始めてからの時刻 (秒)
    pub time: f64,
    /// スクリプトどうしで共有する内部信号
    pub internal: BTreeMap<String, bool>,
    /// `log` / `print` の出力とエラー (新しいものが後ろ)
    pub log: Vec<String>,
}

impl ScriptController {
    /// スクリプトの並びから動かし始める
    pub fn start(scripts: impl IntoIterator<Item = impl Into<ScriptSource>>) -> Self {
        let mut controller = Self::default();
        for script in scripts {
            let ScriptSource {
                node,
                name,
                scope,
                source,
            } = script.into();
            let runner = ScriptRunner::start(&name, &source);
            if let Err(e) = &runner {
                controller.push_log(format!("[{name}] {e}"));
            }
            controller.scripts.push(RunningScript {
                node,
                name,
                scope,
                runner,
            });
        }
        controller
    }

    /// 動いているスクリプトがあれば true
    pub fn is_running(&self) -> bool {
        self.scripts.iter().any(|s| {
            s.runner
                .as_ref()
                .is_ok_and(|r| *r.state() == ScriptState::Running)
        })
    }

    /// 1スキャン分進める。入力 (`io` の入力側・`values`・`sensors`) を見て、出力を `io` に書く。
    pub fn scan(
        &mut self,
        chart: &TimingChart,
        table: &IoTable,
        io: &mut IoValues,
        values: &HashMap<u32, f64>,
        sensors: &[(String, bool)],
        dt: f64,
    ) {
        self.time += dt;
        let mut logs = Vec::new();
        for script in &mut self.scripts {
            let Ok(runner) = &mut script.runner else {
                continue;
            };
            if *runner.state() != ScriptState::Running {
                continue;
            }
            let mut host = IoHost {
                chart,
                table,
                io,
                values,
                sensors,
                internal: &mut self.internal,
                logs: &mut logs,
                prefix: &script.name,
                scope: &script.scope,
            };
            runner.step(&mut host, self.time);
            if let ScriptState::Failed(e) = runner.state() {
                logs.push(format!("[{}] {e}", script.name));
            }
        }
        for line in logs {
            self.push_log(line);
        }
    }

    fn push_log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > MAX_LOG_LINES {
            let extra = self.log.len() - MAX_LOG_LINES;
            self.log.drain(..extra);
        }
    }
}

/// スクリプトから見たシミュレーター (1スキャンの間だけ作る)
struct IoHost<'a> {
    chart: &'a TimingChart,
    table: &'a IoTable,
    io: &'a mut IoValues,
    values: &'a HashMap<u32, f64>,
    sensors: &'a [(String, bool)],
    internal: &'a mut BTreeMap<String, bool>,
    logs: &'a mut Vec<String>,
    prefix: &'a str,
    scope: &'a str,
}

impl IoHost<'_> {
    /// 自作部品のスクリプトなら、部品の名前の頭を付けた名前が I/O・センサー・関節にあればそちらを使う
    fn resolve<'n>(&self, name: &'n str) -> Cow<'n, str> {
        if self.scope.is_empty() || name.starts_with(self.scope) {
            return Cow::Borrowed(name);
        }
        let full = format!("{}{name}", self.scope);
        let known = self.table.points.iter().any(|p| p.name == full)
            || self.sensors.iter().any(|(n, _)| *n == full)
            || self.chart.actuators.iter().any(|a| a.name == full);
        if known {
            Cow::Owned(full)
        } else {
            Cow::Borrowed(name)
        }
    }

    /// 名前の付いた I/O のビット
    fn bit_point(&self, name: &str) -> Option<Point> {
        self.table
            .points
            .iter()
            .find(|p| p.name == name && !p.point.is_word())
            .map(|p| p.point)
    }

    fn actuator(&self, name: &str) -> Result<&Actuator, String> {
        self.chart
            .actuators
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| format!("関節 (アクチュエータ)「{name}」がありません"))
    }
}

impl ScriptHost for IoHost<'_> {
    fn signal(&self, name: &str) -> Option<bool> {
        let resolved = self.resolve(name);
        let name = resolved.as_ref();
        if let Some(point) = self.bit_point(name) {
            return Some(self.io.bit(point));
        }
        if let Some((_, on)) = self.sensors.iter().find(|(n, _)| n == name) {
            return Some(*on);
        }
        self.internal.get(name).copied()
    }

    fn set_signal(&mut self, name: &str, value: bool) -> Result<(), String> {
        let resolved = self.resolve(name);
        let name = resolved.as_ref();
        if let Some(point) = self.bit_point(name) {
            if !point.is_output() {
                return Err(format!("「{name}」は設備からの入力なので書けません"));
            }
            self.io.set_bit(point, value);
            return Ok(());
        }
        if self.sensors.iter().any(|(n, _)| n == name) {
            return Err(format!("「{name}」はセンサーなので書けません"));
        }
        if self.table.points.iter().any(|p| p.name == name) {
            return Err(format!(
                "「{name}」は数値の信号です。move_joint で動かしてください"
            ));
        }
        self.internal.insert(name.to_owned(), value);
        Ok(())
    }

    fn joint(&self, name: &str) -> Option<f64> {
        let resolved = self.resolve(name);
        let name = resolved.as_ref();
        let actuator = self.actuator(name).ok()?;
        Some(self.values.get(&actuator.id).copied().unwrap_or(0.0))
    }

    fn start_joint_move(&mut self, name: &str, target: f64) -> Result<(), String> {
        let resolved = self.resolve(name);
        let name = resolved.as_ref();
        let actuator = self.actuator(name)?;
        let id = actuator.id;
        match actuator.kind {
            ActuatorKind::Cylinder => {
                if target != 0.0 && target != 1.0 {
                    return Err(format!(
                        "シリンダ「{name}」は 0 (戻端) か 1 (出端) で指定してください"
                    ));
                }
                self.io.set_bit(Point::CylinderExtend(id), target == 1.0);
            }
            ActuatorKind::LinearAxis | ActuatorKind::RotaryTable => {
                if let Some([min, max]) = actuator.limits
                    && !(min - TOLERANCE..=max + TOLERANCE).contains(&target)
                {
                    return Err(format!(
                        "「{name}」の目標 {target} は可動範囲 {min} 〜 {max} の外です"
                    ));
                }
                self.io.set_word(Point::AxisTarget(id), target);
            }
        }
        Ok(())
    }

    fn joint_arrived(&self, name: &str) -> bool {
        let resolved = self.resolve(name);
        let name = resolved.as_ref();
        let Ok(actuator) = self.actuator(name) else {
            return false;
        };
        let id = actuator.id;
        match actuator.kind {
            ActuatorKind::Cylinder => {
                if self.io.bit(Point::CylinderExtend(id)) {
                    self.io.bit(Point::CylinderExtended(id))
                } else {
                    self.io.bit(Point::CylinderRetracted(id))
                }
            }
            ActuatorKind::LinearAxis | ActuatorKind::RotaryTable => {
                let value = self.values.get(&id).copied().unwrap_or(0.0);
                (value - self.io.word(Point::AxisTarget(id))).abs() <= TOLERANCE
            }
        }
    }

    fn log(&mut self, message: &str) {
        self.logs.push(format!("[{}] {message}", self.prefix));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plc::{AddressPlan, Plant};

    /// シリンダ「押し出し」と直動軸「スライダ」、センサー「ワーク有」の設備
    fn setup() -> (TimingChart, IoTable) {
        let mut chart = TimingChart::default();
        let cyl = chart.add_actuator(ActuatorKind::Cylinder);
        let a = chart.actuator_mut(cyl).unwrap();
        a.name = "押し出し".into();
        a.default_duration = 0.5;
        let axis = chart.add_actuator(ActuatorKind::LinearAxis);
        let a = chart.actuator_mut(axis).unwrap();
        a.name = "スライダ".into();
        // チャートに動作がないので、速度は 1 / default_duration = 100 / 秒
        a.default_duration = 0.01;
        a.limits = Some([0.0, 200.0]);
        let table = IoTable::from_chart(&chart, &AddressPlan::default());
        (chart, table)
    }

    /// PLC 連携の1スキャンと同じ順で `seconds` 秒ぶん回す
    fn run(
        controller: &mut ScriptController,
        chart: &TimingChart,
        table: &IoTable,
        plant: &mut Plant,
        io: &mut IoValues,
        sensors: &[(String, bool)],
        seconds: f64,
    ) {
        let dt = 0.02;
        let mut t = 0.0;
        while t < seconds - 1e-9 {
            plant.report(chart, io);
            let values = plant.values().clone();
            controller.scan(chart, table, io, &values, sensors, dt);
            plant.take_commands(chart, io);
            plant.advance(chart, dt);
            t += dt;
        }
        plant.report(chart, io);
    }

    #[test]
    fn script_waits_for_sensor_and_drives_cylinder_and_axis() {
        let (chart, table) = setup();
        let src = r#"
            wait_signal("ワーク有", true);
            move_joint("押し出し", 1);
            set("押し出し完了", true);
            move_joint("スライダ", 150.0);
            log("スライダ " + joint("スライダ"));
            move_joint("押し出し", 0);
        "#;
        let mut controller = ScriptController::start([(1, "プッシャー".into(), src.into())]);
        let mut plant = Plant::default();
        let mut io = IoValues::default();
        let off = [("ワーク有".to_string(), false)];
        run(
            &mut controller,
            &chart,
            &table,
            &mut plant,
            &mut io,
            &off,
            0.5,
        );
        assert_eq!(plant.values().get(&1).copied().unwrap_or(0.0), 0.0);

        let on = [("ワーク有".to_string(), true)];
        run(
            &mut controller,
            &chart,
            &table,
            &mut plant,
            &mut io,
            &on,
            10.0,
        );
        assert!(!controller.is_running(), "{:?}", controller.log);
        assert_eq!(controller.scripts[0].state(), ScriptState::Finished);
        assert_eq!(controller.internal.get("押し出し完了"), Some(&true));
        assert!(io.bit(Point::CylinderRetracted(1)));
        assert!((plant.values()[&2] - 150.0).abs() < 1e-6);
        assert_eq!(
            controller.log,
            vec!["[プッシャー] スライダ 150.0".to_string()]
        );
    }

    #[test]
    fn two_scripts_handshake_through_internal_signal() {
        let (chart, table) = setup();
        let a = r#"
            set("ready", false);
            move_joint("スライダ", 100);
            set("ready", true);
        "#;
        let b = r#"
            wait_signal("ready", true);
            move_joint("押し出し", 1);
        "#;
        let mut controller =
            ScriptController::start([(1, "A".into(), a.into()), (2, "B".into(), b.into())]);
        let mut plant = Plant::default();
        let mut io = IoValues::default();
        run(
            &mut controller,
            &chart,
            &table,
            &mut plant,
            &mut io,
            &[],
            3.0,
        );
        assert!(!controller.is_running(), "{:?}", controller.log);
        assert!(io.bit(Point::CylinderExtended(1)));
    }

    #[test]
    fn errors_are_logged_with_part_name() {
        let (chart, table) = setup();
        let mut controller = ScriptController::start([
            (
                1,
                "入力書き".into(),
                r#"set("押し出し.出端", true);"#.into(),
            ),
            (2, "範囲外".into(), r#"move_joint("スライダ", 500);"#.into()),
            (3, "文法".into(), "let = ;".into()),
        ]);
        let mut plant = Plant::default();
        let mut io = IoValues::default();
        run(
            &mut controller,
            &chart,
            &table,
            &mut plant,
            &mut io,
            &[],
            0.1,
        );
        let log = controller.log.join("\n");
        assert!(
            log.contains("[入力書き]") && log.contains("入力なので書けません"),
            "{log}"
        );
        assert!(
            log.contains("[範囲外]") && log.contains("可動範囲"),
            "{log}"
        );
        assert!(log.contains("[文法]"), "{log}");
        assert!(
            controller
                .scripts
                .iter()
                .all(|s| matches!(s.state(), ScriptState::Failed(_)))
        );
    }

    #[test]
    fn scripts_in_custom_parts_use_short_names() {
        // 同じ自作部品を2つ置いたときの名前
        let mut chart = TimingChart::default();
        for prefix in ["P 1/", "P 2/"] {
            let id = chart.add_actuator(ActuatorKind::Cylinder);
            let a = chart.actuator_mut(id).unwrap();
            a.name = format!("{prefix}シリンダ");
            a.default_duration = 0.2;
        }
        let table = IoTable::from_chart(&chart, &AddressPlan::default());
        let src = r#"
            wait_signal("ワーク有", true);
            move_joint("シリンダ", 1);
            set("出た", get("シリンダ.出端"));
        "#;
        let scripts = ["P 1/", "P 2/"].map(|scope| ScriptSource {
            node: 1,
            name: scope.trim_end_matches('/').into(),
            scope: scope.into(),
            source: src.into(),
        });
        let mut controller = ScriptController::start(scripts);
        let mut plant = Plant::default();
        let mut io = IoValues::default();
        // センサーは1つ目の部品のものだけ ON
        let sensors = [
            ("P 1/ワーク有".to_string(), true),
            ("P 2/ワーク有".to_string(), false),
        ];
        run(
            &mut controller,
            &chart,
            &table,
            &mut plant,
            &mut io,
            &sensors,
            2.0,
        );
        assert!(io.bit(Point::CylinderExtended(1)), "{:?}", controller.log);
        assert!(!io.bit(Point::CylinderExtended(2)));
        // 内部信号は頭を付けずに共有する
        assert_eq!(controller.internal.get("出た"), Some(&true));
    }
}
