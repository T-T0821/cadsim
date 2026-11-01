//! 部品ごとの物理の設定 (ドメイン。Bevy に依存しない)
//!
//! 設定は部品 (ノード) が持ち、プロジェクトファイルに保存される。既定値は「機構で動く」で、
//! 何も設定しなければ今までどおりタイミングチャートのとおりに動き、ワークを押したり支えたりする。

use glam::DVec3;

/// 部品が物理演算の中でどう振る舞うか
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BodyMode {
    /// 動かない。タイミングチャートで動かしても設計位置に止まったまま当たる。
    Fixed,
    /// タイミングチャートのとおりに動き、当たったものを押す (重力や衝突では動かない)
    #[default]
    Mechanism,
    /// 重力・接触・摩擦で動く (ワークなど)
    Dynamic,
    /// 当たり判定に入れない (カバーや配線など)
    Ignored,
}

impl BodyMode {
    pub const ALL: [BodyMode; 4] = [
        BodyMode::Fixed,
        BodyMode::Mechanism,
        BodyMode::Dynamic,
        BodyMode::Ignored,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BodyMode::Fixed => "動かない",
            BodyMode::Mechanism => "機構で動く",
            BodyMode::Dynamic => "物理で動く",
            BodyMode::Ignored => "当たらない",
        }
    }
}

/// 当たり判定の形
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColliderShape {
    /// 箱の部品は箱。取り込んだ部品は、物理で動くなら凸包、それ以外はメッシュそのもの。
    #[default]
    Auto,
    /// 部品の外接箱
    Box,
    /// 頂点を包む凸の立体。速いが、くぼみは埋まる。
    ConvexHull,
    /// 凸の立体いくつかに分けたもの。くぼみ (シュートの溝など) も再現できる。
    ConvexDecomposition,
    /// 表示用のメッシュそのもの。動かない・機構で動く部品向け
    Mesh,
}

impl ColliderShape {
    pub const ALL: [ColliderShape; 5] = [
        ColliderShape::Auto,
        ColliderShape::Box,
        ColliderShape::ConvexHull,
        ColliderShape::ConvexDecomposition,
        ColliderShape::Mesh,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ColliderShape::Auto => "自動",
            ColliderShape::Box => "箱",
            ColliderShape::ConvexHull => "凸包",
            ColliderShape::ConvexDecomposition => "凸分解",
            ColliderShape::Mesh => "メッシュ",
        }
    }

    /// 「自動」を実際の形に直す。メッシュは物理で動く部品には使えないので凸包にする。
    pub fn resolve(self, mode: BodyMode, is_box: bool) -> ColliderShape {
        match self {
            ColliderShape::Auto if is_box => ColliderShape::Box,
            ColliderShape::Auto | ColliderShape::Mesh if mode == BodyMode::Dynamic => {
                ColliderShape::ConvexHull
            }
            ColliderShape::Auto => ColliderShape::Mesh,
            shape => shape,
        }
    }
}

/// 部品の座標軸
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LocalAxis {
    #[default]
    X,
    Y,
    Z,
}

impl LocalAxis {
    pub const ALL: [LocalAxis; 3] = [LocalAxis::X, LocalAxis::Y, LocalAxis::Z];

    pub fn label(self) -> &'static str {
        match self {
            LocalAxis::X => "X",
            LocalAxis::Y => "Y",
            LocalAxis::Z => "Z",
        }
    }

    pub fn unit(self) -> DVec3 {
        match self {
            LocalAxis::X => DVec3::X,
            LocalAxis::Y => DVec3::Y,
            LocalAxis::Z => DVec3::Z,
        }
    }
}

/// ベルトの表面の動き。上に載ったものを摩擦で運ぶ。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeltDrive {
    /// 流れる向き (部品の座標系の軸)
    pub axis: LocalAxis,
    pub reversed: bool,
    /// ベルト速度 (モデル単位 / 秒)
    pub speed: f64,
}

impl Default for BeltDrive {
    fn default() -> Self {
        Self {
            axis: LocalAxis::X,
            reversed: false,
            speed: 200.0,
        }
    }
}

impl BeltDrive {
    /// 部品の座標系でのベルト表面の速度
    pub fn local_velocity(&self) -> DVec3 {
        let sign = if self.reversed { -1.0 } else { 1.0 };
        self.axis.unit() * self.speed * sign
    }
}

/// 部品1つ分の物理の設定
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartPhysics {
    pub mode: BodyMode,
    pub shape: ColliderShape,
    /// 摩擦係数 (接する2つの部品の平均を使う)
    pub friction: f64,
    /// 反発係数 (0 = 跳ねない, 1 = 完全に跳ね返る)
    pub restitution: f64,
    /// 密度 (g/cm³)。物理で動く部品の重さになる。
    pub density: f64,
    /// ベルトコンベアとして上に載ったものを運ぶ
    pub belt: Option<BeltDrive>,
}

impl Default for PartPhysics {
    fn default() -> Self {
        Self {
            mode: BodyMode::default(),
            shape: ColliderShape::default(),
            friction: 0.5,
            restitution: 0.0,
            density: 1.0,
            belt: None,
        }
    }
}

impl PartPhysics {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// 長さの単位 1 つが何メートルか。知らない単位は mm とみなす。
pub fn meters_per_unit(unit: &str) -> f64 {
    match unit.trim().to_ascii_lowercase().as_str() {
        "m" | "meter" | "metre" => 1.0,
        "cm" => 0.01,
        "in" | "inch" => 0.0254,
        "ft" | "foot" => 0.3048,
        _ => 0.001,
    }
}

/// 重力加速度 (モデル単位 / 秒²)。Y が上。
pub fn gravity(unit: &str) -> DVec3 {
    DVec3::new(0.0, -9.81 / meters_per_unit(unit), 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_shape_depends_on_part_and_mode() {
        use BodyMode::*;
        use ColliderShape::*;
        assert_eq!(Auto.resolve(Dynamic, true), Box);
        assert_eq!(Auto.resolve(Mechanism, false), Mesh);
        assert_eq!(Auto.resolve(Dynamic, false), ConvexHull);
        // 動く部品にメッシュは使えない
        assert_eq!(Mesh.resolve(Dynamic, false), ConvexHull);
        assert_eq!(
            ConvexDecomposition.resolve(Dynamic, false),
            ConvexDecomposition
        );
    }

    #[test]
    fn gravity_follows_the_unit() {
        assert!((gravity("mm").y + 9810.0).abs() < 1e-9);
        assert!((gravity("m").y + 9.81).abs() < 1e-12);
    }

    #[test]
    fn belt_velocity_points_along_the_axis() {
        let belt = BeltDrive {
            axis: LocalAxis::Z,
            reversed: true,
            speed: 100.0,
        };
        assert_eq!(belt.local_velocity(), DVec3::new(0.0, 0.0, -100.0));
    }
}
