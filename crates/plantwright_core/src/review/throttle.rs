//! 送る回数を抑える
//!
//! 視点やマウスの位置は毎フレーム変わるので、全部送ると回線を埋めてしまう。
//! 最後に送った値から変わっていて、前に送ってから少し時間がたったときだけ送る。
//! 最後に「送った」値と比べるので、動きが止まった直後の値も次の機会に必ず届く。

/// 送る間隔の既定 (秒)。1 秒に 20 回まで
pub const DEFAULT_INTERVAL: f64 = 0.05;

#[derive(Clone, Debug)]
pub struct SendThrottle<T> {
    interval: f64,
    last: Option<(T, f64)>,
}

impl<T: Clone> Default for SendThrottle<T> {
    fn default() -> Self {
        Self::new(DEFAULT_INTERVAL)
    }
}

impl<T: Clone> SendThrottle<T> {
    pub fn new(interval: f64) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    /// `value` をいま送るべきか。送るなら覚えて true を返す。
    /// `changed(新, 前)` は送り直すほど変わったか。
    pub fn offer(&mut self, value: &T, now: f64, changed: impl Fn(&T, &T) -> bool) -> bool {
        if let Some((last, at)) = &self.last
            && (now - at < self.interval || !changed(value, last))
        {
            return false;
        }
        self.last = Some((value.clone(), now));
        true
    }

    /// 送ったことを忘れる (つなぎ直したときに今の値をすぐ送るため)
    pub fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn differs(a: &i32, b: &i32) -> bool {
        a != b
    }

    #[test]
    fn sends_first_value_then_waits_for_interval_and_change() {
        let mut t = SendThrottle::new(0.05);
        assert!(t.offer(&1, 0.0, differs));
        assert!(!t.offer(&2, 0.01, differs), "間隔が短い");
        assert!(!t.offer(&1, 0.10, differs), "変わっていない");
        assert!(t.offer(&2, 0.10, differs));
    }

    #[test]
    fn last_value_after_motion_stops_is_still_sent() {
        let mut t = SendThrottle::new(0.05);
        assert!(t.offer(&1, 0.0, differs));
        // 間隔内に 2 → 3 と動いて止まった。止まった値 3 が次の機会に送られる
        assert!(!t.offer(&3, 0.02, differs));
        assert!(t.offer(&3, 0.06, differs));
        t.reset();
        assert!(t.offer(&3, 0.07, differs));
    }
}
