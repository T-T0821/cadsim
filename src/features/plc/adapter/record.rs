//! PLC 連携の設定をプロジェクトファイル (.plantwright) に書く形 (Bevy に依存しない)
//!
//! 書くのは I/O の割り付け方 (各領域の先頭番号と、CSV で個別に決めた信号) と、外部 PLC の通信方式・接続先だけ。
//! 運転中の I/O の値やモードは書かない。信号はアクチュエータの id と種類で指す。

use serde::{Deserialize, Serialize};

use plantwright_core::plc::melsec::DeviceRef;
use plantwright_core::plc::slmp::{Encoding, FrameKind, Transport};

use super::Protocol;
use super::modbus::ModbusSettings;
use super::opcua::OpcUaSettings;
use super::slmp::SlmpSettings;
use crate::features::plc::domain::device_map::{DeviceArea, DeviceMap};
use crate::features::plc::domain::io::{AddressPlan, Point, PointSettings};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlcRecord {
    pub output_bits: u16,
    pub output_words: u16,
    pub input_bits: u16,
    pub input_words: u16,
    /// CSV で個別に決めた信号 (アクチュエータ id・信号の種類の順に並べる)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom: Vec<PointRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modbus: Option<ModbusRecord>,
    /// 外部 PLC と OPC UA でつなぐなら true (false は Modbus TCP)
    #[serde(default, skip_serializing_if = "is_false")]
    pub use_opcua: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opcua: Option<OpcUaRecord>,
    /// 外部 PLC と SLMP (三菱 PLC) でつなぐなら true。`use_opcua` より優先する。
    #[serde(default, skip_serializing_if = "is_false")]
    pub use_slmp: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slmp: Option<SlmpRecord>,
}

fn is_false(v: &bool) -> bool {
    !*v
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OpcUaRecord {
    pub endpoint: String,
    pub interval_ms: u64,
}

/// SLMP の接続先と、I/O 表の信号を置くデバイス (各領域の先頭を "M100" のような文字列で書く)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SlmpRecord {
    pub host: String,
    pub port: u16,
    pub transport: TransportRecord,
    pub frame: FrameRecord,
    pub encoding: EncodingRecord,
    pub poll_ms: u64,
    pub output_bits: String,
    pub output_words: String,
    pub input_bits: String,
    pub input_words: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TransportRecord {
    Tcp,
    Udp,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameRecord {
    #[serde(rename = "3E")]
    E3,
    #[serde(rename = "4E")]
    E4,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EncodingRecord {
    Binary,
    Ascii,
}

/// 保存する PLC 連携の設定一式
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlcSettings {
    pub plan: AddressPlan,
    pub protocol: Protocol,
    pub modbus: ModbusSettings,
    pub opcua: OpcUaSettings,
    pub slmp: SlmpSettings,
}

fn slmp_record(s: &SlmpSettings) -> SlmpRecord {
    let area = |a: &DeviceArea| a.first().to_string();
    SlmpRecord {
        host: s.host.clone(),
        port: s.port,
        transport: match s.transport {
            Transport::Tcp => TransportRecord::Tcp,
            Transport::Udp => TransportRecord::Udp,
        },
        frame: match s.frame {
            FrameKind::E3 => FrameRecord::E3,
            FrameKind::E4 => FrameRecord::E4,
        },
        encoding: match s.encoding {
            Encoding::Binary => EncodingRecord::Binary,
            Encoding::Ascii => EncodingRecord::Ascii,
        },
        poll_ms: s.poll_ms,
        output_bits: area(&s.devices.output_bits),
        output_words: area(&s.devices.output_words),
        input_bits: area(&s.devices.input_bits),
        input_words: area(&s.devices.input_words),
    }
}

fn slmp_settings(r: &SlmpRecord) -> SlmpSettings {
    let defaults = DeviceMap::default();
    // 読めない書き方なら、その領域は初期値にする
    let area = |text: &str, default: DeviceArea| {
        DeviceRef::parse(text).map_or(default, |d| DeviceArea {
            device: d.device,
            head: d.number,
        })
    };
    SlmpSettings {
        host: r.host.clone(),
        port: r.port,
        transport: match r.transport {
            TransportRecord::Tcp => Transport::Tcp,
            TransportRecord::Udp => Transport::Udp,
        },
        frame: match r.frame {
            FrameRecord::E3 => FrameKind::E3,
            FrameRecord::E4 => FrameKind::E4,
        },
        encoding: match r.encoding {
            EncodingRecord::Binary => Encoding::Binary,
            EncodingRecord::Ascii => Encoding::Ascii,
        },
        poll_ms: r.poll_ms,
        devices: DeviceMap {
            output_bits: area(&r.output_bits, defaults.output_bits),
            output_words: area(&r.output_words, defaults.output_words),
            input_bits: area(&r.input_bits, defaults.input_bits),
            input_words: area(&r.input_words, defaults.input_words),
        },
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SignalRecord {
    CylinderExtend,
    CylinderExtended,
    CylinderRetracted,
    AxisTarget,
    AxisPosition,
    AxisInPosition,
    /// ライン連携の信号。`actuator` の欄にライン連携の信号表の番号を入れる。
    LineInput,
    LineInputWord,
    LineOutput,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PointRecord {
    pub actuator: u32,
    pub signal: SignalRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// OPC UA のノード ID
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node: String,
    /// PLC のログ CSV でこの信号が入っている列の見出し
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub log_column: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModbusRecord {
    pub host: String,
    pub port: u16,
    pub unit_id: u8,
    pub poll_ms: u64,
}

fn split(point: Point) -> (u32, SignalRecord) {
    match point {
        Point::CylinderExtend(id) => (id, SignalRecord::CylinderExtend),
        Point::CylinderExtended(id) => (id, SignalRecord::CylinderExtended),
        Point::CylinderRetracted(id) => (id, SignalRecord::CylinderRetracted),
        Point::AxisTarget(id) => (id, SignalRecord::AxisTarget),
        Point::AxisPosition(id) => (id, SignalRecord::AxisPosition),
        Point::AxisInPosition(id) => (id, SignalRecord::AxisInPosition),
        Point::LineInput(a) => (a.into(), SignalRecord::LineInput),
        Point::LineInputWord(a) => (a.into(), SignalRecord::LineInputWord),
        Point::LineOutput(a) => (a.into(), SignalRecord::LineOutput),
    }
}

fn join(id: u32, signal: SignalRecord) -> Point {
    match signal {
        SignalRecord::CylinderExtend => Point::CylinderExtend(id),
        SignalRecord::CylinderExtended => Point::CylinderExtended(id),
        SignalRecord::CylinderRetracted => Point::CylinderRetracted(id),
        SignalRecord::AxisTarget => Point::AxisTarget(id),
        SignalRecord::AxisPosition => Point::AxisPosition(id),
        SignalRecord::AxisInPosition => Point::AxisInPosition(id),
        SignalRecord::LineInput => Point::LineInput(line_address(id)),
        SignalRecord::LineInputWord => Point::LineInputWord(line_address(id)),
        SignalRecord::LineOutput => Point::LineOutput(line_address(id)),
    }
}

fn line_address(id: u32) -> u16 {
    u16::try_from(id).unwrap_or(u16::MAX)
}

/// 保存する形にする。どちらも初期値のままなら None (項目ごと省く)。
pub fn to_record(settings: &PlcSettings) -> Option<PlcRecord> {
    if *settings == PlcSettings::default() {
        return None;
    }
    let PlcSettings {
        plan,
        protocol,
        modbus,
        opcua,
        slmp,
    } = settings;
    let modbus = (*modbus != ModbusSettings::default()).then(|| ModbusRecord {
        host: modbus.host.clone(),
        port: modbus.port,
        unit_id: modbus.unit_id,
        poll_ms: modbus.poll_ms,
    });
    let opcua = (*opcua != OpcUaSettings::default()).then(|| OpcUaRecord {
        endpoint: opcua.endpoint.clone(),
        interval_ms: opcua.interval_ms,
    });
    let slmp = (*slmp != SlmpSettings::default()).then(|| slmp_record(slmp));
    let mut custom: Vec<PointRecord> = plan
        .custom
        .iter()
        .map(|(&point, s)| {
            let (actuator, signal) = split(point);
            PointRecord {
                actuator,
                signal,
                address: s.address,
                scale: s.scale,
                node: s.node.clone(),
                log_column: s.log_column.clone(),
            }
        })
        .collect();
    // HashMap の順は毎回変わるので、並べてから書く (未保存の判定がぶれないように)
    custom.sort_by_key(|p| (p.actuator, p.signal));
    Some(PlcRecord {
        output_bits: plan.output_bits,
        output_words: plan.output_words,
        input_bits: plan.input_bits,
        input_words: plan.input_words,
        custom,
        modbus,
        use_opcua: *protocol == Protocol::OpcUa,
        opcua,
        use_slmp: *protocol == Protocol::Slmp,
        slmp,
    })
}

/// ファイルから読む。項目がなければ初期値。
pub fn from_record(record: Option<&PlcRecord>) -> PlcSettings {
    let Some(r) = record else {
        return Default::default();
    };
    let plan = AddressPlan {
        output_bits: r.output_bits,
        output_words: r.output_words,
        input_bits: r.input_bits,
        input_words: r.input_words,
        custom: r
            .custom
            .iter()
            .map(|p| {
                (
                    join(p.actuator, p.signal),
                    PointSettings {
                        address: p.address,
                        scale: p.scale,
                        node: p.node.clone(),
                        log_column: p.log_column.clone(),
                    },
                )
            })
            .collect(),
    };
    let modbus = match &r.modbus {
        Some(m) => ModbusSettings {
            host: m.host.clone(),
            port: m.port,
            unit_id: m.unit_id,
            poll_ms: m.poll_ms,
        },
        None => ModbusSettings::default(),
    };
    let opcua = match &r.opcua {
        Some(o) => OpcUaSettings {
            endpoint: o.endpoint.clone(),
            interval_ms: o.interval_ms,
        },
        None => OpcUaSettings::default(),
    };
    let protocol = if r.use_slmp {
        Protocol::Slmp
    } else if r.use_opcua {
        Protocol::OpcUa
    } else {
        Protocol::ModbusTcp
    };
    PlcSettings {
        plan,
        protocol,
        modbus,
        opcua,
        slmp: r.slmp.as_ref().map(slmp_settings).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_not_written() {
        assert_eq!(to_record(&PlcSettings::default()), None);
        assert_eq!(from_record(None), PlcSettings::default());
    }

    #[test]
    fn round_trip_keeps_custom_points_and_modbus() {
        let mut plan = AddressPlan {
            input_bits: 200,
            ..Default::default()
        };
        plan.custom.insert(
            Point::AxisTarget(3),
            PointSettings {
                address: Some(40),
                scale: Some(0.01),
                node: "ns=2;s=X.Target".into(),
                log_column: "D100".into(),
            },
        );
        plan.custom.insert(
            Point::CylinderExtend(1),
            PointSettings {
                address: Some(5),
                ..Default::default()
            },
        );
        let modbus = ModbusSettings {
            host: "10.0.0.5".into(),
            ..Default::default()
        };
        let settings = PlcSettings {
            plan,
            protocol: Protocol::OpcUa,
            modbus,
            ..Default::default()
        };
        let record = to_record(&settings).unwrap();
        assert!(record.opcua.is_none());
        assert_eq!(record.custom[0].actuator, 1);
        let json = serde_json::to_string(&record).unwrap();
        let parsed: PlcRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(from_record(Some(&parsed)), settings);
    }

    #[test]
    fn round_trip_keeps_slmp_settings() {
        use plantwright_core::plc::melsec::Device;
        let mut slmp = SlmpSettings {
            host: "192.168.0.39".into(),
            port: 5001,
            transport: Transport::Udp,
            frame: FrameKind::E4,
            encoding: Encoding::Ascii,
            ..Default::default()
        };
        slmp.devices.output_bits = DeviceArea {
            device: Device::Y,
            head: 0x20,
        };
        let settings = PlcSettings {
            protocol: Protocol::Slmp,
            slmp,
            ..Default::default()
        };
        let record = to_record(&settings).unwrap();
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains(r#""output_bits":"Y20""#), "{json}");
        assert!(json.contains(r#""frame":"4E""#), "{json}");
        assert!(json.contains(r#""use_slmp":true"#), "{json}");
        let parsed: PlcRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(from_record(Some(&parsed)), settings);
    }

    #[test]
    fn old_files_without_slmp_still_open() {
        let json = r#"{"output_bits":0,"output_words":0,"input_bits":100,"input_words":100,"use_opcua":true}"#;
        let parsed: PlcRecord = serde_json::from_str(json).unwrap();
        let settings = from_record(Some(&parsed));
        assert_eq!(settings.protocol, Protocol::OpcUa);
        assert_eq!(settings.slmp, SlmpSettings::default());
    }
}
