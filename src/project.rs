//! プロジェクトファイル (.cadsim) の保存と読み込み
//!
//! ファイルの中身は人が読める JSON。アセンブリの木をそのまま入れ子で書く。
//! 書くのは設計データ (名前・配置・表示/非表示・色・箱の寸法) と、取り込んだ STEP の場所だけ。
//! STEP の形状 (B-rep やメッシュ) は書かず、開くときに STEP から読み直す。
//! 3D 上の寸法と注記 (annotation.rs) も一緒に書く。部品は木の中の位置で指す。
//! そのため取り込んだ部品は「どの STEP の、どの製品か」で形状を引く。
//!
//! ファイル形式の構造体 (`ProjectFile` など) はアプリ内部の `Model` とは別に持つ。
//! 内部の型を変えてもファイルの互換を保ちやすくするため。
//!
//! 操作:
//! - ファイルメニュー / ツールバー: 新規・開く・保存・名前を付けて保存
//! - Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Shift+S
//! - `cargo run -- line.cadsim`、またはウィンドウへのドラッグ&ドロップでも開ける

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::{
    ecs::system::NonSendMarker,
    math::{DMat4, DVec3},
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, block_on, poll_once},
    window::PrimaryWindow,
};
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::annotation::{AnnotationRecord, Annotations, node_at_path, node_path, to_records};
use crate::camera::CameraRequest;
use crate::file_io::{ImportJobs, file_name, load_source, receive_imports, source_name};
use crate::floor::{FloorDoc, FloorView};
use crate::floor_record::{FloorLayout, FloorRecord};
use crate::flow::{self, FlowRes, FlowRun, model::FlowModel, record::FlowRecord};
use crate::formats::site_record::{CloudRecord, UnderlayRecord, load_site};
use crate::history::History;
use crate::interchange::UnitOptions;
use crate::math::{euler_deg_to_quat, quat_to_euler_deg, split_placement};
use crate::model::{BoxPart, Model, NodeId, NodeKind, Selection, StepSource};
use crate::site::model::Site;
use crate::step_import::{ImportNode, ImportedShape};

/// ファイル形式のバージョン。古いファイルを開けなくなる変更をしたときだけ上げる。
pub const FORMAT_VERSION: u32 = 1;
/// プロジェクトファイルの拡張子
pub const FILE_EXTENSION: &str = "cadsim";
/// ウィンドウタイトルの先頭 (後ろにファイル名を付ける)
const APP_TITLE: &str = "CADプロトタイプ";

pub struct ProjectPlugin;

impl Plugin for ProjectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ProjectState>().add_systems(
            Update,
            (
                // STEP の読み込み結果がモデルに入った後で、未保存かどうかを判断する
                run_project_actions.after(receive_imports),
                update_window_title,
            )
                .chain(),
        );
    }
}

pub fn is_project_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(FILE_EXTENSION))
}

// ---------------------------------------------------------------------------
// ファイル形式
// ---------------------------------------------------------------------------

/// .cadsim ファイルの中身そのもの。
///
/// 項目を増やすとき (タイミングチャートなど) は `#[serde(default)]` を付けたフィールドとして足す。
/// そうすれば項目のない古いファイルもそのまま開ける。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProjectFile {
    pub format_version: u32,
    /// 長さの単位 ("mm" など)
    #[serde(default = "default_unit")]
    pub unit: String,
    /// 取り込んだ STEP ファイル。部品からは番号 (0 始まり) で参照する。
    #[serde(default)]
    pub step_files: Vec<StepFileRecord>,
    /// 最上位のノード (ツリーの表示順)。子は `children` に入れ子で書く。
    #[serde(default)]
    pub nodes: Vec<NodeRecord>,
    /// 3D 上の寸法と注記
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<AnnotationRecord>,
    /// 床に敷いた平面図
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub underlays: Vec<UnderlayRecord>,
    /// 読み込んだ点群
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub point_clouds: Vec<CloudRecord>,
    /// 工程モデル (工程シミュレーション)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<FlowRecord>,
    /// 安全機器・作業者・AGV の配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor: Option<FloorRecord>,
}

fn default_unit() -> String {
    "mm".into()
}

/// 取り込んだ STEP ファイルの場所
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum StepFileRecord {
    /// アプリに埋め込みのサンプル
    Sample,
    /// プロジェクトファイルと同じフォルダ以下なら相対パス、それ以外は絶対パス
    File { path: String },
    /// 作成メニューで作った形状。`solid` は truck の B-rep を JSON にしたもの
    Modeled { name: String, solid: String },
    /// 負荷テスト用のシーン (開き直すと同じものを作り直す)
    Stress { parts: usize },
}

/// アセンブリの木の1ノード
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NodeRecord {
    pub name: String,
    /// 親の座標系から見た位置
    #[serde(default)]
    pub position: [f64; 3],
    /// 親の座標系から見た回転。X → Y → Z の順に回す角度 (度)
    #[serde(default)]
    pub rotation_deg: [f64; 3],
    #[serde(default = "visible_by_default")]
    pub visible: bool,
    /// ユーザーが付けた色 (sRGB 0..1)。子孫の部品にも効く。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 3]>,
    pub kind: NodeKindRecord,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<NodeRecord>,
}

fn visible_by_default() -> bool {
    true
}

/// ノードの種類。JSON では `"type"` で見分ける。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum NodeKindRecord {
    /// 子をまとめるだけのノード
    Assembly {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        product: Option<String>,
    },
    /// 寸法で作る箱。寸法・位置・回転はノードの配置とは別に箱自身が持つ。
    Box {
        size: [f64; 3],
        #[serde(default)]
        position: [f64; 3],
        #[serde(default)]
        rotation_deg: [f64; 3],
    },
    /// STEP から取り込んだ部品。`step_files[step_file]` の中の製品 `product` の形状を使う。
    Imported { step_file: usize, product: String },
}

impl ProjectFile {
    /// 今のモデルからファイルの内容を作る。`base_dir` はプロジェクトファイルを置くフォルダ。
    pub fn from_model(model: &Model, base_dir: Option<&Path>) -> Self {
        let mut sources = Vec::new();
        let nodes = model
            .roots
            .iter()
            .filter_map(|&id| node_record(model, id, &mut sources))
            .collect();
        Self {
            format_version: FORMAT_VERSION,
            unit: model.unit.clone(),
            step_files: sources
                .iter()
                .map(|s| StepFileRecord::from_source(s, base_dir))
                .collect(),
            nodes,
            annotations: Vec::new(),
            underlays: crate::formats::site_record::to_records(&model.site, base_dir),
            point_clouds: crate::formats::site_record::cloud_records(&model.site, base_dir),
            flow: None,
            floor: None,
        }
    }

    /// 今のモデルと寸法・注記・工程モデルからファイルの内容を作る
    pub fn capture(
        model: &Model,
        annotations: &Annotations,
        flow: &FlowModel,
        floor: &FloorRecord,
        base_dir: Option<&Path>,
    ) -> Self {
        Self {
            floor: (!floor.is_empty()).then(|| floor.clone()),
            annotations: to_records(annotations, model),
            flow: (!flow.is_empty()).then(|| {
                flow::record::to_record(flow, |id| {
                    model
                        .get(id)
                        .is_some()
                        .then(|| node_path(model, id))
                        .flatten()
                })
            }),
            ..Self::from_model(model, base_dir)
        }
    }

    pub fn to_json(&self) -> String {
        // 構造体には失敗する型 (文字列以外のマップのキーなど) を含めていないので失敗しない
        let pretty =
            serde_json::to_string_pretty(self).expect("ProjectFile is always serializable");
        compact_number_arrays(&pretty)
    }

    pub fn from_json(text: &str) -> Result<Self, ProjectError> {
        // バージョンだけ先に読み、新しすぎるファイルは中身の解釈前に断る
        #[derive(Deserialize)]
        struct VersionOnly {
            format_version: u32,
        }
        let version: VersionOnly = serde_json::from_str(text).map_err(ProjectError::Parse)?;
        if version.format_version > FORMAT_VERSION {
            return Err(ProjectError::NewerVersion(version.format_version));
        }
        let file: Self = serde_json::from_str(text).map_err(ProjectError::Parse)?;
        file.validate()?;
        Ok(file)
    }

    /// 形状を作れない値や、存在しない STEP の番号がないか確かめる。
    fn validate(&self) -> Result<(), ProjectError> {
        let mut stack: Vec<&NodeRecord> = self.nodes.iter().collect();
        while let Some(node) = stack.pop() {
            let invalid = |what: &str| {
                Err(ProjectError::Invalid(format!(
                    "「{}」の{what}が正しくありません",
                    node.name
                )))
            };
            let finite = |values: &[f64]| values.iter().all(|v| v.is_finite());
            if !finite(&node.position) || !finite(&node.rotation_deg) {
                return invalid("配置");
            }
            match &node.kind {
                NodeKindRecord::Assembly { .. } => {}
                NodeKindRecord::Box {
                    size,
                    position,
                    rotation_deg,
                } => {
                    if !finite(size) || size.iter().any(|s| *s <= 0.0) {
                        return invalid("寸法");
                    }
                    if !finite(position) || !finite(rotation_deg) {
                        return invalid("箱の位置か回転");
                    }
                }
                NodeKindRecord::Imported { step_file, .. } => {
                    if *step_file >= self.step_files.len() {
                        return invalid("STEP ファイルの番号");
                    }
                }
            }
            stack.extend(&node.children);
        }
        Ok(())
    }
}

fn node_record(model: &Model, id: NodeId, sources: &mut Vec<StepSource>) -> Option<NodeRecord> {
    let node = model.get(id)?;
    let (translation, rotation) = split_placement(node.placement);
    let kind = match &node.kind {
        NodeKind::Assembly { product } => NodeKindRecord::Assembly {
            product: product.clone(),
        },
        NodeKind::Box(part) => NodeKindRecord::Box {
            size: part.size,
            position: part.position,
            rotation_deg: part.rotation_deg,
        },
        NodeKind::Imported {
            product, source, ..
        } => {
            let step_file = match sources.iter().position(|s| s == source) {
                Some(i) => i,
                None => {
                    sources.push(source.clone());
                    sources.len() - 1
                }
            };
            NodeKindRecord::Imported {
                step_file,
                product: product.clone(),
            }
        }
    };
    Some(NodeRecord {
        name: node.name.clone(),
        position: round_noise(translation.to_array()),
        rotation_deg: quat_to_euler_deg(rotation),
        visible: node.visible,
        color: node.color,
        kind,
        children: node
            .children
            .iter()
            .filter_map(|&child| node_record(model, child, sources))
            .collect(),
    })
}

/// 行列から取り出した位置の 1e-12 のような誤差を消す (ファイルを読みやすくするため)
fn round_noise(values: [f64; 3]) -> [f64; 3] {
    values.map(|v| (v * 1e9).round() / 1e9 + 0.0)
}

impl StepFileRecord {
    fn from_source(source: &StepSource, base_dir: Option<&Path>) -> Self {
        match source {
            StepSource::Sample => StepFileRecord::Sample,
            &StepSource::Stress(parts) => StepFileRecord::Stress { parts },
            StepSource::File(path) => {
                let relative = base_dir.and_then(|dir| path.strip_prefix(dir).ok());
                let path = match relative {
                    // Windows の区切り文字も / にそろえ、どの OS でも開けるようにする
                    Some(rel) => rel.to_string_lossy().replace('\\', "/"),
                    None => path.to_string_lossy().into_owned(),
                };
                StepFileRecord::File { path }
            }
            StepSource::Modeled(modeled) => StepFileRecord::Modeled {
                name: modeled.name.clone(),
                solid: modeled.json.clone(),
            },
        }
    }

    fn to_source(&self, base_dir: &Path) -> StepSource {
        match self {
            StepFileRecord::Sample => StepSource::Sample,
            &StepFileRecord::Stress { parts } => StepSource::Stress(parts),
            StepFileRecord::File { path } => StepSource::File(base_dir.join(path)),
            StepFileRecord::Modeled { name, solid } => {
                StepSource::Modeled(Arc::new(crate::modeling::ModeledSource {
                    id: crate::step_import::next_shape_id(),
                    name: name.clone(),
                    json: solid.clone(),
                }))
            }
        }
    }
}

/// `[1.0, 2.0, 3.0]` のような数値だけの配列を1行にまとめる。
///
/// serde_json の整形は配列の要素を1行ずつ書くので、座標や寸法が縦に長くなって読みにくい。
/// JSON の文字列の中に改行はそのまま現れない (`\n` になる) ので、文字列の外だけを見ればよい。
fn compact_number_arrays(json: &str) -> String {
    let is_number_char = |c: char| c.is_ascii_digit() || "+-.eE,".contains(c) || c.is_whitespace();
    let mut out = String::with_capacity(json.len());
    let mut rest = json;
    let mut in_string = false;
    let mut escaped = false;
    while let Some(c) = rest.chars().next() {
        rest = &rest[c.len_utf8()..];
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            out.push(c);
            continue;
        }
        match c {
            '"' => in_string = true,
            '[' => {
                if let Some(end) = rest.find(']')
                    && rest[..end].chars().all(is_number_char)
                    && !rest[..end].trim().is_empty()
                {
                    let items: Vec<&str> = rest[..end].split(',').map(str::trim).collect();
                    out.push('[');
                    out.push_str(&items.join(", "));
                    out.push(']');
                    rest = &rest[end + 1..];
                    continue;
                }
            }
            _ => {}
        }
        out.push(c);
    }
    out
}

/// 保存・読み込みの失敗。そのまま画面に出す。
#[derive(Debug)]
pub enum ProjectError {
    Io(std::io::Error),
    Parse(serde_json::Error),
    NewerVersion(u32),
    Invalid(String),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ProjectError::Io(e) => write!(f, "{e}"),
            ProjectError::Parse(e) => write!(f, "プロジェクトファイルとして読めません: {e}"),
            ProjectError::NewerVersion(v) => write!(
                f,
                "ファイル形式のバージョン {v} は、このアプリ (バージョン {FORMAT_VERSION}) より新しいため開けません"
            ),
            ProjectError::Invalid(reason) => write!(f, "{reason}"),
        }
    }
}

// ---------------------------------------------------------------------------
// 読み込み (別スレッド) とモデルへの反映
// ---------------------------------------------------------------------------

/// 読み込みの終わったプロジェクト。形状は STEP から読み直してある。
pub struct LoadedProject {
    pub file: ProjectFile,
    /// `file.step_files` と同じ順の読み込み元
    pub sources: Vec<StepSource>,
    /// STEP ごとの「製品名 → 形状」。読めなかった STEP は空。
    pub shapes: Vec<HashMap<String, Arc<ImportedShape>>>,
    /// 平面図など。図面は元のファイルから読み直してある。
    pub site: Site,
    pub warnings: Vec<String>,
}

impl LoadedProject {
    /// ファイルを読み、参照している STEP もすべて読み込む。重いので別スレッドで呼ぶ。
    pub fn load(path: &Path) -> Result<Self, ProjectError> {
        let text = std::fs::read_to_string(path).map_err(ProjectError::Io)?;
        let file = ProjectFile::from_json(&text)?;
        let base_dir = path.parent().unwrap_or(Path::new("."));
        Ok(Self::fetch_shapes(file, base_dir))
    }

    fn fetch_shapes(file: ProjectFile, base_dir: &Path) -> Self {
        let sources: Vec<StepSource> = file
            .step_files
            .iter()
            .map(|r| r.to_source(base_dir))
            .collect();
        let mut warnings = Vec::new();
        let shapes = sources
            .iter()
            .map(|source| {
                let name = source_name(source);
                // OBJ / STL の単位は既定 (mm・Y 上) で読み直す
                let loaded = load_source(source, &UnitOptions::default());
                match loaded.map(|l| l.into_imported(&name, &file.unit)) {
                    Ok(imported) => {
                        let mut products = HashMap::new();
                        collect_shapes(&imported.root, &mut products);
                        products
                    }
                    Err(e) => {
                        warnings.push(format!("{name}: {e}"));
                        HashMap::new()
                    }
                }
            })
            .collect();
        let site = load_site(
            &file.underlays,
            &file.point_clouds,
            base_dir,
            crate::site::underlay::unit_in_mm(&file.unit),
            &mut warnings,
        );
        Self {
            file,
            sources,
            shapes,
            site,
            warnings,
        }
    }

    /// モデルの中身をこのプロジェクトで置き換える。形状が見つからなかった部品の注意を返す。
    pub fn apply_to(&self, model: &mut Model) -> Vec<String> {
        model.clear();
        model.unit = self.file.unit.clone();
        model.site = self.site.clone();
        let mut warnings = self.warnings.clone();
        for record in &self.file.nodes {
            self.add_record(model, None, record, &mut warnings);
        }
        warnings
    }

    fn add_record(
        &self,
        model: &mut Model,
        parent: Option<NodeId>,
        record: &NodeRecord,
        warnings: &mut Vec<String>,
    ) {
        let kind = match &record.kind {
            NodeKindRecord::Assembly { product } => NodeKind::Assembly {
                product: product.clone(),
            },
            NodeKindRecord::Box {
                size,
                position,
                rotation_deg,
            } => NodeKind::Box(BoxPart {
                size: *size,
                position: *position,
                rotation_deg: *rotation_deg,
            }),
            NodeKindRecord::Imported { step_file, product } => {
                match self.shapes.get(*step_file).and_then(|s| s.get(product)) {
                    Some(shape) => NodeKind::Imported {
                        product: product.clone(),
                        shape: shape.clone(),
                        source: self.sources[*step_file].clone(),
                    },
                    // 形状がなくても、名前と配置はツリーに残して気づけるようにする
                    None => {
                        warnings.push(format!(
                            "「{}」の形状 ({product}) が STEP に見つかりませんでした",
                            record.name
                        ));
                        NodeKind::Assembly {
                            product: Some(product.clone()),
                        }
                    }
                }
            }
        };
        let placement = DMat4::from_rotation_translation(
            euler_deg_to_quat(record.rotation_deg),
            DVec3::from(record.position),
        );
        let id = model.add_node(parent, record.name.clone(), placement, kind);
        if let Some(node) = model.get_mut(id) {
            node.visible = record.visible;
            node.color = record.color;
        }
        for child in &record.children {
            self.add_record(model, Some(id), child, warnings);
        }
    }
}

/// STEP の木から「製品名 → 形状」の表を作る
fn collect_shapes(node: &ImportNode, out: &mut HashMap<String, Arc<ImportedShape>>) {
    if let Some(shape) = &node.shape {
        out.entry(node.product.clone())
            .or_insert_with(|| shape.clone());
    }
    for child in &node.children {
        collect_shapes(child, out);
    }
}

// ---------------------------------------------------------------------------
// アプリ側の状態と操作
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum ProjectAction {
    New,
    OpenPath(PathBuf),
    Save,
    SaveAs,
}

impl ProjectAction {
    /// 今のモデルを捨てる操作か (未保存の変更があれば確認する)
    fn discards_model(&self) -> bool {
        matches!(self, ProjectAction::New | ProjectAction::OpenPath(_))
    }
}

type LoadTask = Task<(PathBuf, Result<LoadedProject, ProjectError>)>;

#[derive(Resource)]
pub struct ProjectState {
    /// 今開いているファイル。まだ保存していなければ None
    path: Option<PathBuf>,
    /// 最後に保存または読み込んだときの内容。今のモデルと比べて未保存の変更を見つける。
    saved: ProjectFile,
    /// 起動直後のサンプル読み込みが終わるまでは、それも「保存済み」の状態に含める
    startup: bool,
    /// メニューやショートカットから頼まれた操作。(操作, 破棄の確認が済んでいるか)
    requested: Option<(ProjectAction, bool)>,
    /// 未保存の変更を捨ててよいか確認している操作
    confirm_discard: Option<ProjectAction>,
    loading: Option<LoadTask>,
    /// 直前のフレームで未保存の変更があったか (`update_window_title` が毎フレーム更新する)
    unsaved: bool,
}

impl Default for ProjectState {
    fn default() -> Self {
        Self {
            path: None,
            saved: ProjectFile::from_model(&Model::default(), None),
            startup: true,
            requested: None,
            confirm_discard: None,
            loading: None,
            unsaved: false,
        }
    }
}

impl ProjectState {
    pub fn request(&mut self, action: ProjectAction) {
        self.requested = Some((action, false));
    }

    pub fn has_unsaved_changes(
        &self,
        model: &Model,
        annotations: &Annotations,
        flow: &FlowModel,
        floor: &FloorRecord,
    ) -> bool {
        self.saved != ProjectFile::capture(model, annotations, flow, floor, self.base_dir())
    }

    /// ファイルに結びついているか (保存したか、ファイルから開いたか)
    pub fn has_file(&self) -> bool {
        self.path.is_some()
    }

    /// 直前のフレームで調べた未保存の変更の有無 (比べ直さないので軽い)
    pub fn unsaved(&self) -> bool {
        self.unsaved
    }

    pub fn file_name(&self) -> String {
        match &self.path {
            Some(path) => file_name(path),
            None => "無題".into(),
        }
    }

    fn base_dir(&self) -> Option<&Path> {
        self.path.as_deref().and_then(Path::parent)
    }

    fn mark_saved(
        &mut self,
        model: &Model,
        annotations: &Annotations,
        flow: &FlowModel,
        floor: &FloorRecord,
    ) {
        self.saved = ProjectFile::capture(model, annotations, flow, floor, self.base_dir());
    }

    fn save(
        &mut self,
        path: PathBuf,
        model: &Model,
        annotations: &Annotations,
        flow: &FlowModel,
        floor: &FloorRecord,
    ) -> Result<(), ProjectError> {
        let base_dir = path.parent().map(Path::to_path_buf);
        let file = ProjectFile::capture(model, annotations, flow, floor, base_dir.as_deref());
        std::fs::write(&path, file.to_json()).map_err(ProjectError::Io)?;
        self.path = Some(path);
        self.saved = file;
        Ok(())
    }

    fn start_loading(&mut self, path: PathBuf) {
        self.loading = Some(AsyncComputeTaskPool::get().spawn(async move {
            let result = LoadedProject::load(&path);
            (path, result)
        }));
    }
}

/// 拡張子なしで名前を付けられたら .cadsim を付ける
fn with_extension(path: PathBuf) -> PathBuf {
    if path.extension().is_some() {
        path
    } else {
        path.with_extension(FILE_EXTENSION)
    }
}

/// 頼まれた操作を実行し、読み込みが終わっていればモデルに反映する。
/// 新規作成・開くと元に戻す履歴は消す (前のプロジェクトに戻せてしまわないように)。
/// 「名前を付けて保存」のダイアログは macOS ではメインスレッドで出す必要があるので NonSendMarker を付ける。
#[allow(clippy::too_many_arguments)]
fn run_project_actions(
    mut state: ResMut<ProjectState>,
    mut model: ResMut<Model>,
    mut annotations: ResMut<Annotations>,
    mut flow: ResMut<FlowRes>,
    mut flow_run: ResMut<FlowRun>,
    mut floor: FloorDoc,
    mut selection: ResMut<Selection>,
    mut jobs: ResMut<ImportJobs>,
    mut camera: MessageWriter<CameraRequest>,
    mut history: ResMut<History>,
    _main_thread: NonSendMarker,
) {
    let state = &mut *state;
    if state.startup {
        state.mark_saved(&model, &annotations, &flow, &floor.record());
        state.startup = !jobs.loading.is_empty();
    }

    if let Some((path, result)) = state.loading.as_mut().and_then(|t| block_on(poll_once(t))) {
        state.loading = None;
        let name = file_name(&path);
        jobs.loading.retain(|n| *n != name);
        match result {
            Ok(loaded) => {
                let mut warnings = loaded.apply_to(&mut model);
                let (restored, skipped) =
                    Annotations::from_records(&loaded.file.annotations, &model);
                *annotations = restored;
                let (restored_flow, flow_warnings) = match &loaded.file.flow {
                    Some(record) => flow::record::from_record(record, |path| {
                        node_at_path(&model, path).filter(|&id| model.get(id).is_some())
                    }),
                    None => (FlowModel::default(), Vec::new()),
                };
                flow.0 = restored_flow;
                flow_run.stop();
                warnings.extend(flow_warnings);
                floor.apply(
                    loaded
                        .file
                        .floor
                        .as_ref()
                        .map(FloorRecord::restore)
                        .unwrap_or_default(),
                );
                if skipped > 0 {
                    warnings.push(format!(
                        "部品が見つからない寸法・注記が {skipped} 件あったため読み飛ばしました"
                    ));
                }
                *history = History::default();
                selection.clear();
                camera.write(CameraRequest::FitAll);
                state.path = Some(path);
                state.mark_saved(&model, &annotations, &flow, &floor.record());
                let mut message = format!("{name} を開きました");
                if !warnings.is_empty() {
                    message += &format!("。注意が {} 件あります", warnings.len());
                }
                for warning in warnings {
                    warn!("{warning}");
                    jobs.messages.push(warning);
                }
                jobs.messages.push(message);
            }
            Err(e) => {
                error!("{}: {e}", path.display());
                jobs.messages
                    .push(format!("{name} を開けませんでした: {e}"));
            }
        }
    }

    let Some((action, confirmed)) = state.requested.take() else {
        return;
    };
    if state.loading.is_some() {
        jobs.messages.push("プロジェクトを読み込み中です".into());
        return;
    }
    if action.discards_model()
        && !confirmed
        && state.has_unsaved_changes(&model, &annotations, &flow, &floor.record())
    {
        state.confirm_discard = Some(action);
        return;
    }
    match action {
        ProjectAction::New => {
            model.clear();
            model.site = Site::default();
            *annotations = Annotations::default();
            flow.0 = FlowModel::default();
            flow_run.stop();
            floor.apply(FloorLayout::default());
            *history = History::default();
            selection.clear();
            state.path = None;
            state.mark_saved(&model, &annotations, &flow, &floor.record());
            jobs.messages.push("新しいプロジェクトを作りました".into());
        }
        ProjectAction::OpenPath(path) => {
            jobs.loading.push(file_name(&path));
            state.start_loading(path);
        }
        ProjectAction::Save | ProjectAction::SaveAs => {
            let path = match (&action, &state.path) {
                (ProjectAction::Save, Some(path)) => Some(path.clone()),
                _ => rfd::FileDialog::new()
                    .set_title("名前を付けて保存")
                    .add_filter("プロジェクト", &[FILE_EXTENSION])
                    .set_file_name(match &state.path {
                        Some(_) => state.file_name(),
                        None => format!("無題.{FILE_EXTENSION}"),
                    })
                    .save_file()
                    .map(with_extension),
            };
            // ダイアログをキャンセルしたら何もしない
            let Some(path) = path else {
                return;
            };
            match state.save(path.clone(), &model, &annotations, &flow, &floor.record()) {
                Ok(()) => jobs
                    .messages
                    .push(format!("{} を保存しました", file_name(&path))),
                Err(e) => {
                    error!("{}: {e}", path.display());
                    jobs.messages
                        .push(format!("{} を保存できませんでした: {e}", file_name(&path)));
                }
            }
        }
    }
}

/// 未保存の変更を捨ててよいかの確認。ui/mod.rs から毎フレーム呼ぶ。
pub fn show_confirm_dialog(ctx: &egui::Context, state: &mut ProjectState) {
    let Some(action) = state.confirm_discard.clone() else {
        return;
    };
    let file_name = state.file_name();
    let modal = egui::Modal::new(egui::Id::new("confirm_discard")).show(ctx, |ui| {
        ui.heading("保存していない変更を破棄しますか?");
        ui.label(format!("{file_name} には保存していない変更があります。"));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("破棄する").clicked() {
                Some(Choice::Discard)
            } else if ui.button("キャンセル").clicked() {
                Some(Choice::Cancel)
            } else {
                None
            }
        })
        .inner
    });
    let choice = match modal.inner {
        None if modal.should_close() => Some(Choice::Cancel),
        choice => choice,
    };
    match choice {
        Some(Choice::Discard) => {
            state.confirm_discard = None;
            state.requested = Some((action, true));
        }
        Some(Choice::Cancel) => state.confirm_discard = None,
        None => {}
    }
}

enum Choice {
    Discard,
    Cancel,
}

/// ウィンドウのタイトルにファイル名と未保存の印 (*) を出す。
fn update_window_title(
    mut state: ResMut<ProjectState>,
    model: Res<Model>,
    annotations: Res<Annotations>,
    flow: Res<FlowRes>,
    floor: FloorView,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    let unsaved = state.has_unsaved_changes(&model, &annotations, &flow, &floor.record());
    // 毎フレーム書くと変更検知が走り続けるので、変わったときだけ書く
    if state.unsaved != unsaved {
        state.unsaved = unsaved;
    }
    let marker = if unsaved { " *" } else { "" };
    let title = format!("{APP_TITLE} - {}{marker}", state.file_name());
    // 毎フレーム書き換えると変更検知が走り続けるので、変わったときだけ書く
    if window.title != title {
        window.title = title;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_io::read_step;
    use crate::step_import::import_step;

    /// サンプルの STEP を取り込み、箱を1つ足して、いくつか編集したモデル
    fn sample_model() -> Model {
        let mut model = Model::default();
        let source = StepSource::Sample;
        let imported = import_step(&read_step(&source).unwrap(), &source_name(&source)).unwrap();
        let root = model.add_imported(imported.root, imported.unit, source);
        let first_child = model.get(root).unwrap().children[0];
        let node = model.get_mut(first_child).unwrap();
        node.visible = false;
        node.color = Some([1.0, 0.5, 0.25]);
        node.name = "名前を変えた部品".into();
        let id = model.add_box([0.5, 0.25, 1.5], [300.0, 0.125, -1.0]);
        let node = model.get_mut(id).unwrap();
        node.placement = DMat4::from_rotation_translation(
            euler_deg_to_quat([0.0, 30.0, 0.0]),
            DVec3::new(10.0, 0.0, 5.0),
        );
        model
    }

    fn reload(file: &ProjectFile) -> Model {
        let parsed = ProjectFile::from_json(&file.to_json()).unwrap();
        let loaded = LoadedProject::fetch_shapes(parsed, Path::new("."));
        let mut model = Model::default();
        // 置き換え前のものが残らないことも確かめるため、箱を1つ置いておく
        model.add_box([1.0; 3], [0.0; 3]);
        let warnings = loaded.apply_to(&mut model);
        assert!(warnings.is_empty(), "{warnings:?}");
        model
    }

    #[test]
    fn round_trip_keeps_tree_placement_and_appearance() {
        let model = sample_model();
        let file = ProjectFile::from_model(&model, None);
        let restored = reload(&file);
        assert_eq!(ProjectFile::from_model(&restored, None), file);
        assert_eq!(restored.roots.len(), 2);
        let root = restored.roots[0];
        assert_eq!(restored.measure(root).part_count, 7);
        // 取り込んだ部品の形状は STEP から読み直されている
        let first = restored.get(root).unwrap().children[0];
        assert!(!restored.get(first).unwrap().visible);
        assert_eq!(restored.get(first).unwrap().color, Some([1.0, 0.5, 0.25]));
    }

    #[test]
    fn json_is_human_readable() {
        let json = ProjectFile::from_model(&sample_model(), None).to_json();
        assert!(json.contains("\"format_version\": 1"));
        assert!(json.contains("\"type\": \"Sample\""));
        assert!(json.contains("\"type\": \"Imported\""));
        assert!(json.contains("\"name\": \"名前を変えた部品\""));
        // 座標や寸法は1行に収まり、行列の誤差も出ない
        assert!(json.contains("\"size\": [0.5, 0.25, 1.5]"));
        assert!(json.contains("\"rotation_deg\": [0.0, 30.0, 0.0]"));
        assert!(json.contains("\"position\": [10.0, 0.0, 5.0]"));
    }

    #[test]
    fn missing_shape_keeps_the_node_with_a_warning() {
        let json = r#"{
            "format_version": 1,
            "step_files": [ { "type": "Sample" } ],
            "nodes": [
                { "name": "Ghost", "kind": { "type": "Imported", "step_file": 0, "product": "NoSuchPart" } }
            ]
        }"#;
        let file = ProjectFile::from_json(json).unwrap();
        let loaded = LoadedProject::fetch_shapes(file, Path::new("."));
        let mut model = Model::default();
        let warnings = loaded.apply_to(&mut model);
        assert_eq!(warnings.len(), 1);
        assert_eq!(model.get(model.roots[0]).unwrap().name, "Ghost");
        // 省略した項目は既定値になる
        assert!(model.get(model.roots[0]).unwrap().visible);
        assert_eq!(model.unit, "mm");
    }

    #[test]
    fn rejects_newer_versions_and_bad_values() {
        assert!(matches!(
            ProjectFile::from_json(r#"{ "format_version": 99, "nodes": "anything" }"#),
            Err(ProjectError::NewerVersion(99))
        ));
        let flat_box = r#"{ "format_version": 1, "nodes": [
            { "name": "P", "kind": { "type": "Box", "size": [1, 0, 1] } } ] }"#;
        assert!(matches!(
            ProjectFile::from_json(flat_box),
            Err(ProjectError::Invalid(_))
        ));
        let missing_step = r#"{ "format_version": 1, "nodes": [
            { "name": "P", "kind": { "type": "Imported", "step_file": 0, "product": "X" } } ] }"#;
        assert!(matches!(
            ProjectFile::from_json(missing_step),
            Err(ProjectError::Invalid(_))
        ));
        assert!(matches!(
            ProjectFile::from_json("not json"),
            Err(ProjectError::Parse(_))
        ));
    }

    #[test]
    fn step_paths_are_relative_to_the_project() {
        let dir = Path::new("/work/line1");
        let inside = StepFileRecord::from_source(
            &StepSource::File(dir.join("parts").join("vise.step")),
            Some(dir),
        );
        assert_eq!(
            inside,
            StepFileRecord::File {
                path: "parts/vise.step".into()
            }
        );
        assert_eq!(
            inside.to_source(dir),
            StepSource::File(dir.join("parts/vise.step"))
        );
        let outside =
            StepFileRecord::from_source(&StepSource::File("/other/a.step".into()), Some(dir));
        assert_eq!(
            outside,
            StepFileRecord::File {
                path: "/other/a.step".into()
            }
        );
    }

    #[test]
    fn save_and_open_through_a_file() {
        let dir = std::env::temp_dir().join(format!("cadsim-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("line.cadsim");
        let model = sample_model();
        let mut state = ProjectState::default();
        let none = Annotations::default();
        state
            .save(
                path.clone(),
                &model,
                &none,
                &FlowModel::default(),
                &FloorRecord::default(),
            )
            .unwrap();
        assert!(!state.has_unsaved_changes(
            &model,
            &none,
            &FlowModel::default(),
            &FloorRecord::default()
        ));
        let loaded = LoadedProject::load(&path);
        std::fs::remove_dir_all(&dir).ok();
        let mut restored = Model::default();
        loaded.unwrap().apply_to(&mut restored);
        assert!(!state.has_unsaved_changes(
            &restored,
            &none,
            &FlowModel::default(),
            &FloorRecord::default()
        ));
    }

    #[test]
    fn flow_model_is_saved_with_the_project() {
        let model = sample_model();
        let part = model.roots[0];
        let mut line = crate::flow::model::sample_line();
        line.elements[1].part = Some(part);
        let file = ProjectFile::capture(
            &model,
            &Annotations::default(),
            &line,
            &FloorRecord::default(),
            None,
        );
        let parsed = ProjectFile::from_json(&file.to_json()).unwrap();
        let restored = reload(&parsed);
        let (flow, warnings) = flow::record::from_record(parsed.flow.as_ref().unwrap(), |p| {
            node_at_path(&restored, p)
        });
        assert!(warnings.is_empty());
        assert_eq!(flow.elements.len(), line.elements.len());
        assert_eq!(flow.elements[1].part, Some(restored.roots[0]));
        assert_eq!(flow.elements[2].kind, line.elements[2].kind);
        // 工程モデルが空なら書かない
        let empty = ProjectFile::capture(
            &model,
            &Annotations::default(),
            &FlowModel::default(),
            &FloorRecord::default(),
            None,
        );
        assert!(!empty.to_json().contains("\"flow\""));
    }

    #[test]
    fn annotations_are_saved_with_the_project() {
        use crate::annotation::{Anchor, AnnotationKind};
        let model = sample_model();
        let part = model.roots[1];
        let mut annotations = Annotations::default();
        annotations.add(AnnotationKind::Note {
            at: Anchor {
                node: part,
                local: [1.0, 1.0, 1.0],
            },
            text: "ストッパー位置\n要確認".into(),
        });
        let file = ProjectFile::capture(
            &model,
            &annotations,
            &FlowModel::default(),
            &FloorRecord::default(),
            None,
        );
        let json = file.to_json();
        assert!(json.contains("\"annotations\""), "{json}");
        let restored = reload(&file);
        let parsed = ProjectFile::from_json(&json).unwrap();
        let (reopened, skipped) = Annotations::from_records(&parsed.annotations, &restored);
        assert_eq!(skipped, 0);
        assert_eq!(
            ProjectFile::capture(
                &restored,
                &reopened,
                &FlowModel::default(),
                &FloorRecord::default(),
                None
            ),
            file
        );
        // 寸法・注記のない古いファイルもそのまま開ける
        assert!(
            !ProjectFile::from_model(&model, None)
                .to_json()
                .contains("annotations")
        );
    }

    #[test]
    fn compacting_leaves_strings_alone() {
        let json =
            "{\n  \"name\": \"[1,\\\" 2]\",\n  \"v\": [\n    1.0,\n    -2e-3\n  ],\n  \"e\": []\n}";
        let compact = compact_number_arrays(json);
        assert!(compact.contains("\"name\": \"[1,\\\" 2]\""));
        assert!(compact.contains("\"v\": [1.0, -2e-3]"));
        assert!(compact.contains("\"e\": []"));
    }

    #[test]
    fn save_as_adds_the_extension() {
        assert_eq!(
            with_extension(PathBuf::from("line1")),
            PathBuf::from("line1.cadsim")
        );
        assert!(is_project_file(Path::new("a/LINE.CADSIM")));
        assert!(!is_project_file(Path::new("a/vise.step")));
    }
}
