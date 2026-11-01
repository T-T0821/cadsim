//! 自作部品のファイル (`<ライブラリ>/<部品名>/part.cadpart`)
//!
//! 中身は JSON で、部品の名前・分類・公開パラメータと、部品の木だけを書いたプロジェクトファイル
//! (`.plantwright` と同じ形) を持つ。取り込んだ STEP などは同じフォルダに写し、相対パスで指す。
//! 部品のアクチュエータとセンサーはプロジェクトファイルのタイミングチャートに入れる
//! (ステップは持たない)。位置は部品の一番上を原点にした座標で書く。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bevy::math::DVec3;
use serde::{Deserialize, Serialize};

use crate::document::project::{ProjectFile, StepFileRecord};
use crate::features::annotation::node_path;
use plantwright_core::custom_part::param::TargetOnChart;
use plantwright_core::custom_part::{CustomInstance, ParamTarget, PartParam};
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::motion::chart::TimingChart;
use plantwright_core::motion::record::chart_to_record;

/// ファイル形式の版
const FORMAT_VERSION: u32 = 1;
/// 部品フォルダの中のファイル名
pub const PART_FILE: &str = "part.cadpart";

/// 自作部品1つ分
#[derive(Clone, Debug, PartialEq)]
pub struct CustomPart {
    pub name: String,
    /// パレットでの分類 (例: "治具")
    pub category: String,
    pub description: String,
    pub params: Vec<PartParam>,
    /// 部品の木 (`nodes[0]` が一番上) と、アクチュエータ・センサー
    pub project: ProjectFile,
}

/// 保存するときに選んだもの
pub struct SaveRequest {
    pub name: String,
    pub category: String,
    pub description: String,
    /// 公開するパラメータ (名前, 項目)
    pub params: Vec<(String, ParamTarget)>,
}

/// `root` 以下の部品と一緒に保存するアクチュエータ・センサーの id
pub fn attached(model: &Model, chart: &TimingChart, root: NodeId) -> (Vec<u32>, Vec<u32>) {
    let subtree: HashSet<NodeId> = model.subtree(root).into_iter().collect();
    let actuators: Vec<u32> = chart
        .actuators
        .iter()
        .filter(|a| a.parts.iter().any(|p| subtree.contains(p)))
        .map(|a| a.id)
        .collect();
    // 部品のアクチュエータに載ったセンサーと、どこにも載らず部品の外接箱の中にあるセンサー
    let bounds = model.world_bounds(root);
    let sensors = chart
        .sensors
        .iter()
        .filter(|s| match s.mounted_on {
            Some(a) => actuators.contains(&a),
            None => bounds.is_some_and(|b| {
                let margin = DVec3::splat(b.size().max_element() * 0.01);
                let p = DVec3::from(s.position);
                p.cmpge(b.min - margin).all() && p.cmple(b.max + margin).all()
            }),
        })
        .map(|s| s.id)
        .collect();
    (actuators, sensors)
}

/// モデルの `root` 以下と、それに付いたアクチュエータ・センサーから自作部品を作る
pub fn build(
    model: &Model,
    chart: &TimingChart,
    root: NodeId,
    request: &SaveRequest,
) -> Option<CustomPart> {
    let mut project = ProjectFile::from_subtree(model, root, None)?;
    let origin = model.world_matrix(root).w_axis.truncate();
    let (actuators, sensors) = attached(model, chart, root);
    let subtree: HashSet<NodeId> = model.subtree(root).into_iter().collect();

    let mut sub = TimingChart {
        next_actuator_id: chart.next_actuator_id,
        next_sensor_id: chart.next_sensor_id,
        ..Default::default()
    };
    // 置いた自作部品をもう一度保存するときは、名前の頭 ("<部品名> 1/") を外す
    let prefix = model
        .get(root)
        .and_then(|n| n.custom_part.as_ref())
        .map(|c| c.prefix.clone())
        .unwrap_or_default();
    let strip = |name: &str| {
        name.strip_prefix(prefix.as_str())
            .filter(|_| !prefix.is_empty())
            .unwrap_or(name)
            .to_owned()
    };
    for a in chart.actuators.iter().filter(|a| actuators.contains(&a.id)) {
        let mut a = a.clone();
        a.name = strip(&a.name);
        a.parts.retain(|p| subtree.contains(p));
        a.mounted_on = a.mounted_on.filter(|m| actuators.contains(m));
        a.pivot = (DVec3::from(a.pivot) - origin).to_array();
        sub.actuators.push(a);
    }
    for s in chart.sensors.iter().filter(|s| sensors.contains(&s.id)) {
        let mut s = s.clone();
        s.name = strip(&s.name);
        s.position = (DVec3::from(s.position) - origin).to_array();
        s.force = None;
        sub.sensors.push(s);
    }
    let root_path = node_path(model, root)?;
    project.chart = Some(chart_to_record(&sub, |id| {
        let path = node_path(model, id)?;
        let rest = path.strip_prefix(root_path.as_slice())?;
        Some(std::iter::once(0).chain(rest.iter().copied()).collect())
    }));

    let params = request
        .params
        .iter()
        .filter_map(|(label, target)| {
            Some(PartParam {
                label: label.clone(),
                target: *target,
                value: target.read(&sub)?,
            })
        })
        .collect();
    Some(CustomPart {
        name: request.name.trim().to_owned(),
        category: request.category.trim().to_owned(),
        description: request.description.clone(),
        params,
        project,
    })
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct PartFileRecord {
    format_version: u32,
    name: String,
    #[serde(default)]
    category: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    params: Vec<ParamRecord>,
    /// 部品の木を書いたプロジェクトファイル。読むときは `ProjectFile::from_json` で確かめる
    project: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct ParamRecord {
    label: String,
    target: TargetRecord,
    value: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type")]
enum TargetRecord {
    Stroke { actuator: u32 },
    Duration { actuator: u32 },
    LimitMin { actuator: u32 },
    LimitMax { actuator: u32 },
    SensorRange { sensor: u32 },
}

impl From<ParamTarget> for TargetRecord {
    fn from(t: ParamTarget) -> Self {
        match t {
            ParamTarget::Stroke(actuator) => TargetRecord::Stroke { actuator },
            ParamTarget::Duration(actuator) => TargetRecord::Duration { actuator },
            ParamTarget::LimitMin(actuator) => TargetRecord::LimitMin { actuator },
            ParamTarget::LimitMax(actuator) => TargetRecord::LimitMax { actuator },
            ParamTarget::SensorRange(sensor) => TargetRecord::SensorRange { sensor },
        }
    }
}

impl From<&TargetRecord> for ParamTarget {
    fn from(t: &TargetRecord) -> Self {
        match *t {
            TargetRecord::Stroke { actuator } => ParamTarget::Stroke(actuator),
            TargetRecord::Duration { actuator } => ParamTarget::Duration(actuator),
            TargetRecord::LimitMin { actuator } => ParamTarget::LimitMin(actuator),
            TargetRecord::LimitMax { actuator } => ParamTarget::LimitMax(actuator),
            TargetRecord::SensorRange { sensor } => ParamTarget::SensorRange(sensor),
        }
    }
}

/// 置いた自作部品のしるし (`.plantwright` のノードに書く)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CustomInstanceRecord {
    pub part: String,
    pub prefix: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    params: Vec<ParamRecord>,
}

impl CustomInstanceRecord {
    pub fn from_instance(c: &CustomInstance) -> Self {
        Self {
            part: c.part.clone(),
            prefix: c.prefix.clone(),
            params: c.params.iter().map(ParamRecord::from_param).collect(),
        }
    }

    pub fn to_instance(&self) -> CustomInstance {
        CustomInstance {
            part: self.part.clone(),
            prefix: self.prefix.clone(),
            params: self.params.iter().map(ParamRecord::to_param).collect(),
        }
    }
}

impl ParamRecord {
    fn from_param(p: &PartParam) -> Self {
        Self {
            label: p.label.clone(),
            target: p.target.into(),
            value: p.value,
        }
    }

    fn to_param(&self) -> PartParam {
        PartParam {
            label: self.label.clone(),
            target: (&self.target).into(),
            value: self.value,
        }
    }
}

impl CustomPart {
    pub fn to_json(&self) -> String {
        let record = PartFileRecord {
            format_version: FORMAT_VERSION,
            name: self.name.clone(),
            category: self.category.clone(),
            description: self.description.clone(),
            params: self.params.iter().map(ParamRecord::from_param).collect(),
            project: serde_json::from_str(&self.project.to_json()).unwrap_or_default(),
        };
        serde_json::to_string_pretty(&record).unwrap_or_default()
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let record: PartFileRecord = serde_json::from_str(json)
            .map_err(|e| format!("自作部品のファイルが読めません: {e}"))?;
        if record.format_version > FORMAT_VERSION {
            return Err("新しい版の plantwright で作った自作部品です".into());
        }
        let project = ProjectFile::from_json(&record.project.to_string())
            .map_err(|e| format!("自作部品の中身が正しくありません: {e}"))?;
        if project.nodes.is_empty() {
            return Err("自作部品に部品が入っていません".into());
        }
        Ok(Self {
            name: record.name,
            category: record.category,
            description: record.description,
            params: record.params.iter().map(ParamRecord::to_param).collect(),
            project,
        })
    }
}

// ---------------------------------------------------------------------------
// フォルダへの保存と一覧
// ---------------------------------------------------------------------------

/// 部品名をフォルダ名に使えるようにする (OS で使えない文字を _ にする)
pub fn folder_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').to_owned();
    if cleaned.is_empty() {
        "部品".into()
    } else {
        cleaned
    }
}

/// `library` の下の部品フォルダに保存する。取り込んだファイルはフォルダに写して相対パスにする。
/// 同じ名前の部品があれば置き換える。保存したファイルのパスを返す。
pub fn save(part: &CustomPart, library: &Path) -> Result<PathBuf, String> {
    let dir = library.join(folder_name(&part.name));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    let mut part = part.clone();
    let mut copied: HashMap<PathBuf, String> = HashMap::new();
    let mut used: HashSet<String> = HashSet::new();
    for record in &mut part.project.step_files {
        let StepFileRecord::File { path } = record else {
            continue;
        };
        let source = PathBuf::from(&*path);
        let name = match copied.get(&source) {
            Some(name) => name.clone(),
            None => {
                let file_name = source
                    .file_name()
                    .map_or("shape".into(), |n| n.to_string_lossy().into_owned());
                // 別のフォルダの同じ名前のファイルは、名前の頭に番号を付けて分ける
                let mut name = file_name.clone();
                let mut n = 2;
                while !used.insert(name.clone()) {
                    name = format!("{n}_{file_name}");
                    n += 1;
                }
                let target = dir.join(&name);
                if source != target {
                    std::fs::copy(&source, &target)
                        .map_err(|e| format!("{} を写せません: {e}", source.display()))?;
                }
                copied.insert(source, name.clone());
                name
            }
        };
        *path = name;
    }
    let file = dir.join(PART_FILE);
    std::fs::write(&file, part.to_json())
        .map_err(|e| format!("{} に書けません: {e}", file.display()))?;
    Ok(file)
}

/// ライブラリの一覧の1行
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryEntry {
    pub path: PathBuf,
    pub name: String,
    pub category: String,
    pub description: String,
}

/// `library` の下の自作部品を名前順に並べる。読めないものは飛ばす。
pub fn scan(library: &Path) -> Vec<LibraryEntry> {
    let Ok(dirs) = std::fs::read_dir(library) else {
        return Vec::new();
    };
    let mut entries: Vec<LibraryEntry> = dirs
        .flatten()
        .filter_map(|d| {
            let path = d.path().join(PART_FILE);
            let part = CustomPart::from_json(&std::fs::read_to_string(&path).ok()?).ok()?;
            Some(LibraryEntry {
                path,
                name: part.name,
                category: part.category,
                description: part.description,
            })
        })
        .collect();
    entries.sort_by(|a, b| (&a.category, &a.name).cmp(&(&b.category, &b.name)));
    entries
}
