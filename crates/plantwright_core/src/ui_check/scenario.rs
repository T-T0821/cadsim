//! 手順書 (シナリオ) の読み取り
//!
//! 1行に1つの操作を書く。`#` から行末まではコメント。文字列は `"…"` で囲む。
//!
//! ```text
//! # サンプルを開いて部品を選ぶ
//! open assets/sample_assembly.step
//! wait_loaded
//! click "ベース"
//! screenshot 01_selected
//! key ctrl+z
//! ```
//!
//! 操作の一覧は [`Step`] を参照。

use std::fmt;

use super::Rect;

/// 手順書の1行分の操作
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// `open パス`: ファイルを開く (プロジェクトは開き直し、STEP などは取り込み)
    Open(String),
    /// `wait フレーム数`: 何もせずに待つ
    Wait(u32),
    /// `wait_loaded`: 読み込みや計算が終わるまで待つ
    WaitLoaded,
    /// `click "表示名" [何番目]`: 表示名の部品 (ボタン・メニュー・タブなど) をクリックする
    Click(WidgetQuery),
    /// `double_click "表示名" [何番目]`
    DoubleClick(WidgetQuery),
    /// `hover "表示名" [何番目]`: マウスを乗せるだけ (ツールチップやサブメニューを開く)
    Hover(WidgetQuery),
    /// `click_at x y`: ウィンドウの座標をクリックする (3D ビューの部品を選ぶときなど)
    ClickAt(f32, f32),
    /// `drag x1 y1 x2 y2 [ボタン]`: 押したまま動かして離す。ボタンは left (既定) / right / middle
    Drag {
        from: (f32, f32),
        to: (f32, f32),
        button: MouseButton,
    },
    /// `scroll x y 量`: ホイールを回す (正で手前 = 拡大)
    Scroll { at: (f32, f32), lines: f32 },
    /// `key ctrl+z`: キーを押して離す
    Key(KeyChord),
    /// `hold w 60`: キーを何フレームか押したままにしてから離す (歩行モードで歩くときなど)
    Hold(KeyChord, u32),
    /// `type "文字"`: 入力欄に文字を打つ (先に click で入力欄を選んでおく)
    Type(String),
    /// `expect "表示名"`: 表示名の部品が画面にあることを確かめる (なければ失敗)
    Expect(String),
    /// `expect_not "表示名"`: 表示名の部品が画面にないことを確かめる
    ExpectNot(String),
    /// `mask x y 幅 高さ`: この後に撮る写真で、比べない領域を足す (時計や FPS の表示など)
    Mask(Rect),
    /// `clear_masks`: 比べない領域をすべて取り消す
    ClearMasks,
    /// `screenshot 名前`: 画面を撮り、基準画像 `名前.png` と比べる
    Screenshot(String),
    /// `dump_widgets 名前`: 画面上の部品の表示名と位置を `名前.txt` に書き出す (手順書を書くときの手がかり)
    DumpWidgets(String),
}

/// 表示名で部品を探す条件
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetQuery {
    pub label: String,
    /// 同じ表示名が複数あるとき、上から何番目か (1 から)
    pub nth: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// 修飾キー付きのキー (`ctrl+shift+z` など)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyChord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

/// 押せるキー。アプリ側でキーボードのコードに直す
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A〜Z
    Letter(char),
    /// 0〜9
    Digit(u8),
    /// F1〜F12
    F(u8),
    Enter,
    Escape,
    Space,
    Tab,
    Backspace,
    Delete,
    Home,
    End,
    Left,
    Right,
    Up,
    Down,
    /// 修飾キーだけを押す (`key shift` など)。`ctrl+…` と書いたときにも使う
    Ctrl,
    Shift,
    Alt,
}

impl Key {
    fn parse(name: &str) -> Option<Key> {
        let lower = name.to_ascii_lowercase();
        let key = match lower.as_str() {
            "enter" | "return" => Key::Enter,
            "esc" | "escape" => Key::Escape,
            "space" => Key::Space,
            "tab" => Key::Tab,
            "backspace" => Key::Backspace,
            "delete" | "del" => Key::Delete,
            "home" => Key::Home,
            "end" => Key::End,
            "left" => Key::Left,
            "right" => Key::Right,
            "up" => Key::Up,
            "down" => Key::Down,
            "ctrl" | "control" => Key::Ctrl,
            "shift" => Key::Shift,
            "alt" => Key::Alt,
            _ => {
                let mut chars = lower.chars();
                let first = chars.next()?;
                let rest: String = chars.collect();
                if rest.is_empty() && first.is_ascii_lowercase() {
                    Key::Letter(first.to_ascii_uppercase())
                } else if rest.is_empty() && first.is_ascii_digit() {
                    Key::Digit(first as u8 - b'0')
                } else if first == 'f' {
                    let n: u8 = rest.parse().ok()?;
                    if !(1..=12).contains(&n) {
                        return None;
                    }
                    Key::F(n)
                } else {
                    return None;
                }
            }
        };
        Some(key)
    }
}

impl KeyChord {
    /// `ctrl+shift+z` のような書き方を読む
    pub fn parse(text: &str) -> Option<KeyChord> {
        let mut chord = KeyChord {
            ctrl: false,
            shift: false,
            alt: false,
            key: Key::Escape,
        };
        let parts: Vec<&str> = text.split('+').collect();
        let (key, modifiers) = parts.split_last()?;
        for m in modifiers {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => chord.ctrl = true,
                "shift" => chord.shift = true,
                "alt" => chord.alt = true,
                _ => return None,
            }
        }
        chord.key = Key::parse(key)?;
        Some(chord)
    }
}

/// 手順書の誤り (何行目の何がおかしいか)
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} 行目: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// 手順書の1行 (何行目かを失敗の報告に使う)
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub number: usize,
    pub step: Step,
}

/// 手順書を読み、操作の並びにする
pub fn parse(text: &str) -> Result<Vec<Line>, ParseError> {
    let mut steps = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let err = |message: String| ParseError { line, message };
        let words = split_words(raw).map_err(err)?;
        let Some((command, args)) = words.split_first() else {
            continue;
        };
        steps.push(Line {
            number: line,
            step: parse_step(command, args).map_err(err)?,
        });
    }
    Ok(steps)
}

fn parse_step(command: &str, args: &[String]) -> Result<Step, String> {
    let count = |n: usize| -> Result<(), String> {
        if args.len() == n {
            Ok(())
        } else {
            Err(format!("{command} には値が {n} 個要ります"))
        }
    };
    let num = |i: usize| -> Result<f32, String> {
        args[i]
            .parse::<f32>()
            .map_err(|_| format!("「{}」は数ではありません", args[i]))
    };
    let query = || -> Result<WidgetQuery, String> {
        let (label, nth) = match args {
            [label] => (label.clone(), 1),
            [label, nth] => {
                let nth: usize = nth
                    .parse()
                    .ok()
                    .filter(|&n| n >= 1)
                    .ok_or_else(|| format!("「{nth}」は 1 以上の番号ではありません"))?;
                (label.clone(), nth)
            }
            _ => return Err(format!("{command} には表示名 (と何番目か) を書きます")),
        };
        Ok(WidgetQuery { label, nth })
    };
    let step = match command {
        "open" => {
            count(1)?;
            Step::Open(args[0].clone())
        }
        "wait" => {
            count(1)?;
            let frames: u32 = args[0]
                .parse()
                .map_err(|_| format!("「{}」はフレーム数ではありません", args[0]))?;
            Step::Wait(frames)
        }
        "wait_loaded" => {
            count(0)?;
            Step::WaitLoaded
        }
        "click" => Step::Click(query()?),
        "double_click" => Step::DoubleClick(query()?),
        "hover" => Step::Hover(query()?),
        "click_at" => {
            count(2)?;
            Step::ClickAt(num(0)?, num(1)?)
        }
        "drag" => {
            if args.len() != 4 && args.len() != 5 {
                return Err("drag には x1 y1 x2 y2 (とボタン) を書きます".into());
            }
            let button = match args.get(4).map(|s| s.as_str()) {
                None | Some("left") => MouseButton::Left,
                Some("right") => MouseButton::Right,
                Some("middle") => MouseButton::Middle,
                Some(other) => return Err(format!("「{other}」はボタンの名前ではありません")),
            };
            Step::Drag {
                from: (num(0)?, num(1)?),
                to: (num(2)?, num(3)?),
                button,
            }
        }
        "scroll" => {
            count(3)?;
            Step::Scroll {
                at: (num(0)?, num(1)?),
                lines: num(2)?,
            }
        }
        "key" => {
            count(1)?;
            Step::Key(
                KeyChord::parse(&args[0])
                    .ok_or_else(|| format!("「{}」はキーの名前ではありません", args[0]))?,
            )
        }
        "hold" => {
            count(2)?;
            let frames = args[1]
                .parse::<u32>()
                .map_err(|_| format!("「{}」はフレーム数ではありません", args[1]))?;
            Step::Hold(
                KeyChord::parse(&args[0])
                    .ok_or_else(|| format!("「{}」はキーの名前ではありません", args[0]))?,
                frames,
            )
        }
        "type" => {
            count(1)?;
            Step::Type(args[0].clone())
        }
        "expect" => {
            count(1)?;
            Step::Expect(args[0].clone())
        }
        "expect_not" => {
            count(1)?;
            Step::ExpectNot(args[0].clone())
        }
        "mask" => {
            count(4)?;
            Step::Mask(Rect::new(num(0)?, num(1)?, num(2)?, num(3)?))
        }
        "clear_masks" => {
            count(0)?;
            Step::ClearMasks
        }
        "screenshot" => {
            count(1)?;
            check_file_name(&args[0])?;
            Step::Screenshot(args[0].clone())
        }
        "dump_widgets" => {
            count(1)?;
            check_file_name(&args[0])?;
            Step::DumpWidgets(args[0].clone())
        }
        other => return Err(format!("「{other}」という操作はありません")),
    };
    Ok(step)
}

/// 写真の名前はそのままファイル名にするので、フォルダの区切りなどは使わせない
fn check_file_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "「{name}」は名前に使えません (英数字・ひらがななど・_・- だけにしてください)"
        ))
    }
}

/// 空白で区切る。`"…"` の中の空白は区切らない。`#` から後はコメント
fn split_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '#' {
            break;
        } else if c == '"' {
            chars.next();
            let mut word = String::new();
            loop {
                match chars.next() {
                    Some('"') => break,
                    Some('\\') => match chars.next() {
                        Some(escaped) => word.push(escaped),
                        None => return Err("「\\」の後に文字がありません".into()),
                    },
                    Some(other) => word.push(other),
                    None => return Err("「\"」が閉じていません".into()),
                }
            }
            words.push(word);
        } else {
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                word.push(c);
                chars.next();
            }
            words.push(word);
        }
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 手順書を読む() {
        let text = r#"
            # コメントだけの行
            open assets/sample_assembly.step
            wait_loaded
            click "開く…  (Ctrl+O)"   # 行末のコメント
            click "保存" 2
            key ctrl+shift+z
            type "コンベア \"A\""
            mask 10 20 30 40
            screenshot 01_概要
        "#;
        let lines = parse(text).unwrap();
        assert_eq!(lines[0].number, 3);
        let steps: Vec<Step> = lines.into_iter().map(|l| l.step).collect();
        assert_eq!(
            steps,
            vec![
                Step::Open("assets/sample_assembly.step".into()),
                Step::WaitLoaded,
                Step::Click(WidgetQuery {
                    label: "開く…  (Ctrl+O)".into(),
                    nth: 1
                }),
                Step::Click(WidgetQuery {
                    label: "保存".into(),
                    nth: 2
                }),
                Step::Key(KeyChord {
                    ctrl: true,
                    shift: true,
                    alt: false,
                    key: Key::Letter('Z')
                }),
                Step::Type("コンベア \"A\"".into()),
                Step::Mask(Rect::new(10.0, 20.0, 30.0, 40.0)),
                Step::Screenshot("01_概要".into()),
            ]
        );
    }

    #[test]
    fn キーの名前() {
        let key = |s: &str| KeyChord::parse(s).map(|c| c.key);
        assert_eq!(key("Delete"), Some(Key::Delete));
        assert_eq!(key("f5"), Some(Key::F(5)));
        assert_eq!(key("7"), Some(Key::Digit(7)));
        assert_eq!(key("esc"), Some(Key::Escape));
        assert_eq!(key("f13"), None);
        assert_eq!(key("hyper+a"), None);
        assert_eq!(key("ctrl+"), None);
    }

    #[test]
    fn 誤りは行番号付きで返す() {
        let err = parse("wait 3\n\nclikc \"a\"").unwrap_err();
        assert_eq!(err.line, 3);
        assert!(err.message.contains("clikc"));

        assert_eq!(parse("click \"a").unwrap_err().line, 1);
        assert!(parse("click \"a\" 0").is_err());
        assert!(parse("screenshot ../x").is_err());
        assert!(parse("wait").is_err());
        assert!(parse("drag 1 2 3 4 side").is_err());
    }
}
