//! ライン連携の時間の進め方 (Bevy に依存しない)
//!
//! 工程シミュレーションと AGV を同じ時間だけ少しずつ進め、そのたびに
//! [`LineCoupler`] で出来事を受け渡す。AGV のシミュレーション (`floor::agv_sim`) が
//! アプリ側にあるので、このファイルもアプリ側に置く (AGV が `cadsim_core` に移るときに一緒に移す)。

use cadsim_core::flow::model::FlowModel;
use cadsim_core::flow::sim::Simulation;
use cadsim_core::line::{AgvEvent, LineCommand, LineCoupler};

use crate::floor::agv_sim::{AgvGraph, AgvSim};

/// 1回に進める最大の時間 (秒)。工程と AGV の出来事を受け渡す細かさ。
const MAX_STEP: f64 = 0.25;
/// 1回に処理する工程の事象の上限 (画面が固まらないように)
const EVENTS_PER_STEP: usize = 20_000;

/// 進めた結果、ラインの外にしてほしいこと
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StepResult {
    /// タイミングチャートを頭から再生する
    pub restart_chart: bool,
}

/// `dt` 秒進める。`flow` が None (工程モデルがない) なら AGV だけ、`agv` が None なら
/// AGV に頼む搬送はすぐ届いたことにする。
pub fn step(
    model: &FlowModel,
    mut flow: Option<&mut Simulation>,
    mut agv: Option<&mut AgvSim>,
    coupler: &mut LineCoupler,
    dt: f64,
) -> StepResult {
    let mut result = StepResult::default();
    let mut left = dt.max(0.0);
    while left > 1e-12 {
        let h = left.min(MAX_STEP);
        left -= h;
        if let Some(sim) = flow.as_deref_mut() {
            let target = sim.now() + h;
            sim.advance(target, EVENTS_PER_STEP);
        }
        if let Some(agv) = agv.as_deref_mut() {
            agv.advance(h, false);
        }
        exchange(
            model,
            flow.as_deref_mut(),
            agv.as_deref_mut(),
            coupler,
            &mut result,
        );
    }
    result
}

/// 出来事がなくなるまで、工程 → AGV → 工程 … と受け渡す
fn exchange(
    model: &FlowModel,
    flow: Option<&mut Simulation>,
    mut agv: Option<&mut AgvSim>,
    coupler: &mut LineCoupler,
    result: &mut StepResult,
) {
    let Some(sim) = flow else {
        // 工程モデルがないときは、AGV の出来事を捨てるだけ
        if let Some(agv) = agv {
            agv.take_events();
        }
        return;
    };
    loop {
        let mut progressed = false;
        for command in coupler.on_flow_events(model, sim.take_events()) {
            match command {
                LineCommand::RequestAgv { from, to, tag } => match agv.as_deref_mut() {
                    Some(agv) => agv.request(from, to, tag),
                    None => {
                        coupler.on_agv_event(sim, AgvEvent::PickedUp { tag });
                        coupler.on_agv_event(sim, AgvEvent::Delivered { tag });
                        progressed = true;
                    }
                },
                LineCommand::RestartChart => result.restart_chart = true,
            }
        }
        if let Some(agv) = agv.as_deref_mut() {
            for event in agv.take_events() {
                coupler.on_agv_event(sim, event);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
}

/// AGV で運ぶ設定のうち、そのままでは運べないものの説明
pub fn link_problems(model: &FlowModel, graph: &AgvGraph, vehicles: usize) -> Vec<String> {
    let mut out = Vec::new();
    let name = |id: u32| graph.node(id).map(|n| n.name.clone());
    let mut any = false;
    for e in &model.elements {
        let Some(call) = e.carrier else {
            continue;
        };
        any = true;
        match (name(call.from), name(call.to)) {
            (Some(from), Some(to)) => {
                if graph.shortest_path(call.from, call.to).is_none() {
                    out.push(format!(
                        "{}: AGV の経路で {from} から {to} へ行けません",
                        e.name
                    ));
                }
            }
            _ => out.push(format!(
                "{}: AGV の積み込み・荷降ろしの点がありません",
                e.name
            )),
        }
    }
    if any && vehicles == 0 {
        out.push("AGV の台数が 0 台なので、AGV で送り出す工程から先へ進みません".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::floor::agv_sim::{AgvGraph, AgvParams};
    use cadsim_core::flow::model::{AgvCall, ElementKind, KindTag};

    /// 生成 (20 秒ごと) → 工程A (10 秒) を AGV で 8 m 先へ → 工程B (10 秒) → 排出
    fn setup(agvs: usize) -> (FlowModel, Simulation, AgvSim) {
        let mut g = AgvGraph::default();
        let a = g.add_node([0.0, 0.0], true);
        let b = g.add_node([8000.0, 0.0], true);
        g.toggle_edge(a, b);
        let mut m = FlowModel::default();
        let s = m.append(KindTag::Source);
        let pa = m.append(KindTag::Process);
        let pb = m.append(KindTag::Process);
        m.append(KindTag::Sink);
        if let ElementKind::Source { interval, .. } = &mut m.get_mut(s).unwrap().kind {
            *interval = 20.0;
        }
        for p in [pa, pb] {
            if let ElementKind::Process { cycle_time, .. } = &mut m.get_mut(p).unwrap().kind {
                *cycle_time = 10.0;
            }
        }
        m.get_mut(pa).unwrap().carrier = Some(AgvCall { from: a, to: b });
        m.settings.duration = 3600.0;
        let mut sim = Simulation::new(&m).unwrap();
        sim.link(LineCoupler::carried_elements(&m));
        let params = AgvParams {
            count: agvs,
            load_time: 2.0,
            unload_time: 2.0,
            ..AgvParams::default()
        };
        let agv = AgvSim::new(&g, &[], &params);
        (m, sim, agv)
    }

    #[test]
    fn agv_carries_every_work_between_the_processes() {
        let (m, mut sim, mut agv) = setup(1);
        let mut coupler = LineCoupler::new();
        // 1 時間を 1/60 秒のコマで回す (画面の速さ 60 倍のつもりで 1 秒ずつ)
        for _ in 0..3600 {
            step(&m, Some(&mut sim), Some(&mut agv), &mut coupler, 1.0);
        }
        // 往復 8 m × 2 + 積み降ろし 4 秒 = 約 20 秒で、生成の間隔 (20 秒) に追いつく
        let output = sim.output_total();
        assert!(output > 150, "排出 {output}");
        assert_eq!(coupler.carried() as usize, agv.stats.delivered);
        assert!(output <= coupler.carried());
    }

    #[test]
    fn without_vehicles_the_line_waits_for_transport() {
        let (m, mut sim, mut agv) = setup(0);
        let mut coupler = LineCoupler::new();
        for _ in 0..600 {
            step(&m, Some(&mut sim), Some(&mut agv), &mut coupler, 1.0);
        }
        assert_eq!(sim.output_total(), 0);
        assert_eq!(coupler.waiting_for_agv(), 1);
    }

    #[test]
    fn problems_point_out_missing_stations_routes_and_vehicles() {
        let (mut m, _, _) = setup(1);
        let mut g = AgvGraph::default();
        let a = g.add_node([0.0, 0.0], true);
        let b = g.add_node([1000.0, 0.0], true);
        let pa = m.elements[1].id;
        m.get_mut(pa).unwrap().carrier = Some(AgvCall { from: a, to: b });
        // 通路がない
        assert_eq!(link_problems(&m, &g, 1).len(), 1);
        g.toggle_edge(a, b);
        assert!(link_problems(&m, &g, 1).is_empty());
        assert_eq!(link_problems(&m, &g, 0).len(), 1);
        m.get_mut(pa).unwrap().carrier = Some(AgvCall { from: a, to: 99 });
        assert_eq!(link_problems(&m, &g, 1).len(), 1);
    }

    #[test]
    fn without_agv_simulation_transport_is_instant() {
        let (m, mut sim, _) = setup(1);
        let mut coupler = LineCoupler::new();
        for _ in 0..600 {
            step(&m, Some(&mut sim), None, &mut coupler, 1.0);
        }
        assert!(sim.output_total() > 25);
    }
}
