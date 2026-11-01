//! 工程モデルとスループット統計
//!
//! 生成・工程・バッファ・排出をノーコードで並べ、ラインを数時間分早回しして、
//! 設備ごとの稼働・待ち・詰まり・故障の割合、サイクルタイム、仕掛数、電力量・エア消費量を集計する。
//!
//! - `model` / `sim` / `palette` 工程モデル・シミュレーション・色 (`plantwright_core::flow` のドメイン)
//! - `record`  プロジェクトファイルに書く形 (serde)
//! - `xlsx`    結果の Excel 書き出し (rust_xlsxwriter)
//! - `ui`      「工程シミュレーション」ウィンドウ
//! - `staff_ui` そのウィンドウの「人員」タブ (シフト・名簿・スキル表・人数の比較)
//! - `factory_ui` 「工場」タブ (ラインの一覧・取り込み・ラインごとの結果)
//! - `plan_ui` 「計画」タブ (品種・生産計画・納期の結果)
//! - `scenario_ui` 「案の比較」タブ
//!
//! このファイルは Bevy とのつなぎ: 工程モデルを包む Resource、早送りの再生、
//! 3D の部品に塗る色 ([`FlowColors`]、viewport.rs が使う) を受け持つ。

use plantwright_core::flow::xlsx;
mod panel;

use plantwright_core::flow::{model, palette, rewind, sim};

use std::collections::HashMap;
use std::path::PathBuf;

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;

pub use panel::{FlowUi, show_window};

use crate::document::history::{TrackDocExt, TrackedDoc};
use crate::document::import::ImportJobs;
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;

/// 工程モデルの編集を元に戻す履歴に載せる
struct FlowDoc;

impl TrackedDoc for FlowDoc {
    type Value = FlowModel;
    const LABEL: &'static str = "工程モデルを編集";

    fn read(world: &World) -> FlowModel {
        world.resource::<FlowRes>().0.clone()
    }

    fn write(world: &mut World, value: FlowModel) {
        world.resource_mut::<FlowRes>().0 = value;
        // 前の工程で回した結果は合わなくなるので捨てる
        world.resource_mut::<FlowRun>().stop();
    }

    fn same(a: &FlowModel, b: &FlowModel) -> bool {
        a == b
    }
}
use model::FlowModel;
use rewind::FlowTimeline;
use sim::{Report, Simulation};

pub struct FlowPlugin;

impl Plugin for FlowPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FlowRes>()
            .track_doc::<FlowDoc>()
            .init_resource::<FlowRun>()
            .init_resource::<FlowColors>()
            .init_resource::<panel::FlowUiState>()
            .add_systems(Update, (advance_run, update_colors, export_xlsx).chain());
    }
}

/// 工程モデル (プロジェクトに保存する設計データ)
#[derive(Resource, Default, Deref, DerefMut)]
pub struct FlowRes(pub FlowModel);

/// 3D ビューの色分け
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ColorMode {
    /// 色を付けない
    Off,
    /// 今の状態 (稼働・待ち・詰まり・故障)
    #[default]
    State,
    /// 稼働率。ボトルネックを赤く
    Busy,
}

impl ColorMode {
    pub const ALL: [ColorMode; 3] = [ColorMode::Off, ColorMode::State, ColorMode::Busy];

    pub fn label(self) -> &'static str {
        match self {
            ColorMode::Off => "なし",
            ColorMode::State => "状態",
            ColorMode::Busy => "稼働率・ボトルネック",
        }
    }
}

/// 実行中・実行後のシミュレーションと、ウィンドウの表示状態
#[derive(Resource)]
pub struct FlowRun {
    pub sim: Option<Simulation>,
    /// 途中の状態の写し (時間スライダーで戻すため)。`sim` と一緒に作り直す。
    pub timeline: Option<FlowTimeline>,
    pub report: Option<Report>,
    /// 実行したときのモデル。今のモデルと違えば結果は古い。
    pub ran_with: Option<FlowModel>,
    pub playing: bool,
    /// 実時間 1 秒あたりに進めるシミュレーションの秒数。∞ なら最後まで。
    pub speed: f64,
    pub color_mode: ColorMode,
    /// Excel 書き出しを頼まれた
    pub export_requested: bool,
}

impl Default for FlowRun {
    fn default() -> Self {
        Self {
            sim: None,
            timeline: None,
            report: None,
            ran_with: None,
            playing: false,
            speed: 600.0,
            color_mode: ColorMode::default(),
            export_requested: false,
        }
    }
}

impl FlowRun {
    /// 今のモデルで最初から回し直す準備をする。回せなければ理由を返す。
    pub fn restart(&mut self, model: &FlowModel) -> Result<(), Vec<String>> {
        self.stop();
        let sim = Simulation::new(model)?;
        self.report = Some(sim.report());
        self.timeline = Some(rewind::timeline_for(model.settings));
        self.sim = Some(sim);
        self.ran_with = Some(model.clone());
        Ok(())
    }

    /// 再生を止めて結果を消す
    pub fn stop(&mut self) {
        self.sim = None;
        self.timeline = None;
        self.report = None;
        self.ran_with = None;
        self.playing = false;
    }

    /// 時刻 `until` まで進め、途中の状態を写しに取る
    pub fn advance_to(&mut self, until: f64) {
        let Some(sim) = self.sim.as_mut() else {
            return;
        };
        match self.timeline.as_mut() {
            Some(timeline) => timeline.advance_recording(sim, until),
            None => {
                sim.advance(until, usize::MAX);
            }
        }
        self.report = Some(sim.report());
    }

    /// 時間スライダーで時刻 `time` の状態にする (一時停止する)
    pub fn seek(&mut self, time: f64) {
        self.playing = false;
        let (Some(sim), Some(timeline)) = (self.sim.as_mut(), self.timeline.as_mut()) else {
            return;
        };
        timeline.seek(sim, time);
        self.report = Some(sim.report());
    }

    /// 詰まりを探す推移の記録 (いちばん先まで回した記録。巻き戻していても先の分が入っている)
    pub fn furthest_samples(&self) -> &[sim::Sample] {
        let sim = self.sim.as_ref();
        let latest = self
            .timeline
            .as_ref()
            .and_then(|t| t.last())
            .map(|(_, s)| s)
            .filter(|s| sim.is_none_or(|now| s.now() > now.now()));
        latest.or(sim).map_or(&[], |s| s.samples())
    }

    /// 結果が今のモデルと合っていない
    pub fn is_stale(&self, model: &FlowModel) -> bool {
        self.ran_with.as_ref().is_some_and(|m| !m.same_setup(model))
    }
}

/// 3D の部品に塗る色 (ノード → sRGB)。アセンブリに付けた色は子孫の部品にも効く。
#[derive(Resource, Default)]
pub struct FlowColors(pub HashMap<NodeId, [f32; 3]>);

impl FlowColors {
    /// 部品の色。`path` はルートから部品までのノード。自分か、いちばん近い祖先の色を使う。
    pub fn color_on_path(&self, path: &[NodeId]) -> Option<[f32; 3]> {
        path.iter().rev().find_map(|id| self.0.get(id).copied())
    }
}

fn advance_run(time: Res<Time>, mut run: ResMut<FlowRun>) {
    if !run.playing {
        return;
    }
    let Some(sim) = run.sim.as_ref() else {
        run.playing = false;
        return;
    };
    let target = sim.now() + time.delta_secs_f64() * run.speed;
    run.advance_to(target);
    if run.sim.as_ref().is_some_and(|s| s.finished()) {
        run.playing = false;
    }
}

fn update_colors(
    flow: Res<FlowRes>,
    run: Res<FlowRun>,
    model: Res<Model>,
    mut colors: ResMut<FlowColors>,
) {
    if !(flow.is_changed() || run.is_changed() || model.is_changed()) {
        return;
    }
    let mut map = HashMap::new();
    if !run.is_stale(&flow) {
        for e in &flow.elements {
            let Some(part) = e.part.filter(|&p| model.get(p).is_some()) else {
                continue;
            };
            let rgb = match run.color_mode {
                ColorMode::Off => None,
                ColorMode::State => run
                    .sim
                    .as_ref()
                    .and_then(|s| s.state_of(e.id))
                    .map(palette::state_rgb),
                ColorMode::Busy => run.report.as_ref().and_then(|r| {
                    let stats = r.element(e.id)?;
                    (stats.times.total() > 0.0)
                        .then(|| palette::busy_rgb(stats.busy_ratio(), r.bottleneck == Some(e.id)))
                }),
            };
            if let Some(rgb) = rgb {
                map.insert(part, rgb);
            }
        }
    }
    if colors.0 != map {
        colors.0 = map;
    }
}

/// 保存ダイアログを出して xlsx を書く。macOS ではダイアログをメインスレッドで出す必要がある。
fn export_xlsx(
    flow: Res<FlowRes>,
    mut run: ResMut<FlowRun>,
    mut jobs: ResMut<ImportJobs>,
    _main_thread: NonSendMarker,
) {
    if !run.export_requested {
        return;
    }
    run.export_requested = false;
    let Some(report) = &run.report else {
        return;
    };
    let Some(path) = rfd::FileDialog::new()
        .set_title("工程シミュレーションの結果を書き出し")
        .add_filter("Excel", &["xlsx"])
        .set_file_name("工程シミュレーション.xlsx")
        .save_file()
    else {
        return;
    };
    let path = with_xlsx_extension(path);
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let model = run.ran_with.as_ref().unwrap_or(&flow);
    let result = xlsx::report_to_xlsx(model, report)
        .map_err(|e| e.to_string())
        .and_then(|bytes| std::fs::write(&path, bytes).map_err(|e| e.to_string()));
    match result {
        Ok(()) => jobs.messages.push(format!("{name} に書き出しました")),
        Err(e) => {
            error!("{}: {e}", path.display());
            jobs.messages
                .push(format!("{name} に書き出せませんでした: {e}"));
        }
    }
}

fn with_xlsx_extension(path: PathBuf) -> PathBuf {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("xlsx"))
    {
        path
    } else {
        path.with_extension("xlsx")
    }
}
