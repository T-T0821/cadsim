//! cadsim のドメインとユースケース
//!
//! Bevy・egui・ファイル形式のライブラリには依存しない (docs/architecture.md)。
//! ここに置いたコードは Bevy をビルドせずに `cargo test -p cadsim_core` で確かめられる。

pub mod animation;
pub mod bom;
pub mod bvh;
pub mod conveyor;
pub mod custom_part;
pub mod drawing;
pub mod flow;
pub mod gamify;
pub mod library;
pub mod line;
pub mod math;
pub mod model;
pub mod part_mesh;
pub mod physics;
pub mod remap;
pub mod shape;
pub mod simulation;
pub mod site;
pub mod staffing;
pub mod usecase;
pub mod video;
