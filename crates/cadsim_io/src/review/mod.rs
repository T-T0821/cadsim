//! 複数人レビューの通信 (アダプター層)
//!
//! - `wire`: メッセージ (`cadsim_core::review::Message`) と 1 行の JSON の変換
//! - `net`: TCP での待ち受け (ホスト)・接続 (参加) と、送受信のスレッド
//!
//! 取り決めの中身は docs/review.md にまとめてある。

mod net;
mod wire;

pub use net::{LinkEvent, MAX_LINE_BYTES, ReviewLink, local_addresses, parse_address};
pub use wire::{decode, encode};
