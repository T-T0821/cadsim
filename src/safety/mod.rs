//! 安全機器: ライトカーテン・エリアセンサ・安全柵・非常停止ボタン
//!
//! | ファイル | 役割 |
//! | --- | --- |
//! | `cadsim_core::safety::device` | 機器のデータ、侵入の検知、停止信号のラッチ、ISO 13855 の安全距離 |
//! | `check.rs` | 動作範囲 (危険源) と、柵・ライトカーテンの距離の確認 (Bevy 非依存) |
//! | `view.rs` | 機器と動作範囲の 3D 表示 |
//! | `panel.rs` | 「安全」パネル |
//!
//! 侵入物は [`SafetyIntruders`] に集める。可動部はこのモジュールが入れ、作業者や AGV など
//! ほかの機能は `SafetySet::Intruders` の中で書き足す。検知すると停止信号 ([`SafetyState::is_stopped`])
//! を出し、タイミングチャートの再生を止める。リセットするまで再生できない。

mod check;
pub use cadsim_core::safety::device;
mod panel;
mod view;

use std::collections::BTreeSet;

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

pub use device::Intruder;

use crate::math::Aabb;
use crate::model::Model;
use crate::motion::{ChartSchedule, MotionSet, Playback, SimulationPose, TimingChart};
use check::{Clearance, Envelope, check_clearances, motion_envelopes};
use device::{Detection, SafetyLayout, SafetyMonitor, detect};

pub struct SafetyPlugin;

impl Plugin for SafetyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SafetyLayoutRes>()
            .init_resource::<SafetyState>()
            .init_resource::<SafetyIntruders>()
            .init_gizmo_group::<view::SafetyGizmos>()
            .configure_sets(
                Update,
                (SafetySet::Intruders, SafetySet::Detect)
                    .chain()
                    .after(MotionSet::Pose),
            )
            .add_systems(Startup, view::setup)
            .add_systems(
                Update,
                (
                    collect_moving_parts.in_set(SafetySet::Intruders),
                    (update_envelopes, run_detection, stop_playback)
                        .chain()
                        .in_set(SafetySet::Detect),
                    (view::sync_meshes, view::draw).after(SafetySet::Detect),
                ),
            )
            .add_systems(EguiPrimaryContextPass, panel::safety_panel);
    }
}

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SafetySet {
    /// 侵入物を [`SafetyIntruders`] に書き足す
    Intruders,
    /// 検知と停止
    Detect,
}

/// 配置した安全機器
#[derive(Resource, Default, Deref, DerefMut)]
pub struct SafetyLayoutRes(SafetyLayout);

/// 検知の対象になる物。`SafetySet::Intruders` の中で毎フレーム書き足し、検知に使ったら空にする。
#[derive(Resource, Default)]
pub struct SafetyIntruders(pub Vec<Intruder>);

/// 停止信号と、パネル・表示の状態
#[derive(Resource)]
pub struct SafetyState {
    monitor: SafetyMonitor,
    /// いま検知しているもの
    detections: Vec<Detection>,
    /// 手動で遮光した機器
    blocked: BTreeSet<u32>,
    /// 押されている非常停止ボタン
    pressed: BTreeSet<u32>,
    /// 機械の可動部も検知の対象にする
    detect_machine: bool,
    envelopes: Vec<Envelope>,
    clearances: Vec<Clearance>,
    /// 動作範囲が変わるたびに 1 増える (表示の作り直しの目印)
    envelope_revision: u64,
    show_envelopes: bool,
    /// パネルで選んでいる機器
    selected: Option<u32>,
    /// リセットできなかったときの説明
    message: Option<String>,
}

impl Default for SafetyState {
    fn default() -> Self {
        Self {
            monitor: SafetyMonitor::default(),
            detections: Vec::new(),
            blocked: BTreeSet::new(),
            pressed: BTreeSet::new(),
            detect_machine: true,
            envelopes: Vec::new(),
            clearances: Vec::new(),
            envelope_revision: 0,
            show_envelopes: true,
            selected: None,
            message: None,
        }
    }
}

impl SafetyState {
    /// 停止信号が出ているか (リセットするまで true のまま)
    pub fn is_stopped(&self) -> bool {
        self.monitor.is_stopped()
    }

    /// 停止・遮光・非常停止・選択を最初の状態に戻す (プロジェクトを開き直したとき)
    pub fn clear_runtime(&mut self) {
        *self = Self {
            show_envelopes: self.show_envelopes,
            detect_machine: self.detect_machine,
            ..Self::default()
        };
    }

    fn is_detecting(&self, device: u32) -> bool {
        self.detections.iter().any(|d| d.device == device)
    }

    fn reset(&mut self) {
        self.message = match self.monitor.reset(&self.detections) {
            Ok(()) => None,
            Err(device::ResetBlocked::StillDetected(found)) => Some(format!(
                "まだ検知中のためリセットできません: {}",
                found
                    .iter()
                    .map(|d| d.cause.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        };
    }
}

/// タイミングチャートで動いている部品を侵入物として入れる
fn collect_moving_parts(
    model: Res<Model>,
    pose: Res<SimulationPose>,
    state: Res<SafetyState>,
    mut intruders: ResMut<SafetyIntruders>,
) {
    if !state.detect_machine {
        return;
    }
    for (&node, moved) in &pose.parts {
        if !model.is_shown(node) {
            continue;
        }
        let Some(bounds) = model.world_bounds(node) else {
            continue;
        };
        let corners = bounds.corners().map(|p| moved.transform_point(p));
        let bounds = corners
            .into_iter()
            .map(|p| Aabb { min: p, max: p })
            .reduce(Aabb::union)
            .expect("8 corners");
        intruders.0.push(Intruder {
            label: model
                .get(node)
                .map_or_else(|| format!("#{node}"), |n| n.name.clone()),
            bounds,
        });
    }
}

/// チャートかモデルか機器が変わったら、動作範囲と距離を求め直す
fn update_envelopes(
    model: Res<Model>,
    chart: Res<TimingChart>,
    schedule: Res<ChartSchedule>,
    layout: Res<SafetyLayoutRes>,
    mut state: ResMut<SafetyState>,
) {
    if !(model.is_changed() || chart.is_changed() || schedule.is_changed() || layout.is_changed()) {
        return;
    }
    let envelopes = motion_envelopes(&chart, &schedule, |node| model.world_bounds(node));
    let clearances = check_clearances(&layout.devices, &envelopes);
    if state.envelopes != envelopes {
        state.envelopes = envelopes;
        state.envelope_revision += 1;
    }
    if state.clearances != clearances {
        state.clearances = clearances;
    }
}

fn run_detection(
    layout: Res<SafetyLayoutRes>,
    mut intruders: ResMut<SafetyIntruders>,
    mut state: ResMut<SafetyState>,
) {
    // 侵入物は毎フレーム集め直すので、使ったら空にする
    let found = std::mem::take(&mut intruders.0);
    let detections = detect(&layout.devices, &found, &state.blocked, &state.pressed);
    // 変化がなければ書かない (毎フレームの変更検知を避ける)
    if detections != state.detections {
        state.detections = detections;
    }
    if !state.detections.is_empty() {
        let detections = state.detections.clone();
        state.monitor.update(&detections);
    }
}

/// 停止信号が出ている間は再生させない
fn stop_playback(state: Res<SafetyState>, mut playback: ResMut<Playback>) {
    if state.is_stopped() && playback.playing {
        playback.playing = false;
    }
}
