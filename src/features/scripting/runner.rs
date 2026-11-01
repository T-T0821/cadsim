//! 1つのスクリプトの実行
//!
//! Rhai のスクリプトを、シミュレーションの時間に合わせて動かす。
//! Bevy に依存しない。シミュレーター側は [`ScriptHost`] を実装して渡すだけでよい。
//!
//! スクリプトは専用のスレッドで動き、信号の読み書きや関節の移動、待ちのたびに
//! 要求をチャネルで送って返事を待つ。シミュレーター側は [`ScriptRunner::step`] で
//! 要求を1つずつ処理するので、スクリプトとシミュレーションは常に交互に進み、
//! 結果は毎回同じになる (スレッドの都合で変わらない)。
//!
//! スクリプトから使える関数は [`super::rhai_engine`] を参照。

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use super::rhai_engine::{self, check_syntax};

/// 1回の [`ScriptRunner::step`] で処理する、すぐ終わる要求 (信号の読み書きなど) の上限。
/// `loop { if get("x") { break; } }` のような待ち方をしても、上限で次のコマに回すので固まらない。
const MAX_REQUESTS_PER_STEP: usize = 1000;

/// スクリプトが計算だけして要求を出さないまま、これ以上たったら止める。
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(2);

/// スクリプトから操作されるシミュレーター側。
pub trait ScriptHost {
    /// 信号の値。知らない名前なら `None`。
    fn signal(&self, name: &str) -> Option<bool>;
    /// 信号を書く。書けない (知らない名前・入力専用など) ときは理由を返す。
    fn set_signal(&mut self, name: &str, value: bool) -> Result<(), String>;
    /// 関節の今の値 (直動は mm、回転は度)。知らない名前なら `None`。
    fn joint(&self, name: &str) -> Option<f64>;
    /// 関節を目標値へ動かし始める。
    fn start_joint_move(&mut self, name: &str, target: f64) -> Result<(), String>;
    /// 関節が目標値に着いたか
    fn joint_arrived(&self, name: &str) -> bool;
    /// `print` や `log` の出力
    fn log(&mut self, message: &str);
}

/// スクリプトのスレッドからシミュレーターへの要求
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Request {
    Signal(String),
    SetSignal(String, bool),
    Joint(String),
    MoveJoint(String, f64),
    Wait(f64),
    WaitSignal {
        name: String,
        value: bool,
        timeout: Option<f64>,
    },
    Time,
    Log(String),
    /// スクリプトが最後まで走った (`Err` はスクリプトのエラー)
    Finished(Result<(), String>),
}

/// シミュレーターからスクリプトのスレッドへの返事
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Reply {
    Unit,
    Bool(bool),
    Number(f64),
    Error(String),
}

/// 返事を待っている、時間のかかる要求
#[derive(Debug, Clone, PartialEq)]
enum Pending {
    Until(f64),
    Joint(String),
    Signal {
        name: String,
        value: bool,
        deadline: Option<f64>,
    },
}

/// スクリプトの状態
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptState {
    /// 動いている (待ちの途中を含む)
    Running,
    /// 最後まで走った
    Finished,
    /// エラーで止まった
    Failed(String),
}

/// 1つのスクリプトの実行。落とす (drop) とスクリプトのスレッドも止まる。
pub struct ScriptRunner {
    /// Bevy の Resource に入れられるよう (Sync にするため) Mutex で包む
    requests: Mutex<Receiver<Request>>,
    replies: Sender<Reply>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pending: Option<Pending>,
    state: ScriptState,
}

impl ScriptRunner {
    /// スクリプトを文法チェックして動かし始める。`name` はエラー表示に使う部品名。
    pub fn start(name: &str, source: &str) -> Result<Self, String> {
        check_syntax(source)?;
        let (req_tx, req_rx) = mpsc::channel();
        let (rep_tx, rep_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread = {
            let cancel = cancel.clone();
            let source = source.to_owned();
            std::thread::Builder::new()
                .name(format!("script: {name}"))
                .spawn(move || rhai_engine::run(&source, req_tx, rep_rx, cancel))
                .map_err(|e| format!("スクリプトを開始できません: {e}"))?
        };
        Ok(Self {
            requests: Mutex::new(req_rx),
            replies: rep_tx,
            cancel,
            thread: Some(thread),
            pending: None,
            state: ScriptState::Running,
        })
    }

    pub fn state(&self) -> &ScriptState {
        &self.state
    }

    /// シミュレーションの時刻 `now` (秒) までスクリプトを進める。
    /// 待ち (`wait` や関節の移動) に入るか、上限まで要求を処理したら戻る。
    pub fn step(&mut self, host: &mut dyn ScriptHost, now: f64) {
        if self.state != ScriptState::Running {
            return;
        }
        if let Some(pending) = &self.pending {
            let reply = match pending {
                Pending::Until(t) if now + 1e-9 >= *t => Reply::Unit,
                Pending::Until(_) => return,
                Pending::Joint(name) if host.joint_arrived(name) => Reply::Unit,
                Pending::Joint(_) => return,
                Pending::Signal { name, value, .. } if host.signal(name) == Some(*value) => {
                    Reply::Bool(true)
                }
                Pending::Signal {
                    deadline: Some(d), ..
                } if now + 1e-9 >= *d => Reply::Bool(false),
                Pending::Signal { .. } => return,
            };
            self.pending = None;
            if !self.reply(reply) {
                return;
            }
        }
        for _ in 0..MAX_REQUESTS_PER_STEP {
            let received = match self.requests.lock() {
                Ok(requests) => requests.recv_timeout(SCRIPT_TIMEOUT),
                Err(_) => Err(RecvTimeoutError::Disconnected),
            };
            let request = match received {
                Ok(r) => r,
                Err(RecvTimeoutError::Timeout) => {
                    self.fail(format!(
                        "スクリプトが {} 秒以上応答しません (wait を入れずに無限ループしていませんか)",
                        SCRIPT_TIMEOUT.as_secs()
                    ));
                    return;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.fail("スクリプトが途中で終了しました".into());
                    return;
                }
            };
            let reply = match request {
                Request::Finished(Ok(())) => {
                    self.state = ScriptState::Finished;
                    return;
                }
                Request::Finished(Err(e)) => {
                    self.fail(e);
                    return;
                }
                Request::Signal(name) => match host.signal(&name) {
                    Some(v) => Reply::Bool(v),
                    None => Reply::Error(format!("信号「{name}」がありません")),
                },
                Request::SetSignal(name, value) => match host.set_signal(&name, value) {
                    Ok(()) => Reply::Unit,
                    Err(e) => Reply::Error(e),
                },
                Request::Joint(name) => match host.joint(&name) {
                    Some(v) => Reply::Number(v),
                    None => Reply::Error(format!("関節「{name}」がありません")),
                },
                Request::Time => Reply::Number(now),
                Request::Log(message) => {
                    host.log(&message);
                    Reply::Unit
                }
                Request::Wait(seconds) => {
                    if seconds > 0.0 {
                        self.pending = Some(Pending::Until(now + seconds));
                        return;
                    }
                    Reply::Unit
                }
                Request::MoveJoint(name, target) => match host.start_joint_move(&name, target) {
                    Ok(()) if host.joint_arrived(&name) => Reply::Unit,
                    Ok(()) => {
                        self.pending = Some(Pending::Joint(name));
                        return;
                    }
                    Err(e) => Reply::Error(e),
                },
                Request::WaitSignal {
                    name,
                    value,
                    timeout,
                } => match host.signal(&name) {
                    None => Reply::Error(format!("信号「{name}」がありません")),
                    Some(v) if v == value => Reply::Bool(true),
                    Some(_) if timeout.is_some_and(|t| t <= 0.0) => Reply::Bool(false),
                    Some(_) => {
                        self.pending = Some(Pending::Signal {
                            name,
                            value,
                            deadline: timeout.map(|t| now + t),
                        });
                        return;
                    }
                },
            };
            if !self.reply(reply) {
                return;
            }
        }
    }

    fn reply(&mut self, reply: Reply) -> bool {
        if self.replies.send(reply).is_err() {
            self.fail("スクリプトが途中で終了しました".into());
            return false;
        }
        true
    }

    fn fail(&mut self, message: String) {
        self.cancel.store(true, Ordering::Relaxed);
        self.state = ScriptState::Failed(message);
    }
}

impl Drop for ScriptRunner {
    fn drop(&mut self) {
        // 計算中なら on_progress で、返事待ちならチャネルが閉じて、スクリプトは止まる
        self.cancel.store(true, Ordering::Relaxed);
        let (dead_tx, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.replies, dead_tx));
        if let Some(thread) = self.thread.take() {
            // 止まるのを待つと UI が固まりうるので、待たずに切り離す
            drop(thread);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// テスト用の簡単なシミュレーター。関節は 1 秒あたり `speed` だけ進む。
    #[derive(Default)]
    struct FakeHost {
        signals: HashMap<String, bool>,
        joints: HashMap<String, (f64, f64)>,
        speed: f64,
        logs: Vec<String>,
    }

    impl FakeHost {
        fn new() -> Self {
            let mut h = Self {
                speed: 100.0,
                ..Default::default()
            };
            h.signals.insert("start".into(), false);
            h.signals.insert("done".into(), false);
            h.joints.insert("J1".into(), (0.0, 0.0));
            h
        }

        fn advance(&mut self, dt: f64) {
            for (value, target) in self.joints.values_mut() {
                let step = self.speed * dt;
                let d = *target - *value;
                *value = if d.abs() <= step {
                    *target
                } else {
                    *value + step * d.signum()
                };
            }
        }
    }

    impl ScriptHost for FakeHost {
        fn signal(&self, name: &str) -> Option<bool> {
            self.signals.get(name).copied()
        }
        fn set_signal(&mut self, name: &str, value: bool) -> Result<(), String> {
            match self.signals.get_mut(name) {
                Some(v) => {
                    *v = value;
                    Ok(())
                }
                None => Err(format!("信号「{name}」がありません")),
            }
        }
        fn joint(&self, name: &str) -> Option<f64> {
            self.joints.get(name).map(|j| j.0)
        }
        fn start_joint_move(&mut self, name: &str, target: f64) -> Result<(), String> {
            match self.joints.get_mut(name) {
                Some(j) => {
                    j.1 = target;
                    Ok(())
                }
                None => Err(format!("関節「{name}」がありません")),
            }
        }
        fn joint_arrived(&self, name: &str) -> bool {
            self.joints
                .get(name)
                .is_some_and(|j| (j.0 - j.1).abs() < 1e-9)
        }
        fn log(&mut self, message: &str) {
            self.logs.push(message.to_owned());
        }
    }

    /// `dt` 刻みで `until` 秒まで回し、終わった時刻を返す。
    fn run(runner: &mut ScriptRunner, host: &mut FakeHost, dt: f64, until: f64) -> f64 {
        let mut t = 0.0;
        while t <= until {
            runner.step(host, t);
            if *runner.state() != ScriptState::Running {
                return t;
            }
            host.advance(dt);
            t += dt;
        }
        t
    }

    #[test]
    fn waits_for_signal_then_moves_joint_and_sets_output() {
        let src = r#"
            wait_signal("start", true);
            move_joint("J1", 50.0);
            log("arrived " + joint("J1"));
            wait(0.5);
            set("done", true);
        "#;
        let mut host = FakeHost::new();
        let mut runner = ScriptRunner::start("test", src).unwrap();
        let dt = 0.1;
        // start が入るまでは何も起きない
        let mut t = 0.0;
        for _ in 0..5 {
            runner.step(&mut host, t);
            host.advance(dt);
            t += dt;
        }
        assert_eq!(host.joints["J1"].1, 0.0);
        host.signals.insert("start".into(), true);
        while *runner.state() == ScriptState::Running && t < 5.0 {
            runner.step(&mut host, t);
            host.advance(dt);
            t += dt;
        }
        assert_eq!(*runner.state(), ScriptState::Finished);
        assert!(host.signals["done"]);
        assert_eq!(host.logs, vec!["arrived 50.0".to_string()]);
        // 0.5 秒待ってから入った → 移動 0.5 秒 + 待ち 0.5 秒 で約 1.5 秒
        assert!((t - 1.6).abs() < 0.25, "t = {t}");
    }

    #[test]
    fn wait_signal_timeout_returns_false() {
        let src = r#"
            let ok = wait_signal("start", true, 0.3);
            set("done", !ok);
        "#;
        let mut host = FakeHost::new();
        let mut runner = ScriptRunner::start("test", src).unwrap();
        let t = run(&mut runner, &mut host, 0.1, 2.0);
        assert_eq!(*runner.state(), ScriptState::Finished);
        assert!(host.signals["done"]);
        assert!((0.25..0.45).contains(&t), "t = {t}");
    }

    #[test]
    fn unknown_names_stop_with_error() {
        let mut host = FakeHost::new();
        let mut runner = ScriptRunner::start("test", r#"set("nope", true);"#).unwrap();
        run(&mut runner, &mut host, 0.1, 1.0);
        match runner.state() {
            ScriptState::Failed(e) => assert!(e.contains("nope"), "{e}"),
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn syntax_error_is_reported_before_start() {
        assert!(ScriptRunner::start("test", "let x = ;").is_err());
    }

    #[test]
    fn busy_polling_loop_does_not_hang() {
        // wait を入れずに信号を見続けても、1コマごとに区切られて進む
        let src = r#"
            while !get("start") {}
            set("done", true);
        "#;
        let mut host = FakeHost::new();
        let mut runner = ScriptRunner::start("test", src).unwrap();
        for i in 0..3 {
            runner.step(&mut host, i as f64 * 0.1);
        }
        assert_eq!(*runner.state(), ScriptState::Running);
        host.signals.insert("start".into(), true);
        runner.step(&mut host, 0.3);
        assert_eq!(*runner.state(), ScriptState::Finished);
        assert!(host.signals["done"]);
    }

    #[test]
    fn loop_with_wait_repeats_cycle() {
        // 0.25 秒ごとに出力を反転し、5回で終わる
        let src = r#"
            for i in 0..5 {
                set("done", !get("done"));
                wait(0.25);
            }
            log("t=" + time());
        "#;
        let mut host = FakeHost::new();
        let mut runner = ScriptRunner::start("test", src).unwrap();
        run(&mut runner, &mut host, 0.05, 3.0);
        assert_eq!(*runner.state(), ScriptState::Finished);
        assert!(host.signals["done"]);
        assert_eq!(host.logs.len(), 1);
    }

    #[test]
    fn dropping_runner_stops_the_thread() {
        let mut host = FakeHost::new();
        let mut runner = ScriptRunner::start("test", "loop { wait(1.0); }").unwrap();
        runner.step(&mut host, 0.0);
        drop(runner);
    }
}
