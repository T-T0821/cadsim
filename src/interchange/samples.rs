//! 動作確認用のサンプルファイルを作る (通常のテストでは走らない)
//!
//! `cargo test write_samples -- --ignored` で次を作り直す:
//! - `assets/sample_robot.glb`: 階層とアニメーションを持つ簡単なロボット
//! - `target/samples/`: STEP のサンプルを glb / gltf / obj / stl に書き出したもの

use std::path::Path;

use bevy::math::{DQuat, DVec3};

use super::{
    Animation, NodeTransform, Scene, SceneNode, Track, TriMesh, UnitOptions, export_file,
    test_util::cube,
};

/// 中心 `center`、寸法 `size` の箱のメッシュ
fn block(size: [f64; 3], center: [f64; 3]) -> TriMesh {
    let mut mesh = cube();
    for p in &mut mesh.positions {
        *p = *p * DVec3::from(size) + DVec3::from(center);
    }
    mesh
}

fn at(translation: [f64; 3]) -> NodeTransform {
    NodeTransform {
        translation: DVec3::from(translation),
        ..Default::default()
    }
}

/// 台座・旋回・アーム・ハンドの4軸ロボット (メートル)
pub fn robot() -> Scene {
    let mut nodes = Vec::new();
    let mut add = |name: &str, parent: Option<usize>, transform, mesh, color| {
        nodes.push(SceneNode {
            name: name.into(),
            parent,
            transform,
            mesh,
            color,
        });
        nodes.len() - 1
    };
    let root = add("ロボット", None, at([0.0; 3]), None, None);
    let gray = Some([0.45, 0.47, 0.5]);
    let orange = Some([0.95, 0.55, 0.15]);
    add(
        "台座",
        Some(root),
        at([0.0; 3]),
        Some(block([0.6, 0.2, 0.6], [0.0, 0.1, 0.0])),
        gray,
    );
    let turn = add(
        "旋回",
        Some(root),
        at([0.0, 0.2, 0.0]),
        Some(block([0.4, 0.3, 0.4], [0.0, 0.15, 0.0])),
        orange,
    );
    let arm = add(
        "アーム",
        Some(turn),
        at([0.0, 0.3, 0.0]),
        Some(block([0.15, 0.8, 0.15], [0.0, 0.4, 0.0])),
        orange,
    );
    let hand = add(
        "ハンド",
        Some(arm),
        at([0.0, 0.8, 0.0]),
        Some(block([0.3, 0.1, 0.2], [0.0, 0.05, 0.0])),
        gray,
    );

    let rot = |node: usize, axis: DVec3, degs: [f64; 5], base: NodeTransform| Track {
        node,
        keys: degs
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let mut t = base;
                t.rotation = DQuat::from_axis_angle(axis, d.to_radians());
                (i as f64, t)
            })
            .collect(),
    };
    let base_turn = nodes[turn].transform;
    let base_arm = nodes[arm].transform;
    let base_hand = nodes[hand].transform;
    Scene {
        nodes,
        animations: vec![Animation {
            name: "ピック&プレース".into(),
            tracks: vec![
                rot(turn, DVec3::Y, [0.0, 0.0, 90.0, 90.0, 0.0], base_turn),
                rot(arm, DVec3::Z, [0.0, 45.0, 45.0, 0.0, 0.0], base_arm),
                rot(hand, DVec3::Y, [0.0, 0.0, 0.0, 90.0, 0.0], base_hand),
            ],
        }],
    }
}

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
