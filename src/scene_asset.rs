//! 取り込むファイル (STEP / glTF / OBJ / STL / USD) を Bevy のアセットとして読む
//!
//! - 読み込みは Bevy のアセットサーバーに任せる。重い変換 (STEP のテッセレーションなど) は
//!   非同期の計算スレッドで行うので、読んでいる間も画面は止まらない
//! - 取り込んだファイルを見張り、外のツールでファイルを直して保存すると、開いたまま読み直して
//!   形状を差し替える (動きの設定は残る。1回の「元に戻す」で戻せる)
//!
//! どこにあるファイルでも読めるよう、専用のアセットの読み元 ([`SOURCE`]) を足し、絶対パスと単位を
//! `plantwright_core::formats::asset_key` で相対パスの形に詰めて呼ぶ。ファイルの中身を読んでモデルの形にするのは
//! `file_io::load_source` (Bevy に依存しない) で、読み終わったものは別スレッドの結果と同じ
//! `file_io::receive_imports` がモデルに加える。更新の見張りの判断は `plantwright_core::file_watch`。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSourceBuilder, PathStream, Reader, VecReader,
};
use bevy::asset::{AssetLoadFailedEvent, AssetLoader, AssetPath, LoadContext};
use bevy::prelude::*;
use bevy::tasks::AsyncComputeTaskPool;
use plantwright_core::file_watch::FileWatch;
use plantwright_core::formats::asset_key;

use crate::file_io::{ImportJobs, ImportTarget, Loaded, asset_loadable, load_source, source_name};
use plantwright_core::formats::UnitOptions;
use plantwright_core::model::Model;
use plantwright_core::model::StepSource;

/// 取り込むファイルのアセットの読み元の名前 (`plantwright-file://<鍵>`)
pub const SOURCE: &str = "plantwright-file";

/// ファイルの更新を調べる間隔 (秒)。落ち着くまで 1回待つので、保存してから 1〜2 秒で読み直す
const POLL_INTERVAL: f32 = 0.7;

/// 取り込むファイルの読み元を足す。`DefaultPlugins` (`AssetPlugin`) より前に足す
pub struct SceneSourcePlugin;

impl Plugin for SceneSourcePlugin {
    fn build(&self, app: &mut App) {
        app.register_asset_source(SOURCE, AssetSourceBuilder::new(|| Box::new(FileReader)));
    }
}

/// 読み込みと見張り
pub struct SceneAssetPlugin;

impl Plugin for SceneAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<SceneFile>()
            .register_asset_loader(SceneFileLoader)
            .init_resource::<SceneAssets>()
            .add_systems(
                Update,
                (start_loads, watch_files, receive_loaded)
                    .chain()
                    .before(crate::file_io::receive_imports),
            );
    }
}

/// 鍵を絶対パスに戻して読む読み元。中身は読み込み側がパスから読む (glTF の .bin や OBJ の .mtl も
/// 一緒に読むため) ので、ここではファイルがあるかだけを確かめる
struct FileReader;

fn file_of(key: &Path) -> Result<PathBuf, AssetReaderError> {
    asset_key::decode(key)
        .map(|(path, _)| path)
        .ok_or_else(|| AssetReaderError::NotFound(key.to_owned()))
}

impl AssetReader for FileReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let file = file_of(path)?;
        match std::fs::metadata(&file) {
            Ok(_) => Ok(VecReader::new(Vec::new())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(AssetReaderError::NotFound(file))
            }
            Err(e) => Err(AssetReaderError::Io(Arc::new(e))),
        }
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        // 設定ファイル (.meta) は使わない
        Err::<VecReader, _>(AssetReaderError::NotFound(path.to_owned()))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        Err(AssetReaderError::NotFound(path.to_owned()))
    }

    async fn is_directory<'a>(&'a self, _path: &'a Path) -> Result<bool, AssetReaderError> {
        Ok(false)
    }
}

/// 読み込んだファイル。中身はモデルに加えるときに 1回だけ取り出す (形状は複製できないため)。
/// 読み直すと新しい中身に入れ替わる
#[derive(Asset, TypePath)]
pub struct SceneFile {
    content: Mutex<Option<Loaded>>,
}

impl SceneFile {
    fn take(&self) -> Option<Loaded> {
        self.content.lock().ok()?.take()
    }
}

/// 読み込みの失敗 (画面に出す文)
#[derive(Debug)]
struct SceneLoadError(String);

impl std::fmt::Display for SceneLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SceneLoadError {}

#[derive(Default, TypePath)]
struct SceneFileLoader;

impl AssetLoader for SceneFileLoader {
    type Asset = SceneFile;
    type Settings = ();
    type Error = SceneLoadError;

    async fn load(
        &self,
        _reader: &mut dyn Reader,
        _settings: &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<SceneFile, SceneLoadError> {
        let key = load_context.path().path().to_owned();
        let (path, units) = asset_key::decode(&key)
            .ok_or_else(|| SceneLoadError(format!("{} を読めません", key.display())))?;
        let units = units.unwrap_or_default();
        // 重い変換は計算用のスレッドで行い、ファイル読み込みのスレッドをふさがない
        let loaded = AsyncComputeTaskPool::get()
            .spawn(async move { load_source(&StepSource::File(path), &units) })
            .await
            .map_err(SceneLoadError)?;
        Ok(SceneFile {
            content: Mutex::new(Some(loaded)),
        })
    }

    // 拡張子では選ばせない (Bevy の glTF の読み込みと取り合わない)。型で選ぶ
    fn extensions(&self) -> &[&str] {
        &[]
    }
}

fn asset_path(path: &Path, units: &UnitOptions) -> Option<AssetPath<'static>> {
    asset_key::encode(path, units).map(|key| AssetPath::from_path_buf(key).with_source(SOURCE))
}

/// 読み込みを頼んだもの
struct Pending {
    handle: Handle<SceneFile>,
    name: String,
    path: PathBuf,
    target: ImportTarget,
}

/// ファイルの印: 更新時刻と大きさ
type Stamp = (Option<SystemTime>, u64);

fn stamp(path: &PathBuf) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok(), meta.len()))
}

/// 読み込み中のファイルと、見張っているファイル
#[derive(Resource, Default)]
pub struct SceneAssets {
    pending: Vec<Pending>,
    /// 読んだファイルのハンドル (持っている間はアセットが残り、読み直しに使う)
    handles: HashMap<PathBuf, Handle<SceneFile>>,
    watch: FileWatch<PathBuf, Stamp>,
    since_poll: f32,
}

/// 頼まれた読み込みをアセットサーバーに渡す
fn start_loads(
    mut jobs: ResMut<ImportJobs>,
    mut scenes: ResMut<SceneAssets>,
    server: Res<AssetServer>,
    files: Res<Assets<SceneFile>>,
) {
    for request in std::mem::take(&mut jobs.asset_requests) {
        let Some(path) = asset_path(&request.path, &request.units) else {
            // 鍵にできないパス (`..` を含むなど) は別スレッドで読む
            jobs.spawn_thread(request.name, StepSource::File(request.path), request.target);
            continue;
        };
        let handle: Handle<SceneFile> = server.load(path.clone());
        // 同じファイルを前にも読んでいたら、中身は取り出し済みなので読み直す
        if files.get(&handle).is_some() {
            server.reload(path);
        }
        scenes.handles.insert(request.path.clone(), handle.clone());
        scenes.pending.push(Pending {
            handle,
            name: request.name,
            path: request.path,
            target: request.target,
        });
    }
}

/// 取り込んだファイルの更新を調べ、変わって落ち着いたものを読み直す
fn watch_files(
    time: Res<Time>,
    model: Res<Model>,
    jobs: Res<ImportJobs>,
    mut scenes: ResMut<SceneAssets>,
    server: Res<AssetServer>,
) {
    scenes.since_poll += time.delta_secs();
    if scenes.since_poll < POLL_INTERVAL {
        return;
    }
    scenes.since_poll = 0.0;
    let files: Vec<PathBuf> = model
        .file_sources()
        .into_iter()
        .filter(|p| asset_loadable(p).as_ref() == Some(p))
        .collect();
    scenes.watch.set_keys(files, stamp);
    let keys: Vec<PathBuf> = scenes.watch.keys().cloned().collect();
    for file in keys {
        if !scenes.watch.observe(&file, stamp(&file)) {
            continue;
        }
        // 読み込み中のものは、読み終わってから次の変化を拾う
        if scenes.pending.iter().any(|p| p.path == file) {
            continue;
        }
        let handle = match scenes.handles.get(&file) {
            Some(handle) => {
                if let Some(path) = handle.path() {
                    server.reload(path.clone());
                }
                handle.clone()
            }
            // プロジェクトから開いたファイルは、まだアセットとして読んでいない
            None => match asset_path(&file, &jobs.units) {
                Some(path) => server.load(path),
                None => continue,
            },
        };
        scenes.handles.insert(file.clone(), handle.clone());
        scenes.pending.push(Pending {
            handle,
            name: source_name(&StepSource::File(file.clone())),
            path: file.clone(),
            target: ImportTarget::Refresh(file),
        });
    }
}

/// 読み終わった (読み直した) ファイルを取り出して `receive_imports` に渡す
fn receive_loaded(
    mut events: MessageReader<AssetEvent<SceneFile>>,
    mut failures: MessageReader<AssetLoadFailedEvent<SceneFile>>,
    jobs: Res<ImportJobs>,
    mut scenes: ResMut<SceneAssets>,
    files: Res<Assets<SceneFile>>,
    server: Res<AssetServer>,
) {
    let ready: HashSet<AssetId<SceneFile>> = events
        .read()
        .filter_map(|e| match *e {
            AssetEvent::LoadedWithDependencies { id } | AssetEvent::Modified { id } => Some(id),
            _ => None,
        })
        .collect();
    let failed: HashMap<AssetId<SceneFile>, String> = failures
        .read()
        .map(|e| (e.id, e.error.to_string()))
        .collect();
    if ready.is_empty() && failed.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut scenes.pending);
    for request in pending {
        let id = request.handle.id();
        if let Some(error) = failed.get(&id) {
            jobs.deliver((
                request.name,
                StepSource::File(request.path),
                request.target,
                Err(error.clone()),
            ));
            continue;
        }
        let content = ready
            .contains(&id)
            .then(|| files.get(id).and_then(SceneFile::take))
            .flatten();
        if content.is_none()
            && ready.contains(&id)
            && let Some(path) = request.handle.path()
        {
            // 同じファイルを続けて頼んだときは、先の分が中身を取ったので、もう一度読む
            server.reload(path.clone());
        }
        match content {
            Some(loaded) => jobs.deliver((
                request.name,
                StepSource::File(request.path),
                request.target,
                Ok(loaded),
            )),
            None => scenes.pending.push(request),
        }
    }
}
