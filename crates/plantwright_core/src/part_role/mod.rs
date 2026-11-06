//! 部品の「役割」: 部品を選んで「この部品をシリンダにする」などを押し、3D で向きや長さを決める
//!
//! 役割の実体は、タイミングチャートのアクチュエータ・センサーと、ワーク搬送のコンベア・投入口・
//! 排出口・グリッパの行。ここはそれを部品の側から扱う。
//!
//! | ファイル | 役割 |
//! | --- | --- |
//! | `model.rs` | 役割の種類 ([`RoleKind`])・付いている行 ([`RoleRef`])・下書き ([`RoleDraft`]) |
//! | `assign.rs` | 部品の役割を探す・寸法から既定値で作る・書き込む・外す・足りない設定を見つける |
//! | `tool.rs` | 3D で矢印や点をドラッグして値を決める道具 ([`RoleTool`]) |
//!
//! 画面 (プロパティのボタン、3D の矢印、「!」の印) はアプリ側の `features/part_role`。

pub mod assign;
pub mod model;
pub mod tool;

pub use model::{RoleBody, RoleDraft, RoleKind, RoleRef};
pub use tool::{Constraint, Handle, RoleTool};
