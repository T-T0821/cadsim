use glam::{DMat4, DVec3};

use super::file::compact_number_arrays;
use super::*;
use crate::document::import::{read_step, source_name};
use crate::features::annotation::node_at_path;
use plantwright_core::floor::record::FloorRecord;

use crate::features::library::{self, PartKind};
use plantwright_core::camera_path::{CameraPath, CameraPose};
use plantwright_core::flow::model::FlowModel;
use plantwright_core::formats::step::import_step;
use plantwright_core::math::euler_deg_to_quat;
use plantwright_core::model::NodeKind;
use plantwright_core::model::StepSource;
use plantwright_core::motion::record::chart_from_record;

/// 同梱のデモライン (tools/make_demo_line.py で生成) が今の形式で読めること
#[test]
fn demo_line_sample_parses() {
    let text = include_str!("../../../samples/demo_line.plantwright");
    let file = ProjectFile::from_json(text).expect("demo line should parse");
    assert!(!file.nodes.is_empty());
    assert!(file.chart.is_some());
}

/// デモラインを開くと、モデルと一緒にチャート・工程モデル・床の配置も作り直されること
#[test]
fn opening_the_demo_line_restores_every_document() {
    let loaded = load_demo_line().unwrap();
    let mut model = Model::default();
    let opened = loaded.open(&mut model);
    assert!(!model.roots.is_empty());
    assert!(!opened.chart.steps.is_empty());
    assert!(!opened.flow.is_empty());
    assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
}

/// 保存した内容がそのまま返り、開き直すと同じ内容になること
#[test]
fn save_project_writes_what_it_returns() {
    let model = sample_model();
    let docs = Docs::default();
    let dir = std::env::temp_dir().join("plantwright_save_project_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.plantwright");
    let saved = save_project(&path, &docs.of(&model)).unwrap();
    let reread = ProjectFile::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved, reread);
}

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
    camera_path: CameraPath,
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
            camera_path: &self.camera_path,
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
    let outside = StepFileRecord::from_source(&StepSource::File("/other/a.step".into()), Some(dir));
    assert_eq!(
        outside,
        StepFileRecord::File {
            path: "/other/a.step".into()
        }
    );
}

#[test]
fn save_and_open_through_a_file() {
    let dir = std::env::temp_dir().join(format!("plantwright-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("line.plantwright");
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
        flow: plantwright_core::flow::model::sample_line(),
        ..Default::default()
    };
    docs.flow.elements[1].part = Some(part);
    let line = &docs.flow;
    let file = ProjectFile::capture(&docs.of(&model), None);
    let parsed = ProjectFile::from_json(&file.to_json()).unwrap();
    let restored = reload(&parsed);
    let (flow, warnings) =
        plantwright_core::flow::record::from_record(parsed.flow.as_ref().unwrap(), |p| {
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
    use crate::features::annotation::{Anchor, AnnotationKind};
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
    use plantwright_core::formats::NodeTransform;
    use plantwright_core::formats::Track;
    use plantwright_core::motion::chart::Action;
    use plantwright_core::motion::chart::ActuatorKind;
    use plantwright_core::motion::chart::Step;
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
    docs.animations
        .clips
        .push(crate::features::animation::Clip {
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
    let (conveyors, warnings) =
        plantwright_core::conveyor::record::from_record(parsed.conveyors.as_ref(), find);
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
fn embedded_demo_line_opens_without_warnings() {
    // 起動時に開くデモラインは、外のファイルを参照せずに読めて、動きを持っている
    let loaded = load_demo_line().expect("デモラインの JSON が読める");
    assert!(loaded.file.step_files.is_empty(), "外の STEP を参照しない");
    let mut model = Model::default();
    let warnings = loaded.apply_to(&mut model);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(!model.is_empty());
    let chart = loaded
        .file
        .chart
        .as_ref()
        .expect("タイミングチャートがある");
    let find = |path: &[usize]| node_at_path(&model, path);
    let (chart, chart_warnings) = chart_from_record(chart, find);
    assert!(chart_warnings.is_empty(), "{chart_warnings:?}");
    assert!(!chart.steps.is_empty());
}

#[test]
fn save_as_adds_the_extension() {
    assert_eq!(
        with_extension(PathBuf::from("line1")),
        PathBuf::from("line1.plantwright")
    );
    assert!(is_project_file(Path::new("a/LINE.PLANTWRIGHT")));
    // 旧名のころのファイルも開ける
    assert!(is_project_file(Path::new("a/line.cadsim")));
    assert!(!is_project_file(Path::new("a/vise.step")));
}

/// 本物の装置のサンプル (tools/real_robot/make_demo.py で生成) が、読み込んだ glTF の形状と
/// 関節の対応をすべて保ったまま開け、ロボットが教示点に届くこと
#[test]
fn real_robot_sample_opens_and_reaches_every_point() {
    use plantwright_core::motion::pose::actuator_poses_for;
    use std::collections::HashMap;

    // メニューから開くときと同じく、埋め込みのファイルをフォルダに書き出して読む
    let dir = std::env::temp_dir().join(format!("plantwright-real-robot-{}", std::process::id()));
    let loaded = load_real_robot_sample(&dir).expect("サンプルが読める");
    assert!(
        dir.join("LICENSE-UR5e.txt").exists(),
        "ライセンス文も一緒に書き出す"
    );
    let mut model = Model::default();
    let opened = loaded.open(&mut model);
    assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
    // UR5e の 6 関節 + 指の 6 関節 + 把持の合図
    assert_eq!(opened.chart.actuators.len(), 13);
    assert!(
        opened
            .chart
            .actuators
            .iter()
            .filter(|a| a.kind != plantwright_core::motion::chart::ActuatorKind::Cylinder)
            .all(|a| !a.parts.is_empty()),
        "関節はどれも読み込んだリンクを動かす"
    );
    assert_eq!(opened.conveyors.grippers.len(), 1);

    let schedule = opened.chart.schedule();
    assert_eq!(schedule.moves.len(), 4);
    for m in &schedule.moves {
        assert!(
            m.reached && m.path_reached,
            "教示点 {} に届かない: {m:?}",
            m.point
        );
    }

    // スクリプトで求めた「ピック上」の関節角で、TCP がちょうど教示点に来る (関節の位置と向きが正しい)
    let robot = &opened.chart.robots[0];
    let first = &opened.chart.steps[0];
    let values: HashMap<u32, f64> = first
        .actions
        .iter()
        .map(|a| (a.actuator, a.target))
        .collect();
    let poses = actuator_poses_for(&opened.chart, &values);
    let tcp = poses[&robot.tip].transform_point(DVec3::from_array(robot.tcp));
    let target = DVec3::from_array(robot.points[0].position);
    assert!(
        tcp.distance(target) < 0.5,
        "TCP {tcp} が教示点 {target} からずれている"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// カメラワークを保存して開き直すと、同じ通り道に戻る。変えると未保存の変更になる
#[test]
fn camera_path_round_trips_and_counts_as_a_change() {
    let model = sample_model();
    let mut docs = Docs::default();
    let before = ProjectFile::capture(&docs.of(&model), None);
    assert!(before.camera_path.is_none(), "通り道がなければ書かない");

    docs.camera_path = CameraPath::turntable(
        CameraPose {
            focus: glam::Vec3::new(10.0, 0.0, 5.0),
            yaw: 0.5,
            pitch: 0.3,
            distance: 900.0,
        },
        6.0,
    );
    let file = ProjectFile::capture(&docs.of(&model), None);
    assert_ne!(file, before, "カメラワークの変更は未保存の変更になる");

    let parsed = ProjectFile::from_json(&file.to_json()).unwrap();
    let loaded = LoadedProject::fetch_shapes(parsed, Path::new("."));
    let mut reopened = Model::default();
    let opened = loaded.open(&mut reopened);
    assert_eq!(opened.camera_path.len(), docs.camera_path.len());
    assert_eq!(opened.camera_path.duration(), 6.0);
}
