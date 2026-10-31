//! タイミングチャートから三菱 PLC (MELSEC Q / iQ-R 系) 向けのステップラダーを作る
//!
//! 生成 AI は使わず、チャートのステップを決まった型のラダーに置き換える。
//!
//! - ステップごとに「ステップ中」の M を1つ持ち、完了したら次のステップの M を SET して自分を RST する
//!   (SET / RST によるステップ送り)。
//! - インターロック (開始条件) が成立したら「条件成立」の M が ON になり、そこから動作の出力を出す。
//! - 動作の完了 (シリンダの出端・戻端、軸の位置決め完了、ロボットの完了) とタイマー (dwell) で「完了」の M を ON にする。
//! - 条件が 5 秒 (`INTERLOCK_TIMEOUT`) 成立しなければ異常 (M2) にして、異常ランプを点ける。
//!
//! | デバイス | 内容 |
//! | --- | --- |
//! | X0 / X1 / X2 | 起動 / 停止 / 異常リセット の押しボタン |
//! | X3〜 | シリンダの出端・戻端、軸の位置決め完了、ロボットの完了、センサー |
//! | Y20〜 | シリンダの電磁弁 (シングルソレノイド: ON で出)、軸・ロボットの起動、ランプ |
//! | D100〜 / D200〜 | 軸の目標位置 (0.1 単位) / ロボットの教示点番号 |
//! | M0 / M1 / M2 | 運転中 / サイクル中 / 異常 |
//! | M100〜 / M200〜 / M300〜 | ステップ中 / 条件成立 / 完了 (ステップが 100 を超えたら 1000 番台などに広げる) |
//! | T0〜 / T100〜 | ステップのタイマー / 条件待ちの異常検出 (100 ms タイマー) |
//!
//! Bevy・egui・ファイル形式のライブラリに依存しない。

use crate::motion::chart::INTERLOCK_TIMEOUT;
use crate::motion::{ActuatorKind, TimingChart};

/// I/O 表の向き
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
    Internal,
}

impl Direction {
    pub fn label(self) -> &'static str {
        match self {
            Direction::Input => "入力",
            Direction::Output => "出力",
            Direction::Internal => "内部",
        }
    }
}

/// I/O 表の1行 (デバイスコメントにも使う)
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceRow {
    pub device: String,
    pub name: String,
    pub direction: Direction,
    pub note: String,
}

/// 命令1つ ("LD X0"、"OUT T0 K10" など)
#[derive(Clone, Debug, PartialEq)]
pub struct Instruction {
    pub op: &'static str,
    pub operands: Vec<String>,
}

/// 回路のまとまり。先頭に行間ステートメント (説明) を付ける。
#[derive(Clone, Debug, PartialEq)]
pub struct Rung {
    pub statement: String,
    pub instructions: Vec<Instruction>,
}

/// 作ったラダー一式
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Ladder {
    pub devices: Vec<DeviceRow>,
    pub rungs: Vec<Rung>,
}

impl Ladder {
    /// デバイスのコメント (I/O 表の信号名)
    pub fn comment(&self, device: &str) -> Option<&str> {
        self.devices
            .iter()
            .find(|d| d.device == device)
            .map(|d| d.name.as_str())
    }

    /// 命令の数 (END を含む)
    pub fn instruction_count(&self) -> usize {
        self.rungs.iter().map(|r| r.instructions.len()).sum()
    }
}

/// 回路を組み立てる道具
struct RungBuilder {
    statement: String,
    instructions: Vec<Instruction>,
}

impl RungBuilder {
    fn new(statement: impl Into<String>) -> Self {
        Self {
            statement: statement.into(),
            instructions: Vec::new(),
        }
    }

    fn op(mut self, op: &'static str, operands: &[&str]) -> Self {
        self.instructions.push(Instruction {
            op,
            operands: operands.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    fn build(self) -> Rung {
        Rung {
            statement: self.statement,
            instructions: self.instructions,
        }
    }
}

/// 16 進のビットデバイス番号 ("X1A" など)
fn bit(prefix: char, n: u32) -> String {
    format!("{prefix}{n:X}")
}

/// デバイスの割り付け
struct Plan {
    devices: Vec<DeviceRow>,
    next_x: u32,
    next_y: u32,
}

impl Plan {
    fn input(&mut self, name: impl Into<String>, note: &str) -> String {
        let device = bit('X', self.next_x);
        self.next_x += 1;
        self.push(&device, name, Direction::Input, note);
        device
    }

    fn output(&mut self, name: impl Into<String>, note: &str) -> String {
        let device = bit('Y', self.next_y);
        self.next_y += 1;
        self.push(&device, name, Direction::Output, note);
        device
    }

    fn push(&mut self, device: &str, name: impl Into<String>, direction: Direction, note: &str) {
        self.devices.push(DeviceRow {
            device: device.to_string(),
            name: name.into(),
            direction,
            note: note.to_string(),
        });
    }
}

/// 1つの動作の出力と完了の確認
struct ActionIo {
    /// 動作の出力 (SET / RST / MOV の命令)
    start: Vec<(&'static str, Vec<String>)>,
    /// 完了の接点
    done: String,
    /// ステップを抜けるときに RST する起動信号
    reset: Option<String>,
}

/// タイミングチャートからラダーを作る
pub fn generate(chart: &TimingChart) -> Ladder {
    let n = chart.steps.len() as u32;
    // ステップの M・タイマーの番号の幅。100 ステップを超えたら広げる。
    let block = n.div_ceil(100).max(1) * 100;
    let step_m = |i: u32| format!("M{}", block + i);
    let ok_m = |i: u32| format!("M{}", 2 * block + i);
    let done_m = |i: u32| format!("M{}", 3 * block + i);

    // 入力の数を先に数え、出力は入力と重ならない 16 の倍数 (最低 Y20) から振る
    let cylinders: Vec<_> = chart
        .actuators
        .iter()
        .filter(|a| a.kind == ActuatorKind::Cylinder)
        .collect();
    let axes: Vec<_> = chart
        .actuators
        .iter()
        .filter(|a| a.kind != ActuatorKind::Cylinder)
        .collect();
    let inputs = 3 + 2 * cylinders.len() + axes.len() + chart.robots.len() + chart.sensors.len();
    let mut plan = Plan {
        devices: Vec::new(),
        next_x: 0,
        next_y: (inputs as u32).div_ceil(16).max(2) * 16,
    };

    let start_pb = plan.input("起動", "押しボタン");
    let stop_pb = plan.input(
        "停止",
        "押しボタン (b 接点にするときは ANI / LDI を入れ替える)",
    );
    let reset_pb = plan.input("異常リセット", "押しボタン");

    struct CylIo {
        id: u32,
        sol: String,
        extended: String,
        retracted: String,
    }
    struct AxisIo {
        id: u32,
        start: String,
        in_position: String,
        target: String,
    }
    struct RobotIo {
        id: u32,
        start: String,
        done: String,
        point: String,
    }
    let mut cyl_io: Vec<CylIo> = cylinders
        .iter()
        .map(|a| CylIo {
            id: a.id,
            sol: String::new(),
            extended: plan.input(format!("{}.出端", a.name), "オートスイッチ"),
            retracted: plan.input(format!("{}.戻端", a.name), "オートスイッチ"),
        })
        .collect();
    let mut axis_io: Vec<AxisIo> = axes
        .iter()
        .enumerate()
        .map(|(k, a)| AxisIo {
            id: a.id,
            start: String::new(),
            in_position: plan.input(
                format!("{}.位置決め完了", a.name),
                "起動で OFF、目標位置に着いたら ON になる前提",
            ),
            target: format!("D{}", 100 + k),
        })
        .collect();
    let mut robot_io: Vec<RobotIo> = chart
        .robots
        .iter()
        .enumerate()
        .map(|(k, r)| RobotIo {
            id: r.id,
            start: String::new(),
            done: plan.input(format!("{}.完了", r.name), "教示点への移動が終わったら ON"),
            point: format!("D{}", 200 + k),
        })
        .collect();
    let sensor_x: Vec<(u32, String)> = chart
        .sensors
        .iter()
        .map(|s| (s.id, plan.input(s.name.clone(), s.kind.label())))
        .collect();

    for (io, a) in cyl_io.iter_mut().zip(&cylinders) {
        io.sol = plan.output(format!("{}.出", a.name), "電磁弁 (ON で出、OFF で戻)");
    }
    for (io, a) in axis_io.iter_mut().zip(&axes) {
        io.start = plan.output(format!("{}.起動", a.name), "位置決め起動");
    }
    for (io, r) in robot_io.iter_mut().zip(&chart.robots) {
        io.start = plan.output(format!("{}.起動", r.name), "教示点への移動を始める");
    }
    let alarm_lamp = plan.output("異常ランプ", "");
    let run_lamp = plan.output("運転中ランプ", "");

    for (io, a) in axis_io.iter().zip(&axes) {
        let unit = if a.kind == ActuatorKind::RotaryTable {
            "0.1°"
        } else {
            "0.1 単位"
        };
        plan.push(
            &io.target,
            format!("{}.目標位置", a.name),
            Direction::Internal,
            unit,
        );
    }
    for (io, r) in robot_io.iter().zip(&chart.robots) {
        plan.push(
            &io.point,
            format!("{}.教示点番号", r.name),
            Direction::Internal,
            "1 始まり",
        );
    }
    plan.push("M0", "運転中", Direction::Internal, "");
    plan.push("M1", "サイクル中", Direction::Internal, "");
    plan.push("M2", "異常", Direction::Internal, "条件待ちのタイムアウト");
    for (i, step) in chart.steps.iter().enumerate() {
        let i = i as u32;
        let title = step_title(i, &step.comment);
        plan.push(
            &step_m(i),
            format!("{title} ステップ中"),
            Direction::Internal,
            "",
        );
        plan.push(
            &ok_m(i),
            format!("{title} 条件成立"),
            Direction::Internal,
            "",
        );
        plan.push(&done_m(i), format!("{title} 完了"), Direction::Internal, "");
    }

    let mut rungs = vec![
        RungBuilder::new("運転: 起動で ON、停止・異常で OFF")
            .op("LD", &[&start_pb])
            .op("OR", &["M0"])
            .op("ANI", &[&stop_pb])
            .op("ANI", &["M2"])
            .op("OUT", &["M0"])
            .build(),
    ];

    if n > 0 {
        let mut cycle = RungBuilder::new("サイクル中: どれかのステップが動いている");
        for i in 0..n {
            cycle = cycle.op(if i == 0 { "LD" } else { "OR" }, &[&step_m(i)]);
        }
        rungs.push(cycle.op("OUT", &["M1"]).build());
        rungs.push(
            RungBuilder::new("起動で最初のステップへ")
                .op("LDP", &[&start_pb])
                .op("ANI", &["M1"])
                .op("ANI", &["M2"])
                .op("SET", &[&step_m(0)])
                .build(),
        );
        rungs.push(
            RungBuilder::new("停止で全ステップを解除")
                .op("LD", &[&stop_pb])
                .op("ZRST", &[&step_m(0), &step_m(n - 1)])
                .build(),
        );
    }
    rungs.push(
        RungBuilder::new("異常リセット")
            .op("LD", &[&reset_pb])
            .op("RST", &["M2"])
            .build(),
    );
    rungs.push(
        RungBuilder::new("ランプ")
            .op("LD", &["M2"])
            .op("OUT", &[&alarm_lamp])
            .op("LD", &["M0"])
            .op("OUT", &[&run_lamp])
            .build(),
    );

    for (index, step) in chart.steps.iter().enumerate() {
        let i = index as u32;
        let title = step_title(i, &step.comment);

        // 条件
        let mut cond = RungBuilder::new(format!("{title}: 開始条件")).op("LD", &[&step_m(i)]);
        for interlock in &step.interlocks {
            if let Some((_, x)) = sensor_x.iter().find(|(id, _)| *id == interlock.sensor) {
                cond = cond.op(if interlock.on { "AND" } else { "ANI" }, &[x]);
            }
        }
        rungs.push(cond.op("OUT", &[&ok_m(i)]).build());

        // 動作
        let mut actions: Vec<ActionIo> = Vec::new();
        for action in &step.actions {
            if let Some(io) = cyl_io.iter().find(|c| c.id == action.actuator) {
                let extend = action.target >= 0.5;
                actions.push(ActionIo {
                    start: vec![(if extend { "SET" } else { "RST" }, vec![io.sol.clone()])],
                    done: if extend {
                        io.extended.clone()
                    } else {
                        io.retracted.clone()
                    },
                    reset: None,
                });
            } else if let Some(io) = axis_io.iter().find(|a| a.id == action.actuator) {
                let counts = (action.target * 10.0).round() as i64;
                actions.push(ActionIo {
                    start: vec![
                        ("MOV", vec![format!("K{counts}"), io.target.clone()]),
                        ("SET", vec![io.start.clone()]),
                    ],
                    done: io.in_position.clone(),
                    reset: Some(io.start.clone()),
                });
            }
        }
        for m in &step.moves {
            if let Some(io) = robot_io.iter().find(|r| r.id == m.robot) {
                let number = chart
                    .robot(m.robot)
                    .and_then(|r| r.points.iter().position(|p| p.id == m.point))
                    .map_or(0, |k| k + 1);
                actions.push(ActionIo {
                    start: vec![
                        ("MOV", vec![format!("K{number}"), io.point.clone()]),
                        ("SET", vec![io.start.clone()]),
                    ],
                    done: io.done.clone(),
                    reset: Some(io.start.clone()),
                });
            }
        }
        if !actions.is_empty() {
            let mut out = RungBuilder::new(format!("{title}: 動作")).op("LD", &[&ok_m(i)]);
            for a in &actions {
                for (op, operands) in &a.start {
                    let refs: Vec<&str> = operands.iter().map(String::as_str).collect();
                    out = out.op(op, &refs);
                }
            }
            rungs.push(out.build());
        }

        // 完了 (+ タイマー)
        let mut done = RungBuilder::new(format!("{title}: 完了の確認")).op("LD", &[&ok_m(i)]);
        for a in &actions {
            done = done.op("AND", &[&a.done]);
        }
        let dwell = (step.dwell * 10.0).round() as i64;
        if dwell > 0 {
            let timer = format!("T{i}");
            rungs.push(done.op("OUT", &[&timer, &format!("K{dwell}")]).build());
            rungs.push(
                RungBuilder::new(format!("{title}: タイマー {} 秒", step.dwell))
                    .op("LD", &[&timer])
                    .op("OUT", &[&done_m(i)])
                    .build(),
            );
        } else {
            rungs.push(done.op("OUT", &[&done_m(i)]).build());
        }

        // 条件待ちの異常
        if !step.interlocks.is_empty() {
            let timer = format!("T{}", block + i);
            let k = format!("K{}", (INTERLOCK_TIMEOUT * 10.0).round() as i64);
            rungs.push(
                RungBuilder::new(format!(
                    "{title}: 条件が {INTERLOCK_TIMEOUT} 秒成立しなければ異常"
                ))
                .op("LD", &[&step_m(i)])
                .op("ANI", &[&ok_m(i)])
                .op("OUT", &[&timer, &k])
                .op("LD", &[&timer])
                .op("SET", &["M2"])
                .build(),
            );
        }

        // 次のステップへ
        let mut next = RungBuilder::new(format!("{title}: 次のステップへ"))
            .op("LD", &[&done_m(i)])
            .op("RST", &[&step_m(i)]);
        for a in &actions {
            if let Some(reset) = &a.reset {
                next = next.op("RST", &[reset]);
            }
        }
        if i + 1 < n {
            next = next.op("SET", &[&step_m(i + 1)]);
            rungs.push(next.build());
        } else {
            rungs.push(next.build());
            rungs.push(
                RungBuilder::new("運転中なら最初のステップに戻って繰り返す")
                    .op("LD", &[&done_m(i)])
                    .op("AND", &["M0"])
                    .op("SET", &[&step_m(0)])
                    .build(),
            );
        }
    }
    rungs.push(RungBuilder::new("").op("END", &[]).build());

    Ladder {
        devices: plan.devices,
        rungs,
    }
}

fn step_title(i: u32, comment: &str) -> String {
    if comment.trim().is_empty() {
        format!("S{}", i + 1)
    } else {
        format!("S{} {}", i + 1, comment.trim())
    }
}

/// 人が読む命令リスト (デバイスのコメント付き)
pub fn to_text(ladder: &Ladder) -> String {
    let mut text = String::from(
        "; cadsim がタイミングチャートから作ったラダー (三菱 MELSEC Q / iQ-R 向けの命令リスト)\n\
         ; デバイスの割り付けは I/O 表を見てください。実機に入れる前に必ず内容を確認してください。\n",
    );
    for rung in &ladder.rungs {
        if !rung.statement.is_empty() {
            text += &format!("\n; ---- {} ----\n", rung.statement);
        }
        for ins in &rung.instructions {
            let operands = ins.operands.join(" ");
            let comments: Vec<&str> = ins
                .operands
                .iter()
                .filter_map(|o| ladder.comment(o))
                .collect();
            let line = format!("{:<5} {operands}", ins.op);
            if comments.is_empty() {
                text += line.trim_end();
            } else {
                text += &format!("{line:<24} ; {}", comments.join(" / "));
            }
            text += "\n";
        }
    }
    text
}

fn quote(field: &str) -> String {
    format!("\"{}\"", field.replace('"', "\"\""))
}

/// GX Works の命令リストの CSV に合わせた形
///
/// 列は「ステップ番号, 行間ステートメント, 命令, I/O(デバイス), ブランク, PIステートメント, ノート」。
/// オペランドが 2 つ以上ある命令は、2 つ目から次の行の「I/O(デバイス)」列に書く。
/// ステップ番号は命令の通し番号 (読み込むと GX Works 側で振り直される前提)。
pub fn to_gx_csv(ladder: &Ladder) -> String {
    let mut text = String::new();
    text += "\"MAIN\"\r\n\"命令リスト\"\r\n";
    text += &[
        "ステップ番号",
        "行間ステートメント",
        "命令",
        "I/O(デバイス)",
        "ブランク",
        "PIステートメント",
        "ノート",
    ]
    .map(quote)
    .join(",");
    text += "\r\n";
    let mut step = 0usize;
    for rung in &ladder.rungs {
        let mut statement = rung.statement.as_str();
        for ins in &rung.instructions {
            let first = ins.operands.first().map_or("", String::as_str);
            let row = [
                step.to_string(),
                statement.to_string(),
                ins.op.to_string(),
                first.to_string(),
                String::new(),
                String::new(),
                String::new(),
            ];
            text += &row.iter().map(|f| quote(f)).collect::<Vec<_>>().join(",");
            text += "\r\n";
            statement = "";
            for extra in ins.operands.iter().skip(1) {
                let row = ["", "", "", extra.as_str(), "", "", ""];
                text += &row.map(quote).join(",");
                text += "\r\n";
            }
            step += 1;
        }
    }
    text
}

/// I/O 表の CSV (デバイス, 信号名, 向き, 備考)。Excel で開けるよう BOM 付き UTF-8。
/// 先頭の 2 列はそのまま GX Works のデバイスコメントに貼れる。
pub fn io_csv(ladder: &Ladder) -> String {
    let mut text = String::from('\u{feff}');
    text += "デバイス,信号名,向き,備考\r\n";
    for d in &ladder.devices {
        text += &[
            quote(&d.device),
            quote(&d.name),
            quote(d.direction.label()),
            quote(&d.note),
        ]
        .join(",");
        text += "\r\n";
    }
    text
}

/// I/O 表をタブ区切りの文字にする (クリップボードから Excel やデバイスコメントに貼る用)
pub fn io_tsv(ladder: &Ladder) -> String {
    ladder
        .devices
        .iter()
        .map(|d| {
            format!(
                "{}\t{}\t{}\t{}\n",
                d.device,
                d.name,
                d.direction.label(),
                d.note
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequence_gen::spec::{ApplyMode, apply, tests::sample_spec};

    fn sample_chart() -> TimingChart {
        let mut chart = TimingChart::default();
        apply(&sample_spec(), &mut chart, ApplyMode::Replace);
        chart
    }

    fn ops(ladder: &Ladder) -> Vec<String> {
        ladder
            .rungs
            .iter()
            .flat_map(|r| &r.instructions)
            .map(|i| {
                format!("{} {}", i.op, i.operands.join(" "))
                    .trim()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn assigns_inputs_outputs_and_internal_devices() {
        let ladder = generate(&sample_chart());
        let find = |name: &str| {
            ladder
                .devices
                .iter()
                .find(|d| d.name == name)
                .map(|d| d.device.clone())
        };
        assert_eq!(find("起動").as_deref(), Some("X0"));
        assert_eq!(find("リフタ.出端").as_deref(), Some("X3"));
        assert_eq!(find("リフタ.戻端").as_deref(), Some("X4"));
        assert_eq!(find("X軸.位置決め完了").as_deref(), Some("X5"));
        assert_eq!(find("ワーク有無").as_deref(), Some("X6"));
        assert_eq!(find("リフタ.出").as_deref(), Some("Y20"));
        assert_eq!(find("X軸.起動").as_deref(), Some("Y21"));
        assert_eq!(find("X軸.目標位置").as_deref(), Some("D100"));
        assert!(find("S1 ワーク到着待ちで上昇 ステップ中").is_some_and(|d| d == "M100"));
    }

    #[test]
    fn steps_become_set_reset_ladder_with_interlocks_and_timers() {
        let ladder = generate(&sample_chart());
        let ops = ops(&ladder);
        let has = |s: &str| ops.iter().any(|o| o == s);
        // S1: ワーク有無 (X6) が ON で条件成立 → リフタ出 → 出端で完了 → 0.2 秒のタイマー
        assert!(has("AND X6"));
        assert!(has("SET Y20"));
        assert!(has("AND X3"));
        assert!(has("OUT T0 K2"));
        assert!(has("OUT T100 K50"), "条件待ちの異常検出");
        assert!(has("SET M101"), "S1 完了で S2 へ");
        // S2: X軸を 300 へ (0.1 単位で 3000)
        assert!(has("MOV K3000 D100"));
        assert!(has("SET Y21"));
        assert!(has("RST Y21"), "抜けるときに起動を切る");
        assert!(has("ZRST M100 M101"));
        assert_eq!(ops.last().map(String::as_str), Some("END"));
    }

    #[test]
    fn many_steps_widen_device_blocks() {
        let chart = TimingChart(cadsim_core::motion::TimingChart {
            steps: vec![Default::default(); 150],
            ..Default::default()
        });
        let ladder = generate(&chart);
        assert!(
            ladder
                .comment("M200")
                .is_some_and(|c| c.contains("S1 ステップ中"))
        );
        assert!(
            ladder
                .comment("M600")
                .is_some_and(|c| c.contains("S1 完了"))
        );
    }

    #[test]
    fn empty_chart_still_has_run_circuit() {
        let ladder = generate(&TimingChart::default());
        let ops = ops(&ladder);
        assert!(ops.contains(&"OUT M0".to_string()));
        assert_eq!(ops.last().map(String::as_str), Some("END"));
    }

    #[test]
    fn text_csv_and_io_outputs() {
        let ladder = generate(&sample_chart());
        let text = to_text(&ladder);
        assert!(text.contains("; ---- S1 ワーク到着待ちで上昇: 開始条件 ----"));
        assert!(text.contains("ワーク有無"));
        let csv = to_gx_csv(&ladder);
        assert!(csv.contains("\"0\",\"運転: 起動で ON、停止・異常で OFF\",\"LD\",\"X0\""));
        // 2 つ目のオペランドは次の行
        assert!(csv.contains("\"OUT\",\"T0\"") && csv.contains("\"\",\"\",\"\",\"K2\""));
        let io = io_csv(&ladder);
        assert!(io.starts_with('\u{feff}'));
        assert!(io.contains("\"Y20\",\"リフタ.出\",\"出力\""));
        assert!(io_tsv(&ladder).contains("X0\t起動\t入力"));
    }
}
