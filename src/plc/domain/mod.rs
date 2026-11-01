//! PLC 連携のドメイン: I/O の割り付け表、設備モデル、内蔵の模擬 PLC。
//! Bevy・egui・通信ライブラリには依存しない。

pub mod device_map;
pub mod io;
pub mod io_csv;
pub mod log;
pub mod plant;
pub mod sequencer;
