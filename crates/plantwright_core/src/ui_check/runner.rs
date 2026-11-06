//! 手順を1フレームずつ進める
//!
//! アプリは毎フレーム [`Runner::tick`] に「いまの画面の様子」([`Observation`]) を渡し、
//! 返ってきた [`Command`] のとおりに入力を流したり写真を撮ったりする。
//! クリックは「マウスを動かす → 押す → 離す」を別々のフレームに分け、
//! 本物の操作と同じように画面が反応する時間を置く。

use super::Rect;
use super::scenario::{Key, KeyChord, Line, MouseButton, Step};
use super::widget::{self, Found, Widget};

/// 操作のあと、画面が落ち着くまで待つフレーム数
const SETTLE_FRAMES: u32 = 3;
/// マウスを乗せたあと、ツールチップやサブメニューが出るまで待つフレーム数
const HOVER_FRAMES: u32 = 40;
/// 部品が出てくるのを待つ最大のフレーム数
const FIND_TIMEOUT: u32 = 180;
/// 読み込みや計算が終わるのを待つ最大のフレーム数
const LOAD_TIMEOUT: u32 = 6000;
/// 読み込みが終わったとみなすまでに、続けて「暇」なフレーム数
const IDLE_FRAMES: u32 = 10;
/// ドラッグを何フレームに分けて動かすか
const DRAG_STEPS: u32 = 10;

/// アプリに流してもらう入力
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// マウスをウィンドウの座標へ動かす
    MoveTo(f32, f32),
    Button {
        button: MouseButton,
        pressed: bool,
    },
    /// ホイール (行数。正で手前)
    Scroll(f32),
    Key {
        key: Key,
        pressed: bool,
    },
    /// 入力欄への文字
    Text(String),
}

/// このフレームでアプリにしてほしいこと
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// 何もしない (待っている)
    Idle,
    Inputs(Vec<Input>),
    /// ファイルを開く
    Open(String),
    /// 画面を撮る。`masks` は比べない領域
    Capture {
        name: String,
        masks: Vec<Rect>,
    },
    /// 部品の一覧を書き出す
    DumpWidgets {
        name: String,
        text: String,
    },
    /// 手順が終わった (失敗で止まったときも)。結果は [`Runner::failure`]
    Finished,
}

/// いまの画面の様子
#[derive(Clone, Copy, Debug)]
pub struct Observation<'a> {
    /// 読み込み・計算・書き出しなどが進行中
    pub busy: bool,
    /// 撮った写真がまだ届いていない
    pub capturing: bool,
    /// 画面に出ている部品
    pub widgets: &'a [Widget],
}

/// 手順が失敗した場所と理由
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    /// 手順書の何行目か
    pub line: usize,
    pub message: String,
}

/// 手順の進み具合
pub struct Runner {
    lines: Vec<Line>,
    /// いま実行中の手順
    pc: usize,
    /// 手順の中の何段目か (クリックなら 動かす → 押す → 離す)
    phase: u32,
    /// 待ったフレーム数 (部品を探す・読み込みを待つ)
    counter: u32,
    /// 次の手順の前に待つ残りフレーム数
    cooldown: u32,
    /// 続けて「計算中」だったフレーム数
    busy_frames: u32,
    /// クリックやドラッグの位置
    target: (f32, f32),
    /// 前のフレームで見つけた部品の場所 (止まっているかを見る)
    last_rect: Option<Rect>,
    masks: Vec<Rect>,
    failure: Option<Failure>,
    finished: bool,
}

impl Runner {
    pub fn new(lines: Vec<Line>) -> Self {
        Self {
            lines,
            pc: 0,
            phase: 0,
            counter: 0,
            cooldown: 0,
            busy_frames: 0,
            target: (0.0, 0.0),
            last_rect: None,
            masks: Vec::new(),
            failure: None,
            finished: false,
        }
    }

    /// いま止めても、マウスのボタンやキーを押したままにしないか。
    /// 押す・離すを何フレームかに分ける手順の途中だけ false (時間切れで止めるときに見る)
    pub fn interruptible(&self) -> bool {
        self.phase == 0
            || !matches!(
                self.lines.get(self.pc).map(|l| &l.step),
                Some(
                    Step::Click(_)
                        | Step::DoubleClick(_)
                        | Step::ClickAt { .. }
                        | Step::Drag { .. }
                        | Step::Scroll { .. }
                        | Step::Key(_)
                        | Step::Hold(..)
                )
            )
    }

    /// 失敗して止まったときの理由
    pub fn failure(&self) -> Option<&Failure> {
        self.failure.as_ref()
    }

    /// いま実行中の手順の行番号 (終わっていれば None)
    pub fn current_line(&self) -> Option<usize> {
        self.lines.get(self.pc).map(|l| l.number)
    }

    /// 1フレーム進める
    pub fn tick(&mut self, obs: &Observation) -> Command {
        if self.finished {
            return Command::Finished;
        }
        // 読み込みや別スレッドの計算の途中は、手順も待ち時間も進めない (アプリは時計を止める)。
        // 計算が何フレームで終わるかは毎回違うので、その間を「なかったこと」にして、
        // どの手順も毎回同じ場面で実行されるようにする
        if obs.busy {
            self.busy_frames += 1;
            if self.busy_frames > LOAD_TIMEOUT {
                self.failure = Some(Failure {
                    line: self.current_line().unwrap_or(0),
                    message: "読み込みや計算が終わりません".into(),
                });
                self.finished = true;
                return Command::Finished;
            }
            return Command::Idle;
        }
        self.busy_frames = 0;
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return Command::Idle;
        }
        let Some(line) = self.lines.get(self.pc) else {
            self.finished = true;
            return Command::Finished;
        };
        let (number, step) = (line.number, line.step.clone());
        match self.run(&step, obs) {
            Ok(command) => command,
            Err(message) => {
                self.failure = Some(Failure {
                    line: number,
                    message,
                });
                self.finished = true;
                Command::Finished
            }
        }
    }

    /// 今の手順を終えて、次の手順の前に `settle` フレーム待つ
    fn next(&mut self, settle: u32) {
        self.pc += 1;
        self.phase = 0;
        self.counter = 0;
        self.last_rect = None;
        self.cooldown = settle;
    }

    fn run(&mut self, step: &Step, obs: &Observation) -> Result<Command, String> {
        use Input::*;
        let left = MouseButton::Left;
        let command = match step {
            Step::Open(path) => {
                self.next(SETTLE_FRAMES);
                Command::Open(path.clone())
            }
            Step::Wait(frames) => {
                self.next(*frames);
                Command::Idle
            }
            Step::WaitLoaded => {
                // 計算中のフレームは tick の始めで飛ばすので、ここに来るのは暇なフレームだけ。
                // 次の計算が始まることもあるので、続けて何フレームか暇なら終わったとみなす
                self.phase += 1;
                if self.phase >= IDLE_FRAMES {
                    self.next(0);
                }
                Command::Idle
            }
            Step::Click(query) | Step::DoubleClick(query) | Step::Hover(query) => {
                if self.phase == 0 {
                    match widget::find(obs.widgets, query) {
                        // 開いたばかりの窓などは動いている途中なので、
                        // 2 フレーム続けて同じ場所にあるのを確かめてからクリックする
                        Found::Widget(w) if self.last_rect != Some(w.rect) => {
                            self.last_rect = Some(w.rect);
                            self.wait_or_fail(FIND_TIMEOUT, || {
                                format!("「{}」が動き続けていてクリックできません", query.label)
                            })?;
                            return Ok(Command::Idle);
                        }
                        Found::Widget(w) => self.target = w.rect.center(),
                        Found::Missing { similar } => {
                            self.wait_or_fail(FIND_TIMEOUT, || {
                                missing_message(&query.label, query.nth, &similar)
                            })?;
                            return Ok(Command::Idle);
                        }
                    }
                }
                let presses = match step {
                    Step::Hover(_) => 0,
                    Step::Click(_) => 1,
                    _ => 2,
                };
                self.click_phases(
                    presses,
                    left,
                    if presses == 0 {
                        HOVER_FRAMES
                    } else {
                        SETTLE_FRAMES
                    },
                )
            }
            Step::ClickAt {
                at,
                button,
                presses,
            } => {
                if self.phase == 0 {
                    self.target = *at;
                }
                self.click_phases(*presses, *button, SETTLE_FRAMES)
            }
            Step::Drag { from, to, button } => {
                let phase = self.phase;
                self.phase += 1;
                if phase == 0 {
                    Command::Inputs(vec![MoveTo(from.0, from.1)])
                } else if phase == 1 {
                    Command::Inputs(vec![Button {
                        button: *button,
                        pressed: true,
                    }])
                } else if phase < 2 + DRAG_STEPS {
                    let t = (phase - 1) as f32 / DRAG_STEPS as f32;
                    Command::Inputs(vec![MoveTo(
                        from.0 + (to.0 - from.0) * t,
                        from.1 + (to.1 - from.1) * t,
                    )])
                } else {
                    self.next(SETTLE_FRAMES);
                    Command::Inputs(vec![Button {
                        button: *button,
                        pressed: false,
                    }])
                }
            }
            Step::Scroll { at, lines } => {
                if self.phase == 0 {
                    self.phase = 1;
                    Command::Inputs(vec![MoveTo(at.0, at.1)])
                } else {
                    self.next(SETTLE_FRAMES);
                    Command::Inputs(vec![Scroll(*lines)])
                }
            }
            Step::Key(chord) | Step::Hold(chord, _) => {
                let held = match step {
                    Step::Hold(_, frames) => *frames,
                    _ => 0,
                };
                let phase = self.phase;
                self.phase += 1;
                match phase {
                    // 修飾キーを先に押しておく (Ctrl を押しながら Z、と同じ順)
                    0 => Command::Inputs(modifiers(chord, true)),
                    1 => Command::Inputs(vec![Key {
                        key: chord.key,
                        pressed: true,
                    }]),
                    // hold は押したまま待つ
                    p if p < 2 + held => Command::Idle,
                    _ => {
                        let mut inputs = vec![Key {
                            key: chord.key,
                            pressed: false,
                        }];
                        inputs.extend(modifiers(chord, false));
                        self.next(SETTLE_FRAMES);
                        Command::Inputs(inputs)
                    }
                }
            }
            Step::Type(text) => {
                self.next(SETTLE_FRAMES);
                Command::Inputs(vec![Text(text.clone())])
            }
            Step::Expect(label) => {
                if widget::exists(obs.widgets, label) {
                    self.next(0);
                } else {
                    self.wait_or_fail(FIND_TIMEOUT, || format!("「{label}」が画面にありません"))?;
                }
                Command::Idle
            }
            Step::ExpectNot(label) => {
                if widget::exists(obs.widgets, label) {
                    self.wait_or_fail(FIND_TIMEOUT, || format!("「{label}」が画面に残っています"))?;
                } else {
                    self.next(0);
                }
                Command::Idle
            }
            Step::Mask(rect) => {
                self.masks.push(*rect);
                self.next(0);
                Command::Idle
            }
            Step::ClearMasks => {
                self.masks.clear();
                self.next(0);
                Command::Idle
            }
            Step::Screenshot(name) => {
                if self.phase == 0 {
                    self.phase = 1;
                    self.counter = 0;
                    Command::Capture {
                        name: name.clone(),
                        masks: self.masks.clone(),
                    }
                } else {
                    if !obs.capturing {
                        self.next(0);
                    } else {
                        self.wait_or_fail(FIND_TIMEOUT, || "画面の写真が届きません".to_string())?;
                    }
                    Command::Idle
                }
            }
            Step::DumpWidgets(name) => {
                self.next(0);
                Command::DumpWidgets {
                    name: name.clone(),
                    text: widget::dump(obs.widgets),
                }
            }
        };
        Ok(command)
    }

    /// 動かす → (押す → 離す) × `presses` → `settle` フレーム待つ
    fn click_phases(&mut self, presses: u32, button: MouseButton, settle: u32) -> Command {
        let phase = self.phase;
        self.phase += 1;
        let (x, y) = self.target;
        if phase == 0 {
            if presses == 0 {
                self.next(settle);
            }
            return Command::Inputs(vec![Input::MoveTo(x, y)]);
        }
        let pressed = phase % 2 == 1;
        if !pressed && phase / 2 >= presses {
            self.next(settle);
        }
        Command::Inputs(vec![Input::Button { button, pressed }])
    }

    /// 待つフレームを数え、上限を超えたら失敗にする
    fn wait_or_fail(&mut self, limit: u32, message: impl FnOnce() -> String) -> Result<(), String> {
        self.counter += 1;
        if self.counter > limit {
            Err(message())
        } else {
            Ok(())
        }
    }
}

fn modifiers(chord: &KeyChord, pressed: bool) -> Vec<Input> {
    [
        (chord.ctrl, Key::Ctrl),
        (chord.shift, Key::Shift),
        (chord.alt, Key::Alt),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, key)| Input::Key { key, pressed })
    .collect()
}

fn missing_message(label: &str, nth: usize, similar: &[String]) -> String {
    let which = if nth > 1 {
        format!("{nth} 番目の「{label}」")
    } else {
        format!("「{label}」")
    };
    if similar.is_empty() {
        format!("{which}が画面に見つかりません")
    } else {
        format!(
            "{which}が画面に見つかりません (似た表示名: {})",
            similar
                .iter()
                .map(|s| format!("「{s}」"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_check::scenario::parse;

    fn button(label: &str, x: f32, y: f32) -> Widget {
        Widget {
            label: label.into(),
            role: "Button".into(),
            rect: Rect::new(x, y, 20.0, 10.0),
        }
    }

    /// 終わるまで回して、Idle 以外の命令を集める
    fn run_all(runner: &mut Runner, widgets: &[Widget], busy_frames: u32) -> Vec<Command> {
        let mut out = Vec::new();
        for frame in 0..20_000 {
            let obs = Observation {
                busy: frame < busy_frames,
                capturing: false,
                widgets,
            };
            match runner.tick(&obs) {
                Command::Idle => {}
                Command::Finished => return out,
                other => out.push(other),
            }
        }
        panic!("終わらない");
    }

    #[test]
    fn 押したままの途中では止められない() {
        let widgets = vec![button("保存", 100.0, 50.0)];
        let mut runner = Runner::new(parse("click \"保存\"\nwait_loaded").unwrap());
        let obs = Observation {
            busy: false,
            capturing: false,
            widgets: &widgets,
        };
        assert!(runner.interruptible());
        // 場所が止まっているのを確かめる 2 フレームと、動かす・押す
        let mut seen_pressed = false;
        for _ in 0..40 {
            let command = runner.tick(&obs);
            if let Command::Inputs(inputs) = &command
                && inputs.contains(&Input::Button {
                    button: MouseButton::Left,
                    pressed: true,
                })
            {
                seen_pressed = true;
                assert!(!runner.interruptible(), "押したまま止めてはいけない");
            }
            if command == Command::Finished {
                break;
            }
        }
        assert!(seen_pressed);
        // 離したあと (wait_loaded の途中・終わり) は止められる
        assert!(runner.interruptible());
    }

    #[test]
    fn クリックは動かす_押す_離すの順() {
        let widgets = vec![button("保存", 100.0, 50.0)];
        let mut runner = Runner::new(parse("click \"保存\"\nscreenshot a").unwrap());
        let commands = run_all(&mut runner, &widgets, 0);
        assert_eq!(
            commands,
            vec![
                Command::Inputs(vec![Input::MoveTo(110.0, 55.0)]),
                Command::Inputs(vec![Input::Button {
                    button: MouseButton::Left,
                    pressed: true
                }]),
                Command::Inputs(vec![Input::Button {
                    button: MouseButton::Left,
                    pressed: false
                }]),
                Command::Capture {
                    name: "a".into(),
                    masks: vec![]
                },
            ]
        );
        assert_eq!(runner.failure(), None);
    }

    #[test]
    fn 動いている部品は止まるまでクリックしない() {
        let mut runner = Runner::new(parse("click \"保存\"").unwrap());
        let mut first_move = None;
        for frame in 0..20 {
            // 5 フレーム目までは毎フレーム右へ動く
            let x = (frame.min(5) * 10) as f32;
            let widgets = vec![button("保存", x, 0.0)];
            let obs = Observation {
                busy: false,
                capturing: false,
                widgets: &widgets,
            };
            if let Command::Inputs(inputs) = runner.tick(&obs)
                && let Input::MoveTo(x, _) = inputs[0]
            {
                first_move = Some(x);
                break;
            }
        }
        assert_eq!(first_move, Some(60.0));
    }

    #[test]
    fn ダブルクリックは2回押す() {
        let widgets = vec![button("部品", 0.0, 0.0)];
        let mut runner = Runner::new(parse("double_click \"部品\"").unwrap());
        let commands = run_all(&mut runner, &widgets, 0);
        let presses = commands
            .iter()
            .filter(|c| {
                matches!(c, Command::Inputs(i) if matches!(i[0], Input::Button { pressed: true, .. }))
            })
            .count();
        assert_eq!(presses, 2);
        assert_eq!(commands.len(), 5);
    }

    #[test]
    fn キーは修飾キーから押して逆に離す() {
        let mut runner = Runner::new(parse("key ctrl+z").unwrap());
        let commands = run_all(&mut runner, &[], 0);
        let k = |key, pressed| Input::Key { key, pressed };
        assert_eq!(
            commands,
            vec![
                Command::Inputs(vec![k(Key::Ctrl, true)]),
                Command::Inputs(vec![k(Key::Letter('Z'), true)]),
                Command::Inputs(vec![k(Key::Letter('Z'), false), k(Key::Ctrl, false)]),
            ]
        );
    }

    #[test]
    fn holdは押したままフレーム数だけ待ってから離す() {
        let mut runner = Runner::new(parse("hold w 30").unwrap());
        let k = |key, pressed| Input::Key { key, pressed };
        let mut frames = Vec::new();
        loop {
            let obs = Observation {
                busy: false,
                capturing: false,
                widgets: &[],
            };
            match runner.tick(&obs) {
                Command::Finished => break,
                c => frames.push(c),
            }
        }
        let down = frames
            .iter()
            .position(|c| *c == Command::Inputs(vec![k(Key::Letter('W'), true)]))
            .unwrap();
        let up = frames
            .iter()
            .position(|c| *c == Command::Inputs(vec![k(Key::Letter('W'), false)]))
            .unwrap();
        assert_eq!(up - down, 31, "押してから 30 フレーム待って離す");
    }

    #[test]
    fn 読み込みが終わってから次へ進む() {
        let mut runner = Runner::new(parse("wait_loaded\nscreenshot a").unwrap());
        let mut frame_of_capture = None;
        for frame in 0..1000 {
            let obs = Observation {
                busy: frame < 50,
                capturing: false,
                widgets: &[],
            };
            if let Command::Capture { .. } = runner.tick(&obs) {
                frame_of_capture = Some(frame);
                break;
            }
        }
        assert!(frame_of_capture.unwrap() >= 50 + IDLE_FRAMES);
    }

    #[test]
    fn 計算中は待ち時間も数えない() {
        let mut runner = Runner::new(parse("wait 3\nkey esc").unwrap());
        let obs = |busy| Observation {
            busy,
            capturing: false,
            widgets: &[],
        };
        assert_eq!(runner.tick(&obs(false)), Command::Idle); // wait 3 を始める
        assert_eq!(runner.tick(&obs(false)), Command::Idle); // 残り 2
        for _ in 0..50 {
            assert_eq!(runner.tick(&obs(true)), Command::Idle);
        }
        assert_eq!(runner.tick(&obs(false)), Command::Idle); // 残り 1
        assert_eq!(runner.tick(&obs(false)), Command::Idle); // 残り 0
        assert!(matches!(runner.tick(&obs(false)), Command::Inputs(_)));
    }

    #[test]
    fn 計算が終わらなければ失敗する() {
        let mut runner = Runner::new(parse("wait 1\nwait_loaded").unwrap());
        let obs = Observation {
            busy: true,
            capturing: false,
            widgets: &[],
        };
        while runner.tick(&obs) != Command::Finished {}
        assert!(runner.failure().unwrap().message.contains("終わりません"));
    }

    #[test]
    fn 計算中は撮らずに待つ() {
        let mut runner = Runner::new(parse("screenshot a").unwrap());
        let obs = |busy| Observation {
            busy,
            capturing: false,
            widgets: &[],
        };
        for _ in 0..5 {
            assert_eq!(runner.tick(&obs(true)), Command::Idle);
        }
        assert!(matches!(runner.tick(&obs(false)), Command::Capture { .. }));
    }

    #[test]
    fn 写真が届くまで次へ進まない() {
        let mut runner = Runner::new(parse("screenshot a\nkey esc").unwrap());
        let obs = |capturing| Observation {
            busy: false,
            capturing,
            widgets: &[],
        };
        assert!(matches!(runner.tick(&obs(false)), Command::Capture { .. }));
        for _ in 0..10 {
            assert_eq!(runner.tick(&obs(true)), Command::Idle);
        }
        runner.tick(&obs(false));
        assert!(matches!(runner.tick(&obs(false)), Command::Inputs(_)));
    }

    #[test]
    fn 見つからなければ行番号と似た名前で失敗する() {
        let widgets = vec![button("保存  (Ctrl+S)", 0.0, 0.0)];
        let mut runner = Runner::new(parse("wait 1\n\nclick \"保存する\"").unwrap());
        run_all(&mut runner, &widgets, 0);
        let failure = runner.failure().unwrap();
        assert_eq!(failure.line, 3);
        assert!(
            failure.message.contains("保存  (Ctrl+S)"),
            "{}",
            failure.message
        );
    }

    #[test]
    fn マスクは撮るときに渡す() {
        let mut runner =
            Runner::new(parse("mask 1 2 3 4\nscreenshot a\nclear_masks\nscreenshot b").unwrap());
        let commands = run_all(&mut runner, &[], 0);
        assert_eq!(
            commands,
            vec![
                Command::Capture {
                    name: "a".into(),
                    masks: vec![Rect::new(1.0, 2.0, 3.0, 4.0)]
                },
                Command::Capture {
                    name: "b".into(),
                    masks: vec![]
                },
            ]
        );
    }

    #[test]
    fn expect_not_は消えるまで待つ() {
        let widgets = vec![button("読み込み中", 0.0, 0.0)];
        let mut runner = Runner::new(parse("expect_not \"読み込み中\"").unwrap());
        run_all(&mut runner, &widgets, 0);
        assert!(runner.failure().unwrap().message.contains("残っています"));
    }
}
