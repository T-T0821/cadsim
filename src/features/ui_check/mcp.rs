//! `--mcp`: 標準入力・標準出力の MCP で、AI のツール呼び出しから画面を操作する
//!
//! 標準入力を読むスレッドが1行ずつ `plantwright_core::ui_check::mcp::handle_line` に渡し、
//! すぐ答えられるもの (initialize・tools/list など) はその場で標準出力に返す。
//! ツールの呼び出しは手順 (UI チェックの手順書の行) になって届くので、毎フレームの drive が
//! UI チェックと同じ Runner で1つずつ流し、入力を本物と同じメッセージで流して、
//! 終わったら写真や部品の一覧を付けて返事をする。
//!
//! 標準出力は MCP の返事だけに使う (Bevy のログは標準エラーに出る)。
//! 標準入力が閉じたら (AI のアプリが終わったら) アプリも終わる。
//!
//! 時計は実時間のまま動かすので、描画が遅い (1 FPS 前後の) 環境では、押してから離すまでが
//! 1 秒を超える。egui はそれをクリックと見なさない (長押し扱い) ので、クリックとダブルクリックの
//! 時間の上限を、毎フレームその時のフレームの長さに合わせて延ばす (`click_timing`)。手順はフレーム数で待つため遅い環境では長くかかるので、
//! 1つの呼び出しが `CALL_DEADLINE` を超えたら、押したままにならない所で止めて返事をする。

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{EguiContexts, EguiOutput, EguiPrimaryContextPass, PrimaryEguiContext};
use plantwright_core::ui_check::mcp::{self, Content, Incoming};
use plantwright_core::ui_check::runner::{Command, Observation, Runner};
use plantwright_core::ui_check::scenario::Line;
use plantwright_core::ui_check::widget::Widget;
use serde_json::Value;

use super::{
    Busy, InputWriters, ShotSlot, enable_accesskit, request_screenshot, to_rgba, widgets_from,
};
use crate::document::import::{ImportJobs, open_any};
use crate::document::project::ProjectState;

/// 標準入力を読むスレッドから届くもの
enum Message {
    Call {
        id: Value,
        lines: Vec<Line>,
    },
    /// 標準入力が閉じた
    Closed,
}

#[derive(Resource)]
struct McpServer {
    incoming: Mutex<Receiver<Message>>,
    /// まだ始めていない呼び出し (届いた順に1つずつ流す)
    queue: VecDeque<(Value, Vec<Line>)>,
    current: Option<Running>,
    /// 前のフレームに画面に出ていた部品
    widgets: Vec<Widget>,
}

/// 流している途中の呼び出し
struct Running {
    id: Value,
    runner: Runner,
    /// 手順が2行以上 (失敗の報告に行番号を付ける)
    script: bool,
    contents: Vec<Content>,
    /// 撮影を頼んで、まだ届いていない写真
    shot: Option<ShotSlot>,
    /// 失敗したときの画面を撮った
    failure_shot: bool,
    started: Instant,
    /// 始めてから描いたフレーム数
    frames: u32,
    /// 時間切れで止めたときの理由
    timed_out: Option<String>,
}

pub(super) fn build(app: &mut App) {
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            match mcp::handle_line(&line) {
                Incoming::Reply(reply) => write_line(&reply),
                Incoming::Call { id, lines } => {
                    if sender.send(Message::Call { id, lines }).is_err() {
                        return;
                    }
                }
                Incoming::Ignore => {}
            }
        }
        let _ = sender.send(Message::Closed);
    });
    app.insert_resource(McpServer {
        incoming: Mutex::new(receiver),
        queue: VecDeque::new(),
        current: None,
        widgets: Vec::new(),
    })
    .add_systems(
        EguiPrimaryContextPass,
        (enable_accesskit, allow_slow_clicks),
    )
    .add_systems(First, drive)
    .add_systems(Last, (collect_widgets, receive_capture));
}

/// 返事を1行で標準出力に書く (読むスレッドと drive の両方から呼ぶので、1行ごとにロックする)
fn write_line(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{text}");
    let _ = out.flush();
}

/// 描画が遅くても、押す → 離す (1 フレーム) をクリックと、2 回のクリック (2 フレーム) をダブルクリックと見なす
fn allow_slow_clicks(mut contexts: EguiContexts, time: Res<Time<Real>>) -> Result {
    let (click, double_click) = mcp::click_timing(time.delta_secs_f64());
    contexts.ctx_mut()?.options_mut(|options| {
        options.input_options.max_click_duration = click;
        options.input_options.max_double_click_delay = double_click;
    });
    Ok(())
}

fn collect_widgets(
    mut server: ResMut<McpServer>,
    outputs: Query<&EguiOutput, With<PrimaryEguiContext>>,
) {
    if let Some(widgets) = outputs.single().ok().and_then(widgets_from) {
        server.widgets = widgets;
    }
}

/// 毎フレームの始めに、流している呼び出しを1つ進める
#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    mut server: ResMut<McpServer>,
    busy: Busy,
    mut jobs: ResMut<ImportJobs>,
    mut project: ResMut<ProjectState>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    egui_context: Query<Entity, With<PrimaryEguiContext>>,
    mut writers: InputWriters,
    mut exit: MessageWriter<AppExit>,
) {
    let server = &mut *server;
    let received: Vec<Message> = server
        .incoming
        .lock()
        .map(|r| r.try_iter().collect())
        .unwrap_or_default();
    for message in received {
        match message {
            Message::Call { id, lines } => server.queue.push_back((id, lines)),
            Message::Closed => {
                exit.write(AppExit::Success);
                return;
            }
        }
    }
    if server.current.is_none()
        && let Some((id, lines)) = server.queue.pop_front()
    {
        server.current = Some(Running {
            id,
            script: lines.len() > 1,
            runner: Runner::new(lines),
            contents: Vec::new(),
            shot: None,
            failure_shot: false,
            started: Instant::now(),
            frames: 0,
            timed_out: None,
        });
    }
    let Some(running) = &mut server.current else {
        return;
    };
    let Ok((window_entity, mut window)) = windows.single_mut() else {
        return;
    };
    running.frames += 1;
    let elapsed = running.started.elapsed();
    if running.timed_out.is_none()
        && elapsed > mcp::CALL_DEADLINE
        && running.shot.is_none()
        && running.runner.interruptible()
    {
        running.timed_out = Some(mcp::deadline_message(elapsed, running.frames));
    }
    if running.timed_out.is_some() {
        finish(&mut commands, server);
        return;
    }
    let observation = Observation {
        busy: !jobs.loading.is_empty() || project.is_busy() || busy.any(),
        capturing: running.shot.is_some(),
        widgets: &server.widgets,
    };
    match running.runner.tick(&observation) {
        Command::Idle => {}
        Command::Inputs(inputs) => {
            for input in inputs {
                writers.send(
                    window_entity,
                    &mut window,
                    egui_context.single().ok(),
                    input,
                );
            }
        }
        Command::Open(path) => open_any(PathBuf::from(path), &mut jobs, &mut project),
        Command::Capture { .. } => running.shot = Some(request_screenshot(&mut commands)),
        Command::DumpWidgets { text, .. } => {
            running.contents.push(Content::Text(if text.is_empty() {
                "(表示名のある部品が見つかりません)".into()
            } else {
                format!("種類 x y 幅 高さ 表示名\n{text}")
            }))
        }
        Command::Finished => finish(&mut commands, server),
    }
}

/// 手順が終わった (か時間切れで止めた) ので返事をする。失敗なら、先にそのときの画面を撮る
fn finish(commands: &mut Commands, server: &mut McpServer) {
    let Some(running) = &mut server.current else {
        return;
    };
    if running.shot.is_some() {
        return;
    }
    let failure = running.timed_out.clone().or_else(|| {
        running.runner.failure().map(|f| {
            if running.script {
                format!("{} 行目: {}", f.line, f.message)
            } else {
                f.message.clone()
            }
        })
    });
    // 失敗したら、そのときの画面も返す (AI が様子を見て次の手を考えられるように)
    if failure.is_some() && !running.failure_shot {
        running.failure_shot = true;
        running.shot = Some(request_screenshot(commands));
        return;
    }
    write_line(&mcp::call_result(
        &running.id,
        &running.contents,
        failure.as_deref(),
    ));
    server.current = None;
}

/// 写真が届いたら、クリックの座標と同じ大きさの PNG にして返事に足す
fn receive_capture(mut server: ResMut<McpServer>, windows: Query<&Window, With<PrimaryWindow>>) {
    let Some(running) = &mut server.current else {
        return;
    };
    let Some(image) = running
        .shot
        .as_ref()
        .and_then(|slot| slot.lock().ok().and_then(|mut s| s.take()))
    else {
        return;
    };
    running.shot = None;
    let (width, height) = windows
        .single()
        .map(|w| (w.width().round() as u32, w.height().round() as u32))
        .unwrap_or((0, 0));
    let png = to_rgba(image)
        .ok_or_else(|| "画面写真を画像にできません".to_string())
        .and_then(|img| mcp::screenshot_png(&img, width, height));
    match png {
        Ok(png) => {
            running.contents.push(Content::Png(png));
            running.contents.push(Content::Text(format!(
                "画面写真 {width}×{height} (この座標で click_at・drag・scroll できます)"
            )));
        }
        Err(message) => running.contents.push(Content::Text(message)),
    }
}
