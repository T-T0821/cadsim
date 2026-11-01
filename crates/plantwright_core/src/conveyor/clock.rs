//! 再生位置 (チャート上の時刻) から、サイクルをまたいで増え続ける経過時刻を求める

/// 繰り返し再生で何サイクル目かを数える。ワークはサイクルをまたいでも残るため、
/// 計算にはチャート上の時刻ではなく「最初のサイクルの先頭からの経過時刻」を使う。
#[derive(Clone, Copy, Debug, Default)]
pub struct CycleClock {
    cycles: u32,
    last_time: f64,
}

impl CycleClock {
    /// 再生状態を受け取り、経過時刻を返す。毎フレーム呼ぶ。
    ///
    /// - 止まっていて先頭 (時刻 0) にあるときは、最初のサイクルに戻す
    /// - 再生中に時刻がサイクルの半分以上戻ったら、繰り返しで先頭に戻ったとみなす
    pub fn elapsed(&mut self, chart_time: f64, playing: bool, cycle_time: f64) -> f64 {
        if !playing && chart_time == 0.0 {
            self.cycles = 0;
        } else if playing && self.last_time - chart_time > cycle_time / 2.0 {
            self.cycles += 1;
        }
        self.last_time = chart_time;
        f64::from(self.cycles) * cycle_time + chart_time
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_loops_and_resets_at_the_start() {
        let mut clock = CycleClock::default();
        assert_eq!(clock.elapsed(1.0, true, 4.0), 1.0);
        assert_eq!(clock.elapsed(3.9, true, 4.0), 3.9);
        // 繰り返しで先頭に戻った
        assert_eq!(clock.elapsed(0.1, true, 4.0), 4.1);
        // 一時停止して少し戻しても、同じサイクルのまま
        assert_eq!(clock.elapsed(0.05, false, 4.0), 4.05);
        // 先頭へ戻すと最初のサイクル
        assert_eq!(clock.elapsed(0.0, false, 4.0), 0.0);
    }
}
