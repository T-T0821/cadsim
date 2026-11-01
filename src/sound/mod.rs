//! 設備の音と警報
//!
//! | ファイル | 役割 |
//! | --- | --- |
//! | `cadsim_core::sound` | 波形の合成、何をどの大きさ・向きで鳴らすか、ブザー停止 (Bevy なし) |
//! | `cadsim_io::sound_settings` | ミュートと音量の設定ファイル |
//! | `source.rs` | 作った波形を Bevy の音として鳴らす |
//! | `panel.rs` | 「音の設定」ウィンドウと、警報が鳴っている間の帯 (ブザー停止) |
//!
//! このファイルは毎フレーム「設備の状態を集める → `plan_sounds` を呼ぶ → 音を出し入れする」だけをする。

mod panel;
mod source;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bevy::audio::{AddAudioSource, AudioPlayer, PlaybackSettings};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

use cadsim_core::simulation::interference::PairStatus;
use cadsim_core::sound::{
    AlarmLatch, AlarmReason, Emitter, InterferenceWatch, Listener, MachineFrame, MotionTracker,
    SoundKind, SoundPlan, SoundSettings, VoiceKey, interlock_stopped, plan_sounds, synthesize,
};
use cadsim_io::sound_settings;

use crate::conveyor::ConveyorConfigRes;
use crate::line::LineState;
use crate::model::Model;
use crate::motion::{ChartSchedule, MotionSet, Playback, SimulationPose, TimingChart};
use crate::safety::{SafetySet, SafetyState};
use crate::simulation::InterferenceCheck;
use crate::view::camera::OrbitCamera;
use crate::view::walk::WalkMode;
use source::{SynthSound, VoiceControls};

pub use panel::show_menu_items;

pub struct SoundPlugin;

impl Plugin for SoundPlugin {
    fn build(&self, app: &mut App) {
        app.add_audio_source::<SynthSound>()
            .insert_resource(SoundUi::load())
            .init_resource::<SoundEngine>()
            .add_systems(
                Update,
                (update_alarms, play_sounds, save_settings)
                    .chain()
                    .after(SafetySet::Detect)
                    .after(MotionSet::Sensors),
            )
            .add_systems(
                EguiPrimaryContextPass,
                (panel::sound_window, panel::alarm_banner),
            );
    }
}

/// 音の設定と、警報の確認 (ブザー停止) の状態
#[derive(Resource)]
pub struct SoundUi {
    pub settings: SoundSettings,
    /// 「音の設定」ウィンドウを出す
    pub show_window: bool,
    latch: AlarmLatch,
    path: Option<PathBuf>,
    /// 前に保存した設定 (変わったら保存する)
    saved: SoundSettings,
}

impl SoundUi {
    fn load() -> Self {
        let path = sound_settings::default_path();
        let settings = path
            .as_deref()
            .map(sound_settings::load)
            .unwrap_or_default();
        Self {
            saved: settings.clone(),
            settings,
            show_window: false,
            latch: AlarmLatch::default(),
            path,
        }
    }

    /// ブザー停止
    pub fn acknowledge(&mut self) {
        self.latch.acknowledge();
    }

    pub fn alarms(&self) -> impl Iterator<Item = AlarmReason> + '_ {
        self.latch.active()
    }

    pub fn alarm_sounding(&self) -> bool {
        self.latch.sounding()
    }
}

/// 鳴らしている音と、動きの見張り
#[derive(Resource, Default)]
struct SoundEngine {
    waves: HashMap<SoundKind, Arc<[f32]>>,
    voices: HashMap<VoiceKey, ActiveVoice>,
    tracker: MotionTracker,
    interference: InterferenceWatch,
    /// アクチュエータとコンベアの設計位置での中心 (モデルかチャートが変わったら作り直す)
    actuator_centers: Vec<(u32, cadsim_core::motion::ActuatorKind, bevy::math::DVec3)>,
    conveyor_centers: Vec<(u32, bevy::math::DVec3)>,
}

struct ActiveVoice {
    entity: Entity,
    kind: SoundKind,
    controls: Arc<VoiceControls>,
    /// 止めると決めた時刻 (小さくし終わったら消す)
    fading_since: Option<f64>,
}

/// 消すまでに小さくする時間 (秒)
const FADE_OUT: f64 = 0.15;

impl SoundEngine {
    fn wave(&mut self, kind: SoundKind) -> Arc<[f32]> {
        self.waves
            .entry(kind)
            .or_insert_with(|| synthesize(kind).into())
            .clone()
    }
}

fn update_alarms(
    safety: Res<SafetyState>,
    schedule: Res<ChartSchedule>,
    playback: Res<Playback>,
    mut ui: ResMut<SoundUi>,
) {
    let mut active = Vec::new();
    if safety.is_stopped() {
        active.push(AlarmReason::SafetyStop);
    }
    if interlock_stopped(
        schedule.stop.as_ref().map(|s| s.time),
        playback.time,
        playback.playing,
    ) {
        active.push(AlarmReason::InterlockStop);
    }
    // 変わったときだけ書く (毎フレームの変更検知を避ける)
    let mut next = ui.latch.clone();
    next.update(active);
    if next != ui.latch {
        ui.latch = next;
    }
}

#[derive(SystemParam)]
struct Machine<'w> {
    model: Res<'w, Model>,
    chart: Res<'w, TimingChart>,
    pose: Res<'w, SimulationPose>,
    playback: Res<'w, Playback>,
    conveyors: Res<'w, ConveyorConfigRes>,
    line: Res<'w, LineState>,
    interference: Res<'w, InterferenceCheck>,
}

fn play_sounds(
    mut commands: Commands,
    time: Res<Time<Real>>,
    machine: Machine,
    view: Hearing,
    ui: Res<SoundUi>,
    mut engine: ResMut<SoundEngine>,
    mut assets: ResMut<Assets<SynthSound>>,
) {
    let now = time.elapsed_secs_f64();
    let engine = &mut *engine;
    if machine.model.is_changed() || machine.chart.is_changed() || machine.conveyors.is_changed() {
        refresh_centers(engine, &machine);
    }
    let started = engine
        .tracker
        .observe(now, machine.pose.values.iter().map(|(&id, &v)| (id, v)));
    let running = machine.playback.playing || machine.line.is_running();
    let interfering = machine
        .interference
        .report
        .pairs
        .iter()
        .filter(|p| matches!(p.status, PairStatus::Interfering { .. }))
        .count();
    let interference_began = engine.interference.update(interfering, running);

    let frame = MachineFrame {
        actuators: engine
            .actuator_centers
            .iter()
            .map(|&(id, kind, center)| Emitter {
                id,
                kind,
                position: machine
                    .pose
                    .actuators
                    .get(&id)
                    .map_or(center, |p| p.transform_point(center)),
            })
            .collect(),
        moving: engine.tracker.moving(now),
        started,
        conveyors: engine.conveyor_centers.clone(),
        conveyors_running: running,
        alarm: ui.alarm_sounding(),
        interference_began,
    };
    let listener = view.listener();
    let plan = plan_sounds(&frame, &listener, &ui.settings);
    apply_plan(&mut commands, engine, &mut assets, plan, now);
}

/// 聞く位置: 3D ビューのカメラ
#[derive(SystemParam)]
struct Hearing<'w, 's> {
    camera: Single<'w, 's, &'static GlobalTransform, With<Camera3d>>,
    orbit: Res<'w, OrbitCamera>,
    walk: Res<'w, WalkMode>,
}

/// 歩いているときに、そのままの大きさで聞こえる距離 (m)
const WALK_HEARING_M: f64 = 3.0;

impl Hearing<'_, '_> {
    fn listener(&self) -> Listener {
        // 見回しているときは注視点までの距離、歩いているときは人の耳で数 m を基準にする
        let reference_distance = if self.walk.active() {
            WALK_HEARING_M * self.walk.units_per_meter()
        } else {
            self.orbit.distance as f64
        };
        Listener {
            position: self.camera.translation().as_dvec3(),
            right: self.camera.right().as_vec3().as_dvec3(),
            reference_distance,
        }
    }
}

fn refresh_centers(engine: &mut SoundEngine, machine: &Machine) {
    let center = |parts: &mut dyn Iterator<Item = u32>| {
        parts
            .filter(|&id| machine.model.is_shown(id))
            .filter_map(|id| machine.model.world_bounds(id))
            .reduce(crate::math::Aabb::union)
            .map(|b| b.center())
    };
    engine.actuator_centers = machine
        .chart
        .actuators
        .iter()
        .filter_map(|a| Some((a.id, a.kind, center(&mut a.parts.iter().copied())?)))
        .collect();
    engine.conveyor_centers = machine
        .conveyors
        .conveyors
        .iter()
        .filter_map(|c| Some((c.id, center(&mut c.part.into_iter())?)))
        .collect();
}

/// 決めた音のとおりに、音を出し入れして大きさと向きを合わせる
fn apply_plan(
    commands: &mut Commands,
    engine: &mut SoundEngine,
    assets: &mut Assets<SynthSound>,
    plan: SoundPlan,
    now: f64,
) {
    for voice in &plan.voices {
        let reuse = engine
            .voices
            .get(&voice.key)
            .is_some_and(|active| active.kind == voice.kind);
        if !reuse {
            if let Some(old) = engine.voices.remove(&voice.key) {
                commands.entity(old.entity).despawn();
            }
            let controls = Arc::new(VoiceControls::default());
            let sound = SynthSound {
                samples: engine.wave(voice.kind),
                looping: true,
                controls: controls.clone(),
            };
            let entity = commands
                .spawn((
                    Name::new(format!("音 {:?}", voice.key)),
                    AudioPlayer(assets.add(sound)),
                    PlaybackSettings::LOOP,
                ))
                .id();
            engine.voices.insert(
                voice.key,
                ActiveVoice {
                    entity,
                    kind: voice.kind,
                    controls,
                    fading_since: None,
                },
            );
        }
        let active = engine.voices.get_mut(&voice.key).expect("inserted above");
        active.controls.set(voice.placement);
        active.fading_since = None;
    }
    // 鳴らさなくなった音は小さくしてから消す
    engine.voices.retain(|key, active| {
        if plan.voices.iter().any(|v| v.key == *key) {
            return true;
        }
        let since = *active.fading_since.get_or_insert_with(|| {
            active.controls.set(cadsim_core::sound::Placement {
                gain: 0.0,
                pan: 0.0,
            });
            now
        });
        if now - since < FADE_OUT {
            return true;
        }
        commands.entity(active.entity).despawn();
        false
    });
    for shot in plan.one_shots {
        let controls = Arc::new(VoiceControls::default());
        controls.set(shot.placement);
        let sound = SynthSound {
            samples: engine.wave(shot.kind),
            looping: false,
            controls,
        };
        commands.spawn((
            Name::new(format!("音 {:?}", shot.kind)),
            AudioPlayer(assets.add(sound)),
            PlaybackSettings::DESPAWN,
        ));
    }
}

fn save_settings(mut ui: ResMut<SoundUi>) {
    if ui.settings == ui.saved {
        return;
    }
    ui.saved = ui.settings.clone();
    if let Some(path) = &ui.path
        && let Err(err) = sound_settings::save(path, &ui.settings)
    {
        warn!("音の設定を保存できませんでした ({}): {err}", path.display());
    }
}
