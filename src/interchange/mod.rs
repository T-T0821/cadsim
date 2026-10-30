//! 3D ファイルの読み書き (glTF / GLB, OBJ, STL など)
//!
//! 各形式はアプリのモデルを直接触らず、形式に依存しない中間表現 [`Scene`] とだけやり取りする。
//! - 読み込み: ファイル → `Scene` → モデル (変換は `model` モジュール)
//! - 書き出し: モデル → `Scene` → ファイル
//!
//! `Scene` は常にアプリの座標系 (メートル、Y 軸が上) で持つ。単位や上方向の違いは各形式の側で変換する。
//! 新しい形式は `xxx.rs` に `import` / `export` を書き、[`FORMATS`] に1行足せば UI に出る。

use std::path::Path;

use bevy::math::{DAffine3, DQuat, DVec3};

pub mod gltf;
pub mod model_io;
pub mod obj;
#[cfg(test)]
mod samples;
pub mod stl;
pub mod usd;

/// 読み書きの結果。エラーは利用者に見せる日本語の文
pub type ExchangeResult<T> = Result<T, String>;

/// 形式に依存しないシーン
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    /// 親は必ず子より前に並ぶ (`parent` の番号 < 自分の番号)
    pub nodes: Vec<SceneNode>,
    pub animations: Vec<Animation>,
}

/// シーンの1要素。アセンブリ (子を持つ) にも部品 (メッシュを持つ) にもなる
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneNode {
    pub name: String,
    pub parent: Option<usize>,
    /// 親から見た配置
    pub transform: NodeTransform,
    pub mesh: Option<TriMesh>,
    /// 表示色 (sRGB, 0〜1)。None ならアプリ既定の色
    pub color: Option<[f32; 3]>,
}

/// 平行移動・回転・拡大縮小 (この順で親に対して適用される)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeTransform {
    pub translation: DVec3,
    pub rotation: DQuat,
    pub scale: DVec3,
}

impl Default for NodeTransform {
    fn default() -> Self {
        Self {
            translation: DVec3::ZERO,
            rotation: DQuat::IDENTITY,
            scale: DVec3::ONE,
        }
    }
}

impl NodeTransform {
    pub fn to_affine(self) -> DAffine3 {
        DAffine3::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    pub fn from_affine(affine: DAffine3) -> Self {
        let (scale, rotation, translation) = affine.to_scale_rotation_translation();
        Self {
            translation,
            rotation,
            scale,
        }
    }

    /// 2つの姿勢の間を補間する (位置と拡大は線形、回転は球面線形)
    pub fn lerp(self, other: Self, t: f64) -> Self {
        Self {
            translation: self.translation.lerp(other.translation, t),
            rotation: self.rotation.slerp(other.rotation, t),
            scale: self.scale.lerp(other.scale, t),
        }
    }
}

/// 三角形メッシュ (ノードのローカル座標)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriMesh {
    pub positions: Vec<DVec3>,
    /// 反時計回りが表
    pub triangles: Vec<[u32; 3]>,
}

impl TriMesh {
    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }
}

/// ノードの姿勢の時間変化
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Animation {
    pub name: String,
    pub tracks: Vec<Track>,
}

/// 1つのノードのキーフレーム。キーの間は線形補間する
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub node: usize,
    /// (時刻 [秒], 親から見た配置)。時刻の昇順
    pub keys: Vec<(f64, NodeTransform)>,
}

impl Track {
    /// 時刻 `time` の姿勢。範囲外は端のキーで止める
    pub fn sample(&self, time: f64) -> Option<NodeTransform> {
        let first = self.keys.first()?;
        if time <= first.0 {
            return Some(first.1);
        }
        let index = self.keys.partition_point(|(t, _)| *t <= time);
        let Some(next) = self.keys.get(index) else {
            return self.keys.last().map(|k| k.1);
        };
        let prev = self.keys[index - 1];
        let span = next.0 - prev.0;
        let t = if span > 0.0 {
            (time - prev.0) / span
        } else {
            1.0
        };
        Some(prev.1.lerp(next.1, t))
    }
}

impl Animation {
    /// 最後のキーの時刻 [秒]
    pub fn duration(&self) -> f64 {
        self.tracks
            .iter()
            .filter_map(|t| t.keys.last().map(|k| k.0))
            .fold(0.0, f64::max)
    }
}

impl Scene {
    /// 各ノードのワールド座標での配置 (親の配置を掛けたもの)
    pub fn world_transforms(&self) -> Vec<DAffine3> {
        let mut world: Vec<DAffine3> = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let local = node.transform.to_affine();
            let parent = node.parent.map_or(DAffine3::IDENTITY, |p| world[p]);
            world.push(parent * local);
        }
        world
    }

    /// 子の番号の一覧 (ノードごと)
    pub fn children(&self) -> Vec<Vec<usize>> {
        let mut children = vec![Vec::new(); self.nodes.len()];
        for (index, node) in self.nodes.iter().enumerate() {
            if let Some(parent) = node.parent {
                children[parent].push(index);
            }
        }
        children
    }

    /// 全体を1つのメッシュにまとめる (ワールド座標)。階層を持たない形式の書き出し用
    pub fn merged_mesh(&self) -> TriMesh {
        let mut merged = TriMesh::default();
        for (node, world) in self.nodes.iter().zip(self.world_transforms()) {
            let Some(mesh) = &node.mesh else { continue };
            let offset = merged.positions.len() as u32;
            merged
                .positions
                .extend(mesh.positions.iter().map(|p| world.transform_point3(*p)));
            // 鏡映 (行列式が負) なら表裏が反転するので頂点順を入れ替える
            let flip = world.matrix3.determinant() < 0.0;
            merged
                .triangles
                .extend(mesh.triangles.iter().map(|&[a, b, c]| {
                    if flip {
                        [a + offset, c + offset, b + offset]
                    } else {
                        [a + offset, b + offset, c + offset]
                    }
                }));
        }
        merged
    }

    /// 親が子より前という並びの約束を満たしているか
    pub fn is_well_ordered(&self) -> bool {
        self.nodes
            .iter()
            .enumerate()
            .all(|(i, n)| n.parent.is_none_or(|p| p < i))
    }
}

/// 単位と上方向の指定。ファイル自体に単位の情報がない形式 (OBJ / STL) で使う
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitOptions {
    /// ファイルの1単位が何メートルか (mm なら 0.001)
    pub meters_per_unit: f64,
    /// ファイルが Z 軸を上としているか (アプリは Y 軸が上)
    pub z_up: bool,
}

impl Default for UnitOptions {
    /// 設備 CAD の書き出しで多い「mm・Y 軸が上」
    fn default() -> Self {
        Self {
            meters_per_unit: 0.001,
            z_up: false,
        }
    }
}

impl UnitOptions {
    /// ファイルの座標 → アプリの座標 (メートル、Y 軸が上)
    pub fn app_from_file(self, p: DVec3) -> DVec3 {
        let p = p * self.meters_per_unit;
        if self.z_up {
            DVec3::new(p.x, p.z, -p.y)
        } else {
            p
        }
    }

    /// アプリの座標 → ファイルの座標
    pub fn file_from_app(self, p: DVec3) -> DVec3 {
        let p = if self.z_up {
            DVec3::new(p.x, -p.z, p.y)
        } else {
            p
        };
        p / self.meters_per_unit
    }
}

/// UI で選べる長さの単位 (名前, 1単位のメートル)
pub const LENGTH_UNITS: &[(&str, f64)] =
    &[("mm", 0.001), ("cm", 0.01), ("m", 1.0), ("inch", 0.0254)];

/// 読み込み関数。`UnitOptions` は単位の情報を持たない形式だけが使う
pub type ImportFn = fn(&Path, &UnitOptions) -> ExchangeResult<Scene>;
/// 書き出し関数
pub type ExportFn = fn(&Scene, &Path, &UnitOptions) -> ExchangeResult<()>;

/// 読み書きできるファイル形式
pub struct Format {
    /// メニューに出す名前
    pub name: &'static str,
    /// 小文字・ドットなし
    pub extensions: &'static [&'static str],
    pub import: Option<ImportFn>,
    pub export: Option<ExportFn>,
    /// ファイルに単位の情報がなく、UI で単位と上方向を選ばせる形式か
    pub needs_units: bool,
    /// 書き出しで失われる情報 (UI に注記として出す)。なければ空
    pub export_note: &'static str,
}

/// 対応している形式。上から順にメニューに並ぶ
pub const FORMATS: &[Format] = &[
    Format {
        name: "glTF バイナリ (.glb)",
        extensions: &["glb"],
        import: Some(gltf::import),
        export: Some(gltf::export),
        needs_units: false,
        export_note: "",
    },
    Format {
        name: "glTF (.gltf)",
        extensions: &["gltf"],
        import: Some(gltf::import),
        export: Some(gltf::export),
        needs_units: false,
        export_note: "",
    },
    Format {
        name: "Wavefront OBJ (.obj)",
        extensions: &["obj"],
        import: Some(obj::import),
        export: Some(obj::export),
        needs_units: true,
        export_note: "色と階層 (アニメーションなし)",
    },
    Format {
        name: "STL (.stl)",
        extensions: &["stl"],
        import: Some(stl::import),
        export: Some(stl::export),
        needs_units: true,
        export_note: "形状を1つにまとめる (色・階層なし)",
    },
    Format {
        name: "OpenUSD (.usda / .usdc / .usdz / .usd)",
        extensions: &["usda", "usdc", "usdz", "usd"],
        import: Some(usd::import),
        export: Some(usd::export),
        needs_units: false,
        export_note: "",
    },
];

/// 拡張子から形式を探す
pub fn format_for_path(path: &Path) -> Option<&'static Format> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    FORMATS
        .iter()
        .find(|f| f.extensions.contains(&ext.as_str()))
}

/// 拡張子に応じた形式で読み込む
pub fn import_file(path: &Path, units: &UnitOptions) -> ExchangeResult<Scene> {
    let format = format_for_path(path).ok_or_else(|| unsupported(path))?;
    let import = format
        .import
        .ok_or_else(|| format!("{} は読み込みに対応していません", format.name))?;
    let scene = import(path, units)?;
    debug_assert!(scene.is_well_ordered());
    Ok(scene)
}

/// 拡張子に応じた形式で書き出す
pub fn export_file(scene: &Scene, path: &Path, units: &UnitOptions) -> ExchangeResult<()> {
    let format = format_for_path(path).ok_or_else(|| unsupported(path))?;
    let export = format
        .export
        .ok_or_else(|| format!("{} は書き出しに対応していません", format.name))?;
    export(scene, path, units)
}

fn unsupported(path: &Path) -> String {
    let list: Vec<&str> = FORMATS
        .iter()
        .flat_map(|f| f.extensions.iter().copied())
        .collect();
    format!(
        "対応していない拡張子です: {} (対応: {})",
        path.display(),
        list.join(", ")
    )
}

/// 読み書きのエラーを日本語の文にする
pub(crate) fn io_error(path: &Path, err: impl std::fmt::Display) -> String {
    format!("{}: {err}", path.display())
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;

    /// 単位立方体 (中心が原点、辺 1)
    pub fn cube() -> TriMesh {
        let positions = (0..8)
            .map(|i| {
                DVec3::new(
                    if i & 1 == 0 { -0.5 } else { 0.5 },
                    if i & 2 == 0 { -0.5 } else { 0.5 },
                    if i & 4 == 0 { -0.5 } else { 0.5 },
                )
            })
            .collect();
        let triangles = vec![
            [0, 2, 1],
            [1, 2, 3],
            [4, 5, 6],
            [5, 7, 6],
            [0, 1, 4],
            [1, 5, 4],
            [2, 6, 3],
            [3, 6, 7],
            [0, 4, 2],
            [2, 4, 6],
            [1, 3, 5],
            [3, 7, 5],
        ];
        TriMesh {
            positions,
            triangles,
        }
    }

    /// 親アセンブリ + 色付きの子2つ + 子1つのアニメーション
    pub fn sample_scene() -> Scene {
        let nodes = vec![
            SceneNode {
                name: "ライン".into(),
                transform: NodeTransform {
                    translation: DVec3::new(1.0, 0.0, 0.0),
                    ..default_transform()
                },
                ..Default::default()
            },
            SceneNode {
                name: "台".into(),
                parent: Some(0),
                mesh: Some(cube()),
                color: Some([0.2, 0.4, 0.8]),
                ..Default::default()
            },
            SceneNode {
                name: "シリンダ".into(),
                parent: Some(0),
                transform: NodeTransform {
                    translation: DVec3::new(0.0, 1.0, 0.0),
                    rotation: DQuat::from_rotation_y(0.5),
                    scale: DVec3::new(0.5, 2.0, 0.5),
                },
                mesh: Some(cube()),
                color: Some([0.9, 0.5, 0.1]),
            },
        ];
        let rest = nodes[2].transform;
        let moved = NodeTransform {
            translation: DVec3::new(0.0, 2.0, 0.0),
            ..rest
        };
        Scene {
            nodes,
            animations: vec![Animation {
                name: "上下動".into(),
                tracks: vec![Track {
                    node: 2,
                    keys: vec![(0.0, rest), (1.0, moved), (2.0, rest)],
                }],
            }],
        }
    }

    fn default_transform() -> NodeTransform {
        NodeTransform::default()
    }

    /// 2つのメッシュがワールド座標で同じ形か (頂点の並びは問わず、面積と範囲で比べる)
    pub fn assert_same_shape(a: &TriMesh, b: &TriMesh, tol: f64) {
        assert_eq!(a.triangles.len(), b.triangles.len(), "三角形の数");
        let area = |m: &TriMesh| -> f64 {
            m.triangles
                .iter()
                .map(|t| {
                    let [p, q, r] = t.map(|i| m.positions[i as usize]);
                    (q - p).cross(r - p).length() / 2.0
                })
                .sum()
        };
        let bounds = |m: &TriMesh| {
            m.positions.iter().fold(
                (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)),
                |(lo, hi), p| (lo.min(*p), hi.max(*p)),
            )
        };
        assert!((area(a) - area(b)).abs() < tol, "面積");
        let (alo, ahi) = bounds(a);
        let (blo, bhi) = bounds(b);
        assert!(
            alo.abs_diff_eq(blo, tol) && ahi.abs_diff_eq(bhi, tol),
            "範囲"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_interpolates_and_clamps() {
        let scene = test_util::sample_scene();
        let track = &scene.animations[0].tracks[0];
        assert_eq!(track.sample(-1.0).unwrap().translation.y, 1.0);
        assert!((track.sample(0.5).unwrap().translation.y - 1.5).abs() < 1e-9);
        assert_eq!(track.sample(5.0).unwrap().translation.y, 1.0);
        assert_eq!(scene.animations[0].duration(), 2.0);
    }

    #[test]
    fn world_transform_includes_parent() {
        let scene = test_util::sample_scene();
        let world = scene.world_transforms();
        assert!(
            world[1]
                .translation
                .abs_diff_eq(DVec3::new(1.0, 0.0, 0.0), 1e-12)
        );
        assert!(
            world[2]
                .translation
                .abs_diff_eq(DVec3::new(1.0, 1.0, 0.0), 1e-12)
        );
    }

    #[test]
    fn unit_conversion_round_trips() {
        let units = UnitOptions {
            meters_per_unit: 0.001,
            z_up: true,
        };
        // Z 上の (0, 0, 1000) mm はアプリでは Y 上の 1 m
        let p = units.app_from_file(DVec3::new(0.0, 0.0, 1000.0));
        assert!(p.abs_diff_eq(DVec3::new(0.0, 1.0, 0.0), 1e-12));
        let q = DVec3::new(1.0, 2.0, 3.0);
        assert!(
            units
                .file_from_app(units.app_from_file(q))
                .abs_diff_eq(q, 1e-9)
        );
    }

    #[test]
    fn format_lookup_ignores_case() {
        assert_eq!(
            format_for_path(Path::new("a/B.GLB")).unwrap().extensions,
            &["glb"]
        );
        assert!(format_for_path(Path::new("x.foo")).is_none());
        assert!(import_file(Path::new("x.foo"), &UnitOptions::default()).is_err());
    }
}
