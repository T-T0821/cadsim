//! plantwright のアダプター層
//!
//! ファイル形式など外部とのやり取りを、ドメインの型や形式に依存しない中間表現に変換する。
//! Bevy・egui には依存しない (docs/architecture.md)。

pub mod asset_key;
pub mod claude;
pub mod csv_text;
pub mod flow;
pub mod formats;
pub mod interchange;
pub mod line_draft_json;
pub mod modeled;
pub mod preferences_file;
pub mod review;
pub mod rtde;
pub mod slmp;
pub mod sound_settings;
pub mod step;
pub mod ui_check;
pub mod urdf;
