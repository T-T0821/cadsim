//! コンベアによるワーク搬送と、グリッパによる把持・解放 (ドメイン)
//!
//! - `config` ワーク種類・投入口・排出口・コンベア・グリッパの設定 (ユーザーが編集するデータ)
//! - `sim`    ワークの動きと投入数・排出数の計算 (時刻 0 から一定刻みで進める)
//! - `clock`  繰り返し再生をまたいだ経過時刻
//!
//! グリッパの開閉と、ワークを載せて動く台は [`MachineMotion`] で受け取るので、タイミングチャートには依存しない。
//! 画面とタイミングチャートとのつなぎはアプリ側の `src/conveyor/` にある。

mod clock;
mod config;
mod sim;

pub use clock::CycleClock;
pub use config::{
    BeltAxis, Conveyor, ConveyorConfig, ConveyorEnd, Gripper, Sink, Source, SourceOutput, WorkKind,
};
pub use sim::{
    Bounds, Carrier, ConveyorInputs, ConveyorSim, Counts, Exit, GripperState, MachineMotion, Place,
    Pose, TIME_STEP, Work,
};
