//! 取り込んだ形状 (B-rep と表示用メッシュ) とアセンブリの木
//!
//! STEP・glTF などの読み込み (cadsim_io) が作り、設計データ (Model) が持つ。

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use glam::{DMat4, DVec3};
use truck_meshalgo::prelude::*;
// truck の B-rep の曲線・曲面の型。truck-stepio にあるが、STEP の読み書きではなく形状カーネルの型として使う
use truck_stepio::r#in::alias::{Curve3D, Surface};
use truck_topology::compress::CompressedShell;

use crate::animation::Animation;
use crate::part_mesh::PartMesh;

/// truck の B-rep (f64)。表示用メッシュの元であり、今後の計測や干渉チェックにも使う。
pub type Brep = CompressedShell<Point3, Curve3D, Surface>;

/// 読み込んだファイル全体
pub struct ImportedFile {
    /// アセンブリの木のルート (ファイル名のノード)
    pub root: ImportNode,
    /// 長さの単位 ("mm" など)
    pub unit: String,
    /// 読めなかった形状など、ユーザーに伝えたい注意
    pub warnings: Vec<String>,
    /// ノードの動き (glTF などから)。トラックのノード番号は木を深さ優先でたどった順 (ルートが 0)
    pub animations: Vec<Animation>,
}

/// アセンブリの木の1ノード。子を持つならアセンブリ、shape を持つなら部品。
pub struct ImportNode {
    /// インスタンス名 (例: "Bracket_L")
    pub name: String,
    /// 製品名 (例: "Bracket")。同じ製品の別インスタンスは同じ名前になる。
    pub product: String,
    /// 親の座標系から見た配置
    pub placement: DMat4,
    pub children: Vec<ImportNode>,
    pub shape: Option<Arc<ImportedShape>>,
}

/// 1つの部品 (製品) の形状。複数のインスタンスで共有する。
pub struct ImportedShape {
    /// 共有されている形状を見分けるための番号 (メッシュのキャッシュに使う)
    pub id: usize,
    /// 元の B-rep。表示の簡略化 (穴・フィレットを消す) で使う。
    pub breps: Vec<Brep>,
    /// 面ごとの三角形メッシュ (部品の座標系, f64)
    pub faces: Vec<PolygonMesh>,
    /// 稜線のポリライン (部品の座標系, f64)
    pub edges: Vec<Vec<Point3>>,
    /// STEP に書かれていた色 (sRGB 0..1)
    pub color: Option<[f32; 3]>,
    pub stats: ShapeStats,
    /// 表示用のまとめたメッシュ・BVH・簡略形状。重いので読み込みのスレッドで作っておく。
    pub mesh: Arc<PartMesh>,
}

/// 部品の計測値 (メッシュから計算するので近似値)
#[derive(Clone, Copy, Default)]
pub struct ShapeStats {
    pub min: DVec3,
    pub max: DVec3,
    pub volume: f64,
    pub area: f64,
    pub face_count: usize,
    pub edge_count: usize,
}

/// B-rep のテッセレーションの許容誤差。部品の大きさ (頂点の範囲) に合わせる。
pub fn brep_tolerance(breps: &[Brep]) -> f64 {
    let mut min = DVec3::splat(f64::MAX);
    let mut max = DVec3::splat(f64::MIN);
    for p in breps.iter().flat_map(|b| &b.vertices) {
        min = min.min(DVec3::new(p.x, p.y, p.z));
        max = max.max(DVec3::new(p.x, p.y, p.z));
    }
    tessellation_tolerance(min, max)
}

/// 部品の大きさ (頂点の範囲) に見合うテッセレーションの許容誤差
pub fn tessellation_tolerance(min: DVec3, max: DVec3) -> f64 {
    let (diameter, thinnest) = if min.x <= max.x {
        ((max - min).length(), (max - min).min_element())
    } else {
        (1.0, 1.0)
    };
    // 細長い部品 (軸など) でも丸みが角張らないよう、いちばん薄い方向の寸法も見る
    (diameter * 0.002)
        .min(thinnest * 0.005)
        .max(diameter * 1e-5)
        .max(1e-6)
}

impl ImportedShape {
    /// メッシュから形状を作り、計測値と共有用の番号を付ける
    pub fn new(
        breps: Vec<Brep>,
        faces: Vec<PolygonMesh>,
        edges: Vec<Vec<Point3>>,
        color: Option<[f32; 3]>,
    ) -> Self {
        let mut stats = mesh_stats(&faces);
        stats.edge_count = edges.len();
        let mesh = Arc::new(PartMesh::new(&faces, &edges));
        ImportedShape {
            id: next_shape_id(),
            breps,
            faces,
            edges,
            color,
            stats,
            mesh,
        }
    }
}

/// 形状の番号を払い出す。ファイルをまたいでも重ならない
pub fn next_shape_id() -> usize {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// 三角形メッシュから外接箱・体積・表面積を計算する
pub fn mesh_stats(faces: &[PolygonMesh]) -> ShapeStats {
    let mut stats = ShapeStats {
        min: DVec3::splat(f64::MAX),
        max: DVec3::splat(f64::MIN),
        face_count: faces.len(),
        ..Default::default()
    };
    for poly in faces {
        let positions = poly.positions();
        for p in positions {
            let p = DVec3::new(p.x, p.y, p.z);
            stats.min = stats.min.min(p);
            stats.max = stats.max.max(p);
        }
        for tri in poly.faces().triangle_iter() {
            let [a, b, c] = tri.map(|v| {
                let p = positions[v.pos];
                DVec3::new(p.x, p.y, p.z)
            });
            stats.area += (b - a).cross(c - a).length() / 2.0;
            // 原点と三角形で作る四面体の符号付き体積を足し合わせる
            stats.volume += a.dot(b.cross(c)) / 6.0;
        }
    }
    if stats.min.x > stats.max.x {
        stats.min = DVec3::ZERO;
        stats.max = DVec3::ZERO;
    }
    stats.volume = stats.volume.abs();
    stats
}
