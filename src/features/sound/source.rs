//! 作った波形を Bevy の音として鳴らす
//!
//! 音 1 つごとに [`SynthSound`] のアセットを作る。波形は種類ごとに 1 つを共有し、
//! 左右の大きさ ([`VoiceControls`]) だけを音ごとに持つ。大きさは鳴らしている途中でも
//! 毎フレーム書き換えられ、急に変えてもプツッと鳴らないように少しずつ近づける。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use bevy::audio::{ChannelCount, Decodable, SampleRate, Source};
use bevy::prelude::*;

use plantwright_core::sound::{Placement, SAMPLE_RATE};

/// 左右の大きさ (目標)。音を鳴らす側のスレッドから読む。
#[derive(Default)]
pub struct VoiceControls {
    left: AtomicU32,
    right: AtomicU32,
}

impl VoiceControls {
    pub fn set(&self, placement: Placement) {
        let (l, r) = placement.channel_gains();
        self.left.store(l.to_bits(), Ordering::Relaxed);
        self.right.store(r.to_bits(), Ordering::Relaxed);
    }

    fn get(&self) -> (f32, f32) {
        (
            f32::from_bits(self.left.load(Ordering::Relaxed)),
            f32::from_bits(self.right.load(Ordering::Relaxed)),
        )
    }
}

#[derive(Asset, TypePath, Clone)]
pub struct SynthSound {
    pub samples: Arc<[f32]>,
    pub looping: bool,
    pub controls: Arc<VoiceControls>,
}

impl Decodable for SynthSound {
    type Decoder = SynthDecoder;

    fn decoder(&self) -> SynthDecoder {
        // くり返す音は無音から立ち上げる。1 回だけの音は最初から目標の大きさで鳴らす。
        let start = if self.looping {
            (0.0, 0.0)
        } else {
            self.controls.get()
        };
        SynthDecoder {
            sound: self.clone(),
            position: 0,
            right_next: None,
            gain: start,
        }
    }
}

/// 2 チャンネル (左右) を交互に出す
pub struct SynthDecoder {
    sound: SynthSound,
    position: usize,
    /// 左を出したあと、次に出す右の値
    right_next: Option<f32>,
    gain: (f32, f32),
}

/// 大きさを目標に近づける速さ (1 サンプルあたり)。約 30 ミリ秒で追いつく。
const SMOOTHING: f32 = 1.0 / (0.03 * SAMPLE_RATE as f32);

impl Iterator for SynthDecoder {
    type Item = bevy::audio::Sample;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(right) = self.right_next.take() {
            return Some(right);
        }
        let samples = &self.sound.samples;
        if self.position >= samples.len() {
            if !self.sound.looping || samples.is_empty() {
                return None;
            }
            self.position = 0;
        }
        let value = samples[self.position];
        self.position += 1;
        let (target_l, target_r) = self.sound.controls.get();
        self.gain.0 += (target_l - self.gain.0) * SMOOTHING;
        self.gain.1 += (target_r - self.gain.1) * SMOOTHING;
        self.right_next = Some(value * self.gain.1);
        Some(value * self.gain.0)
    }
}

impl Source for SynthDecoder {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        ChannelCount::new(2).expect("2 channels")
    }

    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(SAMPLE_RATE).expect("sample rate")
    }

    fn total_duration(&self) -> Option<Duration> {
        (!self.sound.looping)
            .then(|| Duration::from_secs_f64(self.sound.samples.len() as f64 / SAMPLE_RATE as f64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound(looping: bool, placement: Placement) -> SynthSound {
        let controls = Arc::new(VoiceControls::default());
        controls.set(placement);
        SynthSound {
            samples: vec![1.0; 10].into(),
            looping,
            controls,
        }
    }

    #[test]
    fn one_shot_plays_once_in_stereo() {
        let s = sound(
            false,
            Placement {
                gain: 1.0,
                pan: 1.0,
            },
        );
        let out: Vec<f32> = s.decoder().collect();
        assert_eq!(out.len(), 20);
        // 右に振り切ると左はほぼ 0
        assert!(out[0].abs() < 1e-6);
        assert!((out[1] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn looping_sound_fades_in_and_repeats() {
        let s = sound(true, Placement::CENTER);
        let out: Vec<f32> = s.decoder().take(20_000).collect();
        assert_eq!(out.len(), 20_000);
        assert!(out[0] < 0.01, "無音から立ち上げる");
        let (target, _) = Placement::CENTER.channel_gains();
        assert!((out[19_998] - target).abs() < 0.01);
    }
}
