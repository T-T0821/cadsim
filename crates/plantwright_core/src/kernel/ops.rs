//! 形を作り変える操作 (フィレット・面取り・シェル・オフセット・回転・パイプ・スイープ・修復) と、
//! 形への問い合わせ (質量特性・断面)。C++ 側は `cpp/ops.cpp`
//!
//! 作り変える操作はどれも元の形を変えずに新しい形を返す。辺と面の番号は
//! [`Brep::tessellate`] が返す [`MeshedBrep`](super::MeshedBrep) の `edges` と `faces` の番号。

use glam::DVec3;

use super::brep::{Brep, check, lock};
use super::ffi;

/// 質量特性 (形の単位系のまま。密度は単位体積あたりの質量)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MassProperties {
    pub area: f64,
    pub volume: f64,
    pub mass: f64,
    /// 重心
    pub centroid: DVec3,
    /// 重心を通り座標軸に平行な軸まわりの慣性モーメント (Ixx, Iyy, Izz)
    pub inertia: DVec3,
    /// 重心まわりの慣性乗積 (Iyz, Izx, Ixy)。慣性テンソルの非対角成分はこの符号を反転したもの
    pub products: DVec3,
}

fn flat(points: &[DVec3]) -> Vec<f64> {
    points.iter().flat_map(|p| p.to_array()).collect()
}

impl Brep {
    /// 辺 `edges` を半径 `radius` で丸める
    pub fn fillet_edges(&self, edges: &[usize], radius: f64) -> Result<Brep, String> {
        self.round_edges(edges, radius, 1, "フィレット")
    }

    /// 辺 `edges` を面取りする。`distance` は辺から両側の面に沿って削る長さ (C 面取りの C)
    pub fn chamfer_edges(&self, edges: &[usize], distance: f64) -> Result<Brep, String> {
        self.round_edges(edges, distance, 0, "面取り")
    }

    fn round_edges(
        &self,
        edges: &[usize],
        size: f64,
        kind: i32,
        what: &str,
    ) -> Result<Brep, String> {
        if edges.is_empty() {
            return Err("辺を選んでください".into());
        }
        if size <= 0.0 {
            return Err("大きさは 0 より大きくしてください".into());
        }
        let ids: Vec<u32> = edges.iter().map(|&e| e as u32).collect();
        // SAFETY: ids は edges.len() 個、self.ptr は有効な SmBrep*
        Self::create(what, |out| unsafe {
            ffi::pw_fillet_edges(self.ptr, ids.as_ptr(), ids.len() as i32, size, kind, out)
        })
    }

    /// 形にあるフィレット (角の丸み) を見つけて消し、角を尖らせる
    pub fn remove_fillets(&self) -> Result<Brep, String> {
        // SAFETY: self.ptr は有効な SmBrep*
        Self::create("フィレットの削除", |out| unsafe {
            ffi::pw_remove_fillets(self.ptr, out)
        })
    }

    /// 厚み `thickness` の中空にする (内側に肉を付ける)。`open_faces` の面は開けておく
    pub fn shell(&self, thickness: f64, open_faces: &[usize]) -> Result<Brep, String> {
        if thickness <= 0.0 {
            return Err("厚みは 0 より大きくしてください".into());
        }
        let ids: Vec<u32> = open_faces.iter().map(|&f| f as u32).collect();
        // SAFETY: ids は open_faces.len() 個、self.ptr は有効な SmBrep*
        Self::create("シェル", |out| unsafe {
            ffi::pw_shell(self.ptr, thickness, ids.as_ptr(), ids.len() as i32, out)
        })
    }

    /// 面を `distance` だけ外へずらした形 (負なら内へ)
    pub fn offset(&self, distance: f64) -> Result<Brep, String> {
        if distance == 0.0 || !distance.is_finite() {
            return Err("ずらす量を指定してください".into());
        }
        // SAFETY: self.ptr は有効な SmBrep*
        Self::create("オフセット", |out| unsafe {
            ffi::pw_offset(self.ptr, distance, out)
        })
    }

    /// 閉じた平らな多角形 `profile` を、`origin` を通る `axis` のまわりに `angle_deg` 度回した形
    pub fn revolve(
        profile: &[DVec3],
        origin: DVec3,
        axis: DVec3,
        angle_deg: f64,
    ) -> Result<Brep, String> {
        if profile.len() < 3 {
            return Err("断面の頂点を3つ以上指定してください".into());
        }
        if axis.length_squared() == 0.0 || !(0.0..=360.0).contains(&angle_deg) || angle_deg == 0.0 {
            return Err("回転の軸と角度 (0〜360°) を指定してください".into());
        }
        let pts = flat(profile);
        let (o, a) = (origin.to_array(), axis.normalize().to_array());
        // SAFETY: pts は profile.len() 個の xyz、o と a は 3 つの f64
        Self::create("回転", |out| unsafe {
            ffi::pw_revolve(
                pts.as_ptr(),
                profile.len() as i32,
                o.as_ptr(),
                a.as_ptr(),
                angle_deg,
                out,
            )
        })
    }

    /// 点 `path` を通る線に沿った、半径 `radius` の管 (配管・ケーブル)。
    /// 4点以上ならなめらかな曲線でつなぐ
    pub fn pipe(path: &[DVec3], radius: f64) -> Result<Brep, String> {
        if path.len() < 2 {
            return Err("通る点を2つ以上指定してください".into());
        }
        if radius <= 0.0 {
            return Err("半径は 0 より大きくしてください".into());
        }
        let pts = flat(path);
        // SAFETY: pts は path.len() 個の xyz
        Self::create("パイプ", |out| unsafe {
            ffi::pw_pipe(pts.as_ptr(), path.len() as i32, radius, out)
        })
    }

    /// 閉じた多角形 `profile` (断面) を、平面上の折れ線 `path` に沿って動かした形。
    /// 断面の中心が `path` の始点に来るように置く
    pub fn sweep(profile: &[DVec3], path: &[DVec3]) -> Result<Brep, String> {
        if profile.len() < 3 || path.len() < 2 {
            return Err("断面は3点以上、道筋は2点以上指定してください".into());
        }
        let (p, q) = (flat(profile), flat(path));
        // SAFETY: p と q はそれぞれ profile.len()・path.len() 個の xyz
        Self::create("スイープ", |out| unsafe {
            ffi::pw_sweep(
                p.as_ptr(),
                profile.len() as i32,
                q.as_ptr(),
                path.len() as i32,
                out,
            )
        })
    }

    /// すき間や面の向きの乱れを直す
    pub fn heal(&mut self) -> Result<(), String> {
        let _guard = lock();
        // SAFETY: self.ptr は有効な SmBrep*
        let status = unsafe { ffi::pw_heal(self.ptr) };
        check(status, "形状の修復")
    }

    /// 密度 `density` での質量特性 (閉じたソリッドだけ)
    pub fn mass_properties(&self, density: f64) -> Result<MassProperties, String> {
        if density <= 0.0 {
            return Err("密度は 0 より大きくしてください".into());
        }
        let mut v = [0.0; 12];
        let _guard = lock();
        // SAFETY: v は 12 個の f64、self.ptr は有効な SmBrep*
        let status = unsafe { ffi::pw_mass_properties(self.ptr, density, v.as_mut_ptr()) };
        check(status, "質量特性の計算")?;
        Ok(MassProperties {
            area: v[0],
            volume: v[1],
            mass: v[2],
            centroid: DVec3::new(v[3], v[4], v[5]),
            inertia: DVec3::new(v[6], v[7], v[8]),
            products: DVec3::new(v[9], v[10], v[11]),
        })
    }

    /// `point` を通り `normal` に垂直な平面で切った断面の線 (曲線は1本 `samples` 点のポリライン)
    pub fn section(&self, point: DVec3, normal: DVec3, samples: usize) -> Vec<Vec<DVec3>> {
        if normal.length_squared() == 0.0 {
            return Vec::new();
        }
        let (o, n) = (point.to_array(), normal.to_array());
        let _guard = lock();
        // SAFETY: o と n は 3 つの f64、self.ptr は有効な SmBrep*
        let lines =
            unsafe { ffi::pw_section(self.ptr, o.as_ptr(), n.as_ptr(), samples.max(2) as i32) };
        if lines.is_null() {
            return Vec::new();
        }
        // SAFETY: lines は pw_section が返した有効な PwLines*。読んだら消す
        unsafe {
            let count = ffi::pw_lines_count(lines).max(0) as usize;
            let (mut pts, mut counts) = (std::ptr::null(), std::ptr::null());
            ffi::pw_lines_get(lines, &mut pts, &mut counts);
            let mut out = Vec::with_capacity(count);
            if count > 0 && !pts.is_null() && !counts.is_null() {
                let counts = std::slice::from_raw_parts(counts, count);
                let total: usize = counts.iter().map(|&c| c as usize).sum();
                let pts = std::slice::from_raw_parts(pts, total * 3);
                let mut at = 0;
                for &c in counts {
                    let c = c as usize;
                    out.push(
                        pts[at * 3..(at + c) * 3]
                            .as_chunks::<3>()
                            .0
                            .iter()
                            .map(|&p| DVec3::from_array(p))
                            .collect(),
                    );
                    at += c;
                }
            }
            ffi::pw_lines_free(lines);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::MeshedBrep;
    use glam::DMat4;
    use std::f64::consts::PI;

    fn volume(meshed: &MeshedBrep) -> f64 {
        meshed
            .faces
            .iter()
            .filter_map(|f| f.mesh.as_ref())
            .flat_map(|m| m.triangle_points())
            .map(|[a, b, c]| a.dot(b.cross(c)) / 6.0)
            .sum()
    }

    fn cube() -> Brep {
        Brep::cuboid(DVec3::splat(10.0)).unwrap()
    }

    /// 辺の番号のうち、両端の点がどちらも z = top と x = side の上にあるもの (立方体の縦でない辺)
    fn edge_at(b: &Brep, x: f64, z: f64) -> usize {
        let m = b.tessellate(0.01, 15.0);
        m.edges
            .iter()
            .position(|e| {
                e.iter()
                    .all(|p| (p.x - x).abs() < 1e-9 && (p.z - z).abs() < 1e-9)
            })
            .unwrap()
    }

    #[test]
    fn fillet_and_chamfer_remove_material() {
        let b = cube();
        let e = edge_at(&b, 10.0, 10.0);
        let r = 2.0;
        let filleted = b.fillet_edges(&[e], r).unwrap();
        assert!(filleted.is_solid());
        let v = volume(&filleted.tessellate(0.001, 5.0));
        let expected = 1000.0 - (4.0 - PI) * r * r / 4.0 * 10.0;
        assert!((v - expected).abs() < 0.05, "{v} {expected}");

        let chamfered = b.chamfer_edges(&[e], 2.0).unwrap();
        assert!(chamfered.is_solid());
        let v = volume(&chamfered.tessellate(0.001, 5.0));
        // 両側 2 ずつ削るので、断面 2 x 2 / 2 の三角柱ぶん減る
        assert!((v - 980.0).abs() < 1e-6, "{v}");
        assert!(b.fillet_edges(&[], 1.0).is_err());
    }

    #[test]
    fn shell_leaves_walls() {
        let b = cube();
        // 上の面 (z = 10) を開けて厚み 1 の箱にする
        let m = b.tessellate(0.01, 15.0);
        let top = m
            .faces
            .iter()
            .position(|f| {
                f.mesh
                    .as_ref()
                    .is_some_and(|m| m.positions.iter().all(|p| (p.z - 10.0).abs() < 1e-9))
            })
            .unwrap();
        let shelled = b.shell(1.0, &[top]).unwrap();
        let v = volume(&shelled.tessellate(0.001, 15.0));
        let expected = 1000.0 - 8.0 * 8.0 * 9.0;
        assert!((v - expected).abs() < 1.0, "{v} {expected}");
    }

    #[test]
    fn offset_grows_the_shape() {
        let v0 = volume(&cube().tessellate(0.001, 5.0));
        let grown = cube().offset(1.0).unwrap();
        let v = volume(&grown.tessellate(0.001, 5.0));
        assert!(v > v0 + 600.0, "{v}");
    }

    #[test]
    fn revolve_makes_a_cylinder() {
        // XZ 平面の長方形 (x 0..2, z 0..3) を Z 軸まわりに1周
        let profile = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(2.0, 0.0, 0.0),
            DVec3::new(2.0, 0.0, 3.0),
            DVec3::new(0.0, 0.0, 3.0),
        ];
        let b = Brep::revolve(&profile, DVec3::ZERO, DVec3::Z, 360.0).unwrap();
        let v = volume(&b.tessellate(0.001, 5.0));
        let expected = PI * 4.0 * 3.0;
        assert!((v - expected).abs() / expected < 1e-2, "{v} {expected}");
        // 半周
        let half = Brep::revolve(&profile, DVec3::ZERO, DVec3::Z, 180.0).unwrap();
        let v = volume(&half.tessellate(0.001, 5.0));
        assert!((v - expected / 2.0).abs() / expected < 1e-2, "{v}");
    }

    #[test]
    fn pipe_follows_path() {
        let path = [DVec3::ZERO, DVec3::new(0.0, 0.0, 10.0)];
        let b = Brep::pipe(&path, 1.0).unwrap();
        let v = volume(&b.tessellate(0.001, 5.0));
        let expected = PI * 10.0;
        assert!((v - expected).abs() / expected < 2e-2, "{v} {expected}");
    }

    #[test]
    fn sweep_along_l_path() {
        // 1 x 1 の正方形を、長さ 10 と 5 の L 字に沿って動かす
        let profile = [
            DVec3::new(-0.5, -0.5, 0.0),
            DVec3::new(0.5, -0.5, 0.0),
            DVec3::new(0.5, 0.5, 0.0),
            DVec3::new(-0.5, 0.5, 0.0),
        ];
        let path = [DVec3::ZERO, DVec3::new(0.0, 0.0, 10.0)];
        let b = Brep::sweep(&profile, &path).unwrap();
        let v = volume(&b.tessellate(0.001, 5.0));
        assert!((v - 10.0).abs() < 0.1, "{v}");
    }

    #[test]
    fn mass_properties_of_a_box() {
        let b = Brep::cuboid(DVec3::new(2.0, 4.0, 6.0))
            .unwrap()
            .transformed(DMat4::from_translation(DVec3::new(1.0, 0.0, 0.0)))
            .unwrap();
        let m = b.mass_properties(0.5).unwrap();
        assert!((m.volume - 48.0).abs() < 1e-6);
        assert!((m.mass - 24.0).abs() < 1e-6);
        assert!((m.area - 2.0 * (8.0 + 12.0 + 24.0)).abs() < 1e-6);
        assert!((m.centroid - DVec3::new(2.0, 2.0, 3.0)).length() < 1e-6);
        // Ixx = m (b² + c²) / 12
        let ixx = 24.0 * (16.0 + 36.0) / 12.0;
        assert!((m.inertia.x - ixx).abs() / ixx < 1e-4, "{:?}", m.inertia);
    }

    #[test]
    fn section_of_a_cylinder_is_a_circle() {
        let c = Brep::cylinder_z(2.0, 4.0).unwrap();
        let lines = c.section(DVec3::new(0.0, 0.0, 1.0), DVec3::Z, 64);
        assert!(!lines.is_empty());
        for p in lines.iter().flatten() {
            assert!((p.z - 1.0).abs() < 1e-6);
            assert!((p.truncate().length() - 2.0).abs() < 1e-3, "{p}");
        }
    }

    #[test]
    fn heal_keeps_a_good_solid() {
        let mut b = cube();
        b.heal().unwrap();
        assert!(b.is_solid());
    }
}
