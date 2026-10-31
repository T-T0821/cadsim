//! 状態と稼働率の色 (3D ビュー・グラフ・Excel で共通。Bevy に依存しない)

use super::sim::State;

/// 状態の色 (sRGB, 0〜1)
pub fn state_rgb(state: State) -> [f32; 3] {
    match state {
        State::Working => [0.30, 0.75, 0.35],
        State::Waiting => [0.95, 0.80, 0.25],
        State::Blocked => [0.95, 0.50, 0.15],
        State::Down => [0.85, 0.20, 0.20],
    }
}

/// 稼働率 (稼働 + 故障の割合, 0〜1) の色。低いと青、高いと赤。
/// ボトルネックは割合によらず濃い赤にして目立たせる。
pub fn busy_rgb(ratio: f64, bottleneck: bool) -> [f32; 3] {
    if bottleneck {
        return [0.85, 0.10, 0.10];
    }
    let t = ratio.clamp(0.0, 1.0) as f32;
    // 青 → 緑 → 黄 → 橙 と、割合が高いほど暖色にする
    const STOPS: [[f32; 3]; 4] = [
        [0.30, 0.50, 0.90],
        [0.30, 0.75, 0.40],
        [0.95, 0.85, 0.25],
        [0.95, 0.50, 0.15],
    ];
    let x = t * (STOPS.len() - 1) as f32;
    let i = (x.floor() as usize).min(STOPS.len() - 2);
    let f = x - i as f32;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_colors_run_from_blue_to_orange() {
        assert_eq!(busy_rgb(0.0, false), [0.30, 0.50, 0.90]);
        assert_eq!(busy_rgb(1.0, false), [0.95, 0.50, 0.15]);
        assert_eq!(busy_rgb(0.2, true), [0.85, 0.10, 0.10]);
    }
}
