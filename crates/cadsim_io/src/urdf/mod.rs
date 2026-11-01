//! URDF (ROS のロボットの記述) の読み込み
//!
//! URDF のリンク・関節・可動範囲を、形式に依存しないロボットの機種 [`RobotDescription`] に、
//! リンクの見た目 (メッシュと箱・円柱・球) を 3D の中間表現 [`Scene`] にする。
//! 関節をチャートにする計算はドメイン (`cadsim_core::motion::robot_import`) が受け持つ。
//!
//! - シーンは「ロボット名のノード → リンクごとのノード (全関節 0 の位置) → 見た目のメッシュ」の 3 段。
//!   リンクのノードは木にせず横に並べる。動きはタイミングチャートの関節が付けるため。
//! - URDF は Z 軸が上・メートル。いちばん上のノードで Y 軸が上のアプリの座標に回す。
//! - メッシュは COLLADA (.dae)・STL・OBJ・glTF を読む。`package://名前/…` は URDF の
//!   あるフォルダから上へたどって、その名前のフォルダを探す。
//! - xacro は読まない (`xacro` コマンドで URDF に直してから読む)。

pub mod collada;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use glam::{DAffine3, DMat3, DQuat, DVec3};
use roxmltree::{Document, Node};

use crate::interchange::{self, NodeTransform, Scene, SceneNode, TriMesh, UnitOptions};
use cadsim_core::motion::robot_import::{JointDescription, JointKind, RobotDescription};

/// メッシュファイルから読んだ形 1 つ (色ごと)
#[derive(Clone, Debug, PartialEq)]
pub struct MeshPart {
    pub name: String,
    pub mesh: TriMesh,
    pub color: Option<[f32; 3]>,
}

/// URDF を読んだ結果
#[derive(Clone, Debug)]
pub struct UrdfImport {
    pub scene: Scene,
    pub robot: RobotDescription,
    /// 読めなかったメッシュなど
    pub warnings: Vec<String>,
}

/// 拡張子が URDF か
pub fn is_urdf_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("urdf"))
}

/// URDF ファイルを読む。メッシュは URDF のあるフォルダを基準に探す
pub fn import(path: &Path) -> Result<UrdfImport, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("{} を開けませんでした: {e}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    read(&text, base)
}

/// URDF の文字列を読む。`base` はメッシュの相対パスの基準のフォルダ
pub fn read(text: &str, base: &Path) -> Result<UrdfImport, String> {
    let doc = Document::parse(text).map_err(|e| format!("URDF (XML) を読めません: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "robot" {
        return Err(
            if root.tag_name().name().contains("xacro") || text.contains("xmlns:xacro") {
                "xacro のままです。xacro コマンドで URDF に直してから読み込んでください".into()
            } else {
                "URDF ではありません (<robot> がありません)".into()
            },
        );
    }
    if text.contains("${") || text.contains("<xacro:") {
        return Err(
            "xacro の書き方 (${…} や <xacro:…>) が残っています。xacro コマンドで URDF に直してから読み込んでください"
                .into(),
        );
    }
    let name = root.attribute("name").unwrap_or("robot").to_owned();
    // 名前の付いた材質 (<robot> の直下) の色
    let materials: HashMap<&str, [f32; 3]> = elements(root, "material")
        .filter_map(|m| Some((m.attribute("name")?, material_color(m)?)))
        .collect();

    let links: Vec<Node> = elements(root, "link").collect();
    let joints: Vec<JointDescription> = elements(root, "joint")
        .map(parse_joint)
        .collect::<Result<_, _>>()?;
    let link_names: Vec<String> = links
        .iter()
        .filter_map(|l| l.attribute("name").map(str::to_owned))
        .collect();
    if link_names.is_empty() {
        return Err("リンクが 1 つもありません".into());
    }
    let known: HashSet<&str> = link_names.iter().map(String::as_str).collect();
    for joint in &joints {
        for link in [&joint.parent, &joint.child] {
            if !known.contains(link.as_str()) {
                return Err(format!(
                    "関節「{}」のリンク「{link}」がありません",
                    joint.name
                ));
            }
        }
    }
    let robot = RobotDescription {
        name: name.clone(),
        links: link_names,
        joints,
    };

    // 各リンクの位置 (全関節 0、URDF の座標)。関節の origin を根元から掛けていく
    let origins: HashMap<&str, DAffine3> = elements(root, "joint")
        .filter_map(|j| {
            let child = child(j, "child")?.attribute("link")?;
            Some((child, origin(j)))
        })
        .collect();
    let frames = link_frames(&robot, &origins);

    let mut scene = Scene::default();
    scene.nodes.push(SceneNode {
        name: name.clone(),
        // URDF (Z 軸が上) → アプリ (Y 軸が上)
        transform: NodeTransform {
            rotation: DQuat::from_rotation_x(-std::f64::consts::FRAC_PI_2),
            ..Default::default()
        },
        ..Default::default()
    });
    let mut warnings = Vec::new();
    let mut used_names = HashSet::new();
    let mut mesh_cache: HashMap<PathBuf, Result<Vec<MeshPart>, String>> = HashMap::new();
    for link in &links {
        let Some(link_name) = link.attribute("name") else {
            continue;
        };
        let frame = frames.get(link_name).copied().unwrap_or(DAffine3::IDENTITY);
        let link_index = scene.nodes.len();
        used_names.insert(link_name.to_owned());
        scene.nodes.push(SceneNode {
            name: link_name.to_owned(),
            parent: Some(0),
            transform: NodeTransform::from_affine(frame),
            ..Default::default()
        });
        for (v, visual) in elements(*link, "visual").enumerate() {
            let color = child(visual, "material").and_then(|m| {
                material_color(m).or_else(|| materials.get(m.attribute("name")?).copied())
            });
            let Some(geometry) = child(visual, "geometry") else {
                continue;
            };
            let parts = match geometry_parts(geometry, base, &mut mesh_cache) {
                Ok(parts) => parts,
                Err(e) => {
                    warnings.push(format!("リンク「{link_name}」の見た目: {e}"));
                    continue;
                }
            };
            let visual_origin = origin(visual);
            for (p, part) in parts.into_iter().enumerate() {
                // プロジェクトを開き直すとき、形状は部品の名前で対応づけるので、名前を重ねない
                let mut node_name = if part.name.is_empty() {
                    format!("{link_name} 見た目{}", v + 1)
                } else {
                    format!("{link_name} {}", part.name)
                };
                if p > 0 || used_names.contains(&node_name) {
                    let stem = node_name.clone();
                    let mut k = p + 1;
                    while used_names.contains(&node_name) {
                        node_name = format!("{stem} {k}");
                        k += 1;
                    }
                }
                used_names.insert(node_name.clone());
                scene.nodes.push(SceneNode {
                    name: node_name,
                    parent: Some(link_index),
                    transform: NodeTransform::from_affine(visual_origin),
                    mesh: Some(part.mesh),
                    color: part.color.or(color),
                });
            }
        }
    }
    Ok(UrdfImport {
        scene,
        robot,
        warnings,
    })
}

/// 根元のリンクから関節の origin を掛けて、各リンクの位置を求める
fn link_frames(
    robot: &RobotDescription,
    origins: &HashMap<&str, DAffine3>,
) -> HashMap<String, DAffine3> {
    let mut frames: HashMap<String, DAffine3> = HashMap::new();
    let mut queue: Vec<String> = robot.root_links().into_iter().map(str::to_owned).collect();
    for root in &queue {
        frames.insert(root.clone(), DAffine3::IDENTITY);
    }
    let mut i = 0;
    while i < queue.len() && i <= robot.links.len() {
        let parent = queue[i].clone();
        i += 1;
        let parent_frame = frames[&parent];
        for joint in robot.joints.iter().filter(|j| j.parent == parent) {
            if frames.contains_key(&joint.child) {
                continue;
            }
            let local = origins
                .get(joint.child.as_str())
                .copied()
                .unwrap_or(DAffine3::IDENTITY);
            frames.insert(joint.child.clone(), parent_frame * local);
            queue.push(joint.child.clone());
        }
    }
    frames
}

fn parse_joint(joint: Node) -> Result<JointDescription, String> {
    let name = joint.attribute("name").unwrap_or("").to_owned();
    let kind = match joint.attribute("type").unwrap_or("") {
        "revolute" => JointKind::Revolute,
        "continuous" => JointKind::Continuous,
        "prismatic" => JointKind::Prismatic,
        "fixed" => JointKind::Fixed,
        "floating" => JointKind::Floating,
        "planar" => JointKind::Planar,
        other => return Err(format!("関節「{name}」の種類「{other}」は分かりません")),
    };
    let link = |tag| {
        child(joint, tag)
            .and_then(|n| n.attribute("link"))
            .map(str::to_owned)
            .ok_or_else(|| format!("関節「{name}」に <{tag} link> がありません"))
    };
    let limit = child(joint, "limit");
    let attr = |node: Option<Node>, key| node.and_then(|n| n.attribute(key)).and_then(parse_f64);
    // 回り続ける関節 (continuous) の範囲は無視する。上下限が同じ (書かれていない) ときも範囲なし
    let lower = attr(limit, "lower").unwrap_or(0.0);
    let upper = attr(limit, "upper").unwrap_or(0.0);
    let limits = (matches!(kind, JointKind::Revolute | JointKind::Prismatic) && lower < upper)
        .then_some([lower, upper]);
    let axis = child(joint, "axis")
        .and_then(|a| a.attribute("xyz"))
        .and_then(vector)
        .filter(|v| v.length() > 1e-12)
        .unwrap_or(DVec3::X)
        .normalize()
        .to_array();
    Ok(JointDescription {
        kind,
        parent: link("parent")?,
        child: link("child")?,
        axis,
        limits,
        velocity: attr(limit, "velocity"),
        mimic: child(joint, "mimic")
            .and_then(|m| m.attribute("joint"))
            .map(str::to_owned),
        name,
    })
}

/// 見た目 1 つの形 (URDF の座標、見た目の origin の前)
fn geometry_parts(
    geometry: Node,
    base: &Path,
    cache: &mut HashMap<PathBuf, Result<Vec<MeshPart>, String>>,
) -> Result<Vec<MeshPart>, String> {
    let shape = geometry
        .children()
        .find(Node::is_element)
        .ok_or("形が書かれていません")?;
    let part = |mesh| {
        Ok(vec![MeshPart {
            name: String::new(),
            mesh,
            color: None,
        }])
    };
    match shape.tag_name().name() {
        "box" => {
            let size = shape
                .attribute("size")
                .and_then(vector)
                .ok_or("箱の size がありません")?;
            part(box_mesh(size))
        }
        "cylinder" => {
            let radius = shape.attribute("radius").and_then(parse_f64).unwrap_or(0.0);
            let length = shape.attribute("length").and_then(parse_f64).unwrap_or(0.0);
            part(cylinder_mesh(radius, length))
        }
        "sphere" => {
            let radius = shape.attribute("radius").and_then(parse_f64).unwrap_or(0.0);
            part(sphere_mesh(radius))
        }
        "mesh" => {
            let filename = shape
                .attribute("filename")
                .ok_or("メッシュの filename がありません")?;
            let path = resolve_mesh(filename, base)
                .ok_or_else(|| format!("メッシュ {filename} が見つかりません"))?;
            let scale = shape
                .attribute("scale")
                .and_then(vector)
                .unwrap_or(DVec3::ONE);
            let parts = cache
                .entry(path.clone())
                .or_insert_with(|| read_mesh(&path))
                .clone()?;
            Ok(parts
                .into_iter()
                .map(|mut p| {
                    scale_mesh(&mut p.mesh, scale);
                    p
                })
                .collect())
        }
        other => Err(format!("形「{other}」は読めません")),
    }
}

/// メッシュファイルを読む (メートル、ファイルの座標のまま)
pub fn read_mesh(path: &Path) -> Result<Vec<MeshPart>, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "dae" {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("{} を開けませんでした: {e}", path.display()))?;
        return collada::read(&text);
    }
    // STL / OBJ / glTF は 3D ファイルの読み込みを使う。URDF のメッシュはメートル・座標はそのまま
    let units = UnitOptions {
        meters_per_unit: 1.0,
        z_up: false,
    };
    let scene = interchange::import_file(path, &units)?;
    Ok(scene_parts(&scene))
}

/// シーンを、配置を焼き込んだメッシュの並びにする
fn scene_parts(scene: &Scene) -> Vec<MeshPart> {
    scene
        .nodes
        .iter()
        .zip(scene.world_transforms())
        .filter_map(|(node, world)| {
            let mesh = node.mesh.as_ref().filter(|m| !m.is_empty())?;
            let flip = world.matrix3.determinant() < 0.0;
            Some(MeshPart {
                name: node.name.clone(),
                mesh: TriMesh {
                    positions: mesh
                        .positions
                        .iter()
                        .map(|p| world.transform_point3(*p))
                        .collect(),
                    triangles: mesh
                        .triangles
                        .iter()
                        .map(|&[a, b, c]| if flip { [a, c, b] } else { [a, b, c] })
                        .collect(),
                },
                color: node.color,
            })
        })
        .collect()
}

/// `package://名前/…`・`file://…`・相対パスを、実際のファイルの場所にする
pub fn resolve_mesh(filename: &str, base: &Path) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = if let Some(rest) = filename.strip_prefix("package://") {
        let (package, relative) = rest.split_once('/')?;
        let mut candidates = Vec::new();
        for dir in base.ancestors() {
            if dir.file_name().is_some_and(|n| n == package) {
                candidates.push(dir.join(relative));
            }
            candidates.push(dir.join(package).join(relative));
            candidates.push(dir.join("share").join(package).join(relative));
        }
        // パッケージのフォルダ名が違っても、URDF のあるフォルダとその親から探す
        candidates.push(base.join(relative));
        if let Some(parent) = base.parent() {
            candidates.push(parent.join(relative));
        }
        candidates
    } else if let Some(rest) = filename.strip_prefix("file://") {
        let path = PathBuf::from(rest);
        vec![if path.is_absolute() {
            path
        } else {
            base.join(path)
        }]
    } else {
        vec![base.join(filename)]
    };
    candidates.into_iter().find(|p| p.is_file())
}

fn scale_mesh(mesh: &mut TriMesh, scale: DVec3) {
    if scale == DVec3::ONE {
        return;
    }
    for p in &mut mesh.positions {
        *p *= scale;
    }
    if scale.x * scale.y * scale.z < 0.0 {
        for t in &mut mesh.triangles {
            t.swap(1, 2);
        }
    }
}

/// 中心が原点の箱
fn box_mesh(size: DVec3) -> TriMesh {
    let h = size / 2.0;
    let positions = (0..8)
        .map(|i| {
            DVec3::new(
                if i & 1 == 0 { -h.x } else { h.x },
                if i & 2 == 0 { -h.y } else { h.y },
                if i & 4 == 0 { -h.z } else { h.z },
            )
        })
        .collect();
    // 面ごとの 4 頂点 (外から見て反時計回り)
    let faces = [
        [0, 2, 3, 1], // -Z
        [4, 5, 7, 6], // +Z
        [0, 1, 5, 4], // -Y
        [2, 6, 7, 3], // +Y
        [0, 4, 6, 2], // -X
        [1, 3, 7, 5], // +X
    ];
    TriMesh {
        positions,
        triangles: faces
            .iter()
            .flat_map(|[a, b, c, d]| [[*a, *b, *c], [*a, *c, *d]])
            .collect(),
    }
}

/// 中心が原点で Z 軸に沿った円柱
fn cylinder_mesh(radius: f64, length: f64) -> TriMesh {
    const N: u32 = 32;
    let h = length / 2.0;
    let mut positions = vec![DVec3::new(0.0, 0.0, -h), DVec3::new(0.0, 0.0, h)];
    for i in 0..N {
        let a = std::f64::consts::TAU * f64::from(i) / f64::from(N);
        let (s, c) = a.sin_cos();
        positions.push(DVec3::new(radius * c, radius * s, -h));
        positions.push(DVec3::new(radius * c, radius * s, h));
    }
    let mut triangles = Vec::new();
    for i in 0..N {
        let j = (i + 1) % N;
        let (b0, t0, b1, t1) = (2 + 2 * i, 3 + 2 * i, 2 + 2 * j, 3 + 2 * j);
        triangles.push([0, b1, b0]);
        triangles.push([1, t0, t1]);
        triangles.push([b0, b1, t1]);
        triangles.push([b0, t1, t0]);
    }
    TriMesh {
        positions,
        triangles,
    }
}

/// 中心が原点の球 (緯度経度で分ける)
fn sphere_mesh(radius: f64) -> TriMesh {
    const LAT: u32 = 12;
    const LON: u32 = 24;
    let mut positions = Vec::new();
    for i in 0..=LAT {
        let theta = std::f64::consts::PI * f64::from(i) / f64::from(LAT);
        for j in 0..LON {
            let phi = std::f64::consts::TAU * f64::from(j) / f64::from(LON);
            positions.push(
                DVec3::new(
                    theta.sin() * phi.cos(),
                    theta.sin() * phi.sin(),
                    theta.cos(),
                ) * radius,
            );
        }
    }
    let mut triangles = Vec::new();
    for i in 0..LAT {
        for j in 0..LON {
            let k = (j + 1) % LON;
            let (a, b) = (i * LON + j, i * LON + k);
            let (c, d) = ((i + 1) * LON + j, (i + 1) * LON + k);
            triangles.push([a, c, d]);
            triangles.push([a, d, b]);
        }
    }
    TriMesh {
        positions,
        triangles,
    }
}

/// `<origin xyz rpy>` (なければ原点)。rpy は固定軸の X → Y → Z の順の回転
fn origin(node: Node) -> DAffine3 {
    let Some(origin) = child(node, "origin") else {
        return DAffine3::IDENTITY;
    };
    let xyz = origin
        .attribute("xyz")
        .and_then(vector)
        .unwrap_or(DVec3::ZERO);
    let rpy = origin
        .attribute("rpy")
        .and_then(vector)
        .unwrap_or(DVec3::ZERO);
    let rotation = DQuat::from_rotation_z(rpy.z)
        * DQuat::from_rotation_y(rpy.y)
        * DQuat::from_rotation_x(rpy.x);
    DAffine3::from_mat3_translation(DMat3::from_quat(rotation), xyz)
}

/// `<material>` に直接書かれた色
fn material_color(material: Node) -> Option<[f32; 3]> {
    let rgba = child(material, "color")?.attribute("rgba")?;
    let v: Vec<f32> = rgba
        .split_ascii_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    match v[..] {
        [r, g, b, ..] => Some([r, g, b]),
        _ => None,
    }
}

fn parse_f64(text: &str) -> Option<f64> {
    text.trim().parse().ok()
}

fn vector(text: &str) -> Option<DVec3> {
    let v: Vec<f64> = text
        .split_ascii_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    match v[..] {
        [x, y, z] => Some(DVec3::new(x, y, z)),
        _ => None,
    }
}

fn elements<'a, 'input>(
    node: Node<'a, 'input>,
    name: &'static str,
) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children()
        .filter(move |n| n.is_element() && n.tag_name().name() == name)
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &'static str) -> Option<Node<'a, 'input>> {
    elements(node, name).next()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 台 (箱) → 旋回 (Z 軸) → 腕 (円柱、Y 軸まわりに回る) → 固定の tool0
    const ARM: &str = r#"<?xml version="1.0"?>
<robot name="test_arm">
  <material name="blue"><color rgba="0 0 1 1"/></material>
  <link name="base_link">
    <visual><geometry><box size="0.2 0.2 0.1"/></geometry><material name="blue"/></visual>
  </link>
  <link name="turret">
    <visual>
      <origin xyz="0 0 0.25" rpy="0 0 0"/>
      <geometry><cylinder radius="0.05" length="0.5"/></geometry>
      <material name="red"><color rgba="1 0 0 1"/></material>
    </visual>
  </link>
  <link name="arm"><visual><geometry><sphere radius="0.05"/></geometry></visual></link>
  <link name="tool0"/>
  <joint name="j1" type="revolute">
    <parent link="base_link"/><child link="turret"/>
    <origin xyz="0 0 0.1" rpy="0 0 0"/>
    <axis xyz="0 0 1"/>
    <limit lower="-2.5" upper="2.5" velocity="2.0" effort="10"/>
  </joint>
  <joint name="j2" type="continuous">
    <parent link="turret"/><child link="arm"/>
    <origin xyz="0 0 0.5" rpy="1.5707963267948966 0 0"/>
    <axis xyz="0 0 1"/>
  </joint>
  <joint name="tool" type="fixed">
    <parent link="arm"/><child link="tool0"/>
    <origin xyz="0 0.3 0"/>
  </joint>
</robot>"#;

    #[test]
    fn reads_links_joints_and_primitive_visuals() {
        let import = read(ARM, Path::new(".")).unwrap();
        let robot = &import.robot;
        assert_eq!(robot.name, "test_arm");
        assert_eq!(robot.links, ["base_link", "turret", "arm", "tool0"]);
        let j1 = &robot.joints[0];
        assert_eq!(
            (j1.kind, j1.limits, j1.velocity),
            (JointKind::Revolute, Some([-2.5, 2.5]), Some(2.0))
        );
        assert_eq!(robot.joints[1].kind, JointKind::Continuous);
        assert_eq!(robot.joints[2].kind, JointKind::Fixed);

        let scene = &import.scene;
        // ロボット + リンク 4 + 見た目 3
        assert_eq!(scene.nodes.len(), 8);
        let world = scene.world_transforms();
        let find = |name: &str| scene.nodes.iter().position(|n| n.name == name).unwrap();
        // tool0 は URDF で (0, 0, 0.6) + 腕の向きで Y → Z に回った 0.3 = (0, 0, 0.9)。アプリでは Y が上
        let tool = world[find("tool0")].translation;
        assert!((tool - DVec3::new(0.0, 0.9, 0.0)).length() < 1e-9, "{tool}");
        // 色: 名前で引く材質と、見た目に直接書いた材質
        assert_eq!(
            scene.nodes[find("base_link 見た目1")].color,
            Some([0.0, 0.0, 1.0])
        );
        assert_eq!(
            scene.nodes[find("turret 見た目1")].color,
            Some([1.0, 0.0, 0.0])
        );
        assert!(import.warnings.is_empty());
    }

    #[test]
    fn missing_meshes_are_warnings() {
        let text = ARM.replace(
            r#"<sphere radius="0.05"/>"#,
            r#"<mesh filename="package://nowhere/meshes/arm.stl"/>"#,
        );
        let import = read(&text, Path::new(".")).unwrap();
        assert_eq!(import.warnings.len(), 1);
        assert!(import.warnings[0].contains("arm.stl"));
    }

    #[test]
    fn xacro_files_are_rejected_with_a_hint() {
        let text = r#"<robot xmlns:xacro="http://www.ros.org/wiki/xacro" name="r">
            <xacro:property name="a" value="1"/><link name="${a}"/></robot>"#;
        let err = read(text, Path::new(".")).unwrap_err();
        assert!(err.contains("xacro"), "{err}");
    }

    #[test]
    fn package_paths_are_found_above_the_urdf() {
        let dir = std::env::temp_dir().join(format!("plantwright-urdf-{}", std::process::id()));
        let urdf_dir = dir.join("my_robot").join("urdf");
        let mesh_dir = dir.join("my_robot").join("meshes");
        std::fs::create_dir_all(&urdf_dir).unwrap();
        std::fs::create_dir_all(&mesh_dir).unwrap();
        std::fs::write(mesh_dir.join("part.dae"), collada::tests_support::TRIANGLE).unwrap();
        let found = resolve_mesh("package://my_robot/meshes/part.dae", &urdf_dir);
        assert_eq!(found, Some(mesh_dir.join("part.dae")));
        let parts = read_mesh(&mesh_dir.join("part.dae")).unwrap();
        assert_eq!(parts[0].mesh.triangles.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn primitive_meshes_are_closed_and_sized() {
        assert_eq!(sphere_mesh(1.0).triangles.len(), 12 * 24 * 2);
        for mesh in [box_mesh(DVec3::new(1.0, 2.0, 3.0)), cylinder_mesh(0.5, 2.0)] {
            // 閉じた形なら、どの辺もちょうど 2 枚の三角形に (逆向きで) 使われる
            let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
            for t in &mesh.triangles {
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    if a == b {
                        continue;
                    }
                    *edges.entry((a.min(b), a.max(b))).or_default() += if a < b { 1 } else { -1 };
                }
            }
            assert!(edges.values().all(|v| *v == 0));
        }
        let b = box_mesh(DVec3::new(1.0, 2.0, 3.0));
        let max = b.positions.iter().fold(DVec3::ZERO, |m, p| m.max(*p));
        assert_eq!(max, DVec3::new(0.5, 1.0, 1.5));
    }
}
