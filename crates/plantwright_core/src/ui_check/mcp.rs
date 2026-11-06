//! MCP (Model Context Protocol) の操作口
//!
//! `plantwright --mcp` で起動すると、標準入力・標準出力で MCP の JSON-RPC (1行に1つの JSON) を受け、
//! Claude などの AI がツール呼び出しで画面を操作できる。ツールは UI チェックの手順書の1行
//! ([`Step`]) に直して、手順書と同じ [`super::runner::Runner`] で流す。
//! ボタンは表示名で探すので、座標を当て推量せずにクリックできる。
//!
//! ここでは、届いた1行を読んで「すぐ返す返事」か「流す手順」にすること ([`handle_line`])、
//! ツールの一覧 ([`tools`]) と引数から手順への直し方 ([`tool_steps`])、
//! 結果の返事の組み立て ([`call_result`])、画面写真を返せる形にすること ([`screenshot_png`]) を受け持つ。
//! 標準入力を読むスレッドや入力の流し込み、撮影はアプリ側 (`src/features/ui_check/mcp.rs`)。

use std::time::Duration;

use serde_json::{Map, Value, json};

use super::compare::Image;
use super::scenario::{self, KeyChord, Line, MouseButton, Step, WidgetQuery};

/// サーバーの名前 (initialize の返事で名乗る)
pub const SERVER_NAME: &str = "plantwright";
/// 相手がプロトコルの版を言わなかったときに使う版
const DEFAULT_PROTOCOL: &str = "2025-06-18";

/// 1つの呼び出しにかけてよい時間。AI のアプリはおよそ 60 秒で待つのをやめるので、その前に返事をする
pub const CALL_DEADLINE: Duration = Duration::from_secs(45);

/// 時間切れで止めたときの理由。`frames` はその間に描けたフレーム数
pub fn deadline_message(elapsed: Duration, frames: u32) -> String {
    let fps = frames as f64 / elapsed.as_secs_f64().max(0.001);
    format!(
        "{} 秒で終わらなかったので止めました (画面の描画が遅く、およそ {fps:.1} FPS)。\
         残りの手順は流していません。screenshot で様子を見てから続けてください",
        elapsed.as_secs()
    )
}

/// egui のクリックの時間の上限 (押してから離すまで, ダブルクリックの2回目まで) を、1 フレームの長さ (秒) から決める。
///
/// 手順はクリックの押す・離すを続くフレームに、ダブルクリックの2回を2フレーム後に流すので、
/// 描画が遅いとそれだけで egui の既定 (0.8 秒・0.3 秒) を超え、長押しや別々のクリックになってしまう。
/// 上限は 3 フレーム分まで延ばす。別々の呼び出しのクリックは 5 フレーム以上離れるので、ダブルクリックにはならない
pub fn click_timing(frame_secs: f64) -> (f64, f64) {
    let frames = 3.0 * frame_secs.max(0.0);
    (frames.max(0.8), frames.max(0.3))
}

/// 初めにAIに渡す使い方の説明
const INSTRUCTIONS: &str = "plantwright (工場の設備とラインの 3D シミュレーター) の画面を操作します。\
まず screenshot で画面を見て、list_widgets でボタンやメニューの表示名を調べてください。\
ボタン・メニュー・タブは click に表示名を渡して押せます (座標より確実です)。\
3D ビューの中の部品は click_at に画面写真の座標を渡して選びます。\
操作のあとに画面を確かめたいときは、操作のツールに screenshot: true を付けると写真も返します。";

/// 届いた1行をどう扱うか
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    /// すぐ返す返事 (1行の JSON)
    Reply(String),
    /// 画面で手順を流し、終わったら [`call_result`] で返事をする
    Call { id: Value, lines: Vec<Line> },
    /// 返事の要らない通知や、相手からの返事
    Ignore,
}

/// ツールの結果に入れるもの
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Text(String),
    /// PNG のバイト列
    Png(Vec<u8>),
}

/// 届いた1行の JSON-RPC を読む
pub fn handle_line(line: &str) -> Incoming {
    let line = line.trim();
    if line.is_empty() {
        return Incoming::Ignore;
    }
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Incoming::Reply(error(&Value::Null, -32700, "JSON として読めません"));
    };
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // こちらから頼んでいないので、相手からの返事は捨てる
        return Incoming::Ignore;
    };
    // id のないものは通知 (notifications/initialized など)。返事はしない
    let Some(id) = message.get("id").cloned() else {
        return Incoming::Ignore;
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_PROTOCOL);
            Incoming::Reply(result(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }),
            ))
        }
        "ping" => Incoming::Reply(result(&id, json!({}))),
        "tools/list" => Incoming::Reply(result(&id, json!({ "tools": tools() }))),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Incoming::Reply(error(&id, -32602, "ツールの名前 (name) がありません"));
            };
            if !tools()
                .as_array()
                .is_some_and(|t| t.iter().any(|t| t["name"] == name))
            {
                return Incoming::Reply(error(
                    &id,
                    -32602,
                    &format!("「{name}」というツールはありません"),
                ));
            }
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match tool_steps(name, &args) {
                Ok(lines) => Incoming::Call { id, lines },
                // 引数の誤りは、AI が読んで直せるようツールの失敗として返す
                Err(message) => Incoming::Reply(call_result(&id, &[], Some(&message))),
            }
        }
        _ => Incoming::Reply(error(
            &id,
            -32601,
            &format!("「{method}」には答えられません"),
        )),
    }
}

/// ツールの結果の返事。`failure` があれば失敗 (isError) として返す
pub fn call_result(id: &Value, contents: &[Content], failure: Option<&str>) -> String {
    let mut items: Vec<Value> = contents
        .iter()
        .map(|c| match c {
            Content::Text(text) => json!({ "type": "text", "text": text }),
            Content::Png(bytes) => {
                json!({ "type": "image", "data": base64(bytes), "mimeType": "image/png" })
            }
        })
        .collect();
    if let Some(message) = failure {
        items.push(json!({ "type": "text", "text": format!("失敗: {message}") }));
    } else if items.is_empty() {
        items.push(json!({ "type": "text", "text": "できました" }));
    }
    result(
        id,
        json!({ "content": items, "isError": failure.is_some() }),
    )
}

fn result(id: &Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error(id: &Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// 操作のツールに共通の「終わったら画面写真も返す」引数
fn with_screenshot(mut properties: Map<String, Value>) -> Value {
    properties.insert(
        "screenshot".into(),
        json!({ "type": "boolean", "description": "true なら、操作のあとの画面写真も返す" }),
    );
    Value::Object(properties)
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required },
    })
}

fn props(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// ツールの一覧 (tools/list の返事)
pub fn tools() -> Value {
    let label = json!({ "type": "string", "description": "ボタン・メニュー・タブなどの表示名 (list_widgets で調べられる)" });
    let nth = json!({ "type": "integer", "minimum": 1, "description": "同じ表示名が複数あるとき、上から何番目か (既定 1)" });
    let x = json!({ "type": "number", "description": "画面写真の横の位置 (左端が 0)" });
    let y = json!({ "type": "number", "description": "画面写真の縦の位置 (上端が 0)" });
    let keys = json!({ "type": "string", "description": "キー。ctrl+z、shift+tab、enter、esc、f5、delete、a〜z、0〜9 など" });
    let query = || with_screenshot(props(json!({ "label": label, "nth": nth })));
    json!([
        tool(
            "screenshot",
            "いまの画面の写真を撮る。写真の座標は click_at・drag・scroll の座標と同じ",
            json!({}),
            &[],
        ),
        tool(
            "list_widgets",
            "画面に出ているボタン・メニュー・ラベル・入力欄の種類・位置 (x y 幅 高さ)・表示名の一覧",
            json!({}),
            &[],
        ),
        tool(
            "click",
            "表示名の部品 (ボタン・メニュー・タブ・たたんだ窓の見出しなど) をクリックする",
            query(),
            &["label"]
        ),
        tool(
            "double_click",
            "表示名の部品をダブルクリックする",
            query(),
            &["label"]
        ),
        tool(
            "hover",
            "表示名の部品にマウスを乗せる (ツールチップやサブメニューを出す)",
            query(),
            &["label"]
        ),
        tool(
            "click_at",
            "画面の座標をクリックする (3D ビューの部品を選ぶときなど)",
            with_screenshot(props(json!({ "x": x, "y": y }))),
            &["x", "y"],
        ),
        tool(
            "drag",
            "押したまま動かして離す。3D ビューでは左ドラッグで回転、右ドラッグで平行移動",
            with_screenshot(props(json!({
                "from_x": x, "from_y": y, "to_x": x, "to_y": y,
                "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "既定 left" },
            }))),
            &["from_x", "from_y", "to_x", "to_y"],
        ),
        tool(
            "scroll",
            "ホイールを回す。3D ビューでは正で拡大、負で縮小",
            with_screenshot(props(
                json!({ "x": x, "y": y, "lines": { "type": "number", "description": "回す量 (行)" } })
            )),
            &["x", "y", "lines"],
        ),
        tool(
            "key",
            "キーを押して離す (ショートカット)",
            with_screenshot(props(json!({ "keys": keys }))),
            &["keys"]
        ),
        tool(
            "hold",
            "キーを何フレームか押したままにする (歩行モードで歩くときなど。60 フレームでおよそ 1 秒)",
            with_screenshot(props(
                json!({ "keys": keys, "frames": { "type": "integer", "minimum": 1 } })
            )),
            &["keys", "frames"],
        ),
        tool(
            "type",
            "選んでいる入力欄に文字を打つ (先に click で入力欄を選ぶ)",
            with_screenshot(props(json!({ "text": { "type": "string" } }))),
            &["text"],
        ),
        tool(
            "open",
            "ファイルを開く (.plantwright のプロジェクトは開き直し、STEP・glTF などは取り込み)。読み込みが終わるまで待つ",
            with_screenshot(props(
                json!({ "path": { "type": "string", "description": "アプリを起動したフォルダからの相対パスか絶対パス" } })
            )),
            &["path"],
        ),
        tool(
            "wait",
            "何フレームか待つ (60 フレームでおよそ 1 秒)。読み込みや計算の途中なら、終わるまでも待つ",
            with_screenshot(props(
                json!({ "frames": { "type": "integer", "minimum": 0 } })
            )),
            &["frames"],
        ),
        tool(
            "run_script",
            "UI チェックの手順書 (docs/ui-check.md の書き方) を何行かまとめて流す。\
             screenshot 名前 の行ごとに写真を、dump_widgets 名前 の行ごとに部品の一覧を返す。\
             expect \"表示名\" で画面にあることを確かめられる",
            json!({ "script": { "type": "string", "description": "1行に1つの操作" } }),
            &["script"],
        ),
    ])
}

fn number(args: &Value, key: &str) -> Result<f32, String> {
    args.get(key)
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .ok_or_else(|| format!("{key} (数) がありません"))
}

fn text(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{key} (文字) がありません"))
}

fn count(args: &Value, key: &str) -> Result<u32, String> {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| format!("{key} (0 以上の整数) がありません"))
}

fn chord(args: &Value) -> Result<KeyChord, String> {
    let keys = text(args, "keys")?;
    KeyChord::parse(keys.trim()).ok_or_else(|| format!("「{keys}」はキーの名前ではありません"))
}

/// ツールの呼び出しを、画面で流す手順にする
pub fn tool_steps(name: &str, args: &Value) -> Result<Vec<Line>, String> {
    let query = || -> Result<WidgetQuery, String> {
        let nth = match args.get("nth") {
            None | Some(Value::Null) => 1,
            Some(v) => v
                .as_u64()
                .filter(|&n| n >= 1)
                .ok_or("nth は 1 以上の整数です")? as usize,
        };
        Ok(WidgetQuery {
            label: text(args, "label")?,
            nth,
        })
    };
    let steps = match name {
        "screenshot" => vec![Step::Screenshot("screenshot".into())],
        "list_widgets" => vec![Step::DumpWidgets("widgets".into())],
        "run_script" => {
            let lines = scenario::parse(&text(args, "script")?).map_err(|e| e.to_string())?;
            if lines.is_empty() {
                return Err("手順が1行もありません".into());
            }
            return Ok(lines);
        }
        "click" => vec![Step::Click(query()?)],
        "double_click" => vec![Step::DoubleClick(query()?)],
        "hover" => vec![Step::Hover(query()?)],
        "click_at" => vec![Step::ClickAt {
            at: (number(args, "x")?, number(args, "y")?),
            button: MouseButton::Left,
            presses: 1,
        }],
        "drag" => {
            let button = match args.get("button").and_then(Value::as_str) {
                None | Some("left") => MouseButton::Left,
                Some("right") => MouseButton::Right,
                Some("middle") => MouseButton::Middle,
                Some(other) => return Err(format!("「{other}」はボタンの名前ではありません")),
            };
            vec![Step::Drag {
                from: (number(args, "from_x")?, number(args, "from_y")?),
                to: (number(args, "to_x")?, number(args, "to_y")?),
                button,
            }]
        }
        "scroll" => vec![Step::Scroll {
            at: (number(args, "x")?, number(args, "y")?),
            lines: number(args, "lines")?,
        }],
        "key" => vec![Step::Key(chord(args)?)],
        "hold" => vec![Step::Hold(chord(args)?, count(args, "frames")?)],
        "type" => vec![Step::Type(text(args, "text")?)],
        "open" => vec![Step::Open(text(args, "path")?), Step::WaitLoaded],
        "wait" => vec![Step::Wait(count(args, "frames")?), Step::WaitLoaded],
        other => return Err(format!("「{other}」というツールはありません")),
    };
    let mut lines: Vec<Line> = steps
        .into_iter()
        .enumerate()
        .map(|(i, step)| Line {
            number: i + 1,
            step,
        })
        .collect();
    if args.get("screenshot").and_then(Value::as_bool) == Some(true) {
        lines.push(Line {
            number: lines.len() + 1,
            step: Step::Screenshot("screenshot".into()),
        });
    }
    Ok(lines)
}

/// 画面写真を、クリックの座標 (ウィンドウの論理ピクセル) と同じ大きさの PNG にする。
/// 高解像度の画面では写真が論理ピクセルの倍の大きさで届くので、縮めて座標をそろえる
pub fn screenshot_png(img: &Image, width: u32, height: u32) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};

    let source = image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone())
        .ok_or("画面写真の大きさが合いません")?;
    let scaled = if (width, height) == (img.width, img.height) || width == 0 || height == 0 {
        source
    } else {
        image::imageops::resize(
            &source,
            width,
            height,
            image::imageops::FilterType::Triangle,
        )
    };
    let mut bytes = Vec::new();
    PngEncoder::new_with_quality(&mut bytes, CompressionType::Fast, FilterType::Adaptive)
        .write_image(
            scaled.as_raw(),
            scaled.width(),
            scaled.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("画面写真を PNG にできません: {e}"))?;
    Ok(bytes)
}

/// 標準の Base64 (RFC 4648、= で埋める)
pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_check::scenario::Key;

    fn reply(line: &str) -> Value {
        match handle_line(line) {
            Incoming::Reply(text) => serde_json::from_str(&text).unwrap(),
            other => panic!("返事ではありません: {other:?}"),
        }
    }

    #[test]
    fn initialize_echoes_protocol_and_offers_tools() {
        let r = reply(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{}}}"#,
        );
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(r["result"]["serverInfo"]["name"], SERVER_NAME);
        assert!(r["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn notifications_and_responses_get_no_reply() {
        assert_eq!(
            handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            Incoming::Ignore
        );
        assert_eq!(
            handle_line(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#),
            Incoming::Ignore
        );
        assert_eq!(handle_line("   "), Incoming::Ignore);
    }

    #[test]
    fn bad_json_and_unknown_methods_are_errors() {
        assert_eq!(reply("{oops")["error"]["code"], -32700);
        let r = reply(r#"{"jsonrpc":"2.0","id":"a","method":"resources/list"}"#);
        assert_eq!(r["id"], "a");
        assert_eq!(r["error"]["code"], -32601);
        let r = reply(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"fly","arguments":{}}}"#,
        );
        assert_eq!(r["error"]["code"], -32602);
    }

    #[test]
    fn every_listed_tool_has_a_schema_and_steps() {
        let r = reply(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let tools = r["result"]["tools"].as_array().unwrap();
        assert!(tools.len() >= 10);
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
            assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
        }
    }

    #[test]
    fn tool_call_becomes_steps() {
        let call = handle_line(
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"click","arguments":{"label":"再生","nth":2,"screenshot":true}}}"#,
        );
        let Incoming::Call { id, lines } = call else {
            panic!("手順になりません: {call:?}");
        };
        assert_eq!(id, 7);
        let steps: Vec<Step> = lines.into_iter().map(|l| l.step).collect();
        assert_eq!(
            steps,
            vec![
                Step::Click(WidgetQuery {
                    label: "再生".into(),
                    nth: 2
                }),
                Step::Screenshot("screenshot".into()),
            ]
        );
    }

    #[test]
    fn arguments_are_checked() {
        let steps = |name: &str, args: Value| {
            tool_steps(name, &args).map(|l| l.into_iter().map(|l| l.step).collect::<Vec<_>>())
        };
        assert_eq!(
            steps("key", json!({ "keys": "ctrl+shift+z" })).unwrap(),
            vec![Step::Key(KeyChord {
                ctrl: true,
                shift: true,
                alt: false,
                key: Key::Letter('Z')
            })]
        );
        assert!(steps("key", json!({ "keys": "hyper+q" })).is_err());
        assert!(steps("click", json!({})).is_err());
        assert!(steps("click", json!({ "label": "x", "nth": 0 })).is_err());
        assert!(
            steps(
                "drag",
                json!({ "from_x": 1, "from_y": 2, "to_x": 3, "to_y": 4, "button": "side" })
            )
            .is_err()
        );
        assert_eq!(
            steps("open", json!({ "path": "a.step" })).unwrap(),
            vec![Step::Open("a.step".into()), Step::WaitLoaded]
        );
        assert_eq!(
            steps("click_at", json!({ "x": 10.5, "y": 20 })).unwrap(),
            vec![Step::ClickAt {
                at: (10.5, 20.0),
                button: MouseButton::Left,
                presses: 1,
            }]
        );
        let script = steps(
            "run_script",
            json!({ "script": "click \"ファイル\"\nscreenshot a\n" }),
        )
        .unwrap();
        assert_eq!(script.len(), 2);
        assert!(steps("run_script", json!({ "script": "# 何もしない" })).is_err());
        assert!(steps("run_script", json!({ "script": "fly 1" })).is_err());
    }

    #[test]
    fn bad_arguments_come_back_as_tool_errors() {
        let r = reply(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"click_at","arguments":{"x":1}}}"#,
        );
        assert_eq!(r["result"]["isError"], true);
        assert!(
            r["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains('y')
        );
    }

    #[test]
    fn results_carry_text_and_images() {
        let r: Value = serde_json::from_str(&call_result(
            &json!(9),
            &[Content::Text("一覧".into()), Content::Png(vec![1, 2, 3])],
            None,
        ))
        .unwrap();
        assert_eq!(r["result"]["isError"], false);
        assert_eq!(r["result"]["content"][0]["text"], "一覧");
        assert_eq!(r["result"]["content"][1]["type"], "image");
        assert_eq!(r["result"]["content"][1]["data"], "AQID");
        let r: Value =
            serde_json::from_str(&call_result(&json!(9), &[], Some("見つかりません"))).unwrap();
        assert_eq!(r["result"]["isError"], true);
        let r: Value = serde_json::from_str(&call_result(&json!(9), &[], None)).unwrap();
        assert_eq!(r["result"]["content"][0]["text"], "できました");
    }

    #[test]
    fn deadline_message_tells_the_frame_rate() {
        let text = deadline_message(Duration::from_secs(45), 36);
        assert!(text.contains("45 秒"), "{text}");
        assert!(text.contains("0.8 FPS"), "{text}");
    }

    #[test]
    fn click_timing_stretches_with_slow_frames() {
        // ふつうの速さでは egui の既定のまま
        assert_eq!(click_timing(1.0 / 60.0), (0.8, 0.3));
        // 1 FPS なら 3 秒まで
        assert_eq!(click_timing(1.0), (3.0, 3.0));
        assert_eq!(click_timing(-1.0), (0.8, 0.3));
    }

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn screenshot_is_scaled_to_logical_size() {
        let img = Image::new(4, 2, vec![255; 4 * 2 * 4]);
        let png = screenshot_png(&img, 2, 1).unwrap();
        let back = image::load_from_memory(&png).unwrap();
        assert_eq!((back.width(), back.height()), (2, 1));
        let same = image::load_from_memory(&screenshot_png(&img, 4, 2).unwrap()).unwrap();
        assert_eq!((same.width(), same.height()), (4, 2));
    }
}
