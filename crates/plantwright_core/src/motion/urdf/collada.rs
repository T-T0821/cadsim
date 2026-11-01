//! COLLADA (.dae) のメッシュの読み込み (URDF の見た目のメッシュ用)
//!
//! 読むのは三角形の形 (`triangles` / `polylist` / `polygons`)、ノードの配置 (`matrix` / `translate` /
//! `rotate` / `scale`)、材質の拡散色、`<unit meter>` だけ。テクスチャ・アニメーション・スキンは読まない。
//! ROS の RViz と同じく `<up_axis>` は見ない (URDF のメッシュはリンクの座標系のまま描く)。

use std::collections::HashMap;

use glam::{DMat4, DQuat, DVec3};
use roxmltree::{Document, Node};

use super::MeshPart;
use crate::formats::TriMesh;

/// ノードの入れ子・参照をたどる深さの上限 (壊れたファイルで止まらなくならないように)
const MAX_DEPTH: usize = 32;

/// COLLADA の文字列を読み、色ごとにまとめたメッシュ (メートル、ファイルの座標系) を返す
pub fn read(text: &str) -> Result<Vec<MeshPart>, String> {
    let doc = Document::parse(text).map_err(|e| format!("COLLADA (XML) を読めません: {e}"))?;
    let root = doc.root_element();
    let library = Library::new(root);
    let meter = child(root, "asset")
        .and_then(|a| child(a, "unit"))
        .and_then(|u| u.attribute("meter"))
        .and_then(|m| m.trim().parse::<f64>().ok())
        .filter(|m| *m > 0.0)
        .unwrap_or(1.0);
    let scale = DMat4::from_scale(DVec3::splat(meter));

    let mut parts = Parts::default();
    let scene = child(root, "scene")
        .and_then(|s| child(s, "instance_visual_scene"))
        .and_then(|i| i.attribute("url"))
        .and_then(|url| {
            library
                .visual_scenes
                .get(url.trim_start_matches('#'))
                .copied()
        })
        .or_else(|| library.visual_scenes.values().next().copied());
    match scene {
        Some(scene) => {
            for node in elements(scene, "node") {
                library.visit_node(node, scale, 0, &mut parts);
            }
        }
        // シーンのないファイルは、形をすべてそのまま置く
        None => {
            for geometry in library.geometries.values() {
                library.add_geometry(*geometry, scale, &HashMap::new(), &mut parts);
            }
        }
    }
    Ok(parts.finish())
}

/// 色ごとにメッシュを集める
#[derive(Default)]
struct Parts {
    /// (色, メッシュ)。色のないものは None
    meshes: Vec<(Option<[f32; 3]>, TriMesh)>,
}

impl Parts {
    fn add(&mut self, color: Option<[f32; 3]>, positions: &[DVec3], triangles: &[[u32; 3]]) {
        let index = match self.meshes.iter().position(|(c, _)| *c == color) {
            Some(i) => i,
            None => {
                self.meshes.push((color, TriMesh::default()));
                self.meshes.len() - 1
            }
        };
        let mesh = &mut self.meshes[index].1;
        let offset = mesh.positions.len() as u32;
        mesh.positions.extend_from_slice(positions);
        mesh.triangles
            .extend(triangles.iter().map(|t| t.map(|i| i + offset)));
    }

    fn finish(self) -> Vec<MeshPart> {
        self.meshes
            .into_iter()
            .filter(|(_, m)| !m.is_empty())
            .map(|(color, mesh)| MeshPart {
                name: String::new(),
                mesh,
                color,
            })
            .collect()
    }
}

/// id で引ける要素
struct Library<'a, 'input> {
    geometries: HashMap<&'a str, Node<'a, 'input>>,
    nodes: HashMap<&'a str, Node<'a, 'input>>,
    visual_scenes: HashMap<&'a str, Node<'a, 'input>>,
    /// 材質の id → 拡散色
    material_colors: HashMap<&'a str, [f32; 3]>,
}

impl<'a, 'input> Library<'a, 'input> {
    fn new(root: Node<'a, 'input>) -> Self {
        let by_id =
            |library: &'static str, element: &'static str| -> HashMap<&'a str, Node<'a, 'input>> {
                elements(root, library)
                    .flat_map(|l| elements(l, element))
                    .filter_map(|n| Some((n.attribute("id")?, n)))
                    .collect()
            };
        let effect_colors: HashMap<&str, [f32; 3]> = elements(root, "library_effects")
            .flat_map(|l| elements(l, "effect"))
            .filter_map(|e| Some((e.attribute("id")?, effect_color(e)?)))
            .collect();
        let material_colors = elements(root, "library_materials")
            .flat_map(|l| elements(l, "material"))
            .filter_map(|m| {
                let effect = child(m, "instance_effect")?.attribute("url")?;
                Some((
                    m.attribute("id")?,
                    *effect_colors.get(effect.trim_start_matches('#'))?,
                ))
            })
            .collect();
        // 入れ子のノードも instance_node から参照されるので、library_nodes の子孫をすべて登録する
        let nodes = elements(root, "library_nodes")
            .flat_map(|l| l.descendants())
            .filter(|n| n.has_tag_name_local("node"))
            .filter_map(|n| Some((n.attribute("id")?, n)))
            .collect();
        Self {
            geometries: by_id("library_geometries", "geometry"),
            nodes,
            visual_scenes: by_id("library_visual_scenes", "visual_scene"),
            material_colors,
        }
    }

    fn visit_node(&self, node: Node, parent: DMat4, depth: usize, parts: &mut Parts) {
        if depth > MAX_DEPTH {
            return;
        }
        let mut transform = parent;
        for item in node.children().filter(Node::is_element) {
            match item.tag_name().name() {
                "matrix" => {
                    let v = floats(item);
                    if let Ok(m) = <[f64; 16]>::try_from(v.as_slice()) {
                        // COLLADA の行列は行優先
                        transform *= DMat4::from_cols_array(&m).transpose();
                    }
                }
                "translate" => {
                    if let [x, y, z] = floats(item)[..] {
                        transform *= DMat4::from_translation(DVec3::new(x, y, z));
                    }
                }
                "rotate" => {
                    if let [x, y, z, angle] = floats(item)[..]
                        && let Some(axis) = DVec3::new(x, y, z).try_normalize()
                    {
                        transform *=
                            DMat4::from_quat(DQuat::from_axis_angle(axis, angle.to_radians()));
                    }
                }
                "scale" => {
                    if let [x, y, z] = floats(item)[..] {
                        transform *= DMat4::from_scale(DVec3::new(x, y, z));
                    }
                }
                _ => {}
            }
        }
        for item in node.children().filter(Node::is_element) {
            match item.tag_name().name() {
                "instance_geometry" => {
                    let Some(geometry) = item
                        .attribute("url")
                        .and_then(|u| self.geometries.get(u.trim_start_matches('#')))
                    else {
                        continue;
                    };
                    // 形の中の材質の名前 (symbol) → 材質の id
                    let bindings: HashMap<&str, &str> = item
                        .descendants()
                        .filter(|n| n.has_tag_name_local("instance_material"))
                        .filter_map(|n| {
                            Some((
                                n.attribute("symbol")?,
                                n.attribute("target")?.trim_start_matches('#'),
                            ))
                        })
                        .collect();
                    self.add_geometry(*geometry, transform, &bindings, parts);
                }
                "instance_node" => {
                    if let Some(target) = item
                        .attribute("url")
                        .and_then(|u| self.nodes.get(u.trim_start_matches('#')))
                    {
                        self.visit_node(*target, transform, depth + 1, parts);
                    }
                }
                "node" => self.visit_node(item, transform, depth + 1, parts),
                _ => {}
            }
        }
    }

    fn add_geometry(
        &self,
        geometry: Node,
        transform: DMat4,
        bindings: &HashMap<&str, &str>,
        parts: &mut Parts,
    ) {
        let Some(mesh) = child(geometry, "mesh") else {
            return;
        };
        let sources: HashMap<&str, Node> = elements(mesh, "source")
            .filter_map(|s| Some((s.attribute("id")?, s)))
            .collect();
        // <vertices id> → 位置の source
        let vertices: HashMap<&str, &str> = elements(mesh, "vertices")
            .filter_map(|v| {
                let position = elements(v, "input")
                    .find(|i| i.attribute("semantic") == Some("POSITION"))?
                    .attribute("source")?;
                Some((v.attribute("id")?, position.trim_start_matches('#')))
            })
            .collect();
        let flip = transform.determinant() < 0.0;
        for primitive in mesh.children().filter(Node::is_element) {
            let kind = primitive.tag_name().name();
            if !matches!(kind, "triangles" | "polylist" | "polygons") {
                continue;
            }
            let inputs: Vec<Node> = elements(primitive, "input").collect();
            let stride = inputs
                .iter()
                .filter_map(|i| i.attribute("offset")?.parse::<usize>().ok())
                .max()
                .map_or(1, |m| m + 1);
            let Some((offset, positions)) = inputs.iter().find_map(|i| {
                let source = i.attribute("source")?.trim_start_matches('#');
                let offset = i.attribute("offset")?.parse::<usize>().ok()?;
                match i.attribute("semantic")? {
                    "VERTEX" => Some((offset, *sources.get(vertices.get(source)?)?)),
                    "POSITION" => Some((offset, *sources.get(source)?)),
                    _ => None,
                }
            }) else {
                continue;
            };
            let positions: Vec<DVec3> = source_vectors(positions)
                .into_iter()
                .map(|p| transform.transform_point3(p))
                .collect();
            // 多角形ごとの頂点 (位置の番号)
            let polygons: Vec<Vec<u32>> = match kind {
                "triangles" => {
                    let p = child(primitive, "p").map(ints).unwrap_or_default();
                    p.chunks_exact(stride * 3)
                        .map(|t| (0..3).map(|k| t[k * stride + offset]).collect())
                        .collect()
                }
                "polylist" => {
                    let p = child(primitive, "p").map(ints).unwrap_or_default();
                    let counts = child(primitive, "vcount").map(ints).unwrap_or_default();
                    let mut cursor = 0;
                    let mut polygons = Vec::new();
                    for count in counts {
                        let count = count as usize;
                        let Some(indices) = p.get(cursor..cursor + count * stride) else {
                            break;
                        };
                        polygons.push((0..count).map(|k| indices[k * stride + offset]).collect());
                        cursor += count * stride;
                    }
                    polygons
                }
                _ => elements(primitive, "p")
                    .map(ints)
                    .map(|p| {
                        p.chunks_exact(stride)
                            .map(|v| v[offset])
                            .collect::<Vec<u32>>()
                    })
                    .collect(),
            };
            let count = positions.len() as u32;
            let triangles: Vec<[u32; 3]> = polygons
                .iter()
                .filter(|poly| poly.iter().all(|&i| i < count))
                .flat_map(|poly| {
                    // 扇形に三角形に分ける
                    (1..poly.len().saturating_sub(1)).map(move |k| {
                        if flip {
                            [poly[0], poly[k + 1], poly[k]]
                        } else {
                            [poly[0], poly[k], poly[k + 1]]
                        }
                    })
                })
                .collect();
            let color = primitive
                .attribute("material")
                .map(|symbol| bindings.get(symbol).copied().unwrap_or(symbol))
                .and_then(|material| self.material_colors.get(material).copied());
            parts.add(color, &positions, &triangles);
        }
    }
}

/// 効果 (effect) の拡散色。テクスチャのときは None
fn effect_color(effect: Node) -> Option<[f32; 3]> {
    let diffuse = effect
        .descendants()
        .find(|n| n.has_tag_name_local("diffuse"))?;
    let values = floats(child(diffuse, "color")?);
    match values[..] {
        [r, g, b, ..] => Some([r as f32, g as f32, b as f32]),
        _ => None,
    }
}

/// source の float_array を、accessor の stride ごとの 3 次元の点にする
fn source_vectors(source: Node) -> Vec<DVec3> {
    let values = child(source, "float_array").map(floats).unwrap_or_default();
    let stride = child(source, "technique_common")
        .and_then(|t| child(t, "accessor"))
        .and_then(|a| a.attribute("stride"))
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|s| *s >= 3)
        .unwrap_or(3);
    values
        .chunks_exact(stride)
        .map(|v| DVec3::new(v[0], v[1], v[2]))
        .collect()
}

fn floats(node: Node) -> Vec<f64> {
    node.text()
        .unwrap_or("")
        .split_ascii_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect()
}

fn ints(node: Node) -> Vec<u32> {
    node.text()
        .unwrap_or("")
        .split_ascii_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect()
}

trait LocalName {
    fn has_tag_name_local(&self, name: &str) -> bool;
}

impl LocalName for Node<'_, '_> {
    /// 名前空間を見ずにタグ名を比べる
    fn has_tag_name_local(&self, name: &str) -> bool {
        self.is_element() && self.tag_name().name() == name
    }
}

fn elements<'a, 'input>(
    node: Node<'a, 'input>,
    name: &'static str,
) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children().filter(move |n| n.has_tag_name_local(name))
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &'static str) -> Option<Node<'a, 'input>> {
    elements(node, name).next()
}

/// ほかのテストからも使う小さな COLLADA
#[cfg(test)]
pub(crate) mod tests_support {
    /// 三角形 1 枚
    pub const TRIANGLE: &str = r##"<COLLADA><library_geometries><geometry id="g"><mesh>
      <source id="p"><float_array id="pa" count="9">0 0 0 1 0 0 0 1 0</float_array></source>
      <vertices id="v"><input semantic="POSITION" source="#p"/></vertices>
      <triangles count="1"><input semantic="VERTEX" source="#v" offset="0"/><p>0 1 2</p></triangles>
    </mesh></geometry></library_geometries></COLLADA>"##;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 辺 2 の正方形 (polylist の四角形 1 枚) を、mm 単位・ノードで X に 1000 動かして置く
    const SQUARE: &str = r##"<?xml version="1.0"?>
<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
  <asset><unit name="millimeter" meter="0.001"/><up_axis>Z_UP</up_axis></asset>
  <library_effects>
    <effect id="blue-effect"><profile_COMMON><technique sid="common"><phong>
      <diffuse><color sid="diffuse">0.1 0.2 0.8 1</color></diffuse>
    </phong></technique></profile_COMMON></effect>
  </library_effects>
  <library_materials>
    <material id="blue-material"><instance_effect url="#blue-effect"/></material>
  </library_materials>
  <library_geometries>
    <geometry id="sq"><mesh>
      <source id="sq-pos">
        <float_array id="sq-pos-array" count="12">0 0 0 2 0 0 2 2 0 0 2 0</float_array>
        <technique_common><accessor source="#sq-pos-array" count="4" stride="3"/></technique_common>
      </source>
      <source id="sq-n">
        <float_array id="sq-n-array" count="3">0 0 1</float_array>
        <technique_common><accessor source="#sq-n-array" count="1" stride="3"/></technique_common>
      </source>
      <vertices id="sq-v"><input semantic="POSITION" source="#sq-pos"/></vertices>
      <polylist material="mat" count="1">
        <input semantic="VERTEX" source="#sq-v" offset="0"/>
        <input semantic="NORMAL" source="#sq-n" offset="1"/>
        <vcount>4</vcount>
        <p>0 0 1 0 2 0 3 0</p>
      </polylist>
    </mesh></geometry>
  </library_geometries>
  <library_visual_scenes>
    <visual_scene id="Scene">
      <node id="n">
        <translate>1000 0 0</translate>
        <instance_geometry url="#sq">
          <bind_material><technique_common>
            <instance_material symbol="mat" target="#blue-material"/>
          </technique_common></bind_material>
        </instance_geometry>
      </node>
    </visual_scene>
  </library_visual_scenes>
  <scene><instance_visual_scene url="#Scene"/></scene>
</COLLADA>"##;

    #[test]
    fn reads_polylists_with_units_transforms_and_colors() {
        let parts = read(SQUARE).unwrap();
        assert_eq!(parts.len(), 1);
        let part = &parts[0];
        assert_eq!(part.color, Some([0.1, 0.2, 0.8]));
        // 四角形は三角形 2 枚に分ける
        assert_eq!(part.mesh.triangles, vec![[0, 1, 2], [0, 2, 3]]);
        // mm → m、ノードで X に 1000 mm
        let expected = [[1.0, 0.0, 0.0], [1.002, 0.0, 0.0], [1.002, 0.002, 0.0]];
        for (p, e) in part.mesh.positions.iter().zip(expected) {
            assert!((*p - DVec3::from_array(e)).length() < 1e-12, "{p}");
        }
    }

    #[test]
    fn mirrored_nodes_keep_faces_outward() {
        let mirrored = SQUARE.replace("<translate>1000 0 0</translate>", "<scale>-1 1 1</scale>");
        let parts = read(&mirrored).unwrap();
        assert_eq!(parts[0].mesh.triangles[0], [0, 2, 1]);
    }

    #[test]
    fn broken_xml_is_an_error() {
        assert!(read("<COLLADA><asset>").is_err());
    }
}
