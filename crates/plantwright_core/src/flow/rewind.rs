//! 工程シミュレーションの巻き戻し (Bevy に依存しない)
//!
//! 工程シミュレーションは乱数の種も状態に持っているので、同じ状態から進めれば同じ結果になる。
//! そこで回しながら途中の状態を [`Timeline`] に取っておき、時間スライダーで戻すときは
//! 直前の写しから目的の時刻まで進め直す (写しの間の時刻にもぴったり戻せる)。
//! 戻した時点から再生しても同じ未来になるので、先の記録はそのまま使える。
//!
//! 詰まりの原因を調べやすいように、推移の記録から「どこかの要素が詰まり始めた直前の時刻」を探す。

use super::model::{ElementId, FlowModel, RunSettings};
use super::sim::{Sample, Simulation, State};
use crate::timeline::{Replay, Timeline};

/// 回す時間のうちに取る写しのおおよその数
const SNAPSHOTS_PER_RUN: f64 = 400.0;
/// 写しの数の上限 (超えたら間引く)
const CAPACITY: usize = 512;

impl Replay for Simulation {
    fn now(&self) -> f64 {
        Simulation::now(self)
    }

    fn advance_to(&mut self, time: f64) {
        self.advance(time, usize::MAX);
    }
}

/// 工程シミュレーションの写しの記録
pub type FlowTimeline = Timeline<Simulation>;

/// この設定で回すときの記録の入れ物 (回す時間を 400 ほどに分けて写しを取る)
pub fn timeline_for(settings: RunSettings) -> FlowTimeline {
    Timeline::new(settings.duration / SNAPSHOTS_PER_RUN, CAPACITY)
}

/// 詰まり始めたところ
#[derive(Clone, Debug, PartialEq)]
pub struct JamMark {
    /// 詰まる直前の記録の時刻 (ここへ戻すと、詰まる前の様子から見られる)
    pub before: f64,
    /// 詰まっているのが記録された時刻
    pub at: f64,
    /// その間に詰まり始めた要素
    pub elements: Vec<ElementId>,
}

/// 推移の記録から、詰まり始めたところを時刻の早い順に並べる
pub fn jam_marks(model: &FlowModel, samples: &[Sample]) -> Vec<JamMark> {
    samples
        .windows(2)
        .filter_map(|w| {
            let (prev, cur) = (&w[0], &w[1]);
            let elements: Vec<ElementId> = cur
                .states
                .iter()
                .enumerate()
                .filter(|&(i, s)| {
                    *s == State::Blocked && prev.states.get(i) != Some(&State::Blocked)
                })
                .filter_map(|(i, _)| model.elements.get(i).map(|e| e.id))
                .collect();
            (!elements.is_empty()).then_some(JamMark {
                before: prev.time,
                at: cur.time,
                elements,
            })
        })
        .collect()
}

/// 時刻 `time` より前で、いちばん近い詰まり始め
pub fn previous_jam(model: &FlowModel, samples: &[Sample], time: f64) -> Option<JamMark> {
    jam_marks(model, samples)
        .into_iter()
        .rev()
        .find(|m| m.before < time - 1e-6)
}

/// 時刻 `time` より後で、いちばん近い詰まり始め
pub fn next_jam(model: &FlowModel, samples: &[Sample], time: f64) -> Option<JamMark> {
    jam_marks(model, samples)
        .into_iter()
        .find(|m| m.before > time + 1e-6)
}

#[cfg(test)]
mod tests {
    use super::super::model::sample_line;
    use super::*;

    fn sample(time: f64, states: &[State]) -> Sample {
        Sample {
            time,
            wip: 0,
            output: 0,
            states: states.to_vec(),
        }
    }

    #[test]
    fn seeking_back_gives_the_same_state_as_running_to_that_time() {
        let model = sample_line();
        let mut sim = Simulation::new(&model).unwrap();
        let mut timeline = timeline_for(model.settings);
        timeline.advance_recording(&mut sim, 3.0 * 3600.0);
        let end_report = sim.report();

        let t = 1234.5;
        timeline.seek(&mut sim, t);
        let mut fresh = Simulation::new(&model).unwrap();
        fresh.advance(t, usize::MAX);
        assert_eq!(sim.now(), t);
        assert_eq!(sim.report(), fresh.report());

        // 戻した時点から先へ進め直しても、同じ結果になる
        timeline.seek(&mut sim, 3.0 * 3600.0);
        assert_eq!(sim.report(), end_report);
    }

    #[test]
    fn recording_a_whole_run_keeps_the_memory_bounded() {
        let model = sample_line();
        let mut sim = Simulation::new(&model).unwrap();
        let mut timeline = timeline_for(model.settings);
        timeline.advance_recording(&mut sim, f64::INFINITY);
        assert!(sim.finished());
        assert!(timeline.len() <= CAPACITY);
        assert_eq!(timeline.span(), Some((0.0, model.settings.duration)));
    }

    #[test]
    fn finds_where_jams_begin() {
        let model = sample_line();
        let ids: Vec<ElementId> = model.elements.iter().map(|e| e.id).collect();
        let w = State::Working;
        let b = State::Blocked;
        let samples = [
            sample(0.0, &[w, w, w, w, w]),
            sample(10.0, &[w, b, w, w, w]),
            sample(20.0, &[w, b, w, w, w]),
            sample(30.0, &[w, w, w, w, w]),
            sample(40.0, &[b, b, w, w, w]),
        ];
        let marks = jam_marks(&model, &samples);
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[0].before, 0.0);
        assert_eq!(marks[0].at, 10.0);
        assert_eq!(marks[0].elements, vec![ids[1]]);
        assert_eq!(marks[1].before, 30.0);
        assert_eq!(marks[1].elements, vec![ids[0], ids[1]]);

        assert_eq!(previous_jam(&model, &samples, 35.0).unwrap().before, 30.0);
        // 詰まりの直前に戻ったあと、もう一度押すとその前の詰まりへ
        assert_eq!(previous_jam(&model, &samples, 30.0).unwrap().before, 0.0);
        assert!(previous_jam(&model, &samples, 0.0).is_none());
        assert_eq!(next_jam(&model, &samples, 0.0).unwrap().before, 30.0);
        assert!(next_jam(&model, &samples, 30.0).is_none());
    }
}
