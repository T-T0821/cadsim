//! ライン連携の巻き戻し (Bevy に依存しない)
//!
//! ラインは PLC の信号や画面の切り替え (運転・投入許可) で進み方が変わるので、
//! 進め直して写しの間の時刻を作ることはせず、いちばん近い前の写しに戻す。
//! 戻した時点から再生し直すと別の未来になりうるので、先の記録は捨てる
//! ([`crate::timeline::Timeline::truncate_after`])。

use crate::conveyor::ConveyorSim;
use crate::floor::agv_sim::AgvSim;
use crate::flow::sim::Simulation;
use crate::line::LineCoupler;
use crate::timeline::Timeline;

/// 写しを取る間隔 (ラインの時刻で秒)
const INTERVAL: f64 = 0.5;
/// 写しの数の上限 (超えたら間引いて間隔を倍にする)
const CAPACITY: usize = 1200;

/// ある時刻のライン一式の状態
#[derive(Clone, Debug)]
pub struct LineSnapshot {
    pub coupler: LineCoupler,
    /// 工程シミュレーション (工程モデルがなければ None)
    pub flow: Option<Simulation>,
    pub agv: Option<AgvSim>,
    /// ワーク搬送 (ライン連携で進めていなければ None)
    pub conveyor: Option<ConveyorSim>,
    /// 作業者と AGV の時計
    pub floor_time: f64,
    /// タイミングチャートの再生位置
    pub chart_time: f64,
}

/// ライン連携の写しの記録
pub type LineTimeline = Timeline<LineSnapshot>;

/// ライン連携を回し始めるときの記録の入れ物
pub fn line_timeline() -> LineTimeline {
    Timeline::new(INTERVAL, CAPACITY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::model::sample_line;

    fn snapshot(flow: &Simulation, chart_time: f64) -> LineSnapshot {
        LineSnapshot {
            coupler: LineCoupler::new(),
            flow: Some(flow.clone()),
            agv: None,
            conveyor: None,
            floor_time: flow.now(),
            chart_time,
        }
    }

    #[test]
    fn going_back_returns_the_nearest_earlier_state_and_drops_the_future_on_resume() {
        let model = sample_line();
        let mut sim = Simulation::new(&model).unwrap();
        let mut tl = line_timeline();
        let mut t = 0.0;
        while t <= 10.0 {
            sim.advance(t, usize::MAX);
            tl.record(t, &snapshot(&sim, t * 0.1));
            t += 0.25;
        }
        let (at, snap) = tl.at_or_before(3.7).unwrap();
        assert_eq!(*at, 3.5);
        assert_eq!(snap.flow.as_ref().unwrap().now(), 3.5);
        assert!((snap.chart_time - 0.35).abs() < 1e-9);

        tl.truncate_after(*at);
        assert_eq!(tl.span(), Some((0.0, 3.5)));
    }
}
