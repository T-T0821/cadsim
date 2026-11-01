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
pub mod effects;
pub mod floor;
pub mod flow;
pub mod gamify;
pub mod history;
pub mod library;
pub mod line;
pub mod line_draft;
pub mod math;
pub mod melsec;
pub mod model;
pub mod modeling;
pub mod motion;
pub mod part_mesh;
pub mod physics;
pub mod remap;
pub mod safety;
pub mod shape;
pub mod simulation;
pub mod site;
pub mod staffing;
pub mod timeline;
pub mod ui_check;
pub mod usecase;
pub mod video;
pub mod workspace;
