//! ライン連携: 工程シミュレーション・AGV・安全機器・PLC の信号を1本のラインとして動かす
//! (Bevy に依存しない)
//!
//! - [`coupler`] 工程シミュレーションの出来事と AGV・コンベアの出来事をつなぐ。
//!   「工程が送り出すワークを AGV が積んで (コンベアに流して) 運び、届いた先の工程へ入れる」と
//!   「工程が加工を始めたらタイミングチャートを頭から再生する」を受け持つ。
//! - [`signals`] PLC とやりとりする信号の一覧 (設備の状態 → PLC、PLC → 運転・投入許可)。
//! - [`workpieces`] 3D に描くワークの位置 (設備の上に積む・設備から設備へ渡す)。
//! - [`stepper`] 工程シミュレーション・AGV・ワーク搬送 (3D のコンベア) を少しずつ進めて出来事を受け渡す。
//!
//! 時計は1つ: ラインの時刻を進めるときは、工程シミュレーション・AGV・作業者・
//! タイミングチャートを同じだけ進める。安全停止中と、PLC の「運転」が OFF の間は全部止める。

pub mod coupler;
pub mod signals;
pub mod stepper;
pub mod workpieces;

pub use coupler::{CarrierEvent, LineCommand, LineCoupler, Transit};
pub use signals::{
    ConveyorCounts, Direction, LineInputs, Signal, SignalSources, Value, line_signals,
};
pub use stepper::{ConveyorLink, LineParts, StepResult, conveyor_problems, link_problems, step};

/// ラインを進めてよいか。安全停止中か、PLC の「運転」が OFF なら止める。
pub fn may_run(safety_stopped: bool, inputs: &LineInputs) -> bool {
    !safety_stopped && inputs.run
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_stops_on_safety_stop_or_when_run_is_off() {
        let on = LineInputs::default();
        assert!(may_run(false, &on));
        assert!(!may_run(true, &on));
        let off = LineInputs {
            run: false,
            ..LineInputs::default()
        };
        assert!(!may_run(false, &off));
    }
}
