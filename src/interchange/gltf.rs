//! glTF 2.0 (.gltf + .bin / .glb)
//!
//! glTF はメートル・Y 軸が上なのでアプリと同じ座標系のまま読み書きする。
//! 階層 (ノードの親子と配置)、名前、ベースカラー、ノードのアニメーション (移動・回転・拡大) に対応。
//! テクスチャ、スキン、モーフターゲットは読み飛ばす。

use std::{collections::HashMap, path::Path};

use bevy::math::{DQuat, DVec3};
use gltf::{
    Gltf,
    animation::{Interpolation, util::ReadOutputs},
    mesh::Mode,
};
use serde_json::{Value, json};

use super::{
    Animation, ExchangeResult, NodeTransform, Scene, SceneNode, Track, TriMesh, UnitOptions,
    io_error,
};

// ---------------------------------------------------------------------------
// 読み込み
// ---------------------------------------------------------------------------

pub fn import(path: &Path, _units: &UnitOptions) -> ExchangeResult<Scene> {
    let gltf = Gltf::open(path).map_err(|e| io_error(path, e))?;
    // テクスチャ画像は使わないので、バッファだけ読む
    let buffers = gltf::import_buffers(&gltf.document, path.parent(), gltf.blob.clone())
        .map_err(|e| io_error(path, e))?;
    let doc = &gltf.document;
    let read = |buffer: gltf::Buffer| buffers.get(buffer.index()).map(|d| d.0.as_slice());

    let mut scene = Scene::default();
    // glTF のノード番号 → Scene のノード番号 (アニメーションの対応付けに使う)
    let mut index_of: HashMap<usize, usize> = HashMap::new();

    // 親が子より前に並ぶよう、ルートから深さ優先でたどる
    let roots: Vec<gltf::Node> = match doc.default_scene().or_else(|| doc.scenes().next()) {
        Some(s) => s.nodes().collect(),
        None => {
            let children: Vec<usize> = doc
                .nodes()
                .flat_map(|n| n.children().map(|c| c.index()))
                .collect();
            doc.nodes()
                .filter(|n| !children.contains(&n.index()))
                .collect()
        }
    };
    let mut stack: Vec<(gltf::Node, Option<usize>)> =
        roots.into_iter().rev().map(|n| (n, None)).collect();
    while let Some((node, parent)) = stack.pop() {
        // 同じノードが複数のシーンから参照されていても1回だけ取り込む
        if index_of.contains_key(&node.index()) {
            continue;
        }
        let (t, r, s) = node.transform().decomposed();
        let index = scene.nodes.len();
        index_of.insert(node.index(), index);
        scene.nodes.push(SceneNode {
            name: node
                .name()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("ノード {}", node.index())),
            parent,
            transform: NodeTransform {
                translation: dvec3(t),
                rotation: dquat(r),
                scale: dvec3(s),
            },
            ..Default::default()
        });

        if let Some(mesh) = node.mesh() {
            let parts: Vec<(TriMesh, Option<[f32; 3]>)> = mesh
                .primitives()
                .filter_map(|p| read_primitive(&p, read))
                .collect();
            if parts.len() == 1 {
                let (mesh, color) = parts.into_iter().next().unwrap();
                scene.nodes[index].mesh = Some(mesh);
                scene.nodes[index].color = color;
            } else {
                // 色の違うプリミティブは子の部品に分ける
                let base = mesh.name().unwrap_or(&scene.nodes[index].name).to_owned();
                for (i, (mesh, color)) in parts.into_iter().enumerate() {
                    scene.nodes.push(SceneNode {
                        name: format!("{base} #{}", i + 1),
                        parent: Some(index),
                        mesh: Some(mesh),
                        color,
                        ..Default::default()
                    });
                }
            }
        }
        for child in node.children().collect::<Vec<_>>().into_iter().rev() {
            stack.push((child, Some(index)));
        }
    }

    for anim in doc.animations() {
        let animation = read_animation(&anim, &scene, &index_of, read);
        if !animation.tracks.is_empty() {
            scene.animations.push(animation);
        }
    }
    Ok(scene)
}

fn read_primitive<'a, 's>(
    primitive: &'a gltf::Primitive<'a>,
    read: impl Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
) -> Option<(TriMesh, Option<[f32; 3]>)> {
    let reader = primitive.reader(read);
    let positions: Vec<DVec3> = reader.read_positions()?.map(dvec3).collect();
    let indices: Vec<u32> = match reader.read_indices() {
        Some(i) => i.into_u32().collect(),
        None => (0..positions.len() as u32).collect(),
    };
    let triangles: Vec<[u32; 3]> = match primitive.mode() {
        Mode::Triangles => indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| [t[0], t[1], t[2]])
            .collect(),
        Mode::TriangleStrip => (0..indices.len().saturating_sub(2))
            .map(|i| {
                let [a, b, c] = [indices[i], indices[i + 1], indices[i + 2]];
                // 奇数番目は向きが逆になる
                if i % 2 == 0 { [a, b, c] } else { [b, a, c] }
            })
            .collect(),
        Mode::TriangleFan => (1..indices.len().saturating_sub(1))
            .map(|i| [indices[0], indices[i], indices[i + 1]])
            .collect(),
        // 点と線は形状として扱わない
        _ => return None,
    };
    let [r, g, b, _] = primitive
        .material()
        .pbr_metallic_roughness()
        .base_color_factor();
    let color = primitive
        .material()
        .index()
        .map(|_| [r, g, b].map(linear_to_srgb));
    Some((
        TriMesh {
            positions,
            triangles,
        },
        color,
    ))
}

/// チャンネル (移動・回転・拡大がばらばら) をノードごとの姿勢のキーにまとめる
fn read_animation<'a, 's>(
    anim: &gltf::Animation<'a>,
    scene: &Scene,
    index_of: &HashMap<usize, usize>,
    read: impl Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
) -> Animation {
    // ノード → 各成分のキー
    #[derive(Default)]
    struct Channels {
        translation: Vec<(f64, DVec3)>,
        rotation: Vec<(f64, DQuat)>,
        scale: Vec<(f64, DVec3)>,
        step: bool,
    }
    let mut by_node: HashMap<usize, Channels> = HashMap::new();
    for channel in anim.channels() {
        let Some(&node) = index_of.get(&channel.target().node().index()) else {
            continue;
        };
        let reader = channel.reader(read.clone());
        let (Some(inputs), Some(outputs)) = (reader.read_inputs(), reader.read_outputs()) else {
            continue;
        };
        let times: Vec<f64> = inputs.map(f64::from).collect();
        let interpolation = channel.sampler().interpolation();
        // CubicSpline は (入りの接線, 値, 出の接線) の3つ組なので値だけ使い、線形で近似する
        let pick = |values: Vec<[f64; 4]>| -> Vec<[f64; 4]> {
            if interpolation == Interpolation::CubicSpline {
                values.as_chunks::<3>().0.iter().map(|c| c[1]).collect()
            } else {
                values
            }
        };
        let entry = by_node.entry(node).or_default();
        entry.step |= interpolation == Interpolation::Step;
        match outputs {
            ReadOutputs::Translations(v) => {
                let v = pick(
                    v.map(|[x, y, z]| [x as f64, y as f64, z as f64, 0.0])
                        .collect(),
                );
                entry.translation = zip_keys(&times, v, |[x, y, z, _]| DVec3::new(x, y, z));
            }
            ReadOutputs::Scales(v) => {
                let v = pick(
                    v.map(|[x, y, z]| [x as f64, y as f64, z as f64, 0.0])
                        .collect(),
                );
                entry.scale = zip_keys(&times, v, |[x, y, z, _]| DVec3::new(x, y, z));
            }
            ReadOutputs::Rotations(v) => {
                let v = pick(v.into_f32().map(|q| q.map(f64::from)).collect());
                entry.rotation = zip_keys(&times, v, |q| DQuat::from_array(q).normalize());
            }
            ReadOutputs::MorphTargetWeights(_) => {}
        }
    }

    let mut tracks: Vec<Track> = by_node
        .into_iter()
        .map(|(node, ch)| {
            let rest = scene.nodes[node].transform;
            let mut times: Vec<f64> = ch
                .translation
                .iter()
                .map(|k| k.0)
                .chain(ch.rotation.iter().map(|k| k.0))
                .chain(ch.scale.iter().map(|k| k.0))
                .collect();
            times.sort_by(f64::total_cmp);
            times.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            let keys = times
                .iter()
                .map(|&t| {
                    let transform = NodeTransform {
                        translation: sample(&ch.translation, t, ch.step, DVec3::lerp)
                            .unwrap_or(rest.translation),
                        rotation: sample(&ch.rotation, t, ch.step, DQuat::slerp)
                            .unwrap_or(rest.rotation),
                        scale: sample(&ch.scale, t, ch.step, DVec3::lerp).unwrap_or(rest.scale),
                    };
                    (t, transform)
                })
                .collect();
            Track { node, keys }
        })
        .collect();
    tracks.sort_by_key(|t| t.node);
    Animation {
        name: anim
            .name()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("アニメーション {}", anim.index() + 1)),
        tracks,
    }
}

fn zip_keys<T>(times: &[f64], values: Vec<[f64; 4]>, f: impl Fn([f64; 4]) -> T) -> Vec<(f64, T)> {
    times
        .iter()
        .copied()
        .zip(values.into_iter().map(f))
        .collect()
}

/// 1成分のキーを時刻 t で補間する。キーが無ければ None
fn sample<T: Copy>(keys: &[(f64, T)], t: f64, step: bool, lerp: fn(T, T, f64) -> T) -> Option<T> {
    let first = keys.first()?;
    if t <= first.0 {
        return Some(first.1);
    }
    let i = keys.partition_point(|k| k.0 <= t);
    let Some(next) = keys.get(i) else {
        return keys.last().map(|k| k.1);
    };
    let prev = keys[i - 1];
    if step {
        return Some(prev.1);
    }
    let span = next.0 - prev.0;
    Some(lerp(
        prev.1,
        next.1,
        if span > 0.0 { (t - prev.0) / span } else { 1.0 },
    ))
}

fn dvec3(v: [f32; 3]) -> DVec3 {
    DVec3::from(v.map(f64::from))
}

fn dquat(q: [f32; 4]) -> DQuat {
    DQuat::from_array(q.map(f64::from)).normalize()
}

/// glTF の色はリニア、アプリの色は sRGB
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

// ---------------------------------------------------------------------------
// 書き出し
// ---------------------------------------------------------------------------

pub fn export(scene: &Scene, path: &Path, _units: &UnitOptions) -> ExchangeResult<()> {
    let glb = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("glb"));
    let mut writer = Writer::default();
    let mut json = writer.build(scene);

    if glb {
        json["buffers"] = json!([{ "byteLength": writer.bin.len() }]);
        let bytes = to_glb(&json, &writer.bin);
        std::fs::write(path, bytes).map_err(|e| io_error(path, e))
    } else {
        let bin_path = path.with_extension("bin");
        let uri = bin_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        json["buffers"] = json!([{ "byteLength": writer.bin.len(), "uri": uri }]);
        let text = serde_json::to_string_pretty(&json).map_err(|e| io_error(path, e))?;
        std::fs::write(&bin_path, &writer.bin).map_err(|e| io_error(&bin_path, e))?;
        std::fs::write(path, text).map_err(|e| io_error(path, e))
    }
}

/// バイナリバッファと bufferView / accessor を組み立てる
#[derive(Default)]
struct Writer {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

/// glTF の定数
const FLOAT: u32 = 5126;
const UNSIGNED_INT: u32 = 5125;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

impl Writer {
    fn build(&mut self, scene: &Scene) -> Value {
        let mut materials: Vec<Value> = Vec::new();
        let mut material_of: HashMap<[u32; 3], usize> = HashMap::new();
        let mut meshes: Vec<Value> = Vec::new();
        let children = scene.children();

        let nodes: Vec<Value> = scene
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let t = node.transform;
                let mut value = json!({
                    "name": node.name,
                    "translation": t.translation.as_vec3().to_array(),
                    "rotation": t.rotation.as_quat().to_array(),
                    "scale": t.scale.as_vec3().to_array(),
                });
                if !children[index].is_empty() {
                    value["children"] = json!(children[index]);
                }
                if let Some(mesh) = node.mesh.as_ref().filter(|m| !m.is_empty()) {
                    let mut primitive = self.mesh_attributes(mesh);
                    if let Some(color) = node.color {
                        let next = materials.len();
                        let material =
                            *material_of
                                .entry(color.map(f32::to_bits))
                                .or_insert_with(|| {
                                    let [r, g, b] = color.map(srgb_to_linear);
                                    materials.push(json!({
                                        "name": format!("color_{next}"),
                                        "pbrMetallicRoughness": {
                                            "baseColorFactor": [r, g, b, 1.0],
                                            "metallicFactor": 0.0,
                                            "roughnessFactor": 0.6,
                                        },
                                        "doubleSided": true,
                                    }));
                                    next
                                });
                        primitive["material"] = json!(material);
                    }
                    value["mesh"] = json!(meshes.len());
                    meshes.push(json!({ "name": node.name, "primitives": [primitive] }));
                }
                value
            })
            .collect();

        let roots: Vec<usize> = (0..scene.nodes.len())
            .filter(|&i| scene.nodes[i].parent.is_none())
            .collect();
        let animations: Vec<Value> = scene
            .animations
            .iter()
            .filter(|a| !a.tracks.is_empty())
            .map(|a| self.animation(a))
            .collect();

        let mut json = json!({
            "asset": { "version": "2.0", "generator": "cadsim" },
            "scene": 0,
            "scenes": [{ "nodes": roots }],
            "nodes": nodes,
        });
        for (key, list) in [
            ("meshes", meshes),
            ("materials", materials),
            ("animations", animations),
        ] {
            if !list.is_empty() {
                json[key] = Value::Array(list);
            }
        }
        json["bufferViews"] = Value::Array(std::mem::take(&mut self.views));
        json["accessors"] = Value::Array(std::mem::take(&mut self.accessors));
        json
    }

    /// 位置と頂点番号を書き、プリミティブの JSON を返す
    fn mesh_attributes(&mut self, mesh: &TriMesh) -> Value {
        let positions: Vec<[f32; 3]> = mesh
            .positions
            .iter()
            .map(|p| p.as_vec3().to_array())
            .collect();
        let position = self.float_accessor(&positions, "VEC3", Some(ARRAY_BUFFER));
        let indices: Vec<u32> = mesh.triangles.iter().flatten().copied().collect();
        let view = self.view(bytemuck_u32(&indices), Some(ELEMENT_ARRAY_BUFFER));
        let accessor = self.accessors.len();
        self.accessors.push(json!({
            "bufferView": view,
            "componentType": UNSIGNED_INT,
            "count": indices.len(),
            "type": "SCALAR",
        }));
        json!({ "attributes": { "POSITION": position }, "indices": accessor, "mode": 4 })
    }

    fn animation(&mut self, animation: &Animation) -> Value {
        let mut samplers = Vec::new();
        let mut channels = Vec::new();
        for track in &animation.tracks {
            let times: Vec<[f32; 1]> = track.keys.iter().map(|k| [k.0 as f32]).collect();
            let input = self.float_accessor(&times, "SCALAR", None);
            let translations: Vec<[f32; 3]> = track
                .keys
                .iter()
                .map(|k| k.1.translation.as_vec3().to_array())
                .collect();
            let rotations: Vec<[f32; 4]> = track
                .keys
                .iter()
                .map(|k| k.1.rotation.as_quat().to_array())
                .collect();
            let scales: Vec<[f32; 3]> = track
                .keys
                .iter()
                .map(|k| k.1.scale.as_vec3().to_array())
                .collect();
            let outputs = [
                (
                    "translation",
                    self.float_accessor(&translations, "VEC3", None),
                ),
                ("rotation", self.float_accessor(&rotations, "VEC4", None)),
                ("scale", self.float_accessor(&scales, "VEC3", None)),
            ];
            for (path, output) in outputs {
                channels.push(json!({
                    "sampler": samplers.len(),
                    "target": { "node": track.node, "path": path },
                }));
                samplers
                    .push(json!({ "input": input, "output": output, "interpolation": "LINEAR" }));
            }
        }
        json!({ "name": animation.name, "samplers": samplers, "channels": channels })
    }

    /// f32 の配列を書いて accessor の番号を返す (min / max 付き)
    fn float_accessor<const N: usize>(
        &mut self,
        values: &[[f32; N]],
        kind: &str,
        target: Option<u32>,
    ) -> usize {
        let mut min = [f32::MAX; N];
        let mut max = [f32::MIN; N];
        let mut bytes = Vec::with_capacity(values.len() * N * 4);
        for v in values {
            for i in 0..N {
                min[i] = min[i].min(v[i]);
                max[i] = max[i].max(v[i]);
                bytes.extend_from_slice(&v[i].to_le_bytes());
            }
        }
        let view = self.view(bytes, target);
        self.accessors.push(json!({
            "bufferView": view,
            "componentType": FLOAT,
            "count": values.len(),
            "type": kind,
            "min": min.to_vec(),
            "max": max.to_vec(),
        }));
        self.accessors.len() - 1
    }

    fn view(&mut self, bytes: Vec<u8>, target: Option<u32>) -> usize {
        // accessor は4バイト境界から始める
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut view = json!({
            "buffer": 0,
            "byteOffset": self.bin.len(),
            "byteLength": bytes.len(),
        });
        if let Some(target) = target {
            view["target"] = json!(target);
        }
        self.bin.extend_from_slice(&bytes);
        self.views.push(view);
        self.views.len() - 1
    }
}

fn bytemuck_u32(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// GLB = ヘッダ + JSON チャンク + BIN チャンク (どれも4バイト境界にそろえる)
fn to_glb(json: &Value, bin: &[u8]) -> Vec<u8> {
    let mut json_bytes = serde_json::to_vec(json).expect("JSON に変換できない値はない");
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let mut bin = bin.to_vec();
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

#[cfg(test)]
mod tests {
    use super::super::test_util::{assert_same_shape, sample_scene};
    use super::*;

    fn round_trip(file: &str) {
        let dir = std::env::temp_dir().join("cadsim_gltf_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(file);
        let scene = sample_scene();
        export(&scene, &path, &UnitOptions::default()).unwrap();
        // gltf crate の検証も通ること
        let back = import(&path, &UnitOptions::default()).unwrap();

        let names: Vec<&str> = back.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["ライン", "台", "シリンダ"]);
        assert_eq!(back.nodes[2].parent, Some(0));
        let t = back.nodes[2].transform;
        assert!(t.scale.abs_diff_eq(DVec3::new(0.5, 2.0, 0.5), 1e-6));
        assert!(t.rotation.abs_diff_eq(DQuat::from_rotation_y(0.5), 1e-6));
        let color = back.nodes[1].color.unwrap();
        assert!((color[2] - 0.8).abs() < 1e-4, "色は sRGB に戻る");
        assert_same_shape(&back.merged_mesh(), &scene.merged_mesh(), 1e-5);

        let anim = &back.animations[0];
        assert_eq!(anim.name, "上下動");
        assert_eq!(anim.tracks[0].node, 2);
        let y = anim.tracks[0].sample(0.5).unwrap().translation.y;
        assert!((y - 1.5).abs() < 1e-6);
    }

    #[test]
    fn round_trip_glb() {
        round_trip("line.glb");
    }

    #[test]
    fn round_trip_gltf_with_bin() {
        round_trip("line.gltf");
        let dir = std::env::temp_dir().join("cadsim_gltf_test");
        assert!(dir.join("line.bin").exists());
    }

    #[test]
    fn splits_primitives_and_merges_channels() {
        // 2つのプリミティブ (色違い) と、移動だけのチャンネルを持つ最小の glTF
        let dir = std::env::temp_dir().join("cadsim_gltf_test");
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = Writer::default();
        let pos = w.float_accessor(
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            "VEC3",
            None,
        );
        let times = w.float_accessor(&[[0.0], [1.0]], "SCALAR", None);
        let moves = w.float_accessor(&[[0.0, 0.0, 0.0], [0.0, 0.0, 2.0]], "VEC3", None);
        let mut json = json!({
            "asset": { "version": "2.0" },
            "scenes": [{ "nodes": [0] }],
            "nodes": [{ "name": "腕", "mesh": 0, "rotation": [0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2] }],
            "meshes": [{ "name": "腕メッシュ", "primitives": [
                { "attributes": { "POSITION": pos }, "material": 0 },
                { "attributes": { "POSITION": pos }, "material": 1 },
            ]}],
            "materials": [
                { "pbrMetallicRoughness": { "baseColorFactor": [1.0, 0.0, 0.0, 1.0] } },
                { "pbrMetallicRoughness": { "baseColorFactor": [0.0, 0.0, 1.0, 1.0] } },
            ],
            "animations": [{
                "samplers": [{ "input": times, "output": moves }],
                "channels": [{ "sampler": 0, "target": { "node": 0, "path": "translation" } }],
            }],
            "bufferViews": w.views,
            "accessors": w.accessors,
        });
        json["buffers"] = json!([{ "byteLength": w.bin.len() }]);
        let path = dir.join("prims.glb");
        std::fs::write(&path, to_glb(&json, &w.bin)).unwrap();

        let scene = import(&path, &UnitOptions::default()).unwrap();
        assert_eq!(scene.nodes.len(), 3);
        assert_eq!(scene.nodes[1].name, "腕メッシュ #1");
        let red = scene.nodes[1].color.unwrap();
        assert!((red[0] - 1.0).abs() < 1e-5 && red[1] == 0.0 && red[2] == 0.0);
        assert_eq!(scene.nodes[2].parent, Some(0));
        let key = scene.animations[0].tracks[0].sample(0.5).unwrap();
        assert!((key.translation.z - 1.0).abs() < 1e-6);
        // 回転のチャンネルが無いので、静止時の回転が保たれる
        assert!(
            key.rotation
                .abs_diff_eq(scene.nodes[0].transform.rotation, 1e-6)
        );
    }
}
