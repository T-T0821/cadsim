//! 物理演算のドメインとユースケース (Bevy・物理エンジンのライブラリに依存しない)
//!
//! - `settings`: 部品ごとの物理の設定 (ドメイン)
//! - `cable`: ケーブル・ホースの紐モデル (ドメイン)
//! - `world`: 部品 → 剛体の変換、一定刻みの進行、物理エンジンのポート (ユースケース)
//!
//! 物理エンジンの実装 (Rapier) と画面はアプリ側の `src/physics/` にある。設計は docs/physics.md。

pub mod cable;
pub mod settings;
pub mod world;
