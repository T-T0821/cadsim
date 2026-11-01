//! タイミングチャートとラダーの自動生成: 下書きの型 (`spec`)、生成 AI とのやり取り (`generate`・`json`)、
//! ラダー (`ladder`)、動作ログからの出来事の取り出し (`log_events`)

pub mod generate;
pub mod json;
pub mod ladder;
pub mod log_events;
pub mod spec;
