//! シミュレーション結果を CSV の文字列にする (ユースケース層)
//!
//! Bevy の ECS・egui・ファイルシステムには触れない。入力はタイミングチャートと部品の形状、
//! 出力は CSV の文字列だけなので、Bevy なしで単体テストできる。
//! 保存ダイアログやファイルへの書き込みは UI 層 (`super::plugin`) が受け持つ。
//!
//! ドメインの型 (`TimingChart`, `TriMesh` など) はまだ Bevy と同じ crate にあるので、
//! ベクトルと行列は `bevy::math` (中身は glam) を使う。core crate に移すときは glam に置き換える。

mod csv;
mod cycle_time;
mod interference;

pub use cycle_time::cycle_time_csv;
pub use interference::{SweepBody, interference_csv};
