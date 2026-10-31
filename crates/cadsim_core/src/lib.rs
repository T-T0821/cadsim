//! cadsim のドメインとユースケース
//!
//! Bevy・egui・ファイル形式のライブラリには依存しない (docs/architecture.md)。
//! ここに置いたコードは Bevy をビルドせずに `cargo test -p cadsim_core` で確かめられる。

pub mod conveyor;
pub mod drawing;
pub mod flow;
pub mod gamify;
pub mod math;
pub mod physics;
pub mod remap;
pub mod simulation;
pub mod site;
