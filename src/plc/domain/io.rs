//! PLC と設備のあいだでやりとりする信号 (I/O) の一覧と、その値
//!
//! 信号はタイミングチャートのアクチュエータから自動で作る。
//!
//! | アクチュエータ | PLC → 設備 (出力) | 設備 → PLC (入力) |
//! | --- | --- | --- |
//! | シリンダ | 出 (ON で出、OFF で戻) | 出端、戻端 |
//! | 直動軸・回転テーブル | 指令位置 (ワード) | 現在位置 (ワード)、位置決め完了 |
//!
//! ビットはコイル、ワードは保持レジスタに割り付ける。番号は「出力ビット」「出力ワード」
//! 「入力ビット」「入力ワード」それぞれの先頭番号から、アクチュエータの並び順に詰めて振る。

use std::collections::HashMap;

use crate::motion::{ActuatorKind, TimingChart};

/// 信号の意味。中の数値はアクチュエータの id。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Point {
    /// シリンダの出指令。ON で出、OFF で戻 (シングルソレノイド)
    CylinderExtend(u32),
    /// シリンダ出端のオートスイッチ
    CylinderExtended(u32),
    /// シリンダ戻端のオートスイッチ
    CylinderRetracted(u32),
    /// 直動軸・回転テーブルの指令位置
    AxisTarget(u32),
    /// 直動軸・回転テーブルの現在位置
    AxisPosition(u32),
    /// 現在位置が指令位置に着いている
    AxisInPosition(u32),
}

impl Point {
    /// PLC の出力 (PLC → 設備) なら true
    pub fn is_output(self) -> bool {
        matches!(self, Point::CylinderExtend(_) | Point::AxisTarget(_))
    }

    /// ワード (数値) の信号なら true。それ以外は ON/OFF のビット。
    pub fn is_word(self) -> bool {
        matches!(self, Point::AxisTarget(_) | Point::AxisPosition(_))
    }

    /// 信号名の後ろに付ける言葉
    pub fn label(self) -> &'static str {
        match self {
            Point::CylinderExtend(_) => "出",
            Point::CylinderExtended(_) => "出端",
            Point::CylinderRetracted(_) => "戻端",
            Point::AxisTarget(_) => "指令位置",
            Point::AxisPosition(_) => "現在位置",
            Point::AxisInPosition(_) => "位置決め完了",
        }
    }
}

/// 番号の割り付け方 (各領域の先頭番号)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AddressPlan {
    /// PLC → 設備 のビット (コイル)
    pub output_bits: u16,
    /// PLC → 設備 のワード (保持レジスタ)
    pub output_words: u16,
    /// 設備 → PLC のビット (コイル)
    pub input_bits: u16,
    /// 設備 → PLC のワード (保持レジスタ)
    pub input_words: u16,
}

impl Default for AddressPlan {
    fn default() -> Self {
        Self {
            output_bits: 0,
            output_words: 0,
            input_bits: 100,
            input_words: 100,
        }
    }
}

/// I/O 一覧の1行
#[derive(Clone, Debug, PartialEq)]
pub struct IoPoint {
    pub point: Point,
    /// 表示名 ("リフタ.出端" など)
    pub name: String,
    /// コイル番号 (ビット) または保持レジスタ番号 (ワード)。0 始まり。
    pub address: u16,
    /// ワード1カウントあたりの値。直動は 0.1 (mm なら 0.1 mm)、回転は 0.1°。
    pub scale: f64,
}

impl IoPoint {
    /// "コイル 100 (00101)" のような表示。括弧内は 1 始まりの Modbus 番地。
    pub fn address_label(&self) -> String {
        if self.point.is_word() {
            format!(
                "保持レジスタ {} (4{:04})",
                self.address,
                self.address as u32 + 1
            )
        } else {
            format!("コイル {} ({:05})", self.address, self.address as u32 + 1)
        }
    }

    /// 値をレジスタに入れる 16 ビット整数 (符号付き) にする。範囲外は端に丸める。
    pub fn encode_word(&self, value: f64) -> u16 {
        let count = (value / self.scale)
            .round()
            .clamp(i16::MIN as f64, i16::MAX as f64);
        count as i16 as u16
    }

    pub fn decode_word(&self, raw: u16) -> f64 {
        raw as i16 as f64 * self.scale
    }
}

/// I/O 一覧 (割り付け表)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IoTable {
    pub points: Vec<IoPoint>,
}

impl IoTable {
    /// タイミングチャートのアクチュエータから I/O 一覧を作る
    pub fn from_chart(chart: &TimingChart, plan: &AddressPlan) -> Self {
        let mut next = *plan;
        let mut points = Vec::new();
        for actuator in &chart.actuators {
            let id = actuator.id;
            let (signals, scale) = match actuator.kind {
                ActuatorKind::Cylinder => (
                    vec![
                        Point::CylinderExtend(id),
                        Point::CylinderExtended(id),
                        Point::CylinderRetracted(id),
                    ],
                    1.0,
                ),
                ActuatorKind::LinearAxis => (axis_points(id), 0.1),
                ActuatorKind::RotaryTable => (axis_points(id), 0.1),
            };
            for point in signals {
                let counter = match (point.is_output(), point.is_word()) {
                    (true, false) => &mut next.output_bits,
                    (true, true) => &mut next.output_words,
                    (false, false) => &mut next.input_bits,
                    (false, true) => &mut next.input_words,
                };
                points.push(IoPoint {
                    point,
                    name: format!("{}.{}", actuator.name, point.label()),
                    address: *counter,
                    scale,
                });
                *counter = counter.saturating_add(1);
            }
        }
        Self { points }
    }

    #[cfg(test)]
    pub fn get(&self, point: Point) -> Option<&IoPoint> {
        self.points.iter().find(|p| p.point == point)
    }

    /// 同じ番号に2つの信号が割り付けられていれば、その信号名の組を返す
    pub fn address_conflicts(&self) -> Vec<(String, String)> {
        let mut used: HashMap<(bool, u16), &str> = HashMap::new();
        let mut conflicts = Vec::new();
        for p in &self.points {
            let key = (p.point.is_word(), p.address);
            if let Some(other) = used.insert(key, &p.name) {
                conflicts.push((other.to_string(), p.name.clone()));
            }
        }
        conflicts
    }
}

fn axis_points(id: u32) -> Vec<Point> {
    vec![
        Point::AxisTarget(id),
        Point::AxisPosition(id),
        Point::AxisInPosition(id),
    ]
}

/// I/O の現在値 (PLC のデバイスメモリにあたる)
#[derive(Clone, Debug, Default)]
pub struct IoValues {
    bits: HashMap<Point, bool>,
    words: HashMap<Point, f64>,
}

impl IoValues {
    pub fn bit(&self, point: Point) -> bool {
        self.bits.get(&point).copied().unwrap_or(false)
    }

    pub fn set_bit(&mut self, point: Point, on: bool) {
        self.bits.insert(point, on);
    }

    pub fn word(&self, point: Point) -> f64 {
        self.words.get(&point).copied().unwrap_or(0.0)
    }

    pub fn set_word(&mut self, point: Point, value: f64) {
        self.words.insert(point, value);
    }

    /// 出力をすべて OFF / 0 に戻す
    pub fn clear_outputs(&mut self) {
        self.bits.retain(|p, _| !p.is_output());
        self.words.retain(|p, _| !p.is_output());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_packed_per_area() {
        let mut chart = TimingChart::default();
        let a = chart.add_actuator(ActuatorKind::Cylinder);
        let b = chart.add_actuator(ActuatorKind::LinearAxis);
        let c = chart.add_actuator(ActuatorKind::Cylinder);
        let table = IoTable::from_chart(&chart, &AddressPlan::default());
        let address = |p| table.get(p).unwrap().address;
        assert_eq!(address(Point::CylinderExtend(a)), 0);
        assert_eq!(address(Point::CylinderExtend(c)), 1);
        assert_eq!(address(Point::CylinderExtended(a)), 100);
        assert_eq!(address(Point::CylinderRetracted(a)), 101);
        assert_eq!(address(Point::AxisInPosition(b)), 102);
        assert_eq!(address(Point::CylinderExtended(c)), 103);
        assert_eq!(address(Point::AxisTarget(b)), 0);
        assert_eq!(address(Point::AxisPosition(b)), 100);
        assert!(table.address_conflicts().is_empty());
    }

    #[test]
    fn overlapping_areas_are_reported() {
        let mut chart = TimingChart::default();
        chart.add_actuator(ActuatorKind::Cylinder);
        let plan = AddressPlan {
            input_bits: 0,
            ..Default::default()
        };
        let table = IoTable::from_chart(&chart, &plan);
        assert_eq!(table.address_conflicts().len(), 1);
    }

    #[test]
    fn words_round_trip_with_sign_and_scale() {
        let p = IoPoint {
            point: Point::AxisTarget(1),
            name: String::new(),
            address: 0,
            scale: 0.001,
        };
        assert_eq!(p.encode_word(-1.5), (-1500i16) as u16);
        assert!((p.decode_word(p.encode_word(-1.5)) + 1.5).abs() < 1e-12);
        // 範囲外は端に丸める
        assert_eq!(p.encode_word(1000.0), i16::MAX as u16);
    }
}
