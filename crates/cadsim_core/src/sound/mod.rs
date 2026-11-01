//! 設備の音と警報
//!
//! | ファイル | 役割 |
//! | --- | --- |
//! | `synth` | 稼働音・エアの排気音・警報の波形をその場で作る (音のファイルは使わない) |
//! | `settings` | ミュートと音量 (全体・設備・エア・警報) |
//! | `spatial` | 聞く位置 (カメラ) から見た音の大きさと左右の振り分け |
//! | `cues` | いまの設備の状態から、鳴らし続ける音と一度だけ鳴らす音を決める |
//! | `alarm` | 警報の理由と、ブザー停止 (確認したら止め、新しい異常でまた鳴らす) |
//!
//! Bevy に依存しない。アプリの `src/sound/` が状態を集めてここを呼び、結果を音に写す。

pub mod alarm;
pub mod cues;
pub mod settings;
pub mod spatial;
pub mod synth;

pub use alarm::{AlarmLatch, AlarmReason, InterferenceWatch, interlock_stopped};
pub use cues::{
    Emitter, MAX_VOICES, MachineFrame, MotionTracker, OneShot, SoundPlan, Voice, VoiceKey,
    plan_sounds,
};
pub use settings::{SoundCategory, SoundSettings};
pub use spatial::{Listener, Placement, place};
pub use synth::{SAMPLE_RATE, SoundKind, synthesize};
