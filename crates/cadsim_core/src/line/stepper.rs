//! ライン連携の時間の進め方
//!
//! 工程シミュレーション・AGV・ワーク搬送 (3D のコンベア) を同じ時間だけ少しずつ進め、
//! そのたびに [`LineCoupler`] で出来事を受け渡す。

use crate::conveyor::{ConveyorConfig, ConveyorEnd, ConveyorEvent, ConveyorInputs, ConveyorSim};
use crate::floor::agv_sim::{AgvGraph, AgvSim};
use crate::flow::model::FlowModel;
use crate::flow::sim::Simulation;
use crate::line::{CarrierEvent, LineCommand, LineCoupler};

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

/// ラインと一緒に進めるワーク搬送。`sim.time` はラインの時刻にそろえておく。
pub struct ConveyorLink<'a, 'b> {
    pub sim: &'a mut ConveyorSim,
    pub inputs: &'a ConveyorInputs<'b>,
}

/// 進めるもの一式。`flow` が None (工程モデルがない) なら AGV とコンベアだけ進める。
/// `agv` が None なら AGV に頼む搬送は、`conveyor` が None ならコンベアに頼む搬送は、すぐ届いたことにする。
pub struct LineParts<'a, 'b> {
    pub flow: Option<&'a mut Simulation>,
    pub agv: Option<&'a mut AgvSim>,
    pub conveyor: Option<ConveyorLink<'a, 'b>>,
}

/// `dt` 秒進める
pub fn step(
    model: &FlowModel,
    mut parts: LineParts,
    coupler: &mut LineCoupler,
    dt: f64,
) -> StepResult {
    let mut result = StepResult::default();
    let mut left = dt.max(0.0);
    while left > 1e-12 {
        let h = left.min(MAX_STEP);
        left -= h;
        if let Some(sim) = parts.flow.as_deref_mut() {
            let target = sim.now() + h;
            sim.advance(target, EVENTS_PER_STEP);
        }
        if let Some(agv) = parts.agv.as_deref_mut() {
            agv.advance(h, false);
        }
        if let Some(c) = parts.conveyor.as_mut() {
            let target = c.sim.time + h;
            c.sim.advance_to(c.inputs, target);
        }
        exchange(model, &mut parts, coupler, &mut result);
    }
    result
}

/// 出来事がなくなるまで、工程 → AGV・コンベア → 工程 … と受け渡す
fn exchange(
    model: &FlowModel,
    parts: &mut LineParts,
    coupler: &mut LineCoupler,
    result: &mut StepResult,
) {
    let LineParts {
        flow,
        agv,
        conveyor,
    } = parts;
    let Some(sim) = flow.as_deref_mut() else {
        // 工程モデルがないときは、AGV とコンベアの出来事を捨てるだけ
        if let Some(agv) = agv {
            agv.take_events();
        }
        if let Some(c) = conveyor {
            c.sim.take_events();
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
                        deliver_now(coupler, sim, tag);
                        progressed = true;
                    }
                },
                LineCommand::FeedConveyor { source, tag } => match conveyor.as_mut() {
                    Some(c) if c.sim.is_driven(source) => c.sim.feed_external(source, tag),
                    _ => {
                        deliver_now(coupler, sim, tag);
                        progressed = true;
                    }
                },
                LineCommand::RestartChart => result.restart_chart = true,
            }
        }
        if let Some(agv) = agv.as_deref_mut() {
            for event in agv.take_events() {
                coupler.on_carrier_event(sim, event);
                progressed = true;
            }
        }
        if let Some(c) = conveyor.as_mut() {
            for event in c.sim.take_events() {
                let event = match event {
                    ConveyorEvent::Fed { tag } => CarrierEvent::PickedUp { tag },
                    ConveyorEvent::Discharged { tag } => CarrierEvent::Delivered { tag },
                };
                coupler.on_carrier_event(sim, event);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
}

/// 運ぶものがないときは、積んですぐ降ろしたことにする
fn deliver_now(coupler: &mut LineCoupler, sim: &mut Simulation, tag: u64) {
    coupler.on_carrier_event(sim, CarrierEvent::PickedUp { tag });
    coupler.on_carrier_event(sim, CarrierEvent::Delivered { tag });
}

/// ワーク搬送の投入口から流す設定のうち、そのままでは流せないものの説明
pub fn conveyor_problems(model: &FlowModel, config: &ConveyorConfig) -> Vec<String> {
    let mut out = Vec::new();
    for e in &model.elements {
        let Some(source) = e.conveyor.filter(|_| e.carrier.is_none()) else {
            continue;
        };
        match config.sources.iter().find(|s| s.id == source) {
            None => out.push(format!("{}: ワーク搬送の投入口がありません", e.name)),
            Some(s)
                if config.sinks.is_empty()
                    && !config
                        .conveyors
                        .iter()
                        .any(|c| c.end == ConveyorEnd::Discharge) =>
            {
                out.push(format!(
                    "{}: {} から流したワークが出ていく排出口・排出端のコンベアがありません",
                    e.name, s.name
                ))
            }
            Some(_) => {}
        }
    }
    out
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
    use crate::floor::agv_sim::AgvParams;
    use crate::flow::model::{AgvCall, ElementKind, KindTag};

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

    fn parts<'a>(
        flow: Option<&'a mut Simulation>,
        agv: Option<&'a mut AgvSim>,
    ) -> LineParts<'a, 'static> {
        LineParts {
            flow,
            agv,
            conveyor: None,
        }
    }

    #[test]
    fn agv_carries_every_work_between_the_processes() {
        let (m, mut sim, mut agv) = setup(1);
        let mut coupler = LineCoupler::new();
        // 1 時間を 1/60 秒のコマで回す (画面の速さ 60 倍のつもりで 1 秒ずつ)
        for _ in 0..3600 {
            step(&m, parts(Some(&mut sim), Some(&mut agv)), &mut coupler, 1.0);
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
            step(&m, parts(Some(&mut sim), Some(&mut agv)), &mut coupler, 1.0);
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
            step(&m, parts(Some(&mut sim), None), &mut coupler, 1.0);
        }
        assert!(sim.output_total() > 25);
    }

    /// 長さ 2 m (2000) のベルトを 0.5 m/秒で流す。投入口 2 はベルトの始端に出し、終端から排出する。
    fn conveyor_config() -> (
        ConveyorConfig,
        std::collections::HashMap<u32, crate::conveyor::Bounds>,
    ) {
        use crate::conveyor::{BeltAxis, Bounds, Conveyor, Source, SourceOutput, WorkKind};
        use glam::DVec3;
        let config = ConveyorConfig {
            kinds: vec![WorkKind {
                id: 1,
                name: "ワーク".into(),
                size: [200.0, 100.0, 200.0],
                color: [1.0; 3],
            }],
            sources: vec![Source {
                id: 2,
                name: "投入口".into(),
                part: None,
                output: SourceOutput::Conveyor(3),
                interval: 1.0,
                kinds: vec![1],
                limit: None,
            }],
            conveyors: vec![Conveyor {
                speed: 500.0,
                part: Some(10),
                end: ConveyorEnd::Discharge,
                axis: BeltAxis::X,
                ..Conveyor::new(3)
            }],
            ..Default::default()
        };
        let parts = std::collections::HashMap::from([(
            10,
            Bounds {
                center: DVec3::new(0.0, 400.0, 0.0),
                size: DVec3::new(2000.0, 800.0, 400.0),
            },
        )]);
        (config, parts)
    }

    struct NoMotion;

    impl crate::conveyor::MachineMotion for NoMotion {
        fn gripper_state(
            &self,
            _: &crate::conveyor::Gripper,
            _: f64,
        ) -> Option<crate::conveyor::GripperState> {
            None
        }
    }

    #[test]
    fn conveyor_carries_work_from_one_process_to_the_next() {
        let (mut m, _, _) = setup(1);
        let pa = m.elements[1].id;
        let pb = m.elements[2].id;
        m.get_mut(pa).unwrap().carrier = None;
        m.get_mut(pa).unwrap().conveyor = Some(2);
        let mut sim = Simulation::new(&m).unwrap();
        sim.link(LineCoupler::carried_elements(&m));
        let (config, belts) = conveyor_config();
        assert!(conveyor_problems(&m, &config).is_empty());
        let inputs = ConveyorInputs {
            config: &config,
            parts: &belts,
            motion: &NoMotion,
        };
        let mut conveyor = ConveyorSim::default();
        conveyor.drive_sources([2]);
        let mut coupler = LineCoupler::new();
        let mut on_belt_seen = false;
        for _ in 0..600 {
            let parts = LineParts {
                flow: Some(&mut sim),
                agv: None,
                conveyor: Some(ConveyorLink {
                    sim: &mut conveyor,
                    inputs: &inputs,
                }),
            };
            step(&m, parts, &mut coupler, 1.0);
            on_belt_seen |= conveyor.works.iter().any(|w| w.tag.is_some());
        }
        // 工程A (10 秒) の後、ベルト 2 m を 0.5 m/秒で約 4 秒流れてから工程B に入る
        assert!(on_belt_seen);
        assert!(coupler.conveyed() >= 25, "運んだ数 {}", coupler.conveyed());
        assert!(sim.output_total() >= 25);
        assert_eq!(
            conveyor.counts.total_fed(),
            coupler.conveyed() + coupler.on_conveyor() as u32
        );
        // 工程B はコンベアから届いたワークを加工している
        assert!(sim.state_of(pb).is_some());
    }

    #[test]
    fn conveyor_problems_point_out_missing_sources_and_exits() {
        let (mut m, _, _) = setup(1);
        let pa = m.elements[1].id;
        m.get_mut(pa).unwrap().carrier = None;
        m.get_mut(pa).unwrap().conveyor = Some(99);
        let (mut config, _) = conveyor_config();
        assert_eq!(conveyor_problems(&m, &config).len(), 1);
        m.get_mut(pa).unwrap().conveyor = Some(2);
        config.conveyors[0].end = ConveyorEnd::Stopper;
        assert_eq!(conveyor_problems(&m, &config).len(), 1);
    }

    #[test]
    fn without_conveyor_simulation_transport_is_instant() {
        let (mut m, _, _) = setup(1);
        let pa = m.elements[1].id;
        m.get_mut(pa).unwrap().carrier = None;
        m.get_mut(pa).unwrap().conveyor = Some(2);
        let mut sim = Simulation::new(&m).unwrap();
        sim.link(LineCoupler::carried_elements(&m));
        let mut coupler = LineCoupler::new();
        for _ in 0..600 {
            step(&m, parts(Some(&mut sim), None), &mut coupler, 1.0);
        }
        assert!(sim.output_total() > 25);
    }
}
