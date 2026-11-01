//! シミュレーション結果を CSV の文字列にする
//!
//! Bevy の ECS・egui・ファイルシステムには触れない。入力はタイミングチャートと部品の形状、
//! 出力は CSV の文字列だけなので、Bevy なしで単体テストできる。
//! 保存ダイアログやファイルへの書き込みは `super::plugin` が受け持つ。
//!
//! 干渉チェックに渡す形 (`Pose`・`TriMesh`) をアプリの `simulation` から使うので、まだアプリ側に置いている。

mod csv;
mod cycle_time;
mod interference;

pub use cycle_time::cycle_time_csv;
pub use interference::{SweepBody, interference_csv};
