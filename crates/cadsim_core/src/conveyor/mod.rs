//! コンベアによるワーク搬送と、グリッパによる把持・解放 (ドメイン)
//!
//! - `config` コンベアとグリッパの設定 (ユーザーが編集するデータ)
//! - `sim`    ワークの動きの計算 (時刻 0 から一定刻みで進める)
//! - `clock`  繰り返し再生をまたいだ経過時刻
//!
//! グリッパの開閉と動きは [`GripperMotion`] で受け取るので、タイミングチャートには依存しない。
//! 画面とタイミングチャートとのつなぎはアプリ側の `src/conveyor/` にある。

mod clock;
mod config;
mod sim;

pub use clock::CycleClock;
pub use config::{BeltAxis, Conveyor, ConveyorConfig, ConveyorEnd, Gripper};
pub use sim::{
    Belt, ConveyorInputs, ConveyorSim, GripperMotion, GripperState, Place, Pose, TIME_STEP, Work,
};
