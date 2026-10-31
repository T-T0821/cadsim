//! cadsim のドメインとユースケース
//!
//! Bevy・egui・ファイル形式のライブラリには依存しない (docs/architecture.md)。
//! ここに置いたコードは Bevy をビルドせずに `cargo test -p cadsim_core` で確かめられる。

pub mod drawing;
pub mod flow;
pub mod math;
pub mod simulation;
pub mod site;
