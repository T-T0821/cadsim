//! OpenUSD (.usda / .usdc / .usdz / .usd) の読み書き
//!
//! 書き出しは USDA のテキストを自前で組み立て、.usdc / .usdz のときは純 Rust の
//! `openusd` crate でバイナリ (Crate) 形式に変換する。読み込みは `openusd` の Stage を
//! 開き、合成 (リファレンス・ペイロード・バリアント・インスタンス) 済みのシーンをたどる。
//!
//! 中間表現 ([`Scene`]) はメートル・Y 上・秒で持つ。USD 側の
//! `metersPerUnit` / `upAxis` / `timeCodesPerSecond` は読み込み時にこれへ変換する。
//!
//! 書き出すもの:
//! - ノードの階層を Xform で (ルートは `/World`、kind = assembly)。日本語名は `displayName` に入れ、
//!   プリム名は ASCII の識別子にする
//! - 変換は `xformOp:translate` / `xformOp:orient` / `xformOp:scale`。動きは同じ属性の
//!   `timeSamples` (Omniverse などでそのまま再生できる)
//! - 形状は Xform の子の `Mesh "Geom"` (三角形, faceVarying 法線, extent)
//! - 色は `UsdPreviewSurface` のマテリアルと `primvars:displayColor`
//!
//! 読み込めないもの (無視する): NURBS 曲面・曲線・点群・PointInstancer・スキニング・
//! ブレンドシェイプ・頂点アニメーション・テクスチャ・UsdPreviewSurface 以外のシェーダー。
//! シアーを含む変換は回転・拡大縮小に近似する。

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Cursor;
use std::path::Path;

use bevy::math::{DAffine3, DMat4, DQuat, DVec3};
use openusd::{gf, sdf, usd, usda, usdc, usdz};

use super::{
    Animation, ExchangeResult, NodeTransform, Scene, SceneNode, Track, TriMesh, UnitOptions,
};

/// 書き出すときの1秒あたりのタイムコード
const TIME_CODES_PER_SECOND: f64 = 60.0;
/// 書き出す法線で、隣の三角形と滑らかにつなぐ最大の角度 (度)
const SMOOTH_ANGLE_DEG: f64 = 30.0;

// ---------------------------------------------------------------------------
// 書き出し
// ---------------------------------------------------------------------------

/// 拡張子に合わせて .usda (テキスト) / .usdc・.usd (バイナリ) / .usdz (パッケージ) で書き出す。
pub fn export(scene: &Scene, path: &Path, _units: &UnitOptions) -> ExchangeResult<()> {
    let text = to_usda(scene);
    let ext = extension(path);
    let bytes = match ext.as_str() {
        "usda" => text.into_bytes(),
        "usdc" | "usd" => to_usdc(&text)?,
        "usdz" => {
            let crate_bytes = to_usdc(&text)?;
            let mut writer = usdz::ArchiveWriter::new(Cursor::new(Vec::new()));
            writer
                .add_layer("scene.usdc", &crate_bytes)
                .map_err(|e| format!("USDZ の作成に失敗しました: {e}"))?;
            writer
                .finish()
                .map_err(|e| format!("USDZ の作成に失敗しました: {e}"))?
                .into_inner()
        }
        _ => return Err(format!("USD の拡張子ではありません: .{ext}")),
    };
    std::fs::write(path, bytes).map_err(|e| format!("{} に書き込めません: {e}", path.display()))
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// USDA のテキストを Crate (バイナリ) 形式に変換する。
fn to_usdc(text: &str) -> Result<Vec<u8>, String> {
    let data = usda::parse(text).map_err(|e| format!("USDA の解析に失敗しました: {e}"))?;
    let mut out = Cursor::new(Vec::new());
    usdc::CrateWriter::write(&data, &mut out)
        .map_err(|e| format!("USDC の書き出しに失敗しました: {e}"))?;
    Ok(out.into_inner())
}

/// シーンを USDA のテキストにする。
pub fn to_usda(scene: &Scene) -> String {
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); scene.nodes.len()];
    let mut roots = Vec::new();
    for (i, node) in scene.nodes.iter().enumerate() {
        match node.parent {
            Some(p) if p < scene.nodes.len() && p != i => children[p].push(i),
            _ => roots.push(i),
        }
    }

    // 色ごとにマテリアルを1つ作る
    let mut materials: Vec<[f32; 3]> = Vec::new();
    for node in &scene.nodes {
        if let (Some(color), Some(_)) = (node.color, &node.mesh)
            && !materials.contains(&color)
        {
            materials.push(color);
        }
    }

    // USD のタイムラインは1本なので、ノードごとに最初に見つかったトラックを使う
    let mut samples: Vec<Vec<(f64, NodeTransform)>> = vec![Vec::new(); scene.nodes.len()];
    for track in scene.animations.iter().flat_map(|a| &a.tracks) {
        if let Some(slot) = samples.get_mut(track.node)
            && slot.is_empty()
        {
            *slot = track.keys.clone();
        }
    }
    let end_time = samples
        .iter()
        .flat_map(|keys| keys.iter().map(|(t, _)| *t))
        .fold(None, |acc: Option<f64>, t| {
            Some(acc.map_or(t, |a| a.max(t)))
        });

    let mut out = String::new();
    out.push_str("#usda 1.0\n(\n");
    out.push_str("    defaultPrim = \"World\"\n");
    out.push_str("    doc = \"cadsim から書き出し\"\n");
    out.push_str("    metersPerUnit = 1\n");
    out.push_str("    upAxis = \"Y\"\n");
    if let Some(end) = end_time {
        let _ = writeln!(out, "    startTimeCode = 0");
        let _ = writeln!(
            out,
            "    endTimeCode = {}",
            num(end * TIME_CODES_PER_SECOND)
        );
        let _ = writeln!(
            out,
            "    timeCodesPerSecond = {}",
            num(TIME_CODES_PER_SECOND)
        );
        let _ = writeln!(out, "    framesPerSecond = {}", num(TIME_CODES_PER_SECOND));
    }
    out.push_str(")\n\n");
    out.push_str("def Xform \"World\" (\n    kind = \"assembly\"\n)\n{\n");

    let mut writer = UsdaWriter {
        scene,
        samples: &samples,
        children: &children,
        materials: &materials,
        out,
    };
    let mut used = HashSet::from(["Looks".to_string()]);
    for &root in &roots {
        let name = unique_name(&scene.nodes[root].name, root, &mut used);
        writer.write_node(root, &name, "/World", 1);
    }
    let mut out = writer.out;

    if !materials.is_empty() {
        out.push_str("\n    def Scope \"Looks\"\n    {\n");
        for (i, color) in materials.iter().enumerate() {
            let path = format!("/World/Looks/Material_{i}");
            let c = vec3f(*color);
            let _ = write!(
                out,
                "        def Material \"Material_{i}\"\n        {{\n            \
                 token outputs:surface.connect = <{path}/Shader.outputs:surface>\n\n            \
                 def Shader \"Shader\"\n            {{\n                \
                 uniform token info:id = \"UsdPreviewSurface\"\n                \
                 color3f inputs:diffuseColor = {c}\n                \
                 float inputs:metallic = 0\n                \
                 float inputs:roughness = 0.6\n                \
                 token outputs:surface\n            }}\n        }}\n"
            );
        }
        out.push_str("    }\n");
    }
    out.push_str("}\n");
    out
}

struct UsdaWriter<'a> {
    scene: &'a Scene,
    /// ノードごとの時刻サンプル (最初に見つかったアニメーションのトラック)
    samples: &'a [Vec<(f64, NodeTransform)>],
    children: &'a [Vec<usize>],
    materials: &'a [[f32; 3]],
    out: String,
}

impl UsdaWriter<'_> {
    fn write_node(&mut self, index: usize, prim_name: &str, parent_path: &str, depth: usize) {
        let node = &self.scene.nodes[index];
        let path = format!("{parent_path}/{prim_name}");
        let pad = "    ".repeat(depth);
        let _ = writeln!(self.out, "\n{pad}def Xform \"{prim_name}\" (");
        let _ = writeln!(self.out, "{pad}    displayName = {}", quote(&node.name));
        if !self.children[index].is_empty() {
            let _ = writeln!(self.out, "{pad}    kind = \"group\"");
        }
        let _ = writeln!(self.out, "{pad})\n{pad}{{");
        self.write_xform_ops(node, &self.samples[index], &pad);

        if let Some(mesh) = &node.mesh {
            let material = node
                .color
                .and_then(|c| self.materials.iter().position(|m| *m == c));
            self.write_mesh(mesh, node.color, material, &format!("{pad}    "));
        }

        let mut used = HashSet::from(["Geom".to_string()]);
        for &child in &self.children[index] {
            let name = unique_name(&self.scene.nodes[child].name, child, &mut used);
            self.write_node(child, &name, &path, depth + 1);
        }
        let _ = writeln!(self.out, "{pad}}}");
    }

    fn write_xform_ops(&mut self, node: &SceneNode, keys: &[(f64, NodeTransform)], pad: &str) {
        let t = &node.transform;
        let animated = !keys.is_empty();
        let scaled = t.scale != DVec3::ONE || keys.iter().any(|(_, s)| s.scale != t.scale);

        let _ = writeln!(
            self.out,
            "{pad}    double3 xformOp:translate = {}",
            vec3d(t.translation)
        );
        if animated {
            self.write_samples(pad, "double3 xformOp:translate", keys, |s| {
                vec3d(s.translation)
            });
        }
        let _ = writeln!(
            self.out,
            "{pad}    quatd xformOp:orient = {}",
            quatd(t.rotation)
        );
        if animated {
            self.write_samples(pad, "quatd xformOp:orient", keys, |s| quatd(s.rotation));
        }
        let mut order = vec!["\"xformOp:translate\"", "\"xformOp:orient\""];
        if scaled {
            let _ = writeln!(
                self.out,
                "{pad}    double3 xformOp:scale = {}",
                vec3d(t.scale)
            );
            if animated {
                self.write_samples(pad, "double3 xformOp:scale", keys, |s| vec3d(s.scale));
            }
            order.push("\"xformOp:scale\"");
        }
        let _ = writeln!(
            self.out,
            "{pad}    uniform token[] xformOpOrder = [{}]",
            order.join(", ")
        );
    }

    fn write_samples(
        &mut self,
        pad: &str,
        decl: &str,
        keys: &[(f64, NodeTransform)],
        value: impl Fn(&NodeTransform) -> String,
    ) {
        let _ = writeln!(self.out, "{pad}    {decl}.timeSamples = {{");
        for (time, sample) in keys {
            let code = num(time * TIME_CODES_PER_SECOND);
            let _ = writeln!(self.out, "{pad}        {code}: {},", value(sample));
        }
        let _ = writeln!(self.out, "{pad}    }}");
    }

    fn write_mesh(
        &mut self,
        mesh: &TriMesh,
        color: Option<[f32; 3]>,
        material: Option<usize>,
        pad: &str,
    ) {
        let out = &mut self.out;
        let _ = write!(out, "\n{pad}def Mesh \"Geom\"");
        if material.is_some() {
            let _ = write!(
                out,
                " (\n{pad}    prepend apiSchemas = [\"MaterialBindingAPI\"]\n{pad})"
            );
        }
        let _ = writeln!(out, "\n{pad}{{");
        let inner = format!("{pad}    ");

        let (min, max) = bounds(&mesh.positions);
        let _ = writeln!(
            out,
            "{inner}float3[] extent = [{}, {}]",
            vec3f(min.as_vec3().to_array()),
            vec3f(max.as_vec3().to_array())
        );
        let _ = writeln!(
            out,
            "{inner}int[] faceVertexCounts = [{}]",
            join(mesh.triangles.iter().map(|_| "3".to_string()))
        );
        let _ = writeln!(
            out,
            "{inner}int[] faceVertexIndices = [{}]",
            join(mesh.triangles.iter().flatten().map(|i| i.to_string()))
        );
        let normals = smooth_normals(mesh, SMOOTH_ANGLE_DEG);
        let _ = writeln!(
            out,
            "{inner}normal3f[] normals = [{}] (\n{inner}    interpolation = \"faceVarying\"\n{inner})",
            join(normals.iter().map(|n| vec3f(n.as_vec3().to_array())))
        );
        let _ = writeln!(
            out,
            "{inner}point3f[] points = [{}]",
            join(mesh.positions.iter().map(|p| vec3f(p.as_vec3().to_array())))
        );
        if let Some(color) = color {
            let _ = writeln!(
                out,
                "{inner}color3f[] primvars:displayColor = [{}] (\n{inner}    interpolation = \"constant\"\n{inner})",
                vec3f(color)
            );
        }
        if let Some(m) = material {
            let _ = writeln!(
                out,
                "{inner}rel material:binding = </World/Looks/Material_{m}>"
            );
        }
        let _ = writeln!(out, "{inner}uniform bool doubleSided = 1");
        let _ = writeln!(out, "{inner}uniform token orientation = \"rightHanded\"");
        let _ = writeln!(out, "{inner}uniform token subdivisionScheme = \"none\"");
        let _ = writeln!(out, "{pad}}}");
    }
}

/// USD のプリム名 (ASCII の識別子) にする。日本語などを含む名前は `Node_<番号>` にし、
/// 元の名前は displayName に残す。
fn unique_name(name: &str, index: usize, used: &mut HashSet<String>) -> String {
    let ascii = name.is_ascii() && name.chars().any(|c| c.is_ascii_alphanumeric());
    let mut base: String = if ascii {
        name.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    } else {
        format!("Node_{index}")
    };
    if base.starts_with(|c: char| c.is_ascii_digit()) {
        base.insert(0, '_');
    }
    let mut candidate = base.clone();
    let mut n = 2;
    while !used.insert(candidate.clone()) {
        candidate = format!("{base}_{n}");
        n += 1;
    }
    candidate
}

/// 三角形の角ごとの法線。隣り合う三角形との角度が `angle_deg` 以下なら面積で重み付けして平均する。
fn smooth_normals(mesh: &TriMesh, angle_deg: f64) -> Vec<DVec3> {
    let p = &mesh.positions;
    let face_normals: Vec<DVec3> = mesh
        .triangles
        .iter()
        .map(|&[a, b, c]| {
            let get = |i: u32| p.get(i as usize).copied().unwrap_or_default();
            (get(b) - get(a)).cross(get(c) - get(a))
        })
        .collect();
    let mut around: HashMap<u32, Vec<usize>> = HashMap::new();
    for (f, tri) in mesh.triangles.iter().enumerate() {
        for &v in tri {
            around.entry(v).or_default().push(f);
        }
    }
    let cos_limit = angle_deg.to_radians().cos();
    let mut out = Vec::with_capacity(mesh.triangles.len() * 3);
    for (f, tri) in mesh.triangles.iter().enumerate() {
        let own = face_normals[f].normalize_or_zero();
        for v in tri {
            let sum: DVec3 = around[v]
                .iter()
                .map(|&g| face_normals[g])
                .filter(|n| n.normalize_or_zero().dot(own) >= cos_limit)
                .sum();
            let n = sum.normalize_or_zero();
            out.push(if n == DVec3::ZERO { own } else { n });
        }
    }
    out
}

fn bounds(points: &[DVec3]) -> (DVec3, DVec3) {
    if points.is_empty() {
        return (DVec3::ZERO, DVec3::ZERO);
    }
    points.iter().fold(
        (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
        |(lo, hi), p| (lo.min(*p), hi.max(*p)),
    )
}

/// 有限の数値をそのまま (指数表記なし) 書く。NaN・無限大は 0 にする。
fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "0".into()
    }
}

fn numf(v: f32) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "0".into()
    }
}

fn vec3d(v: DVec3) -> String {
    format!("({}, {}, {})", num(v.x), num(v.y), num(v.z))
}

fn vec3f(v: [f32; 3]) -> String {
    format!("({}, {}, {})", numf(v[0]), numf(v[1]), numf(v[2]))
}

/// USD の quat は (実部, 虚部 x, y, z) の順
fn quatd(q: DQuat) -> String {
    format!("({}, {}, {}, {})", num(q.w), num(q.x), num(q.y), num(q.z))
}

fn join(items: impl Iterator<Item = String>) -> String {
    items.collect::<Vec<_>>().join(", ")
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// 読み込み
// ---------------------------------------------------------------------------

/// .usda / .usdc / .usdz / .usd を読み込む。
pub fn import(path: &Path, _units: &UnitOptions) -> ExchangeResult<Scene> {
    let path_str = path
        .to_str()
        .ok_or_else(|| format!("パスを文字列にできません: {}", path.display()))?;
    let stage = usd::Stage::open(path_str).map_err(|e| format!("USD を開けません: {e}"))?;
    import_stage(&stage)
}

fn import_stage(stage: &usd::Stage) -> ExchangeResult<Scene> {
    let root_field = |name: &str| {
        stage
            .root_layer()
            .data()
            .get_field(&sdf::Path::abs_root(), name)
            .ok()
            .map(|v| v.into_owned())
    };
    let z_up = matches!(root_field("upAxis"), Some(sdf::Value::Token(t)) if t.as_str() == "Z");
    let meters_per_unit = match root_field("metersPerUnit") {
        Some(sdf::Value::Double(v)) if v > 0.0 => v,
        Some(sdf::Value::Float(v)) if v > 0.0 => v as f64,
        // USD の既定はセンチメートル
        _ => 0.01,
    };
    // USD の単位・上軸 → メートル・Y 上
    let axis = if z_up {
        DQuat::from_rotation_x(-std::f64::consts::FRAC_PI_2)
    } else {
        DQuat::IDENTITY
    };
    let root_fix =
        DMat4::from_scale_rotation_translation(DVec3::splat(meters_per_unit), axis, DVec3::ZERO);

    let mut importer = Importer {
        start: stage.start_time_code(),
        time_codes_per_second: match stage.time_codes_per_second() {
            t if t > 0.0 => t,
            _ => 24.0,
        },
        scene: Scene::default(),
        tracks: Vec::new(),
        root_fix,
    };
    let root = stage
        .prim(sdf::Path::abs_root())
        .map_err(|e| format!("USD のルートを開けません: {e}"))?;
    for child in root.children().map_err(err)? {
        importer.visit(&child, None, true)?;
    }
    let mut scene = prune_empty(importer.scene, &mut importer.tracks);
    if !importer.tracks.is_empty() {
        scene.animations.push(Animation {
            name: "USD".into(),
            tracks: importer.tracks,
        });
    }
    Ok(scene)
}

/// 形状を持つ子孫がないノード (マテリアルの Scope など) を除き、番号を詰め直す
fn prune_empty(scene: Scene, tracks: &mut Vec<Track>) -> Scene {
    let mut keep = vec![false; scene.nodes.len()];
    // 親は子より前に並ぶので、後ろから見れば子の結果が先に決まる
    for i in (0..scene.nodes.len()).rev() {
        if scene.nodes[i].mesh.is_some() {
            keep[i] = true;
        }
        if keep[i]
            && let Some(p) = scene.nodes[i].parent
        {
            keep[p] = true;
        }
    }
    let mut new_index = vec![None; scene.nodes.len()];
    let mut nodes = Vec::new();
    for (i, mut node) in scene.nodes.into_iter().enumerate() {
        if keep[i] {
            node.parent = node.parent.and_then(|p| new_index[p]);
            new_index[i] = Some(nodes.len());
            nodes.push(node);
        }
    }
    tracks.retain_mut(|t| match new_index[t.node] {
        Some(n) => {
            t.node = n;
            true
        }
        None => false,
    });
    Scene {
        nodes,
        animations: scene.animations,
    }
}

/// 行列を平行移動・回転・拡大縮小に分ける (シアーは失われる)
fn transform_of(m: DMat4) -> NodeTransform {
    NodeTransform::from_affine(DAffine3::from_mat4(m))
}

fn err(e: impl std::fmt::Display) -> String {
    format!("USD の読み込みに失敗しました: {e}")
}

struct Importer {
    start: f64,
    time_codes_per_second: f64,
    scene: Scene,
    tracks: Vec<Track>,
    root_fix: DMat4,
}

/// 形状を持つプリムの型
const GPRIMS: &[&str] = &["Mesh", "Cube", "Sphere", "Cylinder", "Cone", "Capsule"];
/// たどらない (形状として扱わない) プリムの型
const SKIP: &[&str] = &[
    "Material",
    "Shader",
    "NodeGraph",
    "Camera",
    "GeomSubset",
    "PointInstancer",
    "Points",
    "BasisCurves",
    "NurbsCurves",
    "NurbsPatch",
    "Skeleton",
    "SkelAnimation",
    "BlendShape",
    "DistantLight",
    "DomeLight",
    "SphereLight",
    "RectLight",
    "DiskLight",
    "CylinderLight",
    "PhysicsScene",
];

impl Importer {
    fn visit(&mut self, prim: &usd::Prim, parent: Option<usize>, top: bool) -> Result<(), String> {
        if !prim.is_active().map_err(err)? || prim.is_abstract().unwrap_or(false) {
            return Ok(());
        }
        let type_name = prim
            .type_name()
            .map_err(err)?
            .map(|t| t.as_str().to_string())
            .unwrap_or_default();
        if SKIP.contains(&type_name.as_str()) || type_name.ends_with("Light") {
            return Ok(());
        }
        if let Some(sdf::Value::Token(p)) = attr_value(prim, "purpose", None)
            && matches!(p.as_str(), "guide" | "proxy")
        {
            return Ok(());
        }

        let name = match prim.get_metadata::<String>("displayName") {
            Ok(Some(n)) if !n.is_empty() => n,
            _ => prim.path().name().unwrap_or_default().to_string(),
        };
        let mut transform = self.local_matrix(prim, None);
        let times = self.xform_times(prim);
        let mut samples: Vec<(f64, DMat4)> = times
            .iter()
            .map(|&t| (t, self.local_matrix(prim, Some(t))))
            .collect();
        if top {
            transform = self.root_fix * transform;
            for (_, m) in &mut samples {
                *m = self.root_fix * *m;
            }
        }
        let mesh = if GPRIMS.contains(&type_name.as_str()) {
            read_gprim(prim, &type_name)
        } else {
            None
        };
        let color = if mesh.is_some() {
            prim_color(prim)
        } else {
            None
        };
        let children = prim.children().map_err(err)?;

        // Xform の下に変換も子もない形状が1つだけなら、その Xform のノードに形状を持たせる
        // (cadsim が書き出す Xform + Mesh "Geom" の形もこれで1ノードに戻る)
        let index = self.scene.nodes.len();
        self.scene.nodes.push(SceneNode {
            name,
            parent,
            transform: transform_of(transform),
            mesh,
            color,
        });
        if !samples.is_empty() {
            self.tracks.push(Track {
                node: index,
                keys: samples
                    .into_iter()
                    .map(|(t, m)| {
                        (
                            (t - self.start) / self.time_codes_per_second,
                            transform_of(m),
                        )
                    })
                    .collect(),
            });
        }
        let mut rest = Vec::new();
        for child in &children {
            if self.scene.nodes[index].mesh.is_none() && self.is_plain_shape(child, &children) {
                let child_type = child
                    .type_name()
                    .ok()
                    .flatten()
                    .map(|t| t.as_str().to_string());
                if let Some(t) = child_type
                    && let Some(m) = read_gprim(child, &t)
                {
                    let node = &mut self.scene.nodes[index];
                    node.mesh = Some(m);
                    node.color = prim_color(child);
                    continue;
                }
            }
            rest.push(child);
        }
        for child in rest {
            self.visit(child, Some(index), false)?;
        }
        Ok(())
    }

    /// 親の Xform に形状として取り込むプリムか。変換も子も持たない形状で、
    /// cadsim が書き出した "Geom" か、兄弟 (マテリアルなどを除く) がいないもの
    fn is_plain_shape(&self, prim: &usd::Prim, siblings: &[usd::Prim]) -> bool {
        let type_of = |p: &usd::Prim| {
            p.type_name()
                .ok()
                .flatten()
                .map(|t| t.as_str().to_string())
                .unwrap_or_default()
        };
        let plain = GPRIMS.contains(&type_of(prim).as_str())
            && prim.children().map(|c| c.is_empty()).unwrap_or(false)
            && xform_op_order(prim).is_empty()
            && prim.is_active().unwrap_or(false);
        let alone = siblings
            .iter()
            .filter(|p| !SKIP.contains(&type_of(p).as_str()))
            .count()
            == 1;
        plain && (alone || prim.path().name() == Some("Geom"))
    }

    /// 変換の属性に付いた時刻サンプルのタイムコード (重複なし, 昇順)
    fn xform_times(&self, prim: &usd::Prim) -> Vec<f64> {
        let mut times: Vec<f64> = xform_op_order(prim)
            .iter()
            .filter(|op| op.as_str() != "!resetXformStack!")
            .flat_map(|op| {
                let name = op.strip_prefix("!invert!").unwrap_or(op);
                prim.attribute(name.to_string())
                    .time_sample_times()
                    .unwrap_or_default()
            })
            .collect();
        times.sort_by(f64::total_cmp);
        times.dedup();
        times
    }

    /// xformOpOrder を順に掛けたローカル変換 (列ベクトル: 最後の op が最初に効く)
    fn local_matrix(&self, prim: &usd::Prim, time: Option<f64>) -> DMat4 {
        let mut m = DMat4::IDENTITY;
        for op in xform_op_order(prim) {
            if op == "!resetXformStack!" {
                continue;
            }
            let (invert, name) = match op.strip_prefix("!invert!") {
                Some(n) => (true, n),
                None => (false, op.as_str()),
            };
            let Some(value) = attr_value(prim, name, time) else {
                continue;
            };
            let Some(op_matrix) = op_matrix(name, &value) else {
                continue;
            };
            m *= if invert {
                op_matrix.inverse()
            } else {
                op_matrix
            };
        }
        m
    }
}

fn xform_op_order(prim: &usd::Prim) -> Vec<String> {
    match attr_value(prim, "xformOpOrder", None) {
        Some(sdf::Value::TokenVec(v)) => v.iter().map(|t| t.as_str().to_string()).collect(),
        _ => Vec::new(),
    }
}

fn attr_value(prim: &usd::Prim, name: &str, time: Option<f64>) -> Option<sdf::Value> {
    let attr = prim.attribute(name.to_string());
    match time {
        Some(t) => attr
            .get_at::<sdf::Value>(usd::TimeCode::new(t))
            .ok()
            .flatten(),
        None => attr.get::<sdf::Value>().ok().flatten(),
    }
}

/// xformOp 1つ分の行列。`name` は "xformOp:rotateXYZ:pivot" のような属性名。
fn op_matrix(name: &str, value: &sdf::Value) -> Option<DMat4> {
    let kind = name.strip_prefix("xformOp:")?.split(':').next()?;
    let deg = |v: f64| v.to_radians();
    let m = match kind {
        "translate" => DMat4::from_translation(as_vec3(value)?),
        "scale" => DMat4::from_scale(as_vec3(value)?),
        "orient" => DMat4::from_quat(as_quat(value)?),
        "transform" => match value {
            // USD の行列は行ベクトル形式 (平行移動が4行目)。列優先で読むとそのまま列ベクトル形式になる
            sdf::Value::Matrix4d(gf::Matrix4d(a)) => DMat4::from_cols_array(a),
            _ => return None,
        },
        "rotateX" => DMat4::from_rotation_x(deg(as_scalar(value)?)),
        "rotateY" => DMat4::from_rotation_y(deg(as_scalar(value)?)),
        "rotateZ" => DMat4::from_rotation_z(deg(as_scalar(value)?)),
        k if k.starts_with("rotate") && k.len() == 9 => {
            let v = as_vec3(value)?;
            // rotateXYZ は X → Y → Z の順に回す: 行列は Rz * Ry * Rx
            let mut m = DMat4::IDENTITY;
            for axis in k[6..].chars() {
                let r = match axis {
                    'X' => DMat4::from_rotation_x(deg(v.x)),
                    'Y' => DMat4::from_rotation_y(deg(v.y)),
                    'Z' => DMat4::from_rotation_z(deg(v.z)),
                    _ => return None,
                };
                m = r * m;
            }
            m
        }
        _ => return None,
    };
    Some(m)
}

fn as_scalar(v: &sdf::Value) -> Option<f64> {
    match v {
        sdf::Value::Double(x) => Some(*x),
        sdf::Value::Float(x) => Some(*x as f64),
        sdf::Value::Half(x) => Some(x.to_f64()),
        sdf::Value::Int(x) => Some(*x as f64),
        _ => None,
    }
}

fn as_vec3(v: &sdf::Value) -> Option<DVec3> {
    match v {
        sdf::Value::Vec3d(p) => Some(DVec3::new(p.x, p.y, p.z)),
        sdf::Value::Vec3f(p) => Some(DVec3::new(p.x as f64, p.y as f64, p.z as f64)),
        sdf::Value::Vec3h(p) => Some(DVec3::new(p.x.to_f64(), p.y.to_f64(), p.z.to_f64())),
        _ => None,
    }
}

fn as_quat(v: &sdf::Value) -> Option<DQuat> {
    let q = match v {
        sdf::Value::Quatd(q) => DQuat::from_xyzw(q.x, q.y, q.z, q.w),
        sdf::Value::Quatf(q) => DQuat::from_xyzw(q.x as f64, q.y as f64, q.z as f64, q.w as f64),
        sdf::Value::Quath(q) => {
            DQuat::from_xyzw(q.x.to_f64(), q.y.to_f64(), q.z.to_f64(), q.w.to_f64())
        }
        _ => return None,
    };
    Some(q.normalize())
}

fn as_points(v: &sdf::Value) -> Option<Vec<DVec3>> {
    match v {
        sdf::Value::Vec3fVec(p) => Some(
            p.iter()
                .map(|p| DVec3::new(p.x as f64, p.y as f64, p.z as f64))
                .collect(),
        ),
        sdf::Value::Vec3dVec(p) => Some(p.iter().map(|p| DVec3::new(p.x, p.y, p.z)).collect()),
        sdf::Value::Vec3hVec(p) => Some(
            p.iter()
                .map(|p| DVec3::new(p.x.to_f64(), p.y.to_f64(), p.z.to_f64()))
                .collect(),
        ),
        _ => None,
    }
}

fn as_ints(v: &sdf::Value) -> Option<Vec<i64>> {
    match v {
        sdf::Value::IntVec(v) => Some(v.iter().map(|&i| i as i64).collect()),
        sdf::Value::Int64Vec(v) => Some(v.clone()),
        sdf::Value::UintVec(v) => Some(v.iter().map(|&i| i as i64).collect()),
        _ => None,
    }
}

/// 形状プリムを三角形メッシュにする (プリム自身の座標系)。
fn read_gprim(prim: &usd::Prim, type_name: &str) -> Option<TriMesh> {
    let scalar = |name: &str, default: f64| {
        attr_value(prim, name, None)
            .and_then(|v| as_scalar(&v))
            .unwrap_or(default)
    };
    let axis = || match attr_value(prim, "axis", None) {
        Some(sdf::Value::Token(t)) => t.as_str().chars().next().unwrap_or('Z'),
        _ => 'Z',
    };
    let mesh = match type_name {
        "Mesh" => read_mesh(prim)?,
        "Cube" => cube(scalar("size", 2.0)),
        "Sphere" => revolve(&sphere_profile(scalar("radius", 1.0)), 'Y'),
        "Cylinder" => {
            let (r, h) = (scalar("radius", 1.0), scalar("height", 2.0));
            revolve(
                &[(0.0, -h / 2.0), (r, -h / 2.0), (r, h / 2.0), (0.0, h / 2.0)],
                axis(),
            )
        }
        "Cone" => {
            let (r, h) = (scalar("radius", 1.0), scalar("height", 2.0));
            revolve(&[(0.0, -h / 2.0), (r, -h / 2.0), (0.0, h / 2.0)], axis())
        }
        "Capsule" => {
            let (r, h) = (scalar("radius", 0.5), scalar("height", 1.0));
            let mut profile = Vec::new();
            for i in 0..=8 {
                let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::FRAC_PI_2 * i as f64 / 8.0;
                profile.push((r * a.cos(), -h / 2.0 + r * a.sin()));
            }
            for i in 0..=8 {
                let a = std::f64::consts::FRAC_PI_2 * i as f64 / 8.0;
                profile.push((r * a.cos(), h / 2.0 + r * a.sin()));
            }
            revolve(&profile, axis())
        }
        _ => return None,
    };
    (!mesh.triangles.is_empty()).then_some(mesh)
}

fn read_mesh(prim: &usd::Prim) -> Option<TriMesh> {
    let positions = as_points(&attr_value(prim, "points", None)?)?;
    let counts = as_ints(&attr_value(prim, "faceVertexCounts", None)?)?;
    let indices = as_ints(&attr_value(prim, "faceVertexIndices", None)?)?;
    let left_handed = matches!(
        attr_value(prim, "orientation", None),
        Some(sdf::Value::Token(t)) if t.as_str() == "leftHanded"
    );
    let n = positions.len() as i64;
    let mut triangles = Vec::new();
    let mut cursor = 0usize;
    for count in counts {
        let count = count.max(0) as usize;
        let Some(face) = indices.get(cursor..cursor + count) else {
            break;
        };
        cursor += count;
        if face.iter().any(|&i| i < 0 || i >= n) {
            continue;
        }
        // 多角形は扇形に分割する
        for k in 1..count.saturating_sub(1) {
            let mut tri = [face[0] as u32, face[k] as u32, face[k + 1] as u32];
            if left_handed {
                tri.swap(1, 2);
            }
            triangles.push(tri);
        }
    }
    Some(TriMesh {
        positions,
        triangles,
    })
}

fn cube(size: f64) -> TriMesh {
    let h = size / 2.0;
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    // 面ごとに頂点を分けて、角の法線を混ぜない
    for (normal, u, v) in [
        (DVec3::X, DVec3::Y, DVec3::Z),
        (DVec3::NEG_X, DVec3::Z, DVec3::Y),
        (DVec3::Y, DVec3::Z, DVec3::X),
        (DVec3::NEG_Y, DVec3::X, DVec3::Z),
        (DVec3::Z, DVec3::X, DVec3::Y),
        (DVec3::NEG_Z, DVec3::Y, DVec3::X),
    ] {
        let base = positions.len() as u32;
        for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            positions.push((normal + u * a + v * b) * h);
        }
        triangles.push([base, base + 1, base + 2]);
        triangles.push([base, base + 2, base + 3]);
    }
    TriMesh {
        positions,
        triangles,
    }
}

fn sphere_profile(r: f64) -> Vec<(f64, f64)> {
    (0..=16)
        .map(|i| {
            let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * i as f64 / 16.0;
            (r * a.cos(), r * a.sin())
        })
        .collect()
}

/// (半径, 高さ) の折れ線を軸まわりに回して回転体のメッシュを作る。
fn revolve(profile: &[(f64, f64)], axis: char) -> TriMesh {
    const SEGMENTS: usize = 32;
    let place = |radius: f64, height: f64, angle: f64| {
        let (s, c) = angle.sin_cos();
        let (a, b) = (radius * c, radius * s);
        match axis {
            'X' => DVec3::new(height, a, b),
            'Y' => DVec3::new(b, height, a),
            _ => DVec3::new(a, b, height),
        }
    };
    let mut positions = Vec::new();
    for &(r, h) in profile {
        for s in 0..SEGMENTS {
            let angle = std::f64::consts::TAU * s as f64 / SEGMENTS as f64;
            positions.push(place(r, h, angle));
        }
    }
    let mut triangles = Vec::new();
    for ring in 0..profile.len().saturating_sub(1) {
        for s in 0..SEGMENTS {
            let a = (ring * SEGMENTS + s) as u32;
            let b = (ring * SEGMENTS + (s + 1) % SEGMENTS) as u32;
            let c = a + SEGMENTS as u32;
            let d = b + SEGMENTS as u32;
            triangles.push([a, b, d]);
            triangles.push([a, d, c]);
        }
    }
    TriMesh {
        positions,
        triangles,
    }
}

/// マテリアルの diffuseColor、なければ displayColor。祖先に付いたバインドもたどる。
fn prim_color(prim: &usd::Prim) -> Option<[f32; 3]> {
    let mut current = Some(prim.clone());
    while let Some(p) = current {
        if let Ok(targets) = p.relationship("material:binding").targets()
            && let Some(material) = targets.first()
            && let Some(c) = material_color(prim.stage(), material)
        {
            return Some(c);
        }
        current = p.parent();
    }
    match attr_value(prim, "primvars:displayColor", None)? {
        sdf::Value::Vec3fVec(v) => v.first().map(|c| [c.x, c.y, c.z]),
        sdf::Value::Vec3f(c) => Some([c.x, c.y, c.z]),
        _ => None,
    }
}

fn material_color(stage: &usd::Stage, material: &sdf::Path) -> Option<[f32; 3]> {
    let material = stage.prim(material.clone()).ok()?;
    let mut shaders: Vec<sdf::Path> = material
        .attribute("outputs:surface")
        .connections()
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.prim_path())
        .collect();
    shaders.extend(
        material
            .children()
            .unwrap_or_default()
            .iter()
            .map(|c| c.path().clone()),
    );
    for shader in shaders {
        let shader = stage.prim(shader).ok()?;
        match attr_value(&shader, "inputs:diffuseColor", None) {
            Some(sdf::Value::Vec3f(c)) => return Some([c.x, c.y, c.z]),
            Some(sdf::Value::Vec3d(c)) => return Some([c.x as f32, c.y as f32, c.z as f32]),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_scene() -> Scene {
        let mut scene = Scene::default();
        scene.nodes.push(SceneNode {
            name: "ライン 1".into(),
            transform: NodeTransform {
                translation: DVec3::new(1.0, 0.0, 0.0),
                ..Default::default()
            },
            ..Default::default()
        });
        let start = NodeTransform {
            translation: DVec3::new(0.0, 0.5, 0.0),
            rotation: DQuat::from_rotation_y(0.3),
            scale: DVec3::ONE,
        };
        let mut moved = start;
        moved.translation.y += 0.2;
        moved.rotation = DQuat::from_rotation_y(1.2);
        scene.nodes.push(SceneNode {
            name: "箱 \"A\"".into(),
            parent: Some(0),
            transform: start,
            mesh: Some(cube(1.0)),
            color: Some([0.7, 0.74, 0.8]),
        });
        scene.nodes.push(SceneNode {
            name: "Base plate".into(),
            parent: Some(0),
            transform: NodeTransform {
                scale: DVec3::new(2.0, 1.0, 1.0),
                ..Default::default()
            },
            mesh: Some(cube(1.0)),
            color: Some([1.0, 0.0, 0.0]),
        });
        scene.animations.push(Animation {
            name: "cycle".into(),
            tracks: vec![Track {
                node: 1,
                keys: vec![(0.0, start), (1.5, moved)],
            }],
        });
        scene
    }

    fn write_and_read(scene: &Scene, ext: &str) -> Scene {
        let dir = std::env::temp_dir().join(format!("cadsim_usd_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // テストは並列に走るので、同じ拡張子どうしでもファイル名が重ならないようにする
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = dir.join(format!("scene_{n}.{ext}"));
        let units = UnitOptions::default();
        export(scene, &path, &units).unwrap();
        let read = import(&path, &units).unwrap();
        let _ = std::fs::remove_file(&path);
        read
    }

    fn assert_close(a: DVec3, b: DVec3) {
        assert!((a - b).length() < 1e-5, "{a} != {b}");
    }

    fn check_round_trip(read: &Scene) {
        let original = sample_scene();
        assert!(read.is_well_ordered());
        // ルートの World が先頭に入る
        assert_eq!(read.nodes[0].name, "World");
        assert_eq!(read.nodes.len(), original.nodes.len() + 1);
        for (got, want) in read.nodes[1..].iter().zip(&original.nodes) {
            assert_eq!(got.name, want.name);
            assert_eq!(got.parent, Some(want.parent.map_or(0, |p| p + 1)));
            assert_close(got.transform.translation, want.transform.translation);
            assert_close(got.transform.scale, want.transform.scale);
            assert!(
                got.transform
                    .rotation
                    .angle_between(want.transform.rotation)
                    < 1e-6
            );
            assert_eq!(got.mesh.is_some(), want.mesh.is_some());
            if let (Some(g), Some(w)) = (&got.mesh, &want.mesh) {
                assert_eq!(g.triangles, w.triangles);
                assert_eq!(g.positions.len(), w.positions.len());
            }
            assert_eq!(got.color, want.color);
        }
        let [animation] = read.animations.as_slice() else {
            panic!("アニメーションが1つ読めるはず");
        };
        let [track] = animation.tracks.as_slice() else {
            panic!("動くノードは1つ");
        };
        let want = &original.animations[0].tracks[0];
        assert_eq!(track.node, want.node + 1);
        assert_eq!(track.keys.len(), want.keys.len());
        for ((gt, gs), (wt, ws)) in track.keys.iter().zip(&want.keys) {
            assert!((gt - wt).abs() < 1e-9);
            assert_close(gs.translation, ws.translation);
            assert!(gs.rotation.angle_between(ws.rotation) < 1e-6);
        }
    }

    #[test]
    fn usda_round_trip() {
        check_round_trip(&write_and_read(&sample_scene(), "usda"));
    }

    #[test]
    fn usdc_round_trip() {
        check_round_trip(&write_and_read(&sample_scene(), "usdc"));
    }

    #[test]
    fn usdz_round_trip() {
        check_round_trip(&write_and_read(&sample_scene(), "usdz"));
    }

    #[test]
    fn usda_text_has_omniverse_friendly_layout() {
        let text = to_usda(&sample_scene());
        assert!(text.contains("defaultPrim = \"World\""));
        assert!(text.contains("upAxis = \"Y\""));
        assert!(text.contains("displayName = \"箱 \\\"A\\\"\""));
        assert!(text.contains("xformOp:translate.timeSamples"));
        assert!(text.contains("info:id = \"UsdPreviewSurface\""));
        assert!(text.contains("def Xform \"Base_plate\""));
        // 自前の USDA が openusd の解析を通る
        usda::parse(&text).unwrap();
    }

    #[test]
    fn reads_z_up_centimeter_files_with_rotate_ops() {
        let text = r#"#usda 1.0
(
    metersPerUnit = 0.01
    upAxis = "Z"
    timeCodesPerSecond = 24
)
def Xform "Root"
{
    double3 xformOp:translate = (100, 0, 50)
    float3 xformOp:rotateXYZ = (0, 0, 90)
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ"]

    def Cube "Box"
    {
        double size = 10
        color3f[] primvars:displayColor = [(0, 1, 0)]
    }
    def Sphere "Ball"
    {
        double3 xformOp:translate.timeSamples = {
            0: (0, 0, 0),
            48: (0, 0, 24),
        }
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }
}
"#;
        let dir = std::env::temp_dir().join(format!("cadsim_usd_zup_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("zup.usda");
        std::fs::write(&path, text).unwrap();
        let scene = import(&path, &UnitOptions::default()).unwrap();
        let _ = std::fs::remove_file(&path);

        let root = &scene.nodes[0];
        assert_eq!(root.name, "Root");
        // Z 上 (100, 0, 50) cm → Y 上 (1, 0.5, 0) m
        assert_close(root.transform.translation, DVec3::new(1.0, 0.5, 0.0));
        assert_close(root.transform.scale, DVec3::splat(0.01));
        assert!(root.mesh.is_none());
        // 兄弟がいる Cube は Root に取り込まず、名前の付いた部品のまま
        let cube = &scene.nodes[1];
        assert_eq!(cube.name, "Box");
        assert_eq!(cube.parent, Some(0));
        assert_eq!(cube.mesh.as_ref().unwrap().triangles.len(), 12);
        assert_eq!(cube.color, Some([0.0, 1.0, 0.0]));
        // Sphere は形状を持つ子ノードで、2秒で 24cm 上がる
        let ball = &scene.nodes[2];
        assert_eq!(ball.parent, Some(0));
        assert!(ball.mesh.is_some());
        let keys = &scene.animations[0].tracks[0].keys;
        assert_eq!(scene.animations[0].tracks[0].node, 2);
        assert_eq!(keys.len(), 2);
        assert!((keys[1].0 - 2.0).abs() < 1e-9);
        assert_close(keys[1].1.translation, DVec3::new(0.0, 0.0, 24.0));
    }

    #[test]
    fn reads_references_instances_and_matrix_ops() {
        let dir = std::env::temp_dir().join(format!("cadsim_usd_ref_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("bolt.usda"),
            r#"#usda 1.0
(
    defaultPrim = "Bolt"
    metersPerUnit = 1
)
def Xform "Bolt"
{
    def Mesh "Body"
    {
        point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)]
        int[] faceVertexCounts = [4]
        int[] faceVertexIndices = [0, 1, 2, 3]
    }
}
"#,
        )
        .unwrap();
        let path = dir.join("line.usda");
        std::fs::write(
            &path,
            r#"#usda 1.0
(
    metersPerUnit = 1
    upAxis = "Y"
)
def Xform "Line"
{
    def "BoltA" (
        instanceable = true
        prepend references = @./bolt.usda@
    )
    {
        matrix4d xformOp:transform = ((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (5, 0, 0, 1))
        uniform token[] xformOpOrder = ["xformOp:transform"]
    }
    def "BoltB" (
        instanceable = true
        prepend references = @./bolt.usda@
    )
    {
    }
    def Scope "Hidden" (
        active = false
    )
    {
        def Cube "Gone"
        {
        }
    }
}
"#,
        )
        .unwrap();
        let scene = import(&path, &UnitOptions::default()).unwrap();
        let names: Vec<&str> = scene.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["Line", "BoltA", "BoltB"]);
        // 四角形は2つの三角形に分ける
        for bolt in &scene.nodes[1..] {
            assert_eq!(bolt.mesh.as_ref().unwrap().triangles.len(), 2);
        }
        assert_close(
            scene.nodes[1].transform.translation,
            DVec3::new(5.0, 0.0, 0.0),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sample_robot_keeps_hierarchy_and_animation() {
        let robot = crate::interchange::samples::robot();
        let read = write_and_read(&robot, "usdz");
        // 先頭に World が入るほかは同じ並び
        assert_eq!(read.nodes.len(), robot.nodes.len() + 1);
        let names: Vec<&str> = read.nodes[1..].iter().map(|n| n.name.as_str()).collect();
        let want: Vec<&str> = robot.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, want);
        let tracks: usize = robot.animations.iter().map(|a| a.tracks.len()).sum();
        assert_eq!(read.animations[0].tracks.len(), tracks);
        // 途中の時刻の姿勢 (ワールド座標) が元と一致する
        let original = robot.animations[0].clone();
        let time = original.duration() * 0.37;
        let mut a = robot.clone();
        let mut b = read.clone();
        for track in &original.tracks {
            a.nodes[track.node].transform = track.sample(time).unwrap();
        }
        for track in &read.animations[0].tracks {
            b.nodes[track.node].transform = track.sample(time).unwrap();
        }
        let (wa, wb) = (a.world_transforms(), b.world_transforms());
        for (i, w) in wa.iter().enumerate() {
            let d = (w.translation - wb[i + 1].translation).length();
            assert!(d < 1e-4, "{} がずれている: {d}", a.nodes[i].name);
        }
    }
}
