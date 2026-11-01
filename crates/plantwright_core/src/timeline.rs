//! シミュレーションの巻き戻し: 途中の状態を記録しておき、好きな時刻に戻す (Bevy に依存しない)
//!
//! [`Timeline`] は「時刻 → そのときの状態の写し」を一定の間隔で取っておく入れ物。
//! 数が上限を超えたら1つおきに間引いて間隔を倍にするので、長く回しても使うメモリは一定で、
//! 最初から最後までどこへでも戻れる。
//!
//! 状態が決まった手順で進む (同じ状態から進めれば同じ結果になる) シミュレーションは
//! [`Replay`] を実装すると、[`Timeline::seek`] で「直前の写しから目的の時刻まで進め直す」ことで
//! 写しの間の時刻にもぴったり戻せる。外からの入力 (PLC の信号など) で進み方が変わるものは、
//! [`Timeline::at_or_before`] で直前の写しに戻し、そこから進めるときに
//! [`Timeline::truncate_after`] で先の記録を捨てる。

/// 決まった手順で進むシミュレーション
pub trait Replay {
    /// 今の時刻 (秒)
    fn now(&self) -> f64;
    /// 時刻 `time` まで進める (終わりの時刻で頭打ちにしてよい)
    fn advance_to(&mut self, time: f64);
}

/// 時刻ごとの状態の写し
#[derive(Clone, Debug)]
pub struct Timeline<S> {
    /// 写しを取る間隔 (秒)。間引くたびに倍になる。
    interval: f64,
    /// 取っておく写しの数の上限
    capacity: usize,
    /// 時刻の早い順
    snapshots: Vec<(f64, S)>,
}

impl<S: Clone> Timeline<S> {
    /// `interval` 秒ごとに写しを取り、`capacity` 個 (2 以上) まで取っておく
    pub fn new(interval: f64, capacity: usize) -> Self {
        Self {
            interval: interval.max(1e-9),
            capacity: capacity.max(2),
            snapshots: Vec::new(),
        }
    }

    /// 記録を全部消す (間隔は最初の値に戻さない)
    pub fn clear(&mut self) {
        self.snapshots.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }

    pub fn len(&self) -> usize {
        self.snapshots.len()
    }

    /// 今の写しの間隔 (秒)
    pub fn interval(&self) -> f64 {
        self.interval
    }

    /// 最初と最後の写しの時刻
    pub fn span(&self) -> Option<(f64, f64)> {
        Some((self.snapshots.first()?.0, self.snapshots.last()?.0))
    }

    /// いちばん後の写し
    pub fn last(&self) -> Option<&(f64, S)> {
        self.snapshots.last()
    }

    /// 次に写しを取る時刻 (まだ何もなければ None = すぐ取る)
    pub fn next_due(&self) -> Option<f64> {
        self.snapshots.last().map(|(t, _)| t + self.interval)
    }

    /// 時刻 `time` の状態を、前の写しから間隔以上たっていれば取っておく。取ったら true。
    /// 最後の写しより前の時刻 (巻き戻して進め直している途中) では取らない。
    pub fn record(&mut self, time: f64, state: &S) -> bool {
        if self.next_due().is_some_and(|due| time < due - 1e-9) {
            return false;
        }
        self.snapshots.push((time, state.clone()));
        if self.snapshots.len() > self.capacity {
            self.thin();
        }
        true
    }

    /// 1つおきに捨てて間隔を倍にする (最初と最後の写しは残す)
    fn thin(&mut self) {
        let last = self.snapshots.len() - 1;
        let mut i = 0;
        self.snapshots.retain(|_| {
            let keep = i % 2 == 0 || i == last;
            i += 1;
            keep
        });
        self.interval *= 2.0;
    }

    /// 時刻 `time` かそれより前で、いちばん近い写し
    pub fn at_or_before(&self, time: f64) -> Option<&(f64, S)> {
        let n = self.snapshots.partition_point(|(t, _)| *t <= time + 1e-9);
        n.checked_sub(1).map(|i| &self.snapshots[i])
    }

    /// 時刻 `time` より後の写しを捨てる (戻した時点から別の進み方をするとき)
    pub fn truncate_after(&mut self, time: f64) {
        let n = self.snapshots.partition_point(|(t, _)| *t <= time + 1e-9);
        self.snapshots.truncate(n);
    }
}

impl<S: Clone + Replay> Timeline<S> {
    /// `sim` を時刻 `until` まで進めながら写しを取る
    pub fn advance_recording(&mut self, sim: &mut S, until: f64) {
        self.record(sim.now(), sim);
        loop {
            let now = sim.now();
            if now >= until - 1e-9 {
                break;
            }
            let target = self.next_due().map_or(until, |due| due.min(until));
            sim.advance_to(target.max(now));
            if sim.now() <= now + 1e-12 {
                // 終わりの時刻に着いた (それ以上進まない)
                break;
            }
            self.record(sim.now(), sim);
        }
    }

    /// `sim` を時刻 `time` の状態にする。戻るときは直前の写しから、先へ行くときは
    /// 今の状態か、それより近い写しから進め直す (進めた分も写しを取る)。
    pub fn seek(&mut self, sim: &mut S, time: f64) {
        let now = sim.now();
        if let Some((t, snap)) = self.at_or_before(time)
            && (time < now - 1e-9 || *t > now)
        {
            *sim = snap.clone();
        }
        self.advance_recording(sim, time);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 時刻と、時刻から決まる値を持つだけのシミュレーション
    #[derive(Clone, Debug, PartialEq)]
    struct Counter {
        now: f64,
        end: f64,
        /// 進めた回数 (戻したことが分かるように)
        steps: u32,
    }

    impl Replay for Counter {
        fn now(&self) -> f64 {
            self.now
        }
        fn advance_to(&mut self, time: f64) {
            let time = time.min(self.end);
            if time > self.now {
                self.now = time;
                self.steps += 1;
            }
        }
    }

    fn counter(end: f64) -> Counter {
        Counter {
            now: 0.0,
            end,
            steps: 0,
        }
    }

    #[test]
    fn records_at_the_interval() {
        let mut tl = Timeline::new(10.0, 100);
        assert!(tl.record(0.0, &0));
        assert!(!tl.record(5.0, &5));
        assert!(tl.record(10.0, &10));
        assert!(tl.record(25.0, &25));
        assert_eq!(tl.len(), 3);
        assert_eq!(tl.span(), Some((0.0, 25.0)));
        assert_eq!(tl.next_due(), Some(35.0));
    }

    #[test]
    fn finds_the_snapshot_at_or_before_a_time() {
        let mut tl = Timeline::new(10.0, 100);
        for t in [0.0, 10.0, 20.0] {
            tl.record(t, &(t as i32));
        }
        assert_eq!(tl.at_or_before(15.0).map(|s| s.1), Some(10));
        assert_eq!(tl.at_or_before(20.0).map(|s| s.1), Some(20));
        assert_eq!(tl.at_or_before(99.0).map(|s| s.1), Some(20));
        assert!(tl.at_or_before(-1.0).is_none());
    }

    #[test]
    fn thins_out_when_full_and_keeps_the_whole_span() {
        let mut tl = Timeline::new(1.0, 10);
        for i in 0..=100 {
            tl.record(i as f64, &i);
        }
        assert!(tl.len() <= 10);
        let (first, last) = tl.span().unwrap();
        assert_eq!(first, 0.0);
        assert!(last >= 90.0, "最後の写し {last}");
        assert!(tl.interval() > 1.0);
    }

    #[test]
    fn truncates_the_future() {
        let mut tl = Timeline::new(1.0, 100);
        for i in 0..10 {
            tl.record(i as f64, &i);
        }
        tl.truncate_after(4.5);
        assert_eq!(tl.span(), Some((0.0, 4.0)));
        // 捨てたあとは、その先から記録し直せる
        assert!(tl.record(5.0, &5));
    }

    #[test]
    fn advance_recording_takes_snapshots_along_the_way() {
        let mut tl = Timeline::new(10.0, 100);
        let mut sim = counter(100.0);
        tl.advance_recording(&mut sim, 45.0);
        assert_eq!(sim.now, 45.0);
        assert_eq!(tl.span(), Some((0.0, 40.0)));
        assert_eq!(tl.len(), 5);
        // 終わりの時刻より先を頼んでも止まる
        tl.advance_recording(&mut sim, 1e9);
        assert_eq!(sim.now, 100.0);
        assert_eq!(tl.span().unwrap().1, 100.0);
    }

    #[test]
    fn seek_back_restores_and_replays_to_the_exact_time() {
        let mut tl = Timeline::new(10.0, 100);
        let mut sim = counter(100.0);
        tl.advance_recording(&mut sim, 80.0);
        let recorded = tl.len();
        tl.seek(&mut sim, 33.0);
        assert_eq!(sim.now, 33.0);
        // 30 秒の写しから 1 回進めただけ
        let from_30 = tl.at_or_before(30.0).unwrap().1.steps;
        assert_eq!(sim.steps, from_30 + 1);
        // 巻き戻しても先の記録は残り、また先へ行ける
        assert_eq!(tl.len(), recorded);
        tl.seek(&mut sim, 75.0);
        assert_eq!(sim.now, 75.0);
        let from_70 = tl.at_or_before(70.0).unwrap().1.steps;
        assert_eq!(sim.steps, from_70 + 1);
    }

    #[test]
    fn seek_forward_beyond_the_record_keeps_recording() {
        let mut tl = Timeline::new(10.0, 100);
        let mut sim = counter(100.0);
        tl.advance_recording(&mut sim, 20.0);
        tl.seek(&mut sim, 55.0);
        assert_eq!(sim.now, 55.0);
        assert_eq!(tl.span(), Some((0.0, 50.0)));
    }
}
