//! PLC とやりとりするラインの信号
//!
//! 工程モデルの要素の名前から自動で作る。名前を変えると信号名も変わる。
//!
//! | 向き | 信号 | 中身 |
//! | --- | --- | --- |
//! | 設備 → PLC | `<工程>.稼働` `<工程>.詰まり` `<工程>.故障` `<工程>.在荷` | 工程の状態 (ビット) |
//! | 設備 → PLC | `<バッファ>.数` (ワード) `<バッファ>.満杯` | バッファの中身 |
//! | 設備 → PLC | `ライン.運転中` `ライン.排出数` (ワード) `安全.停止` | ライン全体 |
//! | 設備 → PLC | `AGV.搬送待ち` (ワード) `AGV<n>.搬送中` | AGV |
//! | 設備 → PLC | `コンベア.投入数` `コンベア.排出数` `コンベア.仕掛数` (ワード) | ワーク搬送 (投入口があるときだけ) |
//! | PLC → 設備 | `ライン.運転` | OFF の間はライン全体を止める |
//! | PLC → 設備 | `<生成>.投入許可` | OFF の間はその生成からワークを出さない |
//!
//! 番号は設備 → PLC のビットを [`TO_PLC_BITS`] から、ワードを [`TO_PLC_WORDS`] から、
//! PLC → 設備 のビットを [`FROM_PLC_BITS`] から、この表の順に詰めて振る。
//! タイミングチャートのアクチュエータの信号 (0 番台・100 番台) と重ならないようにしてある。

use std::collections::BTreeMap;

use crate::flow::model::{ElementId, ElementKind, FlowModel};
use crate::flow::sim::{Simulation, State};

/// 設備 → PLC のビットの先頭番号
pub const TO_PLC_BITS: u16 = 200;
/// 設備 → PLC のワードの先頭番号
pub const TO_PLC_WORDS: u16 = 200;
/// PLC → 設備 のビットの先頭番号
pub const FROM_PLC_BITS: u16 = 300;

/// ラインの運転信号の名前
pub const RUN: &str = "ライン.運転";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    /// 設備 → PLC (PLC から見て入力)
    ToPlc,
    /// PLC → 設備 (PLC から見て出力)
    FromPlc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Bit(bool),
    Word(i32),
}

/// 信号表の1行
#[derive(Clone, Debug, PartialEq)]
pub struct Signal {
    pub name: String,
    pub direction: Direction,
    pub value: Value,
    /// コイル番号 (ビット) か保持レジスタ番号 (ワード)
    pub address: u16,
}

/// PLC → 設備 の信号の値。名前がない信号は ON とみなす (PLC をつないでいなくても動くように)。
#[derive(Clone, Debug, PartialEq)]
pub struct LineInputs {
    /// ライン全体の運転
    pub run: bool,
    /// 生成ごとの投入許可。入っていない生成は許可。
    pub feed: BTreeMap<ElementId, bool>,
}

impl Default for LineInputs {
    fn default() -> Self {
        Self {
            run: true,
            feed: BTreeMap::new(),
        }
    }
}

impl LineInputs {
    pub fn feed_enabled(&self, source: ElementId) -> bool {
        self.feed.get(&source).copied().unwrap_or(true)
    }

    /// 信号名で値を書く (PLC からの書き込み・画面での強制)。知らない名前なら false。
    pub fn set(&mut self, model: &FlowModel, name: &str, on: bool) -> bool {
        if name == RUN {
            self.run = on;
            return true;
        }
        let source = model
            .elements
            .iter()
            .find(|e| matches!(e.kind, ElementKind::Source { .. }) && feed_name(&e.name) == name);
        match source {
            Some(e) => {
                self.feed.insert(e.id, on);
                true
            }
            None => false,
        }
    }
}

fn feed_name(element: &str) -> String {
    format!("{element}.投入許可")
}

/// 信号表を作るのに使う、AGV と安全機器の今の状態
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignalSources {
    /// ラインが動いている (再生中で、止められていない)
    pub running: bool,
    pub safety_stopped: bool,
    /// AGV ごとに荷を積んでいるか (台数分)
    pub agv_loaded: Vec<bool>,
    /// AGV を頼んでまだ積んでいない搬送の数
    pub agv_waiting: usize,
    /// コンベアのワークの数。ワーク搬送を使っていなければ None (信号を作らない)。
    pub conveyor: Option<ConveyorCounts>,
}

/// コンベアで流れるワークの数
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConveyorCounts {
    /// 投入口から出した数
    pub fed: u32,
    /// 排出口・排出端のコンベアから出ていった数
    pub discharged: u32,
    /// 投入されてまだ排出されていない数
    pub in_process: usize,
}

fn word_value(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// 今の信号表。`sim` が None (まだ回していない) なら設備の状態はすべて OFF・0。
pub fn line_signals(
    model: &FlowModel,
    sim: Option<&Simulation>,
    sources: &SignalSources,
    inputs: &LineInputs,
) -> Vec<Signal> {
    let mut table = Table::default();
    let state = |id| sim.and_then(|s| s.state_of(id));
    let content = |id| sim.and_then(|s| s.content_of(id)).unwrap_or(0);

    table.bit(Direction::ToPlc, "ライン.運転中", sources.running);
    table.bit(Direction::ToPlc, "安全.停止", sources.safety_stopped);
    table.word(
        "ライン.排出数",
        sim.map_or(0, |s| s.output_total()).min(i32::MAX as u32) as i32,
    );
    for e in &model.elements {
        let name = &e.name;
        match e.kind {
            ElementKind::Process { .. } => {
                let s = state(e.id);
                table.bit(
                    Direction::ToPlc,
                    &format!("{name}.稼働"),
                    s == Some(State::Working),
                );
                table.bit(
                    Direction::ToPlc,
                    &format!("{name}.詰まり"),
                    s == Some(State::Blocked),
                );
                table.bit(
                    Direction::ToPlc,
                    &format!("{name}.故障"),
                    s == Some(State::Down),
                );
                table.bit(Direction::ToPlc, &format!("{name}.在荷"), content(e.id) > 0);
            }
            ElementKind::Buffer { capacity } => {
                let n = content(e.id);
                table.word(&format!("{name}.数"), n as i32);
                table.bit(
                    Direction::ToPlc,
                    &format!("{name}.満杯"),
                    n >= capacity as usize,
                );
            }
            ElementKind::Transport { .. } => {
                table.word(&format!("{name}.搬送中"), content(e.id) as i32);
            }
            ElementKind::Source { .. } | ElementKind::Sink => {}
        }
    }
    table.word("AGV.搬送待ち", sources.agv_waiting as i32);
    for (i, loaded) in sources.agv_loaded.iter().enumerate() {
        table.bit(Direction::ToPlc, &format!("AGV{}.搬送中", i + 1), *loaded);
    }
    // ワードの後ろに足す (ワーク搬送の有無で工程・AGV の番号がずれないように)
    if let Some(c) = sources.conveyor {
        table.word("コンベア.投入数", word_value(c.fed as usize));
        table.word("コンベア.排出数", word_value(c.discharged as usize));
        table.word("コンベア.仕掛数", word_value(c.in_process));
    }

    table.bit(Direction::FromPlc, RUN, inputs.run);
    for e in &model.elements {
        if matches!(e.kind, ElementKind::Source { .. }) {
            table.bit(
                Direction::FromPlc,
                &feed_name(&e.name),
                inputs.feed_enabled(e.id),
            );
        }
    }
    table.rows
}

#[derive(Default)]
struct Table {
    rows: Vec<Signal>,
    to_bits: u16,
    to_words: u16,
    from_bits: u16,
}

impl Table {
    fn bit(&mut self, direction: Direction, name: &str, on: bool) {
        let address = match direction {
            Direction::ToPlc => {
                self.to_bits += 1;
                TO_PLC_BITS + self.to_bits - 1
            }
            Direction::FromPlc => {
                self.from_bits += 1;
                FROM_PLC_BITS + self.from_bits - 1
            }
        };
        self.rows.push(Signal {
            name: name.to_owned(),
            direction,
            value: Value::Bit(on),
            address,
        });
    }

    fn word(&mut self, name: &str, value: i32) {
        self.to_words += 1;
        self.rows.push(Signal {
            name: name.to_owned(),
            direction: Direction::ToPlc,
            value: Value::Word(value),
            address: TO_PLC_WORDS + self.to_words - 1,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::model::sample_line;

    fn find<'a>(rows: &'a [Signal], name: &str) -> &'a Signal {
        rows.iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} がない"))
    }

    #[test]
    fn signals_follow_element_names_and_states() {
        let model = sample_line();
        let mut sim = Simulation::new(&model).unwrap();
        sim.advance(1.0, usize::MAX);
        let sources = SignalSources {
            running: true,
            agv_loaded: vec![false, true],
            agv_waiting: 3,
            ..SignalSources::default()
        };
        let rows = line_signals(&model, Some(&sim), &sources, &LineInputs::default());
        let process = model
            .elements
            .iter()
            .find(|e| matches!(e.kind, ElementKind::Process { .. }))
            .unwrap();
        let working = sim.state_of(process.id) == Some(State::Working);
        assert_eq!(
            find(&rows, &format!("{}.稼働", process.name)).value,
            Value::Bit(working)
        );
        assert_eq!(find(&rows, "AGV.搬送待ち").value, Value::Word(3));
        assert_eq!(find(&rows, "AGV2.搬送中").value, Value::Bit(true));
        assert_eq!(find(&rows, RUN).direction, Direction::FromPlc);
        assert_eq!(find(&rows, RUN).address, FROM_PLC_BITS);
        assert_eq!(find(&rows, "ライン.運転中").address, TO_PLC_BITS);
        assert_eq!(find(&rows, "ライン.排出数").address, TO_PLC_WORDS);
    }

    #[test]
    fn conveyor_words_come_last_and_only_when_used() {
        let model = sample_line();
        let inputs = LineInputs::default();
        let without = line_signals(&model, None, &SignalSources::default(), &inputs);
        assert!(without.iter().all(|s| !s.name.starts_with("コンベア")));
        let sources = SignalSources {
            conveyor: Some(ConveyorCounts {
                fed: 7,
                discharged: 4,
                in_process: 3,
            }),
            ..SignalSources::default()
        };
        let with = line_signals(&model, None, &sources, &inputs);
        assert_eq!(find(&with, "コンベア.投入数").value, Value::Word(7));
        assert_eq!(find(&with, "コンベア.排出数").value, Value::Word(4));
        assert_eq!(find(&with, "コンベア.仕掛数").value, Value::Word(3));
        // ほかの信号の番号は変わらない
        for s in &without {
            assert_eq!(find(&with, &s.name).address, s.address);
        }
        let last_word = without
            .iter()
            .filter(|s| matches!(s.value, Value::Word(_)))
            .map(|s| s.address)
            .max()
            .unwrap();
        assert_eq!(find(&with, "コンベア.投入数").address, last_word + 1);
    }

    #[test]
    fn addresses_are_unique_per_area() {
        let model = sample_line();
        let rows = line_signals(
            &model,
            None,
            &SignalSources::default(),
            &LineInputs::default(),
        );
        let mut seen = std::collections::HashSet::new();
        for s in &rows {
            let area = (s.direction, matches!(s.value, Value::Word(_)));
            assert!(seen.insert((area, s.address)), "{} の番号が重なる", s.name);
        }
    }

    #[test]
    fn inputs_can_be_written_by_name() {
        let model = sample_line();
        let source = &model.elements[0];
        let mut inputs = LineInputs::default();
        assert!(inputs.feed_enabled(source.id));
        assert!(inputs.set(&model, &format!("{}.投入許可", source.name), false));
        assert!(!inputs.feed_enabled(source.id));
        assert!(inputs.set(&model, RUN, false));
        assert!(!inputs.run);
        assert!(!inputs.set(&model, "そんな信号", true));
    }
}
