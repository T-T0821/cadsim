//! 3D ファイルの読み書き
//!
//! 各形式の読み書きと中間表現 `Scene` は `cadsim_io::interchange` にある。
//! ここでは `Scene` とアプリのモデルの変換 (`model_io`) と、サンプルファイルを作るテストだけを持つ。

pub use cadsim_io::interchange::*;

pub mod model_io;
#[cfg(test)]
mod samples;
