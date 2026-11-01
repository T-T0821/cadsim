//! 自作部品ライブラリ
//!
//! 取り込んだ STEP のアセンブリなどに、アクチュエータ・センサー・スクリプトを付けたものを
//! 「自作部品」として保存し、標準部品と同じ部品ライブラリから何個でも置けるようにする。
//!
//! - `param`: 置いたあとで変えられる公開パラメータ (Bevy なし)
//! - `file`: 部品ファイル (`part.cadpart`) の中身と、ライブラリのフォルダへの保存・一覧
//! - `place`: 部品をモデルとタイミングチャートに置く (名前の頭を付け、id を付け替える)
//! - このファイルと `ui`: ライブラリの Resource、読み込みのスレッド、保存の画面とパレット
//!
//! 置いた部品の一番上のノードには [`CustomInstance`] が付き、プロジェクトファイルにも保存される。
//! 詳しくは docs/custom-parts.md。

mod file;
mod param;
mod place;
mod ui;

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

use bevy::math::DVec3;
use bevy::prelude::*;

pub use file::{CustomInstanceRecord, LibraryEntry};
pub use ui::{CustomDrag, SaveDialog, palette_section, properties_section, save_window};

use crate::file_io::ImportJobs;
use crate::history::History;
use crate::model::Selection;
use crate::project::LoadedProject;
use file::CustomPart;
use param::InstanceOnChart;
use plantwright_core::library::catalog::units_per_mm;
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::motion::chart::TimingChart;

/// 置く位置を丸める刻み (mm)。標準部品と同じ。
const DROP_SNAP_MM: f64 = 10.0;

pub struct CustomPartPlugin;

impl Plugin for CustomPartPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CustomLibrary>()
            .init_resource::<SaveDialog>()
            .add_systems(Update, (rescan_library, receive_placements));
    }
}

/// 別スレッドで読み込んだ部品 (置く位置と一緒に)
struct LoadResult {
    name: String,
    at: DVec3,
    part: Result<(CustomPart, LoadedProject), String>,
}

/// 自作部品ライブラリのフォルダと、その中身の一覧
#[derive(Resource)]
pub struct CustomLibrary {
    /// 部品を保存するフォルダ。ホームフォルダが分からなければ None (保存できない)
    pub dir: Option<PathBuf>,
    pub entries: Vec<LibraryEntry>,
    /// 次のフレームでフォルダを読み直す
    pub needs_scan: bool,
    sender: Sender<LoadResult>,
    receiver: Mutex<Receiver<LoadResult>>,
}

impl Default for CustomLibrary {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            dir: default_dir(),
            entries: Vec::new(),
            needs_scan: true,
            sender,
            receiver: Mutex::new(receiver),
        }
    }
}

/// 設定ファイルと同じ場所の `parts` フォルダ (例: ~/.config/plantwright/parts)
fn default_dir() -> Option<PathBuf> {
    let prefs = plantwright_core::preferences::default_path()?;
    Some(prefs.parent()?.join("parts"))
}

impl CustomLibrary {
    /// 部品を別スレッドで読み込み、読めたら `at` に置く (形状の読み込みが重いため)
    pub fn request_place(&self, entry: &LibraryEntry, jobs: &mut ImportJobs, at: DVec3) {
        let path = entry.path.clone();
        let name = entry.name.clone();
        let sender = self.sender.clone();
        jobs.loading.push(name.clone());
        std::thread::spawn(move || {
            let part = std::fs::read_to_string(&path)
                .map_err(|e| format!("{} を読めません: {e}", path.display()))
                .and_then(|json| CustomPart::from_json(&json))
                .map(|part| {
                    let dir = path.parent().unwrap_or(std::path::Path::new("."));
                    let loaded = LoadedProject::from_file(part.project.clone(), dir);
                    (part, loaded)
                });
            let _ = sender.send(LoadResult { name, at, part });
        });
    }
}

/// 置いた自作部品 `id` の `index` 番目のパラメータを変える (チャートの変更は履歴に積まれる)
pub fn set_param(model: &mut Model, chart: &mut TimingChart, id: NodeId, index: usize, value: f64) {
    if let Some(instance) = model.get_mut(id).and_then(|n| n.custom_part.as_mut()) {
        instance.set(chart, index, value);
    }
}

fn rescan_library(mut library: ResMut<CustomLibrary>) {
    if !library.needs_scan {
        return;
    }
    library.needs_scan = false;
    library.entries = library.dir.as_deref().map(file::scan).unwrap_or_default();
}

/// 読み込みの済んだ部品をモデルに置き、履歴に積んで選ぶ
fn receive_placements(
    library: Res<CustomLibrary>,
    mut model: ResMut<Model>,
    mut chart: ResMut<TimingChart>,
    mut selection: ResMut<Selection>,
    mut history: ResMut<History>,
    mut jobs: ResMut<ImportJobs>,
) {
    let results: Vec<LoadResult> = match library.receiver.lock() {
        Ok(receiver) => receiver.try_iter().collect(),
        Err(_) => return,
    };
    for LoadResult { name, at, part } in results {
        if let Some(i) = jobs.loading.iter().position(|n| *n == name) {
            jobs.loading.remove(i);
        }
        let (part, loaded) = match part {
            Ok(p) => p,
            Err(e) => {
                jobs.messages.push(format!("自作部品「{name}」: {e}"));
                continue;
            }
        };
        let step = DROP_SNAP_MM * units_per_mm(&model.unit);
        let at = (at / step).round() * step + DVec3::ZERO;
        let Some(placed) = place::place(&mut model, &mut chart, &part, &loaded, at) else {
            jobs.messages
                .push(format!("自作部品「{name}」: 部品の中身が空です"));
            continue;
        };
        let label = format!("{} を追加", model.path_label(placed.root));
        history.record_added(&model, placed.root, label);
        selection.select(placed.root);
        selection.reveal_in_tree = true;
        for w in placed.warnings {
            jobs.messages.push(format!("自作部品「{name}」: {w}"));
        }
    }
}
