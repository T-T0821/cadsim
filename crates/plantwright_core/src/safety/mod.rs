//! 安全機器の計算。表示とパネルはアプリ側の `safety`
//!
//! - `device.rs`: 機器のデータ、侵入の検知、停止信号のラッチ、ISO 13855 の安全距離
//! - `check.rs`: 動作範囲 (危険源) と、柵・ライトカーテンの距離の確認

pub mod check;
pub mod device;
