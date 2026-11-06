//! 3D ビューで役割の向き・ストローク・速さ・範囲を決める道具の状態と計算
//!
//! 流れは「プロパティの役割ボタン (か『3D で調整』) → 矢印や点をドラッグ・面をクリック → Enter で確定」。
//! 確定するまでは下書き ([`RoleDraft`]) だけを変え、設定には書かない。Esc で下書きを捨てる。
//! ここにはドラッグした位置から値を決める計算だけを置く。カーソルをどの線・面に落とすかは
//! [`RoleDraft::constraint`] が決め、アプリ側がレイをそこに落として [`RoleDraft::drag`] に渡す。

use glam::DVec3;

use super::assign::{axis_len, r3};
use super::model::{RoleBody, RoleDraft};
use crate::conveyor::{BeltAxis, SourceOutput};
use crate::math::Aabb;
use crate::motion::chart::{ActuatorKind, Axis};

/// コンベアの矢印の長さ = 速さ × この秒数
pub const CONVEYOR_ARROW_SECONDS: f64 = 1.0;

/// ドラッグで動かすつまみ
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    /// 矢印の先 (シリンダのストローク・直動軸の長さ・センサーの範囲・コンベアの向きと速さ)
    Tip,
    /// グリッパの掴む位置
    Grip,
    /// グリッパの掴める距離
    Reach,
    /// 投入口からコンベアへつなぐ点
    Link,
}

/// ドラッグ中のカーソルを落とす先
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Constraint {
    /// 直線の上 (点と向き)
    Line { origin: DVec3, dir: DVec3 },
    /// 平面の上 (点と法線)
    Plane { origin: DVec3, normal: DVec3 },
    /// 点を通り、画面に平行な面の上
    ViewPlane { origin: DVec3 },
}

impl RoleDraft {
    /// 出すつまみ
    pub fn handles(&self) -> Vec<Handle> {
        match &self.body {
            RoleBody::Actuator(a) if a.kind == ActuatorKind::RotaryTable => vec![],
            RoleBody::Actuator(_) | RoleBody::Sensor(_) | RoleBody::Conveyor(_) => {
                vec![Handle::Tip]
            }
            RoleBody::Gripper(_) => vec![Handle::Grip, Handle::Reach],
            RoleBody::Source(_) => vec![Handle::Link],
            RoleBody::Sink(_) => vec![],
        }
    }

    /// 矢印の根元と先 (描く線)。矢印のない役割は None
    pub fn arrow(&self) -> Option<(DVec3, DVec3)> {
        let center = self.bounds.center();
        match &self.body {
            RoleBody::Actuator(a) => {
                let dir = signed(a.axis, a.reversed);
                let half = axis_len(self.bounds.size(), a.axis) / 2.0;
                let travel = match a.kind {
                    ActuatorKind::Cylinder => a.stroke,
                    ActuatorKind::LinearAxis => a.limits.map_or(0.0, |[_, hi]| hi),
                    ActuatorKind::RotaryTable => {
                        // 回転軸を部品の中心に通して描く
                        let d = DVec3::from_array(a.axis.unit_vector());
                        let reach = axis_len(self.bounds.size(), a.axis) / 2.0 * 1.5;
                        let pivot = DVec3::from_array(a.pivot);
                        return Some((pivot - d * reach, pivot + d * reach));
                    }
                };
                let base = center + dir * half;
                Some((base, base + dir * travel))
            }
            RoleBody::Sensor(s) => {
                let origin = DVec3::from_array(s.position);
                Some((origin, origin + signed(s.axis, s.reversed) * s.range))
            }
            RoleBody::Conveyor(c) => {
                let base = self.top_center();
                let dir = belt_dir(c.axis, c.reversed);
                Some((base, base + dir * c.speed * CONVEYOR_ARROW_SECONDS))
            }
            RoleBody::Gripper(_) | RoleBody::Source(_) | RoleBody::Sink(_) => None,
        }
    }

    fn top_center(&self) -> DVec3 {
        let c = self.bounds.center();
        DVec3::new(c.x, self.bounds.max.y, c.z)
    }

    /// つまみの位置
    pub fn handle_position(&self, handle: Handle) -> DVec3 {
        match (handle, &self.body) {
            (Handle::Grip, RoleBody::Gripper(g)) => DVec3::from_array(g.tcp),
            (Handle::Reach, RoleBody::Gripper(g)) => DVec3::from_array(g.tcp) + DVec3::X * g.reach,
            (Handle::Link, _) => self.top_center(),
            _ => self.arrow().map_or(self.bounds.center(), |(_, tip)| tip),
        }
    }

    /// ドラッグ中のカーソルを落とす先
    pub fn constraint(&self, handle: Handle) -> Constraint {
        match (&self.body, handle) {
            (RoleBody::Actuator(a), Handle::Tip) => Constraint::Line {
                origin: self.bounds.center(),
                dir: DVec3::from_array(a.axis.unit_vector()),
            },
            (RoleBody::Sensor(s), Handle::Tip) => Constraint::Line {
                origin: DVec3::from_array(s.position),
                dir: DVec3::from_array(s.axis.unit_vector()),
            },
            (RoleBody::Conveyor(_), Handle::Tip) => Constraint::Plane {
                origin: self.top_center(),
                normal: DVec3::Y,
            },
            (RoleBody::Gripper(g), Handle::Reach) => Constraint::ViewPlane {
                origin: DVec3::from_array(g.tcp),
            },
            _ => Constraint::ViewPlane {
                origin: self.handle_position(handle),
            },
        }
    }

    /// つまみを点 `p` まで動かす。`step` があれば長さをその倍数に丸める
    pub fn drag(&mut self, handle: Handle, p: DVec3, step: Option<f64>) {
        let min = self.min_length();
        let center = self.bounds.center();
        let size = self.bounds.size();
        let top = self.top_center();
        match (&mut self.body, handle) {
            (RoleBody::Actuator(a), Handle::Tip) => {
                // 部品の中心からの位置。向きの反対側へ引くと逆向きになる
                let t = (p - center).dot(DVec3::from_array(a.axis.unit_vector()));
                let half = axis_len(size, a.axis) / 2.0;
                a.reversed = t < 0.0;
                let travel = snap(t.abs() - half, step).max(min);
                match a.kind {
                    ActuatorKind::Cylinder => a.stroke = travel,
                    ActuatorKind::LinearAxis => a.limits = Some([0.0, travel]),
                    ActuatorKind::RotaryTable => {}
                }
            }
            (RoleBody::Sensor(s), Handle::Tip) => {
                let t = (p - DVec3::from_array(s.position))
                    .dot(DVec3::from_array(s.axis.unit_vector()));
                s.reversed = t < 0.0;
                s.range = snap(t.abs(), step).max(min);
            }
            (RoleBody::Conveyor(c), Handle::Tip) => {
                // 水平面の上で、X と Z の近い方の向きにする
                let v = p - top;
                let (axis, along) = if v.x.abs() >= v.z.abs() {
                    (BeltAxis::X, v.x)
                } else {
                    (BeltAxis::Z, v.z)
                };
                c.axis = axis;
                c.reversed = along < 0.0;
                c.speed = (snap(along.abs(), step) / CONVEYOR_ARROW_SECONDS).max(min);
            }
            (RoleBody::Gripper(g), Handle::Grip) => {
                g.tcp = match step {
                    Some(_) => [snap(p.x, step), snap(p.y, step), snap(p.z, step)],
                    None => p.to_array(),
                };
            }
            (RoleBody::Gripper(g), Handle::Reach) => {
                g.reach = snap(p.distance(DVec3::from_array(g.tcp)), step).max(min);
            }
            _ => {}
        }
    }

    /// 長さの下限 (0 にはしない)。部品の大きさの 1/1000
    fn min_length(&self) -> f64 {
        (self.bounds.size().max_element() * 1e-3).max(1e-9)
    }

    /// 部品の面をクリックした: シリンダ・直動軸はその面から出る向き、回転テーブルはその面の向きを軸に、
    /// センサーはその面の上の点から外へ向ける。向きを変えたら true
    pub fn click_face(&mut self, point: DVec3, normal: DVec3) -> bool {
        let (axis, reversed) = dominant(normal);
        match &mut self.body {
            RoleBody::Actuator(a) => {
                a.axis = axis;
                a.reversed = a.kind != ActuatorKind::RotaryTable && reversed;
                if a.kind == ActuatorKind::RotaryTable {
                    a.pivot = self.bounds.center().to_array();
                }
                true
            }
            RoleBody::Sensor(s) => {
                s.position = point.to_array();
                s.axis = axis;
                s.reversed = reversed;
                true
            }
            _ => false,
        }
    }

    /// 投入口の出す先をコンベアにする (None なら部品の上)
    pub fn link_to(&mut self, conveyor: Option<u32>) {
        if let RoleBody::Source(s) = &mut self.body {
            s.output = conveyor.map_or(SourceOutput::OnPart, SourceOutput::Conveyor);
        }
    }

    /// 動いた先の部品の外接箱 (半透明で出す)。動かない役割は None
    pub fn moved_bounds(&self, progress: f64) -> Option<Aabb> {
        let RoleBody::Actuator(a) = &self.body else {
            return None;
        };
        let travel = match a.kind {
            ActuatorKind::Cylinder => a.stroke,
            ActuatorKind::LinearAxis => a.limits.map_or(0.0, |[_, hi]| hi),
            ActuatorKind::RotaryTable => return None,
        };
        let offset = signed(a.axis, a.reversed) * travel * progress.clamp(0.0, 1.0);
        Some(Aabb {
            min: self.bounds.min + offset,
            max: self.bounds.max + offset,
        })
    }

    /// つまみの横に出す値
    pub fn handle_label(&self, handle: Handle, unit: &str) -> String {
        match (&self.body, handle) {
            (RoleBody::Actuator(a), _) => match a.kind {
                ActuatorKind::Cylinder => format!("ストローク {} {unit}", r3(a.stroke)),
                _ => format!(
                    "動ける長さ {} {unit}",
                    r3(a.limits.map_or(0.0, |[_, hi]| hi))
                ),
            },
            (RoleBody::Sensor(s), _) => format!("範囲 {} {unit}", r3(s.range)),
            (RoleBody::Conveyor(c), _) => format!("{} {unit}/s", r3(c.speed)),
            (RoleBody::Gripper(_), Handle::Grip) => "掴む位置".into(),
            (RoleBody::Gripper(g), _) => format!("掴める距離 {} {unit}", r3(g.reach)),
            (RoleBody::Source(s), _) => match s.output {
                SourceOutput::Conveyor(_) => "コンベアへ出す".into(),
                SourceOutput::OnPart => "ドラッグでコンベアへつなぐ".into(),
            },
            (RoleBody::Sink(_), _) => String::new(),
        }
    }
}

/// 道具の状態
#[derive(bevy_ecs::prelude::Resource, Clone, PartialEq, Debug, Default)]
pub struct RoleTool {
    /// 置きかけの役割。Some の間は道具を使っている
    pub draft: Option<RoleDraft>,
    pub dragging: Option<Handle>,
    pub hovered: Option<Handle>,
    /// 投入口からつなぐ線をドラッグしている先
    pub link_point: Option<DVec3>,
}

impl RoleTool {
    pub fn active(&self) -> bool {
        self.draft.is_some()
    }

    pub fn start(&mut self, draft: RoleDraft) {
        *self = RoleTool {
            draft: Some(draft),
            ..RoleTool::default()
        };
    }

    /// 下書きを捨てて道具をやめる
    pub fn stop(&mut self) {
        *self = RoleTool::default();
    }

    /// 画面の下に出す案内
    pub fn hint(&self) -> String {
        let Some(draft) = &self.draft else {
            return String::new();
        };
        let label = draft.body.kind().label();
        let what = match (&draft.body, self.dragging) {
            (_, Some(Handle::Link)) => {
                "つなぐコンベアの部品の上で離してください (何もないところで離すと部品の上に出す)"
                    .to_owned()
            }
            (_, Some(Handle::Grip)) => {
                "ドラッグで掴む位置を決めます (Ctrl で刻みにそろえる)".into()
            }
            (_, Some(_)) => "ドラッグで値を決めます (Ctrl で刻みにそろえる)".into(),
            (RoleBody::Actuator(a), None) if a.kind == ActuatorKind::RotaryTable => {
                "面をクリックすると、その面の向きを回転軸にします".into()
            }
            (RoleBody::Actuator(_), None) => {
                "面をクリックで動く向き、矢印の先をドラッグで長さ".into()
            }
            (RoleBody::Sensor(_), None) => {
                "面をクリックで取り付け位置と向き、光の先をドラッグで範囲".into()
            }
            (RoleBody::Conveyor(_), None) => "緑の矢印の先をドラッグで流れる向きと速さ".into(),
            (RoleBody::Gripper(_), None) => "青い球で掴む位置、外の点で掴める距離".into(),
            (RoleBody::Source(_), None) => "上の点をコンベアまでドラッグしてつなぐ".into(),
            (RoleBody::Sink(_), None) => "このまま確定できます".into(),
        };
        let verb = if draft.existing.is_some() {
            "を調整"
        } else {
            "にする"
        };
        format!("{label}{verb}: {what}。Enter で確定・Esc で取り消し")
    }
}

/// 法線にいちばん近い軸と、その - 側か
fn dominant(n: DVec3) -> (Axis, bool) {
    let a = n.abs();
    if a.x >= a.y && a.x >= a.z {
        (Axis::X, n.x < 0.0)
    } else if a.y >= a.z {
        (Axis::Y, n.y < 0.0)
    } else {
        (Axis::Z, n.z < 0.0)
    }
}

fn signed(axis: Axis, reversed: bool) -> DVec3 {
    let d = DVec3::from_array(axis.unit_vector());
    if reversed { -d } else { d }
}

fn belt_dir(axis: BeltAxis, reversed: bool) -> DVec3 {
    let d = axis.unit_vector();
    if reversed { -d } else { d }
}

fn snap(value: f64, step: Option<f64>) -> f64 {
    match step {
        Some(s) if s > 0.0 => (value / s).round() * s,
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::super::assign::new_draft;
    use super::super::model::RoleKind;
    use super::*;
    use crate::motion::chart::TimingChart;

    fn bounds() -> Aabb {
        Aabb {
            min: DVec3::new(0.0, 0.0, 0.0),
            max: DVec3::new(400.0, 50.0, 100.0),
        }
    }

    fn draft(kind: RoleKind) -> RoleDraft {
        let mut chart = TimingChart::default();
        chart.add_actuator(ActuatorKind::Cylinder);
        chart.actuators[0].parts = vec![7];
        new_draft(kind, 7, "台", bounds(), 1.0, &chart).unwrap()
    }

    #[test]
    fn cylinder_arrow_starts_on_face_and_tip_drag_sets_stroke() {
        let mut d = draft(RoleKind::Cylinder);
        let (base, tip) = d.arrow().unwrap();
        assert_eq!(base, DVec3::new(400.0, 25.0, 50.0));
        assert_eq!(tip, DVec3::new(600.0, 25.0, 50.0));
        assert_eq!(d.handle_position(Handle::Tip), tip);

        d.drag(Handle::Tip, DVec3::new(473.0, 99.0, 50.0), None);
        let RoleBody::Actuator(a) = &d.body else {
            panic!()
        };
        assert!((a.stroke - 73.0).abs() < 1e-9);
        assert!(!a.reversed);

        // Ctrl で刻みにそろえる
        d.drag(Handle::Tip, DVec3::new(473.0, 0.0, 0.0), Some(10.0));
        let RoleBody::Actuator(a) = &d.body else {
            panic!()
        };
        assert_eq!(a.stroke, 70.0);

        // 反対側へ引くと逆向き
        d.drag(Handle::Tip, DVec3::new(-50.0, 0.0, 0.0), None);
        let RoleBody::Actuator(a) = &d.body else {
            panic!()
        };
        assert!(a.reversed);
        assert!((a.stroke - 50.0).abs() < 1e-9);
        assert_eq!(d.arrow().unwrap().0, DVec3::new(0.0, 25.0, 50.0));
    }

    #[test]
    fn clicking_top_face_makes_cylinder_push_up() {
        let mut d = draft(RoleKind::Cylinder);
        assert!(d.click_face(DVec3::new(10.0, 50.0, 10.0), DVec3::new(0.1, 0.99, 0.0)));
        let RoleBody::Actuator(a) = &d.body else {
            panic!()
        };
        assert_eq!((a.axis, a.reversed), (Axis::Y, false));
        let moved = d.moved_bounds(1.0).unwrap();
        assert_eq!(moved.min.y, a.stroke);
        assert_eq!(d.moved_bounds(0.5).unwrap().min.y, a.stroke / 2.0);
    }

    #[test]
    fn conveyor_arrow_drag_picks_direction_and_speed() {
        let mut d = draft(RoleKind::Conveyor);
        let (base, tip) = d.arrow().unwrap();
        assert_eq!(base, DVec3::new(200.0, 50.0, 50.0));
        assert_eq!(tip, base + DVec3::X * 200.0);
        assert_eq!(
            d.constraint(Handle::Tip),
            Constraint::Plane {
                origin: base,
                normal: DVec3::Y
            }
        );
        d.drag(Handle::Tip, base + DVec3::new(10.0, 0.0, -150.0), None);
        let RoleBody::Conveyor(c) = &d.body else {
            panic!()
        };
        assert_eq!((c.axis, c.reversed), (BeltAxis::Z, true));
        assert_eq!(c.speed, 150.0);
    }

    #[test]
    fn sensor_face_click_and_range_drag() {
        let mut d = draft(RoleKind::Proximity);
        d.click_face(DVec3::new(0.0, 20.0, 30.0), -DVec3::X);
        d.drag(Handle::Tip, DVec3::new(-45.0, 0.0, 0.0), None);
        let RoleBody::Sensor(s) = &d.body else {
            panic!()
        };
        assert_eq!(s.position, [0.0, 20.0, 30.0]);
        assert_eq!((s.axis, s.reversed, s.range), (Axis::X, true, 45.0));
    }

    #[test]
    fn gripper_handles_move_tcp_and_reach() {
        let mut d = draft(RoleKind::Gripper);
        assert_eq!(d.handles(), vec![Handle::Grip, Handle::Reach]);
        d.drag(Handle::Grip, DVec3::new(100.0, 10.0, 20.0), None);
        d.drag(Handle::Reach, DVec3::new(100.0, 10.0, 55.0), None);
        let RoleBody::Gripper(g) = &d.body else {
            panic!()
        };
        assert_eq!(g.tcp, [100.0, 10.0, 20.0]);
        assert_eq!(g.reach, 35.0);
        assert_eq!(
            d.handle_position(Handle::Reach),
            DVec3::new(135.0, 10.0, 20.0)
        );
    }

    #[test]
    fn source_links_to_conveyor() {
        let mut d = draft(RoleKind::Source);
        d.link_to(Some(4));
        let RoleBody::Source(s) = &d.body else {
            panic!()
        };
        assert_eq!(s.output, SourceOutput::Conveyor(4));
        d.link_to(None);
        assert_eq!(
            d.handle_label(Handle::Link, "mm"),
            "ドラッグでコンベアへつなぐ"
        );
    }

    #[test]
    fn hint_says_what_to_do() {
        let mut tool = RoleTool::default();
        assert!(!tool.active());
        tool.start(draft(RoleKind::Cylinder));
        assert!(tool.active());
        assert!(tool.hint().starts_with("シリンダにする: 面をクリック"));
        tool.stop();
        assert!(!tool.active());
    }
}
