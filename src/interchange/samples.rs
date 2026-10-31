//! 動作確認用のサンプルファイルを作る (通常のテストでは走らない)
//!
//! `cargo test write_samples -- --ignored` で次を作り直す:
//! - `assets/sample_robot.glb`: 階層とアニメーションを持つ簡単なロボット
//! - `target/samples/`: STEP のサンプルを glb / gltf / obj / stl に書き出したもの

use std::path::Path;

use cadsim_io::interchange::{UnitOptions, export_file, samples::robot};

#[test]
#[ignore = "サンプルファイルを作り直すときだけ手で実行する"]
fn write_samples() {
    use crate::interchange::model_io::model_to_scene;
    use crate::model::Model;
    use crate::step_import::import_step;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let units = UnitOptions::default();
    export_file(&robot(), &root.join("assets/sample_robot.glb"), &units).unwrap();

    let out = root.join("target/samples");
    std::fs::create_dir_all(&out).unwrap();
    let file = import_step(
        include_str!("../../assets/sample_assembly.step"),
        "sample_assembly.step",
    )
    .unwrap();
    let mut model = Model::default();
    model.add_imported(file.root, file.unit, crate::model::StepSource::Sample);
    let (scene, _) = model_to_scene(&model, &[]);
    for ext in ["glb", "gltf", "obj", "stl", "usda", "usdz"] {
        export_file(&scene, &out.join(format!("vise.{ext}")), &units).unwrap();
    }
    for ext in ["usda", "usdz"] {
        export_file(&robot(), &out.join(format!("robot.{ext}")), &units).unwrap();
    }

    // タイミングチャートのサンプル装置を、1サイクルの動き付きで書き出す
    let mut model = Model::default();
    let mut chart = crate::motion::TimingChart::default();
    crate::motion::load_demo(&mut model, &mut chart);
    let schedule = chart.schedule();
    let (mut scene, index_of) = model_to_scene(&model, &[]);
    let parents: Vec<_> = scene.nodes.iter().map(|n| n.parent).collect();
    scene.animations =
        crate::motion::chart_animation(&chart, &schedule, &model, &index_of, &parents)
            .into_iter()
            .collect();
    for ext in ["glb", "usda", "usdz", "html"] {
        export_file(&scene, &out.join(format!("line_demo.{ext}")), &units).unwrap();
    }
}
