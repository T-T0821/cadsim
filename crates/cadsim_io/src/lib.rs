//! cadsim のアダプター層
//!
//! ファイル形式など外部とのやり取りを、形式に依存しない中間表現に変換する。
//! Bevy・egui には依存しない (docs/architecture.md)。

pub mod interchange;
pub mod rtde;
pub mod step;
