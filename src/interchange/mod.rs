//! 3D ファイルの読み書き
//!
//! 各形式の読み書き、中間表現 `Scene`、`Scene` と設計データの変換 (`model_io`) は
//! `cadsim_io::interchange` にある。ここではサンプルファイルを作るテストだけを持つ。

pub use cadsim_io::interchange::*;

#[cfg(test)]
mod samples;
