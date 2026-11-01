//! 標準部品 (シリンダ・コンベア・センサー・シーケンサーなど) の種類・パラメータと形状。
//! モデルへの追加やパネルはアプリ側の `library`

pub mod catalog;
pub mod geometry;

pub use catalog::{IoDir, Motion, PartKind, Spec};
