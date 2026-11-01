//! ミュートと音量

/// 音量のつまみの分け方
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SoundCategory {
    /// モーター・サーボ・コンベアの稼働音
    Machine,
    /// エアシリンダの排気音
    Air,
    /// 停止・干渉の警報
    Alarm,
}

impl SoundCategory {
    pub const ALL: [SoundCategory; 3] = [
        SoundCategory::Machine,
        SoundCategory::Air,
        SoundCategory::Alarm,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SoundCategory::Machine => "設備の稼働音",
            SoundCategory::Air => "エアの排気音",
            SoundCategory::Alarm => "警報",
        }
    }
}

/// 音の設定。アプリを閉じても覚えておく。音量は 0〜1。
#[derive(Clone, Debug, PartialEq)]
pub struct SoundSettings {
    /// true なら何も鳴らさない
    pub muted: bool,
    pub master: f32,
    pub machine: f32,
    pub air: f32,
    pub alarm: f32,
}

impl Default for SoundSettings {
    fn default() -> Self {
        Self {
            muted: false,
            master: 0.7,
            machine: 0.8,
            air: 0.8,
            alarm: 1.0,
        }
    }
}

impl SoundSettings {
    pub fn volume(&self, category: SoundCategory) -> f32 {
        match category {
            SoundCategory::Machine => self.machine,
            SoundCategory::Air => self.air,
            SoundCategory::Alarm => self.alarm,
        }
    }

    pub fn volume_mut(&mut self, category: SoundCategory) -> &mut f32 {
        match category {
            SoundCategory::Machine => &mut self.machine,
            SoundCategory::Air => &mut self.air,
            SoundCategory::Alarm => &mut self.alarm,
        }
    }

    /// 全体の音量と種類ごとの音量を掛けたもの。ミュート中は 0。
    pub fn gain(&self, category: SoundCategory) -> f32 {
        if self.muted {
            0.0
        } else {
            self.master.clamp(0.0, 1.0) * self.volume(category).clamp(0.0, 1.0)
        }
    }

    /// 範囲外や数でない値を直す (設定ファイルを手で書き換えられたときのため)
    pub fn sanitized(mut self) -> Self {
        for v in [
            &mut self.master,
            &mut self.machine,
            &mut self.air,
            &mut self.alarm,
        ] {
            *v = if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                1.0
            };
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_multiplies_master_and_category() {
        let s = SoundSettings {
            muted: false,
            master: 0.5,
            machine: 0.4,
            air: 1.0,
            alarm: 0.0,
        };
        assert!((s.gain(SoundCategory::Machine) - 0.2).abs() < 1e-6);
        assert!((s.gain(SoundCategory::Air) - 0.5).abs() < 1e-6);
        assert_eq!(s.gain(SoundCategory::Alarm), 0.0);
    }

    #[test]
    fn mute_silences_everything() {
        let s = SoundSettings {
            muted: true,
            ..SoundSettings::default()
        };
        for c in SoundCategory::ALL {
            assert_eq!(s.gain(c), 0.0);
        }
    }

    #[test]
    fn sanitized_clamps_bad_values() {
        let s = SoundSettings {
            muted: false,
            master: 3.0,
            machine: -1.0,
            air: f32::NAN,
            alarm: 0.5,
        }
        .sanitized();
        assert_eq!((s.master, s.machine, s.air, s.alarm), (1.0, 0.0, 1.0, 0.5));
    }
}
