//! 3D ビューで面をクリックして穴をあける道具の状態と計算
//!
//! 流れは「部品の面をクリック → 入口の中心が決まる → 丸い点をドラッグで直径、矢印の先を
//! ドラッグで深さ → Enter で確定」。ここにはその状態 ([`HoleTool`]) と、クリック位置・ドラッグ位置から
//! 穴の寸法を決める計算だけを置く。画面への描き方とマウスの読み方はアプリ側 (`features/modeling`)。
//!
//! 穴の向きは、クリックした面の法線にいちばん近い X / Y / Z 軸にする (穴あけが軸に平行な穴だけを
//! 掘れるため)。入口は法線が軸の + 側を向いていれば + 側、そうでなければ - 側。

use glam::DVec3;

use super::{HoleAxis, HoleSpec};
use crate::math::Aabb;
use crate::model::NodeId;

/// 直径・深さの下限 (モデル単位)。0 にすると穴にならない
const MIN_SIZE: f64 = 0.001;

/// ドラッグで動かすつまみ
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    /// 入口の円の上の点。ドラッグで直径
    Diameter,
    /// 穴の底の中心。ドラッグで深さ (部品の奥まで引くと貫通)
    Depth,
}

/// 置きかけの穴 (ワールド座標)
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct HoleDraft {
    /// 穴をあける部品
    pub node: NodeId,
    /// 入口の中心
    pub center: DVec3,
    pub axis: HoleAxis,
    /// 入口が軸の + 側にある (true なら - 向きに掘る)
    pub from_positive: bool,
    pub diameter: f64,
    /// 入口からの深さ。None なら貫通
    pub depth: Option<f64>,
    /// 部品の軸方向の長さ (貫通の判定と、貫通のときの矢印の長さ)
    pub extent: f64,
}

impl HoleDraft {
    /// 面をクリックした位置と法線から、その面を入口にする穴を作る。最初は貫通
    pub fn from_face(
        node: NodeId,
        point: DVec3,
        normal: DVec3,
        bounds: &Aabb,
        diameter: f64,
    ) -> HoleDraft {
        let axis = dominant_axis(normal);
        let from_positive = normal.dot(axis_dir(axis)) >= 0.0;
        let extent = bounds.size().dot(axis_dir(axis)).abs().max(MIN_SIZE);
        HoleDraft {
            node,
            center: point,
            axis,
            from_positive,
            diameter: diameter.max(MIN_SIZE),
            depth: None,
            extent,
        }
    }

    /// 掘る向き (部品の中へ向かう単位ベクトル)
    pub fn dir(&self) -> DVec3 {
        let d = axis_dir(self.axis);
        if self.from_positive { -d } else { d }
    }

    /// 穴の底の中心 (貫通なら部品の反対側)
    pub fn bottom(&self) -> DVec3 {
        self.center + self.dir() * self.depth.unwrap_or(self.extent)
    }

    /// 入口の面の中で直径の点を置く向き (軸に垂直な単位ベクトル)
    pub fn radial(&self) -> DVec3 {
        match self.axis {
            HoleAxis::X => DVec3::Z,
            HoleAxis::Y => DVec3::X,
            HoleAxis::Z => DVec3::X,
        }
    }

    /// つまみの位置
    pub fn handle_position(&self, handle: Handle) -> DVec3 {
        match handle {
            Handle::Diameter => self.center + self.radial() * (self.diameter / 2.0),
            Handle::Depth => self.bottom(),
        }
    }

    /// 入口の面の上の点 `p` に直径の点を合わせる。`step` があればその倍数に丸める
    pub fn set_diameter_from_point(&mut self, p: DVec3, step: Option<f64>) {
        let dir = self.dir();
        let in_plane = (p - self.center) - dir * (p - self.center).dot(dir);
        let diameter = in_plane.length() * 2.0;
        self.diameter = snap(diameter, step).max(MIN_SIZE);
    }

    /// 軸の上の点 `p` に穴の底を合わせる。部品の奥まで引くと貫通になる
    pub fn set_depth_from_point(&mut self, p: DVec3, step: Option<f64>) {
        let depth = (p - self.center).dot(self.dir());
        let depth = snap(depth, step);
        self.depth = (depth < self.extent * 0.999).then_some(depth.max(MIN_SIZE));
    }

    /// 数値で深さを決める。0 以下なら貫通
    pub fn set_depth(&mut self, depth: f64) {
        self.depth = (depth > 0.0).then_some(depth);
    }

    /// 穴あけに渡す指定
    pub fn spec(&self) -> HoleSpec {
        let depth = match self.depth {
            None => 0.0,
            Some(d) if self.from_positive => d,
            Some(d) => -d,
        };
        HoleSpec {
            center: self.center,
            axis: self.axis,
            diameter: self.diameter,
            depth,
        }
    }

    /// 入口 (または底) の円の点列。描画用
    pub fn circle(&self, center: DVec3, segments: usize) -> Vec<DVec3> {
        let u = self.radial();
        let v = self.dir().cross(u).normalize();
        let r = self.diameter / 2.0;
        (0..segments)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / segments as f64;
                center + (u * a.cos() + v * a.sin()) * r
            })
            .collect()
    }

    /// 画面に出す寸法の文字
    pub fn summary(&self) -> String {
        match self.depth {
            None => format!("直径 {} / 貫通", round3(self.diameter)),
            Some(d) => format!("直径 {} / 深さ {}", round3(self.diameter), round3(d)),
        }
    }
}

/// 道具の状態
#[derive(bevy_ecs::prelude::Resource, Clone, PartialEq, Debug, Default)]
pub struct HoleTool {
    pub active: bool,
    /// 置きかけの穴
    pub draft: Option<HoleDraft>,
    /// ドラッグ中のつまみ
    pub dragging: Option<Handle>,
    /// カーソルが乗っているつまみ
    pub hovered: Option<Handle>,
    /// 前回使った直径 (次の穴の初期値)
    pub last_diameter: f64,
    /// 直前の穴あけのエラー (置きかけの穴は残す)
    pub error: Option<String>,
}

impl HoleTool {
    pub fn start(&mut self) {
        self.active = true;
        self.draft = None;
        self.dragging = None;
        self.hovered = None;
        self.error = None;
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.draft = None;
        self.dragging = None;
        self.hovered = None;
    }

    /// 穴をあけた後: 直径を次の初期値にして道具をやめる
    pub fn finish(&mut self) {
        if let Some(draft) = &self.draft {
            self.last_diameter = draft.diameter;
        }
        self.error = None;
        self.stop();
    }

    /// 直径の初期値 (前回の穴の直径。初回は部品の大きさから)
    pub fn default_diameter(&self, bounds: &Aabb) -> f64 {
        if self.last_diameter > 0.0 {
            self.last_diameter
        } else {
            (bounds.size().min_element() * 0.2).max(MIN_SIZE)
        }
    }

    /// Esc: 置きかけの穴があれば取り消し、なければ道具をやめる
    pub fn cancel(&mut self) {
        if self.draft.is_some() {
            self.draft = None;
            self.dragging = None;
        } else {
            self.stop();
        }
    }

    /// 画面の下に出す案内
    pub fn hint(&self) -> &'static str {
        match (self.draft.is_some(), self.dragging) {
            (false, _) => "穴をあける面をクリックしてください (Esc でやめる)",
            (true, Some(Handle::Diameter)) => "ドラッグで直径を決めます (Ctrl で刻みにそろえる)",
            (true, Some(Handle::Depth)) => "ドラッグで深さを決めます (奥まで引くと貫通)",
            (true, None) => {
                "丸い点をドラッグで直径、矢印の先をドラッグで深さ。Enter で穴をあける・Esc で取り消し"
            }
        }
    }
}

fn dominant_axis(n: DVec3) -> HoleAxis {
    let a = n.abs();
    if a.x >= a.y && a.x >= a.z {
        HoleAxis::X
    } else if a.y >= a.z {
        HoleAxis::Y
    } else {
        HoleAxis::Z
    }
}

fn axis_dir(axis: HoleAxis) -> DVec3 {
    match axis {
        HoleAxis::X => DVec3::X,
        HoleAxis::Y => DVec3::Y,
        HoleAxis::Z => DVec3::Z,
    }
}

fn snap(value: f64, step: Option<f64>) -> f64 {
    match step {
        Some(s) if s > 0.0 => (value / s).round() * s,
        _ => value,
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Aabb {
        Aabb {
            min: DVec3::new(0.0, 0.0, 0.0),
            max: DVec3::new(100.0, 40.0, 60.0),
        }
    }

    #[test]
    fn top_face_drills_downwards_through() {
        let draft =
            HoleDraft::from_face(1, DVec3::new(50.0, 40.0, 30.0), DVec3::Y, &bounds(), 10.0);
        assert_eq!(draft.axis, HoleAxis::Y);
        assert!(draft.from_positive);
        assert_eq!(draft.dir(), -DVec3::Y);
        assert_eq!(draft.depth, None);
        assert_eq!(draft.bottom(), DVec3::new(50.0, 0.0, 30.0));
        let spec = draft.spec();
        assert_eq!(spec.depth, 0.0, "最初は貫通");
        assert_eq!(spec.diameter, 10.0);
    }

    #[test]
    fn slanted_normal_picks_nearest_axis_and_side() {
        let draft =
            HoleDraft::from_face(1, DVec3::ZERO, DVec3::new(-0.9, 0.3, 0.1), &bounds(), 10.0);
        assert_eq!(draft.axis, HoleAxis::X);
        assert!(!draft.from_positive);
        assert_eq!(draft.dir(), DVec3::X);
    }

    #[test]
    fn dragging_depth_handle_sets_depth_and_sign() {
        let mut draft =
            HoleDraft::from_face(1, DVec3::new(50.0, 40.0, 30.0), DVec3::Y, &bounds(), 10.0);
        draft.set_depth_from_point(DVec3::new(50.0, 25.0, 30.0), None);
        assert_eq!(draft.depth, Some(15.0));
        assert_eq!(draft.spec().depth, 15.0, "+ 側の入口から掘るので正");

        // 奥まで引くと貫通
        draft.set_depth_from_point(DVec3::new(50.0, 0.0, 30.0), None);
        assert_eq!(draft.depth, None);

        // - 側の入口なら負
        let mut bottom =
            HoleDraft::from_face(1, DVec3::new(50.0, 0.0, 30.0), -DVec3::Y, &bounds(), 10.0);
        bottom.set_depth_from_point(DVec3::new(50.0, 12.0, 30.0), None);
        assert_eq!(bottom.spec().depth, -12.0);
    }

    #[test]
    fn dragging_diameter_handle_ignores_height_and_snaps() {
        let mut draft =
            HoleDraft::from_face(1, DVec3::new(50.0, 40.0, 30.0), DVec3::Y, &bounds(), 10.0);
        draft.set_diameter_from_point(DVec3::new(57.3, 45.0, 30.0), None);
        assert!((draft.diameter - 14.6).abs() < 1e-9);
        draft.set_diameter_from_point(DVec3::new(57.3, 40.0, 30.0), Some(1.0));
        assert_eq!(draft.diameter, 15.0);
        draft.set_diameter_from_point(draft.center, None);
        assert_eq!(draft.diameter, MIN_SIZE, "0 にはならない");
    }

    #[test]
    fn handles_sit_on_entrance_circle_and_bottom() {
        let draft =
            HoleDraft::from_face(1, DVec3::new(50.0, 40.0, 30.0), DVec3::Y, &bounds(), 10.0);
        assert_eq!(
            draft.handle_position(Handle::Diameter),
            DVec3::new(55.0, 40.0, 30.0)
        );
        assert_eq!(draft.handle_position(Handle::Depth), draft.bottom());
        let circle = draft.circle(draft.center, 8);
        for p in circle {
            assert!((p.distance(draft.center) - 5.0).abs() < 1e-9);
            assert!((p.y - 40.0).abs() < 1e-9, "入口の面の上");
        }
    }

    #[test]
    fn tool_cancel_steps_back_then_stops() {
        let mut tool = HoleTool::default();
        tool.start();
        tool.draft = Some(HoleDraft::from_face(
            1,
            DVec3::ZERO,
            DVec3::Y,
            &bounds(),
            10.0,
        ));
        tool.cancel();
        assert!(tool.active && tool.draft.is_none());
        tool.cancel();
        assert!(!tool.active);
    }

    #[test]
    fn default_diameter_comes_from_part_then_last_hole() {
        let mut tool = HoleTool::default();
        assert_eq!(tool.default_diameter(&bounds()), 8.0);
        tool.last_diameter = 12.0;
        assert_eq!(tool.default_diameter(&bounds()), 12.0);
    }

    #[test]
    fn summary_reads_naturally() {
        let mut draft = HoleDraft::from_face(1, DVec3::ZERO, DVec3::Y, &bounds(), 20.0);
        assert_eq!(draft.summary(), "直径 20 / 貫通");
        draft.set_depth(12.5);
        assert_eq!(draft.summary(), "直径 20 / 深さ 12.5");
    }
}
