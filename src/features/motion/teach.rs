//! 教示点の作り方 (3D ビューで部品の面をクリックして置くときの計算)
//!
//! クリックした面の点から、面の外へ「離す距離」だけ浮かせた位置を教示点にし、
//! 工具は面に向かう向き (面の法線の逆) にする。軸に近い向きは軸にそろえる。

use bevy::math::DVec3;

use plantwright_core::motion::chart::Robot;

/// この角度 (度) より軸に近い向きは、軸の向きにそろえる
pub const AXIS_SNAP_DEGREES: f64 = 10.0;

/// 向き `v` が座標軸 (±X / ±Y / ±Z) から `degrees` 度以内なら、その軸の向きにする
pub fn round_direction(v: DVec3, degrees: f64) -> DVec3 {
    let Some(v) = v.try_normalize() else {
        return v;
    };
    [
        DVec3::X,
        DVec3::NEG_X,
        DVec3::Y,
        DVec3::NEG_Y,
        DVec3::Z,
        DVec3::NEG_Z,
    ]
    .into_iter()
    .find(|axis| v.angle_between(*axis) < degrees.to_radians())
    .unwrap_or(v)
}

/// 面の上の点 `position` (外向きの法線 `normal`) から作る教示点の位置と工具の向き
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfacePoint {
    pub position: DVec3,
    /// 工具を向ける方向 (面に向かう向き)
    pub approach: DVec3,
}

/// 面の点から `standoff` だけ外へ離した教示点を求める。法線は軸に近ければ軸にそろえる。
pub fn point_on_surface(position: DVec3, normal: DVec3, standoff: f64) -> SurfacePoint {
    let normal = round_direction(normal, AXIS_SNAP_DEGREES)
        .try_normalize()
        .unwrap_or(DVec3::Y);
    SurfacePoint {
        position: position + normal * standoff,
        approach: -normal,
    }
}

/// 新しい教示点の名前 (P1, P2, …)
pub fn next_point_name(robot: &Robot) -> String {
    format!("P{}", robot.next_point_id.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_face_gives_a_downward_tool_above_the_surface() {
        // ほぼ上向きの面 (5° 傾き) は上向きにそろえ、工具は真下を向く
        let tilted = DVec3::new(5f64.to_radians().sin(), 5f64.to_radians().cos(), 0.0);
        let p = point_on_surface(DVec3::new(1.0, 2.0, 3.0), tilted, 50.0);
        assert!((p.position - DVec3::new(1.0, 52.0, 3.0)).length() < 1e-9);
        assert_eq!(p.approach, DVec3::NEG_Y);
    }

    #[test]
    fn slanted_face_keeps_its_own_direction() {
        let normal = DVec3::new(1.0, 1.0, 0.0).normalize();
        let p = point_on_surface(DVec3::ZERO, normal, 2.0);
        assert!((p.approach + normal).length() < 1e-12);
        assert!((p.position - normal * 2.0).length() < 1e-12);
    }

    #[test]
    fn round_direction_snaps_only_near_axes() {
        assert_eq!(
            round_direction(DVec3::new(0.01, -1.0, 0.0), 1.0),
            DVec3::NEG_Y
        );
        let diagonal = DVec3::new(1.0, 1.0, 0.0);
        assert!((round_direction(diagonal, 10.0) - diagonal.normalize()).length() < 1e-12);
    }
}
