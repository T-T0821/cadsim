//! PLC 連携: I/O 表・シーケンス・設備のふるまい・通信 (Modbus TCP・SLMP・OPC UA)・プロジェクトファイルの形
//!
//! アプリの `src/features/plc/` が、毎フレームの進め方と画面を受け持つ。

pub mod device_map;
pub mod io;
pub mod io_csv;
pub mod log;
pub mod melsec;
pub mod modbus;
pub mod opcua;
pub mod plant;
pub mod record;
pub mod sequencer;
pub mod session;
pub mod slmp;
pub mod slmp_link;

/// 外部の PLC との通信方式
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Protocol {
    #[default]
    ModbusTcp,
    OpcUa,
    /// 三菱電機の PLC (SLMP / MC プロトコル)
    Slmp,
}
