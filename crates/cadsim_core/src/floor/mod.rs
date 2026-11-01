//! 床の上を動くもの (作業者と AGV) の計算。表示とパネルはアプリ側の `floor`
//!
//! | ファイル | 役割 |
//! | --- | --- |
//! | `geom.rs` | 床の上 (XZ) の長方形・距離の計算 |
//! | `operator_plan.rs` | 作業者の手順 → 時刻表・作業時間・歩行距離・経路 |
//! | `agv_sim.rs` | AGV の経路グラフ・配車・交差点の排他・台数比較 |
//! | `figure.rs` | 作業者と AGV の 3D の形 (単純な形の組み合わせ)・歩く動き・向き |

pub mod agv_sim;
pub mod figure;
pub mod geom;
pub mod operator_plan;
