//! Rhai でスクリプトを動かす部分
//!
//! スクリプトから呼べる関数を登録し、呼ばれるたびに [`Request`] を送って [`Reply`] を待つ。

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Position};

use super::{Reply, Request};

/// スクリプトから使える関数の説明 (エディタの横に出す)
pub const API_HELP: &str = "\
get(\"信号\") → true / false        信号を読む
set(\"信号\", true)                  信号を書く
joint(\"関節\") → 数値               関節の今の値 (mm / 度)
move_joint(\"関節\", 90.0)           関節を動かし、着くまで待つ
wait(0.5)                            秒数だけ待つ
wait_signal(\"信号\", true)          信号がその値になるまで待つ
wait_signal(\"信号\", true, 3.0)     最大 3 秒待つ。来たら true、時間切れなら false
time() → 数値                        シミュレーションの時刻 (秒)
log(\"文字\") / print(\"文字\")       ログに出す";

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;

struct Channel {
    requests: Sender<Request>,
    replies: Receiver<Reply>,
}

impl Channel {
    fn call(&self, request: Request) -> RhaiResult<Reply> {
        let stopped = || -> Box<EvalAltResult> { "シミュレーションが止まりました".into() };
        self.requests.send(request).map_err(|_| stopped())?;
        match self.replies.recv().map_err(|_| stopped())? {
            Reply::Error(e) => Err(e.into()),
            reply => Ok(reply),
        }
    }

    fn call_unit(&self, request: Request) -> RhaiResult<()> {
        self.call(request).map(|_| ())
    }

    fn call_bool(&self, request: Request) -> RhaiResult<bool> {
        match self.call(request)? {
            Reply::Bool(b) => Ok(b),
            r => Err(format!("予期しない返事: {r:?}").into()),
        }
    }

    fn call_number(&self, request: Request) -> RhaiResult<f64> {
        match self.call(request)? {
            Reply::Number(n) => Ok(n),
            r => Err(format!("予期しない返事: {r:?}").into()),
        }
    }
}

/// 文法だけ確かめる (実行はしない)
pub fn check_syntax(source: &str) -> Result<(), String> {
    Engine::new_raw()
        .compile(source)
        .map(|_| ())
        .map_err(|e| format!("スクリプトの書き方に誤りがあります: {e}"))
}

/// スクリプトのスレッドの本体。最後に必ず [`Request::Finished`] を送る。
pub(super) fn run(
    source: &str,
    requests: Sender<Request>,
    replies: Receiver<Reply>,
    cancel: Arc<AtomicBool>,
) {
    let ch = Rc::new(Channel {
        requests: requests.clone(),
        replies,
    });
    let mut engine = Engine::new();
    engine.on_progress(move |_| {
        cancel
            .load(Ordering::Relaxed)
            .then(|| Dynamic::from("停止しました"))
    });
    {
        let ch = ch.clone();
        engine.on_print(move |s| {
            let _ = ch.call_unit(Request::Log(s.to_owned()));
        });
    }
    register(&mut engine, &ch);

    let result = engine.run(source).map_err(|e| describe(&e));
    let _ = requests.send(Request::Finished(result));
}

fn describe(e: &EvalAltResult) -> String {
    match e.position() {
        Position::NONE => format!("スクリプトのエラー: {e}"),
        p => format!(
            "スクリプトのエラー ({}行目): {}",
            p.line().unwrap_or(0),
            e.unwrap_inner()
        ),
    }
}

fn register(engine: &mut Engine, ch: &Rc<Channel>) {
    let c = ch.clone();
    engine.register_fn("get", move |name: ImmutableString| {
        c.call_bool(Request::Signal(name.into()))
    });
    let c = ch.clone();
    engine.register_fn("set", move |name: ImmutableString, value: bool| {
        c.call_unit(Request::SetSignal(name.into(), value))
    });
    let c = ch.clone();
    engine.register_fn("joint", move |name: ImmutableString| {
        c.call_number(Request::Joint(name.into()))
    });
    let c = ch.clone();
    engine.register_fn("move_joint", move |name: ImmutableString, target: f64| {
        c.call_unit(Request::MoveJoint(name.into(), target))
    });
    let c = ch.clone();
    engine.register_fn("move_joint", move |name: ImmutableString, target: i64| {
        c.call_unit(Request::MoveJoint(name.into(), target as f64))
    });
    let c = ch.clone();
    engine.register_fn("wait", move |seconds: f64| c.call_unit(Request::Wait(seconds)));
    let c = ch.clone();
    engine.register_fn("wait", move |seconds: i64| {
        c.call_unit(Request::Wait(seconds as f64))
    });
    let c = ch.clone();
    engine.register_fn("wait_signal", move |name: ImmutableString, value: bool| {
        c.call_bool(Request::WaitSignal {
            name: name.into(),
            value,
            timeout: None,
        })
    });
    let c = ch.clone();
    engine.register_fn(
        "wait_signal",
        move |name: ImmutableString, value: bool, timeout: f64| {
            c.call_bool(Request::WaitSignal {
                name: name.into(),
                value,
                timeout: Some(timeout),
            })
        },
    );
    let c = ch.clone();
    engine.register_fn(
        "wait_signal",
        move |name: ImmutableString, value: bool, timeout: i64| {
            c.call_bool(Request::WaitSignal {
                name: name.into(),
                value,
                timeout: Some(timeout as f64),
            })
        },
    );
    let c = ch.clone();
    engine.register_fn("time", move || c.call_number(Request::Time));
    let c = ch.clone();
    engine.register_fn("log", move |message: ImmutableString| {
        c.call_unit(Request::Log(message.into()))
    });
    let c = ch.clone();
    engine.register_fn("log", move |value: Dynamic| {
        c.call_unit(Request::Log(value.to_string()))
    });
}
