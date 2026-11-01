//! CSV 出力の Bevy 側 (UI・フレームワーク層)
//!
//! 保存ダイアログはメインスレッドで出し (macOS の制約)、CSV の作成と書き込みは別スレッドで行う。
//! 干渉の1サイクル分のチェックは部品が多いと時間がかかるので、その間も画面は止めない。
//! 結果はステータスバーに出す。

use std::path::PathBuf;

use bevy::{
    ecs::system::NonSendMarker,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, block_on, poll_once},
};

use crate::file_io::ImportJobs;
use crate::motion::ChartSchedule;
use crate::simulation::{InterferenceCheck, SimBody};
use plantwright_core::model::Model;
use plantwright_core::motion::chart::Schedule;
use plantwright_core::motion::chart::TimingChart;
use plantwright_core::simulation::interference::CheckSettings;

use super::report::{self, SweepBody};

pub struct ResultExportPlugin;

impl Plugin for ResultExportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ResultExport>()
            .add_systems(Update, (start_export, finish_export));
    }
}

/// どの CSV を出すか
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResultKind {
    CycleTime,
    Interference,
}

impl ResultKind {
    fn file_name(self) -> &'static str {
        match self {
            ResultKind::CycleTime => "cycle_time.csv",
            ResultKind::Interference => "interference.csv",
        }
    }
}

/// CSV 出力の設定と、実行中の書き出し
#[derive(Resource)]
pub struct ResultExport {
    /// 干渉チェックで時刻を進める間隔 (秒)
    pub sample_interval: f64,
    /// メニューで押された (ダイアログは次の Update でメインスレッドから出す)
    requested: Option<ResultKind>,
    task: Option<Task<Result<PathBuf, String>>>,
}

impl Default for ResultExport {
    fn default() -> Self {
        Self {
            sample_interval: 0.02,
            requested: None,
            task: None,
        }
    }
}

impl ResultExport {
    pub fn request(&mut self, kind: ResultKind) {
        self.requested = Some(kind);
    }

    /// 書き出し中 (メニューのボタンを押せなくする)
    pub fn is_busy(&self) -> bool {
        self.task.is_some()
    }
}

/// CSV を作るのに必要なものの写し。別スレッドに渡す。
enum Job {
    CycleTime {
        chart: TimingChart,
        schedule: Schedule,
    },
    Interference {
        bodies: Vec<SweepBody>,
        chart: TimingChart,
        schedule: Schedule,
        settings: CheckSettings,
        interval: f64,
    },
}

impl Job {
    fn build_csv(&self) -> String {
        match self {
            Job::CycleTime { chart, schedule } => report::cycle_time_csv(chart, schedule),
            Job::Interference {
                bodies,
                chart,
                schedule,
                settings,
                interval,
            } => report::interference_csv(bodies, chart, schedule, settings, *interval),
        }
    }
}

/// 保存先を選ばせ、CSV の作成と書き込みを別スレッドで始める
#[allow(clippy::too_many_arguments)]
fn start_export(
    mut export: ResMut<ResultExport>,
    chart: Res<TimingChart>,
    schedule: Res<ChartSchedule>,
    check: Res<InterferenceCheck>,
    model: Res<Model>,
    bodies: Query<(&SimBody, Option<&Name>)>,
    _main_thread: NonSendMarker,
) {
    let Some(kind) = export.requested.take() else {
        return;
    };
    if export.task.is_some() {
        return;
    }
    let Some(mut path) = rfd::FileDialog::new()
        .set_title("シミュレーション結果を CSV に出力")
        .add_filter("CSV", &["csv"])
        .set_file_name(kind.file_name())
        .save_file()
    else {
        return;
    };
    if path.extension().is_none() {
        path.set_extension("csv");
    }

    let job = match kind {
        ResultKind::CycleTime => Job::CycleTime {
            chart: chart.clone(),
            schedule: (**schedule).clone(),
        },
        ResultKind::Interference => Job::Interference {
            bodies: bodies
                .iter()
                .filter(|(body, _)| model.get(body.id).is_some())
                .map(|(body, name)| SweepBody {
                    id: body.id,
                    name: name.map_or_else(|| format!("#{}", body.id), |n| n.to_string()),
                    mesh: body.mesh.clone(),
                    placement: model.world_matrix(body.id),
                    ancestors: model.ancestors(body.id),
                })
                .collect(),
            chart: chart.clone(),
            schedule: (**schedule).clone(),
            settings: check.settings,
            interval: export.sample_interval,
        },
    };
    export.task = Some(AsyncComputeTaskPool::get().spawn(async move {
        std::fs::write(&path, job.build_csv())
            .map(|()| path.clone())
            .map_err(|e| format!("{} に書き込めませんでした: {e}", path.display()))
    }));
}

/// 書き出しが終わっていれば、結果をステータスバーに出す
fn finish_export(mut export: ResMut<ResultExport>, mut jobs: ResMut<ImportJobs>) {
    let Some(result) = export.task.as_mut().and_then(|t| block_on(poll_once(t))) else {
        return;
    };
    export.task = None;
    let message = match result {
        Ok(path) => format!("{} に出力しました", path.display()),
        Err(error) => {
            error!("{error}");
            format!("CSV の出力に失敗しました: {error}")
        }
    };
    jobs.messages.push(message);
}
