//! PLC 連携のアダプター: 実機の PLC との通信 ([`PlcLink`](super::usecase::PlcLink) の実装)

pub mod modbus;
pub mod opcua;
pub mod record;
pub mod slmp;

/// 外部の PLC との通信方式
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Protocol {
    #[default]
    ModbusTcp,
    OpcUa,
    /// 三菱電機の PLC (SLMP / MC プロトコル)
    Slmp,
}
