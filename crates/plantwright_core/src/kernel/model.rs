//! 形状カーネルとやりとりするデータの型: 面のメッシュ、メッシュにした B-rep、
//! B-rep を組み立てるときの曲線・曲面の記述

use glam::DVec3;

/// 1つの面の三角形メッシュ。三角形は面の外向きに反時計回り
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceMesh {
    pub positions: Vec<DVec3>,
    /// 頂点ごとの曲面の法線 (面の外向き)。曲面の正確な法線なので、粗いメッシュでも平らな隣の面と比べられる
    pub normals: Vec<DVec3>,
    pub triangles: Vec<[u32; 3]>,
}

impl FaceMesh {
    pub fn new(positions: Vec<DVec3>, normals: Vec<DVec3>, triangles: Vec<[u32; 3]>) -> Self {
        Self {
            positions,
            normals,
            triangles,
        }
    }

    /// 裏返す (三角形の向きと法線を逆にする)
    pub fn invert(&mut self) {
        for t in &mut self.triangles {
            t.swap(1, 2);
        }
        for n in &mut self.normals {
            *n = -*n;
        }
    }

    /// 三角形を頂点の座標で並べる
    pub fn triangle_points(&self) -> impl Iterator<Item = [DVec3; 3]> + '_ {
        self.triangles
            .iter()
            .map(|t| t.map(|i| self.positions[i as usize]))
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }
}

/// メッシュにした B-rep の面
#[derive(Clone, Debug, Default)]
pub struct MeshedFace {
    /// 面のメッシュ。テッセレーションできなかった面は None
    pub mesh: Option<FaceMesh>,
    /// 面の輪郭 (外周が最初)。中身は [`MeshedBrep::edges`] の番号
    pub loops: Vec<Vec<usize>>,
}

/// メッシュにした B-rep。面のメッシュの縁の頂点と、辺のポリラインの点は同じ座標になる
#[derive(Clone, Debug, Default)]
pub struct MeshedBrep {
    pub faces: Vec<MeshedFace>,
    /// 辺のポリライン
    pub edges: Vec<Vec<DVec3>>,
}

/// 座標系 (原点・Z 軸・X 軸)。Y 軸は Z × X
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: DVec3,
    pub z: DVec3,
    pub x: DVec3,
}

/// B スプライン曲線 (STEP と同じく、重なりを除いた節点と重なりの数で持つ)
#[derive(Clone, Debug, PartialEq)]
pub struct BSplineCurve {
    pub degree: u32,
    pub points: Vec<DVec3>,
    /// 有理 B スプラインの重み (点と同じ数)
    pub weights: Option<Vec<f64>>,
    pub knots: Vec<f64>,
    pub multiplicities: Vec<u32>,
}

/// 面の輪郭をつくる辺の曲線。どれも輪郭をたどる向きで、始点から終点まで
#[derive(Clone, Debug, PartialEq)]
pub enum EdgeCurve {
    Line {
        start: DVec3,
        end: DVec3,
    },
    /// 円弧: center + radius (cos t · x + sin t · y)、t は度で start_deg から end_deg (start_deg < end_deg)
    Arc {
        center: DVec3,
        x: DVec3,
        y: DVec3,
        radius: f64,
        start_deg: f64,
        end_deg: f64,
    },
    /// 楕円弧: center + rx cos t · x + ry sin t · y
    Ellipse {
        center: DVec3,
        x: DVec3,
        y: DVec3,
        rx: f64,
        ry: f64,
        start_deg: f64,
        end_deg: f64,
    },
    /// B スプラインの一部。start・end は輪郭の向きでの端の点。
    /// `reverse` は輪郭の向きが曲線のパラメータと逆のとき、`full` は曲線全体を使うとき (閉じた辺)
    BSpline {
        curve: BSplineCurve,
        start: DVec3,
        end: DVec3,
        full: bool,
        reverse: bool,
    },
}

/// B スプライン曲面。制御点は u ごとに v を並べる (STEP と同じ)
#[derive(Clone, Debug, PartialEq)]
pub struct BSplineSurface {
    pub u_degree: u32,
    pub v_degree: u32,
    /// u 方向の制御点の数
    pub nu: usize,
    /// v 方向の制御点の数
    pub nv: usize,
    pub points: Vec<DVec3>,
    pub weights: Option<Vec<f64>>,
    pub u_knots: Vec<f64>,
    pub u_multiplicities: Vec<u32>,
    pub v_knots: Vec<f64>,
    pub v_multiplicities: Vec<u32>,
}

/// 面の曲面。パラメータの付け方は STEP と同じ
#[derive(Clone, Debug, PartialEq)]
pub enum Surface {
    Plane(Frame),
    Cylinder {
        frame: Frame,
        radius: f64,
    },
    /// 円錐: frame の原点での半径と、軸から面までの角度 (度)
    Cone {
        frame: Frame,
        radius: f64,
        semi_angle_deg: f64,
    },
    Sphere {
        frame: Frame,
        radius: f64,
    },
    Torus {
        frame: Frame,
        major: f64,
        minor: f64,
    },
    BSpline(BSplineSurface),
    /// 母線 `profile` (曲線全体) を `origin` を通る `axis` のまわりに回した面
    Revolution {
        profile: EdgeCurve,
        origin: DVec3,
        axis: DVec3,
    },
    /// 母線 `profile` (曲線全体) を `direction` (長さ込み) に押し出した面
    Extrusion {
        profile: EdgeCurve,
        direction: DVec3,
    },
}

/// 組み立てる面: 曲面と、輪郭 (外周が最初、内側は穴)。
/// 外周は面の外向きに見て反時計回り、穴は時計回り (STEP と同じ)
#[derive(Clone, Debug, PartialEq)]
pub struct FaceSpec {
    pub surface: Surface,
    /// false なら面の外向きは曲面の法線と逆
    pub same_sense: bool,
    pub loops: Vec<Vec<EdgeCurve>>,
}
