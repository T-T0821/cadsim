//! プロジェクトを開く・保存する手順 (ユースケース層)
//!
//! 開くときは、ファイルを読んで参照している STEP の形状を読み直し ([`LoadedProject::load`])、
//! モデルとほかの文書 (寸法・工程モデル・チャートなど) を作り直す ([`LoadedProject::open`])。
//! 保存は今の文書からファイルの内容を作って書く ([`save_project`])。Bevy には依存しない。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::{DMat4, DVec3};

use super::file::{
    FILE_EXTENSION, NodeKindRecord, NodeRecord, ProjectDocs, ProjectError, ProjectFile,
};
use crate::document::import::{load_source, source_name};
use crate::features::animation::Animations;
use crate::features::annotation::{Annotations, node_at_path};
use crate::features::custom_part::CustomInstanceRecord;
use crate::features::floor::record::{FloorLayout, FloorRecord};
use crate::features::library::{self, PartKind, Spec};
use crate::features::physics::PartPhysicsRecord;
use plantwright_core::camera_path::CameraPath;
use plantwright_core::conveyor::ConveyorConfig;
use plantwright_core::flow::model::FlowModel;
use plantwright_core::formats::UnitOptions;
use plantwright_core::formats::step::ImportNode;
use plantwright_core::formats::step::ImportedShape;
use plantwright_core::math::euler_deg_to_quat;
use plantwright_core::model::BoxPart;
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::model::NodeKind;
use plantwright_core::model::StepSource;
use plantwright_core::motion::TimingChart;
use plantwright_core::motion::record::chart_from_record;
use plantwright_core::site::model::Site;
use plantwright_core::site::record::load_site;

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

    pub(crate) fn fetch_shapes(file: ProjectFile, base_dir: &Path) -> Self {
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
            plantwright_core::site::underlay::unit_in_mm(&file.unit),
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

    /// すでに読んだ中身から作る (自作部品の読み込み)。参照している STEP をすべて読むので重い。
    pub fn from_file(file: ProjectFile, base_dir: &Path) -> Self {
        Self::fetch_shapes(file, base_dir)
    }

    /// モデルを消さずに、ファイルの一番上のノードを最上位に加える。加えた一番上の id と注意を返す。
    pub fn add_nodes(&self, model: &mut Model) -> (Vec<NodeId>, Vec<String>) {
        let mut warnings = self.warnings.clone();
        let mut roots = Vec::new();
        for record in &self.file.nodes {
            let before = model.roots.len();
            self.add_record(model, None, record, &mut warnings);
            roots.extend(model.roots.get(before).copied());
        }
        (roots, warnings)
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

    /// モデルをこのプロジェクトで置き換え、寸法・工程モデル・チャートなど残りの文書も作り直す。
    /// 部品が見つからずに読み飛ばしたものは `warnings` にまとめる。
    pub fn open(&self, model: &mut Model) -> OpenedDocs {
        let mut warnings = self.apply_to(model);
        let file = &self.file;
        let (annotations, skipped) = Annotations::from_records(&file.annotations, model);
        let (flow, flow_warnings) = match &file.flow {
            Some(record) => plantwright_core::flow::record::from_record(record, |path| {
                node_at_path(model, path).filter(|&id| model.get(id).is_some())
            }),
            None => (FlowModel::default(), Vec::new()),
        };
        warnings.extend(flow_warnings);
        let find = |path: &[usize]| node_at_path(model, path);
        let (chart, chart_warnings) = match &file.chart {
            Some(record) => chart_from_record(record, find),
            None => Default::default(),
        };
        warnings.extend(chart_warnings);
        let (animations, missing) = Animations::from_records(&file.animations, find);
        if missing > 0 {
            warnings.push(format!(
                "部品が見つからないアニメーションの動きが {missing} 件あったため読み飛ばしました"
            ));
        }
        let (conveyors, conveyor_warnings) =
            plantwright_core::conveyor::record::from_record(file.conveyors.as_ref(), find);
        warnings.extend(conveyor_warnings);
        let floor = file
            .floor
            .as_ref()
            .map(FloorRecord::restore)
            .unwrap_or_default();
        if skipped > 0 {
            warnings.push(format!(
                "部品が見つからない寸法・注記が {skipped} 件あったため読み飛ばしました"
            ));
        }
        let camera_path =
            plantwright_core::camera_path::record::from_record(file.camera_path.as_ref());
        OpenedDocs {
            camera_path,
            annotations,
            flow,
            chart,
            animations,
            conveyors,
            floor,
            warnings,
        }
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
            node.custom_part = record
                .custom_part
                .as_ref()
                .map(CustomInstanceRecord::to_instance);
            node.skip_inner_interference = record.skip_inner_interference;
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

/// 起動時に動かして見せるデモライン (ロボットセル・コンベア・安全機器・AGV・工程モデル)
const DEMO_LINE: &str = include_str!("../../../samples/demo_line.plantwright");
/// 読み込み中・読み込み後の表示名
pub const DEMO_LINE_NAME: &str = "デモライン";

/// 埋め込みのデモラインを読む。サンプルは STEP を参照しないので軽い。
pub fn load_demo_line() -> Result<LoadedProject, ProjectError> {
    let file = ProjectFile::from_json(DEMO_LINE)?;
    Ok(LoadedProject::fetch_shapes(file, Path::new(".")))
}

/// 本物の装置のサンプル (UR5e と Robotiq 2F-85 のピック & プレース、`samples/real_robot`)。
/// 形状の glTF とライセンス文も埋め込んでおき、開くときにフォルダへ書き出して読む
/// (プロジェクトは glTF を相対パスで参照するので、同じフォルダに並べる必要がある)。
const REAL_ROBOT_FILES: [(&str, &[u8]); 6] = [
    (
        REAL_ROBOT_PROJECT,
        include_bytes!("../../../samples/real_robot/ur5e_pick_place.plantwright"),
    ),
    (
        "ur5e.glb",
        include_bytes!("../../../samples/real_robot/ur5e.glb"),
    ),
    (
        "robotiq_2f85.glb",
        include_bytes!("../../../samples/real_robot/robotiq_2f85.glb"),
    ),
    (
        "README.md",
        include_bytes!("../../../samples/real_robot/README.md"),
    ),
    (
        "LICENSE-UR5e.txt",
        include_bytes!("../../../samples/real_robot/LICENSE-UR5e.txt"),
    ),
    (
        "LICENSE-Robotiq-2F-85.txt",
        include_bytes!("../../../samples/real_robot/LICENSE-Robotiq-2F-85.txt"),
    ),
];
const REAL_ROBOT_PROJECT: &str = "ur5e_pick_place.plantwright";
/// 読み込み中・読み込み後の表示名
pub const REAL_ROBOT_NAME: &str = "UR5e のピック & プレース";

/// 埋め込みの本物の装置のサンプルを `dir` に書き出して読む。中身が同じファイルは書き直さない。
pub fn load_real_robot_sample(dir: &Path) -> Result<LoadedProject, ProjectError> {
    std::fs::create_dir_all(dir).map_err(ProjectError::Io)?;
    for (name, bytes) in REAL_ROBOT_FILES {
        let path = dir.join(name);
        if std::fs::read(&path).ok().as_deref() != Some(bytes) {
            std::fs::write(&path, bytes).map_err(ProjectError::Io)?;
        }
    }
    LoadedProject::load(&dir.join(REAL_ROBOT_PROJECT))
}

/// 本物の装置のサンプルを書き出すフォルダ。設定ファイルと同じ場所の `samples/real_robot`
pub fn real_robot_sample_dir() -> PathBuf {
    plantwright_core::preferences::default_path()
        .and_then(|p| p.parent().map(|dir| dir.join("samples").join("real_robot")))
        .unwrap_or_else(|| std::env::temp_dir().join("plantwright").join("real_robot"))
}

/// 拡張子なしで名前を付けられたら .plantwright を付ける
pub fn with_extension(path: PathBuf) -> PathBuf {
    if path.extension().is_some() {
        path
    } else {
        path.with_extension(FILE_EXTENSION)
    }
}

/// 開いたプロジェクトから作り直した、モデル以外の文書。新規プロジェクトは `Default`。
/// PLC 連携の設定は `LoadedProject::file.plc` をそのまま使う。
#[derive(Default)]
pub struct OpenedDocs {
    pub annotations: Annotations,
    pub flow: FlowModel,
    pub chart: TimingChart,
    pub animations: Animations,
    pub conveyors: ConveyorConfig,
    pub camera_path: CameraPath,
    pub floor: FloorLayout,
    /// 部品が見つからずに読み飛ばしたものなどの注意
    pub warnings: Vec<String>,
}

/// 今の文書をプロジェクトファイルに書く。STEP の場所は `path` のフォルダからの相対パスにする。
/// 書いた内容を返す (未保存の変更を見つけるために覚えておく)。
pub fn save_project(path: &Path, docs: &ProjectDocs) -> Result<ProjectFile, ProjectError> {
    let file = ProjectFile::capture(docs, path.parent());
    std::fs::write(path, file.to_json()).map_err(ProjectError::Io)?;
    Ok(file)
}
