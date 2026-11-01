//! 人員配置
//!
//! - `model`   作業者の名簿・スキル・シフトと休憩、工程に人が要るか ([`model::Labor`])
//! - `compare` 人数を 1〜N 人に変えて工程シミュレーションを回し、並べて比べる
//!
//! 作業者を工程に割り当てて動かすのは工程シミュレーション (`flow::sim`) の中で行う。設計は docs/staffing.md。

pub mod compare;
pub mod model;
