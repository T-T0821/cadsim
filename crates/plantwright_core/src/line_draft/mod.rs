//! 言葉で組むラインの下書き
//!
//! 「コンベア2本とロボット1台で箱詰め」のような文から作る、ラインの設備と動作の下書き。
//!
//! - [`model`] … 下書きのデータ (設備の一覧と、ステップの並び)
//! - [`check`] … 下書きを使える形に整える (名前の重なり、存在しない設備への動作などを直す)
//! - [`layout`] … 設備を床に並べる位置と大きさを決める
//! - [`rules`] … 生成 AI を使わずに、文のキーワードから下書きを作る
//!
//! 生成 AI に下書きを作らせるときの流れは [`crate::line_draft::compose`]。

pub mod check;
pub mod compose;
pub mod json;
pub mod layout;
pub mod model;
pub mod rules;

pub use model::{DraftAction, DraftStep, Equipment, EquipmentKind, LineDraft, Verb};
