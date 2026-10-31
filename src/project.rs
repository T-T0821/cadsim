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

use std::collections::{BTreeMap, HashMap};
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

use crate::animation::Animations;
use crate::annotation::{AnnotationRecord, Annotations, node_at_path, node_path, to_records};
use crate::camera::CameraRequest;
use crate::conveyor::ConveyorConfigRes;
use crate::file_io::{ImportJobs, file_name, load_source, receive_imports, source_name};
use crate::floor::{FloorDoc, FloorView};
use crate::floor_record::{FloorLayout, FloorRecord};
use crate::flow::{self, FlowRes, FlowRun, model::FlowModel, record::FlowRecord};
use crate::formats::chart_record::{
    AnimationRecord, ChartRecord, chart_from_record, chart_to_record,
};
use crate::formats::conveyor_record::{self, ConveyorsRecord};
use crate::formats::site_record::{CloudRecord, UnderlayRecord, load_site};
use crate::history::History;
use crate::interchange::UnitOptions;
use crate::library::{self, PartKind, Spec};
use crate::math::{euler_deg_to_quat, quat_to_euler_deg, split_placement};
use crate::model::{BoxPart, Model, NodeId, NodeKind, Selection, StepSource};
use crate::motion::{Playback, TimingChart};
use crate::physics::{CableRecord, PartPhysicsRecord};
use crate::plc::{Plc, PlcRecord};
use crate::recording::is_recording;
use crate::site::model::Site;
use crate::step_import::{ImportNode, ImportedShape};
use cadsim_core::conveyor::ConveyorConfig;

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
                // 動画の書き出し中は進み具合をタイトルに出すので書き換えない
                update_window_title.run_if(not(is_recording)),
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
    /// 部品の間に渡したケーブル・ホース
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cables: Vec<CableRecord>,
    /// タイミングチャート (アクチュエータとステップ)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart: Option<ChartRecord>,
    /// glTF などから取り込んだアニメーション
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub animations: Vec<AnimationRecord>,
    /// PLC 連携の設定 (I/O の割り付けと外部 PLC の接続先)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plc: Option<PlcRecord>,
    /// コンベアのワーク搬送とグリッパの設定
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conveyors: Option<ConveyorsRecord>,
}

/// プロジェクトに保存する設計データ一式 (モデル以外の文書も含む)
pub struct ProjectDocs<'a> {
    pub model: &'a Model,
    pub annotations: &'a Annotations,
    pub flow: &'a FlowModel,
    pub floor: FloorRecord,
    pub chart: &'a TimingChart,
    pub animations: &'a Animations,
    pub plc: Option<PlcRecord>,
    pub conveyors: &'a ConveyorConfig,
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
    /// 物理演算での振る舞い。既定 (機構で動く) なら省く。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physics: Option<PartPhysicsRecord>,
    /// 部品スクリプト (Rhai のソース)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
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
    /// 標準部品。本体 (子) は書かず、開くときにパラメータから作り直す。
    /// `part` は部品の種類 (`air_cylinder` など)、`params` は寸法パラメータ (mm)。
    Standard {
        part: String,
        #[serde(default)]
        params: BTreeMap<String, f64>,
    },
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
            cables: model
                .cables
                .iter()
                .map(|c| CableRecord::from_cable(c, |id| node_path(model, id)))
                .collect(),
            chart: None,
            animations: Vec::new(),
            plc: None,
            conveyors: None,
        }
    }

    /// 今のモデルと、寸法・注記・工程モデル・床の配置・タイミングチャートなどからファイルの内容を作る
    pub fn capture(docs: &ProjectDocs, base_dir: Option<&Path>) -> Self {
        let model = docs.model;
        let path_of = |id: NodeId| {
            model
                .get(id)
                .is_some()
                .then(|| node_path(model, id))
                .flatten()
        };
        let chart = docs.chart;
        Self {
            floor: (!docs.floor.is_empty()).then(|| docs.floor.clone()),
            annotations: to_records(docs.annotations, model),
            flow: (!docs.flow.is_empty()).then(|| flow::record::to_record(docs.flow, path_of)),
            chart: (!chart.actuators.is_empty() || !chart.steps.is_empty())
                .then(|| chart_to_record(chart, path_of)),
            animations: docs.animations.to_records(path_of),
            plc: docs.plc.clone(),
            conveyors: conveyor_record::to_record(docs.conveyors, path_of),
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
            if node.physics.as_ref().is_some_and(|p| !p.is_valid()) {
                return invalid("物理の設定");
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
                NodeKindRecord::Standard { part, params } => {
                    if PartKind::from_key(part).is_none() {
                        return invalid("標準部品の種類");
                    }
                    if !params.values().all(|v| v.is_finite()) {
                        return invalid("標準部品のパラメータ");
                    }
                }
            }
            stack.extend(&node.children);
        }
        if let Some(cable) = self.cables.iter().find(|c| !c.is_valid()) {
            return Err(ProjectError::Invalid(format!(
                "ケーブル「{}」の寸法が正しくありません",
                cable.name
            )));
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
        NodeKind::Standard(spec) => NodeKindRecord::Standard {
            part: spec.kind.key().into(),
            params: spec
                .kind
                .param_specs()
                .iter()
                .map(|p| (p.key.to_string(), spec.get(p.key)))
                .collect(),
        },
        // 本体は親の標準部品のパラメータから作り直すので書かない
        NodeKind::StandardBody { .. } => return None,
    };
    Some(NodeRecord {
        name: node.name.clone(),
        position: round_noise(translation.to_array()),
        rotation_deg: quat_to_euler_deg(rotation),
        visible: node.visible,
        color: node.color,
        kind,
        physics: PartPhysicsRecord::from_settings(&node.physics),
        script: node.script.clone(),
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
        for record in &self.file.cables {
            match record.to_cable(|path| node_at_path(model, path)) {
                Some(cable) => model.cables.push(cable),
                None => warnings.push(format!(
                    "ケーブル「{}」の端の部品が見つからないため読み飛ばしました",
                    record.name
                )),
            }
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
            NodeKindRecord::Standard { part, params } => {
                let kind = PartKind::from_key(part).expect("validate で確かめ済み");
                let mut spec = Spec::new(kind);
                for (value, p) in spec.params.0.iter_mut().zip(kind.param_specs()) {
                    if let Some(v) = params.get(p.key) {
                        *value = *v;
                    }
                }
                spec.clamp_params();
                NodeKind::Standard(spec)
            }
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
            node.physics = record
                .physics
                .as_ref()
                .map(PartPhysicsRecord::to_settings)
                .unwrap_or_default();
            node.script = record.script.clone();
        }
        if matches!(record.kind, NodeKindRecord::Standard { .. }) {
            library::rebuild_bodies(model, id);
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

    pub fn has_unsaved_changes(&self, docs: &ProjectDocs) -> bool {
        self.saved != ProjectFile::capture(docs, self.base_dir())
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

    fn mark_saved(&mut self, docs: &ProjectDocs) {
        self.saved = ProjectFile::capture(docs, self.base_dir());
    }

    fn save(&mut self, path: PathBuf, docs: &ProjectDocs) -> Result<(), ProjectError> {
        let base_dir = path.parent().map(Path::to_path_buf);
        let file = ProjectFile::capture(docs, base_dir.as_deref());
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

/// タイミングチャート・取り込んだアニメーション・PLC 連携の設定の読み書き
#[derive(bevy::ecs::system::SystemParam)]
pub struct MotionDoc<'w> {
    chart: ResMut<'w, TimingChart>,
    playback: ResMut<'w, Playback>,
    animations: ResMut<'w, Animations>,
    plc: ResMut<'w, Plc>,
    conveyors: ResMut<'w, ConveyorConfigRes>,
}

impl MotionDoc<'_> {
    /// 開いた (または新規の) プロジェクトのものに置き換え、再生位置を最初に戻す
    fn replace(
        &mut self,
        chart: TimingChart,
        animations: Animations,
        plc: Option<&PlcRecord>,
        conveyors: ConveyorConfig,
    ) {
        *self.chart = chart;
        self.conveyors.0 = conveyors;
        *self.animations = animations;
        self.plc.restore(plc);
        let playback = &mut *self.playback;
        playback.time = 0.0;
        playback.playing = false;
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
    mut motion: MotionDoc,
    _main_thread: NonSendMarker,
) {
    // 保存・比較する設計データ一式 (呼ぶたびに今の値から作る)
    macro_rules! docs {
        () => {
            ProjectDocs {
                model: &model,
                annotations: &annotations,
                flow: &flow,
                floor: floor.record(),
                chart: &motion.chart,
                animations: &motion.animations,
                plc: motion.plc.record(),
                conveyors: &motion.conveyors,
            }
        };
    }
    let state = &mut *state;
    if state.startup {
        state.mark_saved(&docs!());
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
                let find = |path: &[usize]| node_at_path(&model, path);
                let (chart, chart_warnings) = match &loaded.file.chart {
                    Some(record) => chart_from_record(record, find),
                    None => (TimingChart::default(), Vec::new()),
                };
                warnings.extend(chart_warnings);
                let (animations, missing) = Animations::from_records(&loaded.file.animations, find);
                if missing > 0 {
                    warnings.push(format!(
                        "部品が見つからないアニメーションの動きが {missing} 件あったため読み飛ばしました"
                    ));
                }
                let (conveyors, conveyor_warnings) =
                    conveyor_record::from_record(loaded.file.conveyors.as_ref(), find);
                warnings.extend(conveyor_warnings);
                motion.replace(chart, animations, loaded.file.plc.as_ref(), conveyors);
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
                history.clear();
                selection.clear();
                camera.write(CameraRequest::FitAll);
                state.path = Some(path);
                state.mark_saved(&docs!());
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
    if action.discards_model() && !confirmed && state.has_unsaved_changes(&docs!()) {
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
            motion.replace(
                TimingChart::default(),
                Animations::default(),
                None,
                ConveyorConfig::default(),
            );
            history.clear();
            selection.clear();
            state.path = None;
            state.mark_saved(&docs!());
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
            match state.save(path.clone(), &docs!()) {
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
#[allow(clippy::too_many_arguments)]
fn update_window_title(
    mut state: ResMut<ProjectState>,
    model: Res<Model>,
    annotations: Res<Annotations>,
    flow: Res<FlowRes>,
    floor: FloorView,
    chart: Res<TimingChart>,
    animations: Res<Animations>,
    plc: Res<Plc>,
    conveyors: Res<ConveyorConfigRes>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    let docs = ProjectDocs {
        model: &model,
        annotations: &annotations,
        flow: &flow,
        floor: floor.record(),
        chart: &chart,
        animations: &animations,
        plc: plc.record(),
        conveyors: &conveyors,
    };
    let unsaved = state.has_unsaved_changes(&docs);
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

    /// モデル以外は空の文書。テストで必要なものだけ書き換えて使う
    #[derive(Default)]
    struct Docs {
        annotations: Annotations,
        flow: FlowModel,
        chart: TimingChart,
        animations: Animations,
        conveyors: ConveyorConfig,
    }

    impl Docs {
        fn of<'a>(&'a self, model: &'a Model) -> ProjectDocs<'a> {
            ProjectDocs {
                model,
                annotations: &self.annotations,
                flow: &self.flow,
                floor: FloorRecord::default(),
                chart: &self.chart,
                animations: &self.animations,
                plc: None,
                conveyors: &self.conveyors,
            }
        }
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
        let mut model = sample_model();
        let root = model.roots[0];
        model.get_mut(root).unwrap().script = Some("wait(1.0);".into());
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
        assert_eq!(
            restored.get(root).unwrap().script.as_deref(),
            Some("wait(1.0);")
        );
    }

    #[test]
    fn standard_parts_are_saved_as_parameters_and_rebuilt() {
        let mut model = Model::default();
        let id = library::add_standard(
            &mut model,
            PartKind::AirCylinder,
            DVec3::new(100.0, 0.0, 0.0),
        );
        if let Some(NodeKind::Standard(spec)) = model.get_mut(id).map(|n| &mut n.kind) {
            spec.params.0[1] = 250.0;
        }
        library::rebuild_bodies(&mut model, id);
        let body_count = model.get(id).unwrap().children.len();
        let file = ProjectFile::from_model(&model, None);
        let json = file.to_json();
        assert!(json.contains("\"part\": \"air_cylinder\""));
        assert!(json.contains("\"stroke\": 250.0"));
        // 本体は書かない
        assert!(file.nodes[0].children.is_empty());

        let restored = reload(&file);
        let root = restored.roots[0];
        assert_eq!(restored.get(root).unwrap().children.len(), body_count);
        let Some(NodeKind::Standard(spec)) = restored.get(root).map(|n| &n.kind) else {
            panic!("標準部品として読めていない");
        };
        assert_eq!(spec.get("stroke"), 250.0);
        assert_eq!(ProjectFile::from_model(&restored, None), file);
    }

    #[test]
    fn unknown_standard_part_is_rejected() {
        let json = r#"{
            "format_version": 1,
            "nodes": [ { "name": "?", "kind": { "type": "Standard", "part": "warp_drive" } } ]
        }"#;
        assert!(matches!(
            ProjectFile::from_json(json),
            Err(ProjectError::Invalid(_))
        ));
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
        let none = Docs::default();
        state.save(path.clone(), &none.of(&model)).unwrap();
        assert!(!state.has_unsaved_changes(&none.of(&model)));
        let loaded = LoadedProject::load(&path);
        std::fs::remove_dir_all(&dir).ok();
        let mut restored = Model::default();
        loaded.unwrap().apply_to(&mut restored);
        assert!(!state.has_unsaved_changes(&none.of(&restored)));
    }

    #[test]
    fn flow_model_is_saved_with_the_project() {
        let model = sample_model();
        let part = model.roots[0];
        let mut docs = Docs {
            flow: crate::flow::model::sample_line(),
            ..Default::default()
        };
        docs.flow.elements[1].part = Some(part);
        let line = &docs.flow;
        let file = ProjectFile::capture(&docs.of(&model), None);
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
        let empty = ProjectFile::capture(&Docs::default().of(&model), None);
        assert!(!empty.to_json().contains("\"flow\""));
    }

    #[test]
    fn annotations_are_saved_with_the_project() {
        use crate::annotation::{Anchor, AnnotationKind};
        let model = sample_model();
        let part = model.roots[1];
        let mut docs = Docs::default();
        docs.annotations.add(AnnotationKind::Note {
            at: Anchor {
                node: part,
                local: [1.0, 1.0, 1.0],
            },
            text: "ストッパー位置\n要確認".into(),
        });
        let file = ProjectFile::capture(&docs.of(&model), None);
        let json = file.to_json();
        assert!(json.contains("\"annotations\""), "{json}");
        let restored = reload(&file);
        let parsed = ProjectFile::from_json(&json).unwrap();
        let (reopened, skipped) = Annotations::from_records(&parsed.annotations, &restored);
        assert_eq!(skipped, 0);
        let docs = Docs {
            annotations: reopened,
            ..Default::default()
        };
        assert_eq!(ProjectFile::capture(&docs.of(&restored), None), file);
        // 寸法・注記のない古いファイルもそのまま開ける
        assert!(
            !ProjectFile::from_model(&model, None)
                .to_json()
                .contains("annotations")
        );
    }

    #[test]
    fn chart_and_animations_are_saved_with_the_project() {
        use crate::interchange::{NodeTransform, Track};
        use crate::motion::{Action, ActuatorKind, Step};
        let model = sample_model();
        let part = model.roots[1];
        let mut docs = Docs::default();
        let id = docs.chart.add_actuator(ActuatorKind::LinearAxis);
        docs.chart.actuator_mut(id).unwrap().parts = vec![part];
        docs.chart.steps.push(Step {
            comment: "送る".into(),
            actions: vec![Action {
                actuator: id,
                target: 120.0,
                duration: 1.0,
            }],
            dwell: 0.5,
            ..Default::default()
        });
        docs.conveyors.add_conveyor().part = Some(part);
        let key = |t: f64, y: f64| {
            (
                t,
                NodeTransform {
                    translation: DVec3::new(0.0, y, 0.0),
                    ..Default::default()
                },
            )
        };
        docs.animations.clips.push(crate::animation::Clip {
            name: "lift".into(),
            tracks: vec![(
                part,
                Track {
                    node: 0,
                    keys: vec![key(0.0, 0.0), key(2.0, 50.0)],
                },
            )],
            duration: 2.0,
        });
        let file = ProjectFile::capture(&docs.of(&model), None);
        let json = file.to_json();
        assert!(json.contains("\"chart\"") && json.contains("\"animations\""));

        let parsed = ProjectFile::from_json(&json).unwrap();
        let restored = reload(&parsed);
        let find = |p: &[usize]| node_at_path(&restored, p);
        let (chart, warnings) = chart_from_record(parsed.chart.as_ref().unwrap(), find);
        assert!(warnings.is_empty());
        assert_eq!(chart.actuators[0].parts, vec![restored.roots[1]]);
        assert_eq!(chart.steps, docs.chart.steps);
        let (conveyors, warnings) = conveyor_record::from_record(parsed.conveyors.as_ref(), find);
        assert!(warnings.is_empty());
        assert_eq!(conveyors.conveyors[0].part, Some(restored.roots[1]));
        let (animations, missing) = Animations::from_records(&parsed.animations, find);
        assert_eq!(missing, 0);
        assert_eq!(animations.clips[0].duration, 2.0);
        assert_eq!(animations.clips[0].tracks[0].0, restored.roots[1]);
        // 開き直した状態をもう一度保存しても同じ内容になる (未保存の印が付かない)
        let reopened = Docs {
            chart,
            animations,
            conveyors,
            ..Default::default()
        };
        assert_eq!(ProjectFile::capture(&reopened.of(&restored), None), file);
        // チャートもアニメーションもなければ書かない
        let empty = ProjectFile::capture(&Docs::default().of(&model), None).to_json();
        assert!(!empty.contains("\"chart\"") && !empty.contains("\"animations\""));
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
