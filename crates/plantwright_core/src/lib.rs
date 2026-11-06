//! plantwright のうち、Bevy の描画と egui を使わないもの全部
//!
//! データの型・計算・プロジェクトファイルの JSON の形・ファイル形式・通信を、機能ごとのフォルダに置く。
//! アプリ (`src/features/<機能>/`) と同じ名前のフォルダにする (docs/architecture.md)。
//! Bevy をビルドせずに `cargo test -p plantwright_core` で確かめられる。

pub mod animation;
pub mod assistant;
pub mod bom;
pub mod bvh;
pub mod camera_path;
pub mod conveyor;
pub mod custom_part;
pub mod drawing;
pub mod effects;
pub mod factory_hall;
pub mod file_watch;
pub mod floor;
pub mod flow;
pub mod formats;
pub mod gamify;
pub mod history;
pub mod kernel;
pub mod library;
pub mod line;
pub mod line_draft;
pub mod math;
pub mod model;
pub mod modeling;
pub mod motion;
pub mod part_mesh;
pub mod part_role;
pub mod physics;
pub mod plc;
pub mod pointer;
pub mod preferences;
pub mod remap;
pub mod render_quality;
pub mod render_settings;
pub mod review;
pub mod safety;
pub mod scripting;
pub mod sequence_gen;
pub mod shape;
pub mod simulation;
pub mod site;
pub mod sound;
pub mod staffing;
pub mod timeline;
pub mod ui_check;
pub mod video;
pub mod walk;
pub mod workspace;
