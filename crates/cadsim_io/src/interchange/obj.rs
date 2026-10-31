//! Wavefront OBJ (+ 色を入れる .mtl)
//!
//! OBJ は階層と単位を持たないので、読み込みはファイル名のアセンブリの下に `o` / `g` ごとの部品を並べ、
//! 書き出しは部品ごとにワールド座標へ焼き込んだ `o` を並べる。

use std::{collections::HashMap, fmt::Write as _, path::Path};

use glam::DVec3;

use super::{ExchangeResult, Scene, SceneNode, TriMesh, UnitOptions, io_error, stl::file_stem};

pub fn import(path: &Path, units: &UnitOptions) -> ExchangeResult<Scene> {
    let options = tobj::LoadOptions {
        triangulate: true,
        single_index: true,
        ..Default::default()
    };
    let (models, materials) = tobj::load_obj(path, &options).map_err(|e| io_error(path, e))?;
    // .mtl が無い・壊れているときは色なしで読む
    let materials = materials.unwrap_or_default();

    let mut nodes = Vec::new();
    // 部品が複数あるときだけファイル名のアセンブリでまとめる
    let parent = (models.len() > 1).then(|| {
        nodes.push(SceneNode {
            name: file_stem(path),
            ..Default::default()
        });
        0
    });
    for (index, model) in models.iter().enumerate() {
        let mesh = &model.mesh;
        let positions = mesh
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| units.app_from_file(DVec3::new(p[0].into(), p[1].into(), p[2].into())))
            .collect();
        let triangles = mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| [t[0], t[1], t[2]])
            .collect();
        let name = if model.name.is_empty() || model.name == "unnamed_object" {
            format!("{} {}", file_stem(path), index + 1)
        } else {
            model.name.clone()
        };
        nodes.push(SceneNode {
            name,
            parent,
            mesh: Some(TriMesh {
                positions,
                triangles,
            }),
            color: mesh
                .material_id
                .and_then(|m| materials.get(m))
                .and_then(|m| m.diffuse),
            ..Default::default()
        });
    }
    Ok(Scene {
        nodes,
        animations: Vec::new(),
    })
}

pub fn export(scene: &Scene, path: &Path, units: &UnitOptions) -> ExchangeResult<()> {
    let mtl_path = path.with_extension("mtl");
    let mtl_name = mtl_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    // 色ごとにマテリアルを1つ作る
    let mut material_of: HashMap<[u32; 3], usize> = HashMap::new();
    let mut mtl = String::new();
    let mut obj = format!("# cadsim から書き出し\nmtllib {mtl_name}\n");
    let mut vertex_base = 1; // OBJ の頂点番号は 1 始まりでファイル全体の通し番号
    for (node, world) in scene.nodes.iter().zip(scene.world_transforms()) {
        let Some(mesh) = node.mesh.as_ref().filter(|m| !m.is_empty()) else {
            continue;
        };
        let color = node.color.unwrap_or([0.7, 0.74, 0.8]);
        let key = color.map(f32::to_bits);
        let next = material_of.len();
        let material = *material_of.entry(key).or_insert_with(|| {
            let [r, g, b] = color;
            let _ = writeln!(mtl, "newmtl material_{next}\nKd {r} {g} {b}\n");
            next
        });

        let _ = writeln!(obj, "o {}", one_line(&node.name));
        let _ = writeln!(obj, "usemtl material_{material}");
        for p in &mesh.positions {
            let p = units.file_from_app(world.transform_point3(*p));
            let _ = writeln!(obj, "v {} {} {}", p.x, p.y, p.z);
        }
        let flip = world.matrix3.determinant() < 0.0;
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|i| i as usize + vertex_base);
            let (b, c) = if flip { (c, b) } else { (b, c) };
            let _ = writeln!(obj, "f {a} {b} {c}");
        }
        vertex_base += mesh.positions.len();
    }
    std::fs::write(path, obj).map_err(|e| io_error(path, e))?;
    std::fs::write(&mtl_path, mtl).map_err(|e| io_error(&mtl_path, e))
}

/// 名前に改行があると OBJ の行が壊れるので空白にする
fn one_line(name: &str) -> String {
    name.replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::super::test_util::{assert_same_shape, sample_scene};
    use super::*;

    #[test]
    fn round_trip_keeps_parts_and_colors() {
        let dir = std::env::temp_dir().join("cadsim_obj_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("line.obj");
        let units = UnitOptions::default();
        let scene = sample_scene();
        export(&scene, &path, &units).unwrap();
        let back = import(&path, &units).unwrap();

        // ファイル名のアセンブリ + 部品2つ
        assert_eq!(back.nodes.len(), 3);
        assert_eq!(back.nodes[0].name, "line");
        assert_eq!(back.nodes[2].name, "シリンダ");
        assert_eq!(back.nodes[2].color, Some([0.9, 0.5, 0.1]));
        assert_same_shape(&back.merged_mesh(), &scene.merged_mesh(), 1e-6);
    }
}
