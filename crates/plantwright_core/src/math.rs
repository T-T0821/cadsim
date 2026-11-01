//! 回転・行列・外接箱の計算 (f64)

use std::f64::consts::{FRAC_PI_2, PI};

use glam::{DMat3, DMat4, DQuat, DVec3};

/// X → Y → Z の順に回す角度 (度) をクォータニオンにする
pub fn euler_deg_to_quat(deg: [f64; 3]) -> DQuat {
    let [x, y, z] = deg.map(f64::to_radians);
    DQuat::from_rotation_z(z) * DQuat::from_rotation_y(y) * DQuat::from_rotation_x(x)
}

/// euler_deg_to_quat の逆。R = Rz * Ry * Rx から X → Y → Z の角度 (度) を取り出す。
pub fn quat_to_euler_deg(q: DQuat) -> [f64; 3] {
    let m = DMat3::from_quat(q);
    let y = (-m.x_axis.z).clamp(-1.0, 1.0).asin();
    let x = m.y_axis.z.atan2(m.z_axis.z);
    let z = m.x_axis.y.atan2(m.x_axis.x);
    // (x, y, z) と (x ± 180°, 180° - y, z ± 180°) は同じ回転。Y だけ回したときに
    // (180, 85, 180) のような表示にならないよう、X と Z が小さい方を選ぶ。
    let (x, y, z) = if x.abs() > FRAC_PI_2 && z.abs() > FRAC_PI_2 {
        (x - PI.copysign(x), PI.copysign(y) - y, z - PI.copysign(z))
    } else {
        (x, y, z)
    };
    // 表示上の -0 や 1e-15 のような誤差を消す
    [x, y, z].map(|r| (r.to_degrees() * 1e9).round() / 1e9 + 0.0)
}

/// 配置行列を (平行移動, 回転) に分ける。スケールは使わない前提で捨てる。
pub fn split_placement(m: DMat4) -> (DVec3, DQuat) {
    let (_, rotation, translation) = m.to_scale_rotation_translation();
    (translation, rotation)
}

/// 外接箱 (軸に平行な箱)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Aabb {
    pub fn size(&self) -> DVec3 {
        self.max - self.min
    }

    pub fn center(&self) -> DVec3 {
        (self.min + self.max) / 2.0
    }

    pub fn union(self, other: Aabb) -> Aabb {
        Aabb {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    /// 8つの角を `m` で動かし、それを囲む新しい外接箱を返す
    pub fn transformed(&self, m: DMat4) -> Aabb {
        self.corners()
            .into_iter()
            .map(|p| {
                let p = m.transform_point3(p);
                Aabb { min: p, max: p }
            })
            .reduce(Aabb::union)
            .expect("角は8つある")
    }

    pub fn corners(&self) -> [DVec3; 8] {
        let (a, b) = (self.min, self.max);
        [
            DVec3::new(a.x, a.y, a.z),
            DVec3::new(b.x, a.y, a.z),
            DVec3::new(b.x, b.y, a.z),
            DVec3::new(a.x, b.y, a.z),
            DVec3::new(a.x, a.y, b.z),
            DVec3::new(b.x, a.y, b.z),
            DVec3::new(b.x, b.y, b.z),
            DVec3::new(a.x, b.y, b.z),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_round_trip() {
        for deg in [[0.0, 0.0, 0.0], [15.0, 30.0, 45.0], [-70.0, 10.0, 120.0]] {
            let back = quat_to_euler_deg(euler_deg_to_quat(deg));
            for (a, b) in deg.iter().zip(back) {
                assert!((a - b).abs() < 1e-9, "{deg:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn y_only_rotation_past_90_stays_on_y() {
        let back = quat_to_euler_deg(DQuat::from_rotation_y(95f64.to_radians()));
        assert_eq!(back, [0.0, 95.0, 0.0]);
    }

    #[test]
    fn aabb_rotated_90_degrees() {
        let aabb = Aabb {
            min: DVec3::ZERO,
            max: DVec3::new(2.0, 1.0, 1.0),
        };
        let rotated = aabb.transformed(DMat4::from_rotation_z(FRAC_PI_2));
        assert!((rotated.size() - DVec3::new(1.0, 2.0, 1.0)).length() < 1e-9);
    }
}
