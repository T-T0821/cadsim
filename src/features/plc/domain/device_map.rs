//! I/O 表の信号を三菱 PLC のデバイス (M100、D0 など) に割り当てる
//!
//! I/O 表の番号はコイル・保持レジスタの番号 (Modbus と同じ 0 始まり) で振ってある。
//! 三菱 PLC と SLMP でつなぐときは、4つの領域 (出力ビット・出力ワード・入力ビット・入力ワード) ごとに
//! 「どのデバイスの何番から始めるか」を決め、領域の先頭番号からのずれをそのデバイスの番号に足す。
//!
//! 初期値は M・D で、先頭番号も I/O 表の初期値 (出力 0、入力 100) と同じにしてあるので、
//! 番号はそのまま (コイル 100 → M100、保持レジスタ 0 → D0) になる。

use std::collections::HashMap;

use plantwright_core::plc::melsec::{Device, DeviceRef};

use super::io::{AddressPlan, AreaStarts, IoPoint, IoTable};

/// 1つの領域の割り当て: 領域の先頭の信号を `head` に置く
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DeviceArea {
    pub device: Device,
    pub head: u32,
}

impl DeviceArea {
    pub fn first(&self) -> DeviceRef {
        DeviceRef::new(self.device, self.head)
    }
}

/// 4つの領域のデバイスの割り当て
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DeviceMap {
    /// PLC → 設備 のビット
    pub output_bits: DeviceArea,
    /// PLC → 設備 のワード
    pub output_words: DeviceArea,
    /// 設備 → PLC のビット
    pub input_bits: DeviceArea,
    /// 設備 → PLC のワード
    pub input_words: DeviceArea,
}

impl Default for DeviceMap {
    fn default() -> Self {
        let plan = AddressPlan::default();
        let area = |device, head: u16| DeviceArea {
            device,
            head: head.into(),
        };
        Self {
            output_bits: area(Device::M, plan.output_bits),
            output_words: area(Device::D, plan.output_words),
            input_bits: area(Device::M, plan.input_bits),
            input_words: area(Device::D, plan.input_words),
        }
    }
}

impl DeviceMap {
    /// 信号の向きと種類に合う領域
    pub fn area(&self, output: bool, word: bool) -> &DeviceArea {
        match (output, word) {
            (true, false) => &self.output_bits,
            (true, true) => &self.output_words,
            (false, false) => &self.input_bits,
            (false, true) => &self.input_words,
        }
    }

    pub fn area_mut(&mut self, output: bool, word: bool) -> &mut DeviceArea {
        match (output, word) {
            (true, false) => &mut self.output_bits,
            (true, true) => &mut self.output_words,
            (false, false) => &mut self.input_bits,
            (false, true) => &mut self.input_words,
        }
    }

    /// 信号を置くデバイス。領域の先頭番号より前の番号 (CSV で個別に決めたものなど) は置けないので None。
    /// `starts` は I/O 表を作ったときの各領域の先頭番号 ([`IoTable::starts`])。
    pub fn locate(&self, point: &IoPoint, starts: &AreaStarts) -> Option<DeviceRef> {
        let (output, word) = (point.point.is_output(), point.point.is_word());
        let offset = point.address.checked_sub(starts.get(output, word))?;
        let area = self.area(output, word);
        let number = area.head.checked_add(offset.into())?;
        Some(DeviceRef::new(area.device, number))
    }

    /// I/O 表の全信号のデバイス (置けない信号は None)
    pub fn locate_all(&self, table: &IoTable) -> Vec<Option<DeviceRef>> {
        table
            .points
            .iter()
            .map(|p| self.locate(p, &table.starts))
            .collect()
    }

    /// 割り当ての問題 (デバイスの種類の誤り、置けない信号、同じデバイスに2つの信号) を文章で返す
    pub fn problems(&self, table: &IoTable) -> Vec<String> {
        let mut problems = Vec::new();
        for (word, label) in [(false, "ビット"), (true, "ワード")] {
            for output in [true, false] {
                let area = self.area(output, word);
                if area.device.is_bit() == word {
                    problems.push(format!("{} は{}の信号に使えません", area.device, label));
                }
            }
        }
        let mut used: HashMap<DeviceRef, &str> = HashMap::new();
        for p in &table.points {
            match self.locate(p, &table.starts) {
                None => problems.push(format!(
                    "{} の番号 {} は領域の先頭番号より前なので、デバイスに置けません",
                    p.name, p.address
                )),
                Some(device) => {
                    if let Some(other) = used.insert(device, &p.name) {
                        problems.push(format!(
                            "{device} に {other} と {} が重なっています",
                            p.name
                        ));
                    }
                }
            }
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::plc::domain::io::{Point, PointSettings};
    use plantwright_core::motion::chart::ActuatorKind;
    use plantwright_core::motion::chart::TimingChart;

    fn chart() -> (TimingChart, u32, u32) {
        let mut chart = TimingChart::default();
        let cylinder = chart.add_actuator(ActuatorKind::Cylinder);
        let axis = chart.add_actuator(ActuatorKind::LinearAxis);
        (chart, cylinder, axis)
    }

    #[test]
    fn default_map_keeps_the_numbers() {
        let (chart, cylinder, axis) = chart();
        let plan = AddressPlan::default();
        let table = IoTable::from_chart(&chart, &plan);
        let map = DeviceMap::default();
        let at = |point| {
            map.locate(table.get(point).unwrap(), &table.starts)
                .unwrap()
                .to_string()
        };
        assert_eq!(at(Point::CylinderExtend(cylinder)), "M0");
        assert_eq!(at(Point::CylinderExtended(cylinder)), "M100");
        assert_eq!(at(Point::AxisTarget(axis)), "D0");
        assert_eq!(at(Point::AxisPosition(axis)), "D100");
        assert!(map.problems(&table).is_empty());
    }

    #[test]
    fn areas_can_move_to_other_devices() {
        let (chart, cylinder, axis) = chart();
        let plan = AddressPlan::default();
        let table = IoTable::from_chart(&chart, &plan);
        let map = DeviceMap {
            output_bits: DeviceArea {
                device: Device::Y,
                head: 0x20,
            },
            input_bits: DeviceArea {
                device: Device::X,
                head: 0x10,
            },
            output_words: DeviceArea {
                device: Device::W,
                head: 0x100,
            },
            input_words: DeviceArea {
                device: Device::D,
                head: 500,
            },
        };
        let at = |point| {
            map.locate(table.get(point).unwrap(), &table.starts)
                .unwrap()
                .to_string()
        };
        assert_eq!(at(Point::CylinderExtend(cylinder)), "Y20");
        // 入力ビットは 出端 → 戻端 → 位置決め完了 の順に詰める
        assert_eq!(at(Point::CylinderExtended(cylinder)), "X10");
        assert_eq!(at(Point::CylinderRetracted(cylinder)), "X11");
        assert_eq!(at(Point::AxisInPosition(axis)), "X12");
        assert_eq!(at(Point::AxisTarget(axis)), "W100");
        assert_eq!(at(Point::AxisPosition(axis)), "D500");
        assert!(map.problems(&table).is_empty());
    }

    #[test]
    fn problems_are_reported() {
        let (chart, cylinder, _) = chart();
        let mut plan = AddressPlan::default();
        // 入力ビットの先頭より前の番号を個別に決めた
        plan.custom.insert(
            Point::CylinderRetracted(cylinder),
            PointSettings {
                address: Some(5),
                ..Default::default()
            },
        );
        let table = IoTable::from_chart(&chart, &plan);
        let map = DeviceMap {
            // 入力ビットを出力と同じ M0 から始める → 重なる
            input_bits: DeviceArea {
                device: Device::M,
                head: 0,
            },
            // ワードの領域にビットデバイス
            output_words: DeviceArea {
                device: Device::M,
                head: 0,
            },
            ..Default::default()
        };
        let problems = map.problems(&table);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("ワードの信号に使えません"))
        );
        assert!(problems.iter().any(|p| p.contains("先頭番号より前")));
        assert!(problems.iter().any(|p| p.starts_with("M0 に")));
    }
}
