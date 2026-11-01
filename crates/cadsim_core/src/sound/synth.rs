//! 音の波形をその場で作る
//!
//! 録音した音のファイルは使わず、正弦波と雑音を組み合わせて作る (使用許諾の心配がなく、
//! 実行ファイルも大きくならない)。くり返し鳴らす音は、最後と最初がつながるように作る。
//! 1 チャンネル (モノラル)、[`SAMPLE_RATE`] Hz、値は -1〜1。

use std::f32::consts::TAU;

use super::settings::SoundCategory;

/// 1 秒あたりのサンプル数
pub const SAMPLE_RATE: u32 = 44_100;

/// 鳴らす音の種類
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SoundKind {
    /// 回転テーブルなどのモーターのうなり (くり返し)
    MotorHum,
    /// 直動軸 (サーボ) の高めの駆動音 (くり返し)
    ServoWhine,
    /// コンベアのベルトとローラーの音 (くり返し)
    ConveyorRumble,
    /// エアシリンダの排気「プシュッ」(1 回)
    AirExhaust,
    /// 停止したときの警報 (2 つの高さを交互に鳴らす。くり返し)
    Alarm,
    /// 干渉したときの短い「ピピッ」(1 回)
    WarningBeep,
}

impl SoundKind {
    pub const ALL: [SoundKind; 6] = [
        SoundKind::MotorHum,
        SoundKind::ServoWhine,
        SoundKind::ConveyorRumble,
        SoundKind::AirExhaust,
        SoundKind::Alarm,
        SoundKind::WarningBeep,
    ];

    /// くり返し鳴らす音か
    pub fn looping(self) -> bool {
        matches!(
            self,
            SoundKind::MotorHum
                | SoundKind::ServoWhine
                | SoundKind::ConveyorRumble
                | SoundKind::Alarm
        )
    }

    /// 音量のつまみの分け方
    pub fn category(self) -> SoundCategory {
        match self {
            SoundKind::MotorHum | SoundKind::ServoWhine | SoundKind::ConveyorRumble => {
                SoundCategory::Machine
            }
            SoundKind::AirExhaust => SoundCategory::Air,
            SoundKind::Alarm | SoundKind::WarningBeep => SoundCategory::Alarm,
        }
    }

    /// 種類ごとの基本の大きさ (警報は目立たせ、稼働音は控えめにする)
    pub fn base_gain(self) -> f32 {
        match self {
            SoundKind::MotorHum => 0.35,
            SoundKind::ServoWhine => 0.3,
            SoundKind::ConveyorRumble => 0.3,
            SoundKind::AirExhaust => 0.6,
            SoundKind::Alarm => 0.7,
            SoundKind::WarningBeep => 0.6,
        }
    }

    /// 長さ (秒)。くり返す音は 1 回分の長さ。
    fn seconds(self) -> f32 {
        match self {
            SoundKind::MotorHum | SoundKind::ServoWhine | SoundKind::Alarm => 1.0,
            SoundKind::ConveyorRumble => 2.0,
            SoundKind::AirExhaust => 0.45,
            SoundKind::WarningBeep => 0.32,
        }
    }
}

/// 音の波形を作る。同じ種類なら毎回同じ波形になる。
pub fn synthesize(kind: SoundKind) -> Vec<f32> {
    let n = (kind.seconds() * SAMPLE_RATE as f32) as usize;
    let mut samples = match kind {
        SoundKind::MotorHum => motor_hum(n),
        SoundKind::ServoWhine => servo_whine(n),
        SoundKind::ConveyorRumble => conveyor_rumble(n),
        SoundKind::AirExhaust => air_exhaust(n),
        SoundKind::Alarm => alarm(n),
        SoundKind::WarningBeep => warning_beep(n),
    };
    normalize(&mut samples, 0.8);
    samples
}

fn time(i: usize) -> f32 {
    i as f32 / SAMPLE_RATE as f32
}

fn sine(freq: f32, t: f32) -> f32 {
    (TAU * freq * t).sin()
}

/// 決まった順に値を出す雑音 (xorshift)。毎回同じ音にするため、乱数の種を固定する。
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// 1 次の低域通過 (ならし)。`a` が小さいほど低い音だけ残る。
fn low_pass(input: &[f32], a: f32) -> Vec<f32> {
    let mut y = 0.0;
    input
        .iter()
        .map(|&x| {
            y += a * (x - y);
            y
        })
        .collect()
}

/// 雑音のくり返しの継ぎ目を目立たなくする: 少し長く作り、はみ出した分を先頭に重ねて消していく
fn seamless(long: Vec<f32>, n: usize) -> Vec<f32> {
    let fade = long.len() - n;
    let mut out = long[..n].to_vec();
    for i in 0..fade {
        let w = i as f32 / fade as f32;
        out[i] = out[i] * w + long[n + i] * (1.0 - w);
    }
    out
}

fn motor_hum(n: usize) -> Vec<f32> {
    // 周波数はすべて整数 Hz なので、1 秒でちょうど元に戻る
    let mut noise = Noise(0x1234_5678);
    let hiss = low_pass(&(0..n).map(|_| noise.next()).collect::<Vec<_>>(), 0.2);
    let tail = (0..n / 10).map(|_| noise.next()).collect::<Vec<_>>();
    let hiss = seamless([hiss, low_pass(&tail, 0.2)].concat(), n);
    (0..n)
        .map(|i| {
            let t = time(i);
            0.6 * sine(100.0, t)
                + 0.3 * sine(200.0, t)
                + 0.15 * sine(300.0, t)
                + 0.05 * sine(1150.0, t)
                + 0.08 * hiss[i]
        })
        .collect()
}

fn servo_whine(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = time(i);
            // 8 Hz でゆれる高い音 (インバーターのキャリア音) と低いうなり
            let wobble = 0.75 + 0.25 * sine(8.0, t);
            0.45 * sine(120.0, t)
                + 0.2 * sine(240.0, t)
                + 0.18 * wobble * sine(1800.0, t)
                + 0.08 * sine(3600.0, t)
        })
        .collect()
}

fn conveyor_rumble(n: usize) -> Vec<f32> {
    let mut noise = Noise(0x0bad_cafe);
    let long = n + n / 8;
    let raw: Vec<f32> = (0..long).map(|_| noise.next()).collect();
    let rumble = seamless(low_pass(&raw, 0.02), n);
    (0..n)
        .map(|i| {
            let t = time(i);
            // ローラーの継ぎ目を 6 Hz で「コトコト」と鳴らす (2 秒でちょうど 12 回)
            let phase = (t * 6.0).fract();
            let click = (-phase * 60.0).exp() * sine(700.0, t);
            0.9 * rumble[i] * 6.0 + 0.25 * sine(50.0, t) + 0.15 * click
        })
        .collect()
}

fn air_exhaust(n: usize) -> Vec<f32> {
    let mut noise = Noise(0x5eed_a1b2);
    let raw: Vec<f32> = (0..n).map(|_| noise.next()).collect();
    // 高い音を残す (雑音から低い音を引く)
    let low = low_pass(&raw, 0.15);
    (0..n)
        .map(|i| {
            let t = time(i);
            let attack = (t / 0.005).min(1.0);
            let decay = (-t * 9.0).exp();
            (raw[i] - low[i]) * attack * decay
        })
        .collect()
}

fn alarm(n: usize) -> Vec<f32> {
    let half = n / 2;
    (0..n)
        .map(|i| {
            let t = time(i);
            let (freq, local) = if i < half {
                (880.0, i)
            } else {
                (660.0, i - half)
            };
            // 音と音のあいだに少し間を空け、角を丸めてプツッと鳴らないようにする
            let local_t = time(local);
            let len = time(half);
            let edge = 0.01;
            let gate = (local_t / edge).min(1.0) * ((len - 0.06 - local_t) / edge).clamp(0.0, 1.0);
            // 少し角ばった音 (奇数倍音を足す) にして聞き取りやすくする
            let tone = sine(freq, t) + 0.3 * sine(freq * 3.0, t) + 0.1 * sine(freq * 5.0, t);
            tone * gate
        })
        .collect()
}

fn warning_beep(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = time(i);
            // 0.12 秒の音を 2 回
            let local = t % 0.16;
            let on = t < 0.32 && local < 0.12;
            if !on {
                return 0.0;
            }
            let edge = 0.008;
            let gate = (local / edge).min(1.0) * ((0.12 - local) / edge).clamp(0.0, 1.0);
            gate * (sine(1320.0, t) + 0.2 * sine(2640.0, t))
        })
        .collect()
}

/// いちばん大きい値が `peak` になるようにそろえる
fn normalize(samples: &mut [f32], peak: f32) {
    let max = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if max > 0.0 {
        let scale = peak / max;
        for s in samples {
            *s *= scale;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn every_sound_is_audible_and_within_range() {
        for kind in SoundKind::ALL {
            let samples = synthesize(kind);
            assert_eq!(
                samples.len(),
                (kind.seconds() * SAMPLE_RATE as f32) as usize,
                "{kind:?}"
            );
            assert!(samples.iter().all(|s| s.abs() <= 0.8 + 1e-4), "{kind:?}");
            assert!(rms(&samples) > 0.05, "{kind:?} が小さすぎる");
            assert!(samples.iter().all(|s| s.is_finite()), "{kind:?}");
        }
    }

    #[test]
    fn same_kind_gives_same_wave() {
        for kind in SoundKind::ALL {
            assert_eq!(synthesize(kind), synthesize(kind), "{kind:?}");
        }
    }

    #[test]
    fn looping_sounds_join_without_a_jump() {
        // 最後と最初の差が、となり合うサンプルどうしの差の大きさ程度に収まる
        for kind in SoundKind::ALL.into_iter().filter(|k| k.looping()) {
            let s = synthesize(kind);
            let max_step = s
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0f32, f32::max);
            let seam = (s[0] - s[s.len() - 1]).abs();
            assert!(
                seam <= max_step + 1e-3,
                "{kind:?}: 継ぎ目 {seam} > {max_step}"
            );
        }
    }

    #[test]
    fn one_shots_fade_out() {
        for kind in SoundKind::ALL.into_iter().filter(|k| !k.looping()) {
            let s = synthesize(kind);
            let tail = &s[s.len() - 200..];
            assert!(rms(tail) < 0.05, "{kind:?} の終わりが急に切れる");
        }
    }

    #[test]
    fn alarm_has_two_pitches() {
        // 前半と後半で 0 をまたぐ回数が違う (880 Hz と 660 Hz)
        let s = synthesize(SoundKind::Alarm);
        let crossings = |part: &[f32]| {
            part.windows(2)
                .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
                .count()
        };
        let half = s.len() / 2;
        let first = crossings(&s[..half]);
        let second = crossings(&s[half..]);
        assert!(first > second + 50, "{first} / {second}");
    }

    #[test]
    fn categories() {
        assert_eq!(SoundKind::MotorHum.category(), SoundCategory::Machine);
        assert_eq!(SoundKind::AirExhaust.category(), SoundCategory::Air);
        assert_eq!(SoundKind::Alarm.category(), SoundCategory::Alarm);
        assert!(SoundKind::Alarm.looping());
        assert!(!SoundKind::AirExhaust.looping());
    }
}
