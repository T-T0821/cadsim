//! 開発用: 負荷テスト用のシーン (`--stress`)、FPS の計測 (`--bench`)、動作確認用のサンプルファイルの作り直し

pub mod bench;
#[cfg(test)]
mod sample_files;
pub mod stress;
