//! 工程シミュレーションと AGV・タイミングチャートのつなぎ
//!
//! 流れ:
//! 1. 工程シミュレーションで、AGV で送り出す要素 ([`Element::carrier`]) にワークができると
//!    [`FlowEvent::CarrierNeeded`] が出る → AGV に搬送を頼む ([`LineCommand::RequestAgv`])。
//! 2. AGV が積み込みを終えると [`AgvEvent::PickedUp`] → 工程シミュレーションからワークを取り出す
//!    ([`Simulation::pick_up`])。空いた設備には上流から次のワークが入る。
//! 3. AGV が荷降ろしを終えると [`AgvEvent::Delivered`] → 送り先の要素へ入れる
//!    ([`Simulation::deliver`])。
//!
//! 降ろし先へ行けずに AGV が搬送をあきらめたとき ([`AgvEvent::Abandoned`]) は、
//! ワークがなくならないように、その場で送り先へ入れる。
//!
//! [`Element::carrier`]: crate::flow::model::Element::carrier

use std::collections::BTreeMap;

use crate::flow::model::{ElementId, ElementKind, FlowModel};
use crate::flow::sim::{FlowEvent, LoadId, Simulation};

/// AGV シミュレーションから届く出来事。`tag` は [`LineCommand::RequestAgv`] で渡したもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgvEvent {
    /// 積み込みが終わった
    PickedUp { tag: u64 },
    /// 荷降ろしが終わった
    Delivered { tag: u64 },
    /// 降ろし先へ行けないので搬送をやめた
    Abandoned { tag: u64 },
}

/// ラインの外 (AGV・タイミングチャート) にしてほしいこと
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineCommand {
    /// AGV に `from` で積んで `to` で降ろす搬送を頼む
    RequestAgv { from: u32, to: u32, tag: u64 },
    /// タイミングチャートを頭から再生する
    RestartChart,
}

/// 設備から設備へ渡っている途中のワーク (3D の表示用)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transit {
    pub from: ElementId,
    pub to: ElementId,
    /// 渡し始めた時刻
    pub start: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Job {
    /// AGV を頼んだが、まだ積んでいない
    Requested { element: ElementId },
    /// 積んで運んでいる
    Loaded { element: ElementId, load: LoadId },
}

/// ライン連携の実行中の状態
#[derive(Clone, Debug, Default)]
pub struct LineCoupler {
    jobs: BTreeMap<u64, Job>,
    next_tag: u64,
    transits: Vec<Transit>,
    /// AGV が運んだワークの数
    carried: u32,
}

impl LineCoupler {
    pub fn new() -> Self {
        Self::default()
    }

    /// AGV で送り出す要素の一覧 ([`Simulation::link`] に渡す)
    pub fn carried_elements(model: &FlowModel) -> Vec<ElementId> {
        model
            .elements
            .iter()
            .filter(|e| e.carrier.is_some() && !matches!(e.kind, ElementKind::Sink))
            .map(|e| e.id)
            .collect()
    }

    /// 工程シミュレーションの出来事を受け取り、外にしてほしいことを返す
    pub fn on_flow_events(
        &mut self,
        model: &FlowModel,
        events: impl IntoIterator<Item = FlowEvent>,
    ) -> Vec<LineCommand> {
        let mut out = Vec::new();
        for event in events {
            match event {
                FlowEvent::CarrierNeeded { element, .. } => {
                    let Some(call) = model.get(element).and_then(|e| e.carrier) else {
                        continue;
                    };
                    let tag = self.next_tag;
                    self.next_tag += 1;
                    self.jobs.insert(tag, Job::Requested { element });
                    out.push(LineCommand::RequestAgv {
                        from: call.from,
                        to: call.to,
                        tag,
                    });
                }
                FlowEvent::Moved {
                    time,
                    from,
                    to,
                    carried,
                } => {
                    if !carried {
                        self.transits.push(Transit {
                            from,
                            to,
                            start: time,
                        });
                    }
                    let starts = model.get(to).is_some_and(|e| {
                        e.starts_chart && matches!(e.kind, ElementKind::Process { .. })
                    });
                    if starts && !out.contains(&LineCommand::RestartChart) {
                        out.push(LineCommand::RestartChart);
                    }
                }
            }
        }
        out
    }

    /// AGV の出来事を工程シミュレーションに伝える。その結果の工程シミュレーションの出来事は
    /// [`Simulation::take_events`] で取り出して [`LineCoupler::on_flow_events`] に渡す。
    pub fn on_agv_event(&mut self, sim: &mut Simulation, event: AgvEvent) {
        match event {
            AgvEvent::PickedUp { tag } => {
                let Some(Job::Requested { element }) = self.jobs.get(&tag).copied() else {
                    return;
                };
                match sim.pick_up(element) {
                    Some(load) => {
                        self.jobs.insert(tag, Job::Loaded { element, load });
                    }
                    // 積むワークがない (起こらないはず): 空で走らせる
                    None => {
                        self.jobs.remove(&tag);
                    }
                }
            }
            AgvEvent::Delivered { tag } | AgvEvent::Abandoned { tag } => {
                match self.jobs.remove(&tag) {
                    Some(Job::Loaded { load, .. }) => {
                        if sim.deliver(load) {
                            self.carried += 1;
                        }
                    }
                    // 積む前にあきらめた: ワークはまだ設備にあるので、すぐ積んで降ろす
                    Some(Job::Requested { element }) => {
                        if let Some(load) = sim.pick_up(element) {
                            sim.deliver(load);
                        }
                    }
                    None => {}
                }
            }
        }
    }

    /// AGV に積まれているワークの荷札 (3D で AGV の上に描く)
    pub fn is_loaded(&self, tag: u64) -> bool {
        matches!(self.jobs.get(&tag), Some(Job::Loaded { .. }))
    }

    /// AGV を頼んでまだ積んでいない数
    pub fn waiting_for_agv(&self) -> usize {
        self.jobs
            .values()
            .filter(|j| matches!(j, Job::Requested { .. }))
            .count()
    }

    /// AGV が運び終えたワークの数
    pub fn carried(&self) -> u32 {
        self.carried
    }

    /// 時刻 `now` に渡っている途中のワークと、その進み (0〜1)。
    /// 渡すのにかかる見た目の時間は `duration` 秒。終わったものは捨てる。
    pub fn transits(&mut self, now: f64, duration: f64) -> Vec<(Transit, f64)> {
        let duration = duration.max(1e-6);
        self.transits.retain(|t| now - t.start < duration);
        self.transits
            .iter()
            .map(|t| (*t, ((now - t.start) / duration).clamp(0.0, 1.0)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::model::{AgvCall, KindTag};

    /// 生成 → 工程1 (AGV で送り出す) → 工程2 → 排出
    fn model() -> (FlowModel, ElementId, ElementId) {
        let mut m = FlowModel::default();
        let s = m.append(KindTag::Source);
        if let ElementKind::Source { interval, .. } = &mut m.get_mut(s).unwrap().kind {
            *interval = 10.0;
        }
        let p1 = m.append(KindTag::Process);
        let p2 = m.append(KindTag::Process);
        for (p, c) in [(p1, 5.0), (p2, 5.0)] {
            if let ElementKind::Process { cycle_time, .. } = &mut m.get_mut(p).unwrap().kind {
                *cycle_time = c;
            }
        }
        m.append(KindTag::Sink);
        m.get_mut(p1).unwrap().carrier = Some(AgvCall { from: 1, to: 2 });
        m.get_mut(p2).unwrap().starts_chart = true;
        m.settings.duration = 3600.0;
        (m, p1, p2)
    }

    fn linked(m: &FlowModel) -> Simulation {
        let mut sim = Simulation::new(m).unwrap();
        sim.link(LineCoupler::carried_elements(m));
        sim
    }

    #[test]
    fn work_waits_for_the_agv_and_moves_on_after_delivery() {
        let (m, p1, p2) = model();
        let mut sim = linked(&m);
        let mut coupler = LineCoupler::new();

        // 工程1 の加工が終わる (t=5) と AGV を頼む
        sim.advance(6.0, usize::MAX);
        let commands = coupler.on_flow_events(&m, sim.take_events());
        assert_eq!(
            commands,
            vec![LineCommand::RequestAgv {
                from: 1,
                to: 2,
                tag: 0
            }]
        );
        assert_eq!(coupler.waiting_for_agv(), 1);
        // AGV が来るまで工程1 は詰まり、工程2 は空のまま
        sim.advance(30.0, usize::MAX);
        coupler.on_flow_events(&m, sim.take_events());
        assert_eq!(sim.state_of(p1), Some(crate::flow::sim::State::Blocked));
        assert_eq!(sim.content_of(p2), Some(0));

        // 積み込み: 工程1 が空き、次のワークを受け取る
        coupler.on_agv_event(&mut sim, AgvEvent::PickedUp { tag: 0 });
        assert!(coupler.is_loaded(0));
        assert_eq!(sim.in_transit(), 1);
        assert_eq!(sim.content_of(p1), Some(1));
        coupler.on_flow_events(&m, sim.take_events());

        // 荷降ろし: 工程2 に入り、チャートを頭から再生する
        sim.advance(40.0, usize::MAX);
        coupler.on_flow_events(&m, sim.take_events());
        coupler.on_agv_event(&mut sim, AgvEvent::Delivered { tag: 0 });
        assert_eq!(sim.in_transit(), 0);
        assert_eq!(sim.content_of(p2), Some(1));
        assert_eq!(coupler.carried(), 1);
        let commands = coupler.on_flow_events(&m, sim.take_events());
        assert!(commands.contains(&LineCommand::RestartChart));
    }

    #[test]
    fn without_agv_the_carried_station_blocks_the_line() {
        let (m, _, _) = model();
        let mut sim = linked(&m);
        sim.advance(3600.0, usize::MAX);
        // 頼んだ搬送が来ないので、排出まで届くワークはない
        assert_eq!(sim.output_total(), 0);

        // 連携しなければ同じモデルでもワークは流れる
        let report = Simulation::run(&m).unwrap();
        assert!(report.output > 300);
    }

    #[test]
    fn abandoned_job_still_delivers_the_work() {
        let (m, _, p2) = model();
        let mut sim = linked(&m);
        let mut coupler = LineCoupler::new();
        sim.advance(6.0, usize::MAX);
        coupler.on_flow_events(&m, sim.take_events());
        coupler.on_agv_event(&mut sim, AgvEvent::Abandoned { tag: 0 });
        assert_eq!(sim.content_of(p2), Some(1));
    }

    #[test]
    fn plain_moves_become_transits_that_expire() {
        let (m, _, _) = model();
        let mut sim = linked(&m);
        let mut coupler = LineCoupler::new();
        sim.advance(0.5, usize::MAX);
        coupler.on_flow_events(&m, sim.take_events());
        let t = coupler.transits(0.5, 1.0);
        assert_eq!(t.len(), 1);
        assert!((t[0].1 - 0.5).abs() < 1e-9);
        assert!(coupler.transits(2.0, 1.0).is_empty());
    }

    #[test]
    fn held_source_does_not_release_work_until_enabled() {
        let (m, _, _) = model();
        let mut sim = linked(&m);
        let source = m.elements[0].id;
        sim.set_source_enabled(source, false);
        assert!(!sim.source_enabled(source));
        sim.advance(100.0, usize::MAX);
        // 最初のワーク (t=0) から止めているので、何も出ない
        assert_eq!(sim.wip(), 0);
        sim.set_source_enabled(source, true);
        sim.advance(101.0, usize::MAX);
        assert_eq!(sim.wip(), 1);
    }
}
