//! 工程モデルとスループット統計 (ドメイン)
//!
//! - `model`   生成・工程・バッファ・排出をつないだラインのデータと検証
//! - `sim`     離散事象シミュレーションと集計
//! - `factory` ラインのまとまり (工場)、別ファイルのラインの取り込み、ラインごとの集計
//! - `plan`    品種・段取り替え・生産計画 (注文) と納期の集計
//! - `scenario` 案の比較 (種を変えた繰り返し実行)
//! - `rewind`  巻き戻しと時間スライダー (途中の状態の記録・詰まりの直前へ飛ぶ)
//! - `palette` 状態と稼働率の色 (3D ビュー・グラフ・Excel で共通)
//!
//! 画面・保存形式・Excel 書き出しはアプリ側の `src/flow/` にある。設計は docs/flow.md。

pub mod factory;
pub mod model;
pub mod palette;
pub mod plan;
pub mod plan_table;
pub mod record;
pub mod rewind;
pub mod scenario;
pub mod sim;
pub mod xlsx;
