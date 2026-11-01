//! 警報の理由とブザー停止
//!
//! 異常が続いている間は警報を鳴らし続ける。「ブザー停止」を押すと今ある異常の分は止め、
//! 別の異常が起きたか、一度直ってからまた起きたときに鳴らし直す (設備の表示灯と同じ考え方)。

use std::collections::BTreeSet;

/// 警報を鳴らす理由
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AlarmReason {
    /// 安全機器の検知や非常停止で止まった
    SafetyStop,
    /// インターロックの条件が成立せず、タイミングチャートが止まった
    InterlockStop,
}

impl AlarmReason {
    pub fn label(self) -> &'static str {
        match self {
            AlarmReason::SafetyStop => "安全停止",
            AlarmReason::InterlockStop => "インターロック停止",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlarmLatch {
    active: BTreeSet<AlarmReason>,
    acknowledged: BTreeSet<AlarmReason>,
}

impl AlarmLatch {
    /// いま起きている異常を伝える。直った異常は確認済みからも外す。
    pub fn update(&mut self, active: impl IntoIterator<Item = AlarmReason>) {
        self.active = active.into_iter().collect();
        let active = &self.active;
        self.acknowledged.retain(|r| active.contains(r));
    }

    /// ブザー停止: 今ある異常の分の警報を止める
    pub fn acknowledge(&mut self) {
        self.acknowledged = self.active.clone();
    }

    /// 警報を鳴らすか (確認していない異常があるか)
    pub fn sounding(&self) -> bool {
        self.active.iter().any(|r| !self.acknowledged.contains(r))
    }

    /// 起きている異常 (確認したものも含む)
    pub fn active(&self) -> impl Iterator<Item = AlarmReason> + '_ {
        self.active.iter().copied()
    }

    pub fn is_active(&self) -> bool {
        !self.active.is_empty()
    }
}

/// インターロックで止まったチャートの再生が、止まった時刻まで来て止まっているか
pub fn interlock_stopped(stop_time: Option<f64>, time: f64, playing: bool) -> bool {
    stop_time.is_some_and(|stop| !playing && time >= stop - 1e-9)
}

/// 干渉の数を見て、設備が動いている間に干渉が増えたときだけ知らせる
/// (止まっている間に部品を置き直して干渉しても鳴らさない)
#[derive(Clone, Debug, Default)]
pub struct InterferenceWatch {
    last: usize,
}

impl InterferenceWatch {
    /// 干渉が増えたフレームで true
    pub fn update(&mut self, interfering: usize, running: bool) -> bool {
        let began = running && interfering > self.last;
        self.last = interfering;
        began
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AlarmReason::*;

    #[test]
    fn sounds_while_a_fault_is_active() {
        let mut latch = AlarmLatch::default();
        assert!(!latch.sounding());
        latch.update([SafetyStop]);
        assert!(latch.sounding());
        latch.update([]);
        assert!(!latch.sounding());
    }

    #[test]
    fn acknowledge_silences_until_a_new_fault() {
        let mut latch = AlarmLatch::default();
        latch.update([SafetyStop]);
        latch.acknowledge();
        assert!(!latch.sounding());
        assert!(latch.is_active());
        // 同じ異常が続く間は鳴らさない
        latch.update([SafetyStop]);
        assert!(!latch.sounding());
        // 別の異常が加わったら鳴らす
        latch.update([SafetyStop, InterlockStop]);
        assert!(latch.sounding());
    }

    #[test]
    fn a_fault_that_clears_and_returns_sounds_again() {
        let mut latch = AlarmLatch::default();
        latch.update([InterlockStop]);
        latch.acknowledge();
        latch.update([]);
        latch.update([InterlockStop]);
        assert!(latch.sounding());
    }

    #[test]
    fn interlock_alarm_waits_until_playback_reaches_the_stop() {
        assert!(!interlock_stopped(None, 5.0, false));
        assert!(!interlock_stopped(Some(3.0), 1.0, true));
        assert!(
            !interlock_stopped(Some(3.0), 1.0, false),
            "止まる前に一時停止しただけ"
        );
        assert!(interlock_stopped(Some(3.0), 3.0, false));
    }

    #[test]
    fn interference_beeps_only_when_it_grows_while_running() {
        let mut watch = InterferenceWatch::default();
        assert!(!watch.update(1, false), "止まっている間は鳴らさない");
        assert!(!watch.update(1, true), "前からある干渉では鳴らさない");
        assert!(watch.update(2, true));
        assert!(!watch.update(2, true));
        assert!(!watch.update(0, true));
        assert!(watch.update(1, true));
    }
}
