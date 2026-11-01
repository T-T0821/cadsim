//! SLMP (MC プロトコル) で三菱電機の PLC (MELSEC iQ-R / iQ-F / Q / L) とつなぐ
//!
//! - `frame`: 電文の組み立てと読み取り (3E / 4E フレーム、バイナリ / ASCII)
//! - `client`: TCP / UDP で要求を送り、応答を待つクライアント
//! - `mock`: テスト用の PLC (デバイスメモリを持つ SLMP サーバー)
//!
//! デバイス (X・Y・M・D など) の型は `crate::plc::melsec` にある。

pub mod client;
pub mod frame;
pub mod mock;

pub use client::{ConnectOptions, SlmpClient, SlmpError, Transport};
pub use frame::{Encoding, FrameKind, Route};
