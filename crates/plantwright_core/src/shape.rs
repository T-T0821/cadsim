//! 取り込んだ形状 (B-rep と表示用メッシュ) とアセンブリの木
//!
//! STEP・glTF などの読み込み (`formats`) が作り、設計データ (Model) が持つ。

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use glam::{DMat4, DVec3};

use crate::animation::Animation;
use crate::kernel::{Brep, FaceMesh};
use crate::part_mesh::PartMesh;

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
#[derive(Clone)]
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
    /// 元の B-rep (SMLib)。表示の簡略化 (穴・フィレットを消す) で使う。
    pub breps: Vec<Brep>,
    /// 面ごとの三角形メッシュ (部品の座標系, f64)
    pub faces: Vec<FaceMesh>,
    /// 稜線のポリライン (部品の座標系, f64)
    pub edges: Vec<Vec<DVec3>>,
    /// STEP に書かれていた色 (sRGB 0..1)
    pub color: Option<[f32; 3]>,
    pub stats: ShapeStats,
    /// 表示用のまとめたメッシュ・BVH・簡略形状。重いので読み込みのスレッドで作っておく。
    pub mesh: Arc<PartMesh>,
    /// 読み込みで形状を直した結果 (STEP)
    pub repair: ShapeRepair,
}

/// 読み込みで形状を直した結果。STEP の面を縫い合わせても閉じなかったシェル (面の集まり) を数える
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct ShapeRepair {
    /// すき間を閉じて閉じた形にできたシェルの数
    pub healed: usize,
    /// 直しても閉じなかったシェルの数 (質量は計算できない。表示・断面は使える)
    pub open: usize,
}

impl ShapeRepair {
    pub fn is_empty(&self) -> bool {
        self.healed == 0 && self.open == 0
    }
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

/// テッセレーションで1つの三角形・線分のあいだに曲面や曲線が曲がってよい角度 (度)
pub const TESSELLATION_ANGLE_DEG: f64 = 20.0;

/// B-rep を表示用の面メッシュと稜線にする。`tol` は曲面からのずれの上限。
/// メッシュにできなかった面は飛ばす (面の番号は飛ばした後の並び)
pub fn tessellate_breps(breps: &[Brep], tol: f64) -> (Vec<FaceMesh>, Vec<Vec<DVec3>>) {
    let mut faces = Vec::new();
    let mut edges = Vec::new();
    for brep in breps {
        let meshed = brep.tessellate(tol, TESSELLATION_ANGLE_DEG);
        faces.extend(meshed.faces.into_iter().filter_map(|f| f.mesh));
        edges.extend(meshed.edges.into_iter().filter(|e| e.len() >= 2));
    }
    (faces, edges)
}

/// B-rep のテッセレーションの許容誤差。部品の大きさ (外接箱) に合わせる。
pub fn brep_tolerance(breps: &[Brep]) -> f64 {
    let mut min = DVec3::splat(f64::MAX);
    let mut max = DVec3::splat(f64::MIN);
    for b in breps.iter().filter_map(Brep::loose_bounds) {
        min = min.min(b.min);
        max = max.max(b.max);
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
        faces: Vec<FaceMesh>,
        edges: Vec<Vec<DVec3>>,
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
            repair: ShapeRepair::default(),
        }
    }
}

/// 形状の番号を払い出す。ファイルをまたいでも重ならない
pub fn next_shape_id() -> usize {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// 三角形メッシュから外接箱・体積・表面積を計算する
pub fn mesh_stats(faces: &[FaceMesh]) -> ShapeStats {
    let mut stats = ShapeStats {
        min: DVec3::splat(f64::MAX),
        max: DVec3::splat(f64::MIN),
        face_count: faces.len(),
        ..Default::default()
    };
    for poly in faces {
        for &p in &poly.positions {
            stats.min = stats.min.min(p);
            stats.max = stats.max.max(p);
        }
        for [a, b, c] in poly.triangle_points() {
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
