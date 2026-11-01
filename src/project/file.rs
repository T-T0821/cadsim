//! プロジェクトファイル (.cadsim) の形 (アダプター層)
//!
//! ファイルの中身は人が読める JSON。アセンブリの木をそのまま入れ子で書く。
//! 書くのは設計データ (名前・配置・表示/非表示・色・箱の寸法) と、取り込んだ STEP の場所だけ。
//! STEP の形状 (B-rep やメッシュ) は書かず、開くときに STEP から読み直す。
//! 3D 上の寸法と注記 (annotation.rs) も一緒に書く。部品は木の中の位置で指す。
//! そのため取り込んだ部品は「どの STEP の、どの製品か」で形状を引く。
//!
//! ファイル形式の構造体 (`ProjectFile` など) はアプリ内部の `Model` とは別に持つ。
//! 内部の型を変えてもファイルの互換を保ちやすくするため。Bevy には依存しない。

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::animation::Animations;
use crate::annotation::{AnnotationRecord, Annotations, node_path, to_records};
use crate::custom_part::CustomInstanceRecord;
use crate::floor_record::FloorRecord;
use crate::flow::{self, model::FlowModel, record::FlowRecord};
use crate::formats::camera_path_record::{self, CameraPathRecord};
use crate::formats::chart_record::{AnimationRecord, ChartRecord, chart_to_record};
use crate::formats::conveyor_record::{self, ConveyorsRecord};
use crate::formats::site_record::{CloudRecord, UnderlayRecord};
use crate::library::PartKind;
use crate::math::{quat_to_euler_deg, split_placement};
use crate::model::{Model, NodeId, NodeKind, StepSource};
use crate::physics::{CableRecord, PartPhysicsRecord};
use crate::plc::PlcRecord;
use cadsim_core::camera_path::CameraPath;
use cadsim_core::conveyor::ConveyorConfig;
use cadsim_core::motion::TimingChart;

/// ファイル形式のバージョン。古いファイルを開けなくなる変更をしたときだけ上げる。
pub const FORMAT_VERSION: u32 = 1;
/// プロジェクトファイルの拡張子
pub const FILE_EXTENSION: &str = "cadsim";
pub fn is_project_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(FILE_EXTENSION))
}

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
    /// 紹介動画のカメラワーク (カメラの通り道)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_path: Option<CameraPathRecord>,
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
    pub camera_path: &'a CameraPath,
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
    /// 自作部品ライブラリから置いた部品なら、その部品名と公開パラメータ
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_part: Option<CustomInstanceRecord>,
    /// 中の部品どうしの干渉を調べない (購入品の CAD など)。既定 (調べる) なら省く。
    #[serde(default, skip_serializing_if = "is_false")]
    pub skip_inner_interference: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<NodeRecord>,
}

fn visible_by_default() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !value
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
            camera_path: None,
        }
    }

    /// `id` 以下だけを書いたファイル (自作部品の保存に使う)。`id` が `nodes[0]` になり、
    /// その配置はワールドでの回転だけを残して原点に置く (置くときに位置を決めるため)。
    pub fn from_subtree(model: &Model, id: NodeId, base_dir: Option<&Path>) -> Option<Self> {
        let mut sources = Vec::new();
        let mut root = node_record(model, id, &mut sources)?;
        let (_, rotation) = split_placement(model.world_matrix(id));
        root.position = [0.0; 3];
        root.rotation_deg = quat_to_euler_deg(rotation);
        let mut file = Self::from_model(&Model::default(), base_dir);
        file.unit = model.unit.clone();
        file.step_files = sources
            .iter()
            .map(|s| StepFileRecord::from_source(s, base_dir))
            .collect();
        file.nodes = vec![root];
        Some(file)
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
            flow: (!docs.flow.is_empty() || !docs.flow.scenarios.is_empty())
                .then(|| flow::record::to_record(docs.flow, path_of)),
            chart: (!chart.actuators.is_empty() || !chart.steps.is_empty())
                .then(|| chart_to_record(chart, path_of)),
            animations: docs.animations.to_records(path_of),
            plc: docs.plc.clone(),
            conveyors: conveyor_record::to_record(docs.conveyors, path_of),
            camera_path: camera_path_record::to_record(docs.camera_path),
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
        custom_part: node
            .custom_part
            .as_ref()
            .map(CustomInstanceRecord::from_instance),
        skip_inner_interference: node.skip_inner_interference,
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
    pub(crate) fn from_source(source: &StepSource, base_dir: Option<&Path>) -> Self {
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

    pub(crate) fn to_source(&self, base_dir: &Path) -> StepSource {
        match self {
            StepFileRecord::Sample => StepSource::Sample,
            &StepFileRecord::Stress { parts } => StepSource::Stress(parts),
            StepFileRecord::File { path } => StepSource::File(base_dir.join(path)),
            StepFileRecord::Modeled { name, solid } => {
                StepSource::Modeled(Arc::new(crate::model::ModeledSource {
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
pub(super) fn compact_number_arrays(json: &str) -> String {
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
