//! 機能ごとのフォルダ。1 フォルダ = 1 プラグイン
//!
//! Bevy を使わない中身 (データの型・計算・ファイル形式) は `plantwright_core` の同じ名前のフォルダにある。

pub mod animation;
pub mod annotation;
pub mod assistant;
pub mod conveyor;
pub mod custom_part;
pub mod drawing;
pub mod effects;
pub mod floor;
pub mod flow;
pub mod library;
pub mod line;
pub mod line_compose;
pub mod modeling;
pub mod motion;
pub mod part_role;
pub mod physics;
pub mod plc;
pub mod result_export;
pub mod review;
pub mod safety;
pub mod scripting;
pub mod sequence_gen;
pub mod simulation;
pub mod site;
pub mod sound;
pub mod ui_check;
pub mod video;
