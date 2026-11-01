//! COLLADA (.dae) のメッシュを、色ごとの部品を並べた glTF バイナリ (.glb) にする。
//! URDF の見た目のメッシュを小さくして同梱するのに使う (samples/real_robot/ur5e_links)。
//! 座標と単位 (m) は変えない。
//!
//! ```sh
//! cargo run -p cadsim_io --example dae_to_glb -- 入力.dae 出力.glb [部品名の頭]
//! ```

use std::path::Path;

use cadsim_io::interchange::{Scene, SceneNode, UnitOptions, gltf};
use cadsim_io::urdf::collada;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(input), Some(output)) = (args.get(1), args.get(2)) else {
        eprintln!("使い方: dae_to_glb 入力.dae 出力.glb [部品名の頭]");
        std::process::exit(2);
    };
    let prefix = args.get(3).cloned().unwrap_or_else(|| {
        Path::new(input)
            .file_stem()
            .map_or("mesh".into(), |s| s.to_string_lossy().into_owned())
    });
    let text = std::fs::read_to_string(input).expect("COLLADA を読めません");
    let parts = collada::read(&text).expect("COLLADA を読めません");
    let mut scene = Scene::default();
    scene.nodes.push(SceneNode {
        name: prefix.clone(),
        ..Default::default()
    });
    for (i, part) in parts.into_iter().enumerate() {
        scene.nodes.push(SceneNode {
            name: format!("{prefix}_{}", i + 1),
            parent: Some(0),
            mesh: Some(part.mesh),
            color: part.color,
            ..Default::default()
        });
    }
    let units = UnitOptions {
        meters_per_unit: 1.0,
        z_up: false,
    };
    gltf::export(&scene, Path::new(output), &units).expect("glb を書けません");
}
