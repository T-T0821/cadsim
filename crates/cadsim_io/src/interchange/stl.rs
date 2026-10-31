//! STL (三角形の集まり)。アスキー / バイナリとも読めて、書き出しはバイナリ
//!
//! STL は色・階層・名前・単位を持たないので、読み込みはファイル名の部品1つになり、
//! 書き出しはシーン全体を1つの形状にまとめる。

use std::{fs::File, io::BufReader, path::Path};

use glam::DVec3;

use super::{ExchangeResult, Scene, SceneNode, TriMesh, UnitOptions, io_error};

pub fn import(path: &Path, units: &UnitOptions) -> ExchangeResult<Scene> {
    let file = File::open(path).map_err(|e| io_error(path, e))?;
    let stl = stl_io::read_stl(&mut BufReader::new(file)).map_err(|e| io_error(path, e))?;
    let mesh = TriMesh {
        positions: stl
            .vertices
            .iter()
            .map(|v| units.app_from_file(DVec3::from(v.0.map(f64::from))))
            .collect(),
        triangles: stl
            .faces
            .iter()
            .map(|f| f.vertices.map(|i| i as u32))
            .collect(),
    };
    Ok(Scene {
        nodes: vec![SceneNode {
            name: file_stem(path),
            mesh: Some(mesh),
            ..Default::default()
        }],
        animations: Vec::new(),
    })
}

pub fn export(scene: &Scene, path: &Path, units: &UnitOptions) -> ExchangeResult<()> {
    let merged = scene.merged_mesh();
    let triangles: Vec<stl_io::Triangle> = merged
        .triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| units.file_from_app(merged.positions[i as usize]));
            // 単位変換で鏡映にはならないので、法線は変換後の座標から求めてよい
            let normal = (b - a).cross(c - a).normalize_or_zero();
            stl_io::Triangle {
                normal: to_stl(normal),
                vertices: [to_stl(a), to_stl(b), to_stl(c)],
            }
        })
        .collect();
    let mut file = File::create(path).map_err(|e| io_error(path, e))?;
    stl_io::write_stl(&mut file, triangles.iter()).map_err(|e| io_error(path, e))
}

fn to_stl(v: DVec3) -> stl_io::Vector<f32> {
    stl_io::Vector::new(v.as_vec3().to_array())
}

/// 拡張子を除いたファイル名 (部品名に使う)
pub(super) fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "読み込んだ形状".into())
}

#[cfg(test)]
mod tests {
    use super::super::test_util::{assert_same_shape, sample_scene};
    use super::*;

    #[test]
    fn round_trip_merges_scene_into_one_mesh() {
        let dir = std::env::temp_dir().join("cadsim_stl_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("line.stl");
        let units = UnitOptions {
            meters_per_unit: 0.001,
            z_up: true,
        };
        let scene = sample_scene();
        export(&scene, &path, &units).unwrap();
        let back = import(&path, &units).unwrap();
        assert_eq!(back.nodes.len(), 1);
        assert_eq!(back.nodes[0].name, "line");
        // mm の f32 に丸めるので 1e-4 m 程度の誤差は許す
        assert_same_shape(
            back.nodes[0].mesh.as_ref().unwrap(),
            &scene.merged_mesh(),
            1e-4,
        );
    }

    #[test]
    fn reads_ascii_stl() {
        let dir = std::env::temp_dir().join("cadsim_stl_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ascii.stl");
        std::fs::write(
            &path,
            "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1000 0 0\nvertex 0 1000 0\nendloop\nendfacet\nendsolid t\n",
        )
        .unwrap();
        let scene = import(&path, &UnitOptions::default()).unwrap();
        let mesh = scene.nodes[0].mesh.as_ref().unwrap();
        assert_eq!(mesh.triangles.len(), 1);
        assert!(mesh.positions.iter().any(|p| p.abs_diff_eq(DVec3::X, 1e-9)));
    }
}
