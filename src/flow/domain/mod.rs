//! ワーク搬送のドメイン (Bevy・egui に依存しない)
//!
//! - `config`: コンベアとグリッパの設定 (ユーザーが編集するデータ)
//! - `sim`: ワークの動きの計算 (時刻 0 から一定刻みで進める)
//! - `clock`: 繰り返し再生をまたいだ経過時刻

mod clock;
mod config;
mod sim;

pub use clock::CycleClock;
pub use config::{Conveyor, ConveyorEnd, FlowConfig, Gripper};
pub use sim::{Belt, FlowInputs, FlowSim, Place, Work};
