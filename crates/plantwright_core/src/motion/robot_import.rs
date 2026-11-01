//! 読み込んだロボットの機種 (リンクと関節の木) を、タイミングチャートの関節とロボットにする
//!
//! URDF などのファイルの読み方は`crate::motion::urdf` が受け持ち、ここでは
//! ファイル形式によらない [`RobotDescription`] だけを扱う。
//!
//! - 回転関節 (revolute / continuous) は回転テーブル、直動関節 (prismatic) は直動軸にする。
//! - 固定の関節 (fixed) でつながったリンクは、根元側の関節と一緒に動かす。
//! - 関節の位置と向きは、モデルに置いたリンクの配置 (全関節 0 の設計位置) から求める。
//!   URDF では子リンクの座標系が関節の座標系と同じなので、子リンクの原点が回転の中心になる。
//! - 枝分かれ (2本指のグリッパなど) の手前までを腕とみなし、その先端の関節をロボットの先端にする。
//!   TCP は先端から固定の関節でつながったリンクのうち `tool0` などの名前のもの (なければいちばん先)。

use std::collections::HashMap;

use glam::{DMat4, DVec3};

use super::chart::{ActuatorKind, Axis, RobotSpeed, TimingChart};

/// 関節の種類 (URDF の type)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JointKind {
    Revolute,
    /// 回り続けられる回転関節 (可動範囲なし)
    Continuous,
    Prismatic,
    Fixed,
    /// 6 自由度。チャートでは動かせないので固定として扱う
    Floating,
    /// 平面内の 2 自由度。チャートでは動かせないので固定として扱う
    Planar,
}

impl JointKind {
    /// チャートの関節にするか
    pub fn is_movable(self) -> bool {
        matches!(
            self,
            JointKind::Revolute | JointKind::Continuous | JointKind::Prismatic
        )
    }
}

/// 関節 1 つ
#[derive(Clone, Debug, PartialEq)]
pub struct JointDescription {
    pub name: String,
    pub kind: JointKind,
    pub parent: String,
    pub child: String,
    /// 関節の座標系 (= 子リンクの座標系) での軸の向き
    pub axis: [f64; 3],
    /// 可動範囲 [下限, 上限] (回転はラジアン、直動はメートル)
    pub limits: Option<[f64; 2]>,
    /// 最高速度 (回転はラジアン/秒、直動はメートル/秒)
    pub velocity: Option<f64>,
    /// 別の関節に連動する (mimic) ときの、その関節の名前
    pub mimic: Option<String>,
}

/// ロボットの機種 (リンクの名前と関節の木)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RobotDescription {
    pub name: String,
    pub links: Vec<String>,
    pub joints: Vec<JointDescription>,
}

/// モデルに置いたリンク
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedLink {
    /// リンクの形状のノード。形状のないリンク (`tool0` など) は None
    pub node: Option<u32>,
    /// 設計位置 (全関節 0) でのリンクの座標系 (モデル座標)
    pub frame: DMat4,
}

/// チャートに加えたロボット
#[derive(Clone, Debug, PartialEq)]
pub struct AddedRobot {
    pub robot: u32,
    /// 加えた関節 (根元から)
    pub joints: Vec<u32>,
    /// 読み込めたが、そのままでは扱えなかったことの説明
    pub warnings: Vec<String>,
}

impl RobotDescription {
    /// 根元のリンク (どの関節の子でもないリンク)
    pub fn root_links(&self) -> Vec<&str> {
        self.links
            .iter()
            .filter(|l| !self.joints.iter().any(|j| &j.child == *l))
            .map(String::as_str)
            .collect()
    }

    fn child_joints<'a>(&'a self, link: &'a str) -> impl Iterator<Item = &'a JointDescription> {
        self.joints.iter().filter(move |j| j.parent == link)
    }

    /// `link` から先に動く関節があるか
    fn has_movable_below(&self, link: &str, depth: usize) -> bool {
        depth < self.joints.len()
            && self
                .child_joints(link)
                .any(|j| j.kind.is_movable() || self.has_movable_below(&j.child, depth + 1))
    }

    /// 腕の先端の関節: 根元から、動く関節のある枝が 1 本だけのあいだたどった最後の動く関節
    fn arm_tip<'a>(&'a self, root: &'a str) -> Option<&'a JointDescription> {
        let mut link = root;
        let mut tip = None;
        for _ in 0..=self.joints.len() {
            let branches: Vec<&JointDescription> = self
                .child_joints(link)
                .filter(|j| j.kind.is_movable() || self.has_movable_below(&j.child, 0))
                .collect();
            let [next] = branches[..] else { break };
            if next.kind.is_movable() {
                tip = Some(next);
            }
            link = &next.child;
        }
        tip
    }

    /// TCP にするリンク: `link` から固定の関節だけでつながったリンクのうち、名前で選ぶ
    fn tool_link<'a>(&'a self, link: &'a str) -> &'a str {
        // (リンク, 深さ)
        let mut reachable = vec![(link, 0usize)];
        let mut i = 0;
        while i < reachable.len() && reachable.len() <= self.links.len() {
            let (current, depth) = reachable[i];
            for joint in self.child_joints(current) {
                if !joint.kind.is_movable() {
                    reachable.push((&joint.child, depth + 1));
                }
            }
            i += 1;
        }
        let named = |pred: fn(&str) -> bool| -> Option<&'a str> {
            reachable
                .iter()
                .find(|(l, _)| pred(&l.to_ascii_lowercase()))
                .map(|(l, _)| *l)
        };
        named(|l| l == "tool0" || l.ends_with("_tool0") || l.ends_with("/tool0"))
            .or_else(|| named(|l| l.contains("tcp")))
            .or_else(|| named(|l| l.contains("tool")))
            .unwrap_or_else(|| {
                reachable
                    .iter()
                    .max_by_key(|(_, depth)| *depth)
                    .map_or(link, |(l, _)| *l)
            })
    }
}

/// 向き `v` が X / Y / Z のどれか (逆向きも含む) にそろっていれば、その軸と逆向きかを返す
fn principal_axis(v: DVec3) -> Option<(Axis, bool)> {
    let v = v.try_normalize()?;
    [Axis::X, Axis::Y, Axis::Z].into_iter().find_map(|axis| {
        let d = v.dot(DVec3::from_array(axis.unit_vector()));
        (d.abs() > 1.0 - 1e-9).then_some((axis, d < 0.0))
    })
}

/// 機種 `description` の関節とロボットをチャートに加える。
///
/// `links` はリンクの名前 → モデルに置いたリンク。`meters_to_model` は 1 メートルがモデルの何単位か
/// (直動関節の可動範囲と速度の換算に使う)。
pub fn add_robot(
    chart: &mut TimingChart,
    description: &RobotDescription,
    links: &HashMap<String, PlacedLink>,
    meters_to_model: f64,
) -> Result<AddedRobot, String> {
    let roots = description.root_links();
    let Some(&root) = roots.first() else {
        return Err("根元のリンクが見つかりません (関節がループしています)".into());
    };
    let mut warnings = Vec::new();
    if roots.len() > 1 {
        warnings.push(format!(
            "根元のリンクが {} 個あります。「{root}」につながる部分だけを関節にしました",
            roots.len()
        ));
    }
    if !description.joints.iter().any(|j| j.kind.is_movable()) {
        return Err("動く関節 (revolute / continuous / prismatic) がありません".into());
    }

    // リンク → そのリンクを動かすアクチュエータ (根元のリンクは動かない)
    let mut owner: HashMap<&str, Option<u32>> = HashMap::from([(root, None)]);
    let mut by_joint: HashMap<&str, u32> = HashMap::new();
    let mut added = Vec::new();
    // 根元から順にたどる (親のアクチュエータを先に作る)
    let mut queue = vec![root];
    let mut i = 0;
    while i < queue.len() && i <= description.links.len() {
        let link = queue[i];
        i += 1;
        let parent_owner = owner.get(link).copied().flatten();
        for joint in description.child_joints(link) {
            if owner.contains_key(joint.child.as_str()) {
                continue;
            }
            queue.push(&joint.child);
            let placed = links.get(&joint.child);
            if !joint.kind.is_movable() {
                if matches!(joint.kind, JointKind::Floating | JointKind::Planar) {
                    warnings.push(format!(
                        "関節「{}」({:?}) は動かせないので固定にしました",
                        joint.name, joint.kind
                    ));
                }
                owner.insert(&joint.child, parent_owner);
                if let (Some(id), Some(node)) = (parent_owner, placed.and_then(|p| p.node))
                    && let Some(a) = chart.actuator_mut(id)
                {
                    a.parts.push(node);
                }
                continue;
            }
            let Some(placed) = placed else {
                warnings.push(format!(
                    "関節「{}」の子リンク「{}」の位置が分からないので、読み飛ばしました",
                    joint.name, joint.child
                ));
                continue;
            };
            let kind = if joint.kind == JointKind::Prismatic {
                ActuatorKind::LinearAxis
            } else {
                ActuatorKind::RotaryTable
            };
            let id = chart.add_actuator(kind);
            let axis = placed
                .frame
                .transform_vector3(DVec3::from_array(joint.axis))
                .try_normalize()
                .unwrap_or(DVec3::Z);
            let a = chart.actuator_mut(id).expect("今加えた");
            a.name = joint.name.clone();
            match principal_axis(axis) {
                Some((principal, reversed)) => {
                    a.axis = principal;
                    a.reversed = reversed;
                }
                None => a.custom_axis = Some(axis.to_array()),
            }
            a.pivot = placed.frame.w_axis.truncate().to_array();
            a.mounted_on = parent_owner;
            a.parts = placed.node.into_iter().collect();
            a.limits = match (joint.kind, joint.limits) {
                (JointKind::Continuous, _) | (_, None) => None,
                (JointKind::Prismatic, Some([lo, hi])) => {
                    Some([lo * meters_to_model, hi * meters_to_model])
                }
                (_, Some([lo, hi])) => Some([lo.to_degrees(), hi.to_degrees()]),
            };
            if joint.mimic.is_some() {
                warnings.push(format!(
                    "関節「{}」は別の関節に連動 (mimic) しますが、独立した関節として読み込みました",
                    joint.name
                ));
            }
            owner.insert(&joint.child, Some(id));
            by_joint.insert(&joint.name, id);
            added.push(id);
        }
    }

    let tip_joint = description
        .arm_tip(root)
        .ok_or_else(|| "腕の関節が見つかりません".to_string())?;
    let tip = *by_joint
        .get(tip_joint.name.as_str())
        .ok_or_else(|| format!("先端の関節「{}」を作れませんでした", tip_joint.name))?;
    let tool = description.tool_link(&tip_joint.child);
    let frame = links
        .get(tool)
        .or_else(|| links.get(&tip_joint.child))
        .map_or(DMat4::IDENTITY, |p| p.frame);

    let robot = chart.add_robot(tip);
    // 回転関節の最高速度のうち、いちばん遅いものをロボットの関節速度にする
    let joint_speed = description
        .joints
        .iter()
        .filter(|j| by_joint.contains_key(j.name.as_str()) && j.kind != JointKind::Prismatic)
        .filter_map(|j| j.velocity.filter(|v| *v > 0.0))
        .map(f64::to_degrees)
        .reduce(f64::min);
    let r = chart.robot_mut(robot).expect("今加えた");
    if !description.name.is_empty() {
        r.name = description.name.clone();
    }
    r.tcp = frame.w_axis.truncate().to_array();
    r.tool_axis = frame
        .transform_vector3(DVec3::Z)
        .try_normalize()
        .unwrap_or(DVec3::Y)
        .to_array();
    r.speed = RobotSpeed {
        joint: joint_speed.unwrap_or(RobotSpeed::default().joint),
        ..RobotSpeed::default()
    };
    let joints = super::kinematics::joints(chart, chart.robot(robot).expect("今加えた"));
    Ok(AddedRobot {
        robot,
        joints,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::kinematics;
    use glam::DQuat;

    fn joint(name: &str, kind: JointKind, parent: &str, child: &str) -> JointDescription {
        JointDescription {
            name: name.into(),
            kind,
            parent: parent.into(),
            child: child.into(),
            axis: [0.0, 0.0, 1.0],
            limits: Some([-1.0, 1.0]),
            velocity: Some(std::f64::consts::PI),
            mimic: None,
        }
    }

    fn placed(node: Option<u32>, at: [f64; 3]) -> PlacedLink {
        PlacedLink {
            node,
            frame: DMat4::from_translation(DVec3::from_array(at)),
        }
    }

    /// 旋回 (Y 軸) + 肩 (Z 軸) の 2 軸アームに、固定の手先 (tool0) と 2 本指のグリッパ
    fn arm() -> (RobotDescription, HashMap<String, PlacedLink>) {
        let mut j1 = joint("j1", JointKind::Revolute, "base", "l1");
        j1.axis = [0.0, 1.0, 0.0];
        let j2 = joint("j2", JointKind::Continuous, "l1", "l2");
        let mut flange = joint("flange", JointKind::Fixed, "l2", "hand");
        flange.limits = None;
        let tool = joint("tool", JointKind::Fixed, "hand", "tool0");
        let mut left = joint("left", JointKind::Prismatic, "hand", "finger_l");
        left.axis = [-1.0, 0.0, 0.0];
        let mut right = joint("right", JointKind::Prismatic, "hand", "finger_r");
        right.mimic = Some("left".into());
        let description = RobotDescription {
            name: "テストアーム".into(),
            links: ["base", "l1", "l2", "hand", "tool0", "finger_l", "finger_r"]
                .map(String::from)
                .to_vec(),
            joints: vec![j1, j2, flange, tool, left, right],
        };
        let links = HashMap::from([
            ("base".into(), placed(Some(10), [0.0; 3])),
            ("l1".into(), placed(Some(11), [0.0, 100.0, 0.0])),
            ("l2".into(), placed(Some(12), [0.0, 200.0, 0.0])),
            ("hand".into(), placed(Some(13), [0.0, 700.0, 0.0])),
            ("tool0".into(), placed(None, [0.0, 800.0, 0.0])),
            ("finger_l".into(), placed(Some(14), [10.0, 750.0, 0.0])),
            ("finger_r".into(), placed(Some(15), [-10.0, 750.0, 0.0])),
        ]);
        (description, links)
    }

    #[test]
    fn joints_become_actuators_on_their_parents() {
        let (description, links) = arm();
        let mut chart = TimingChart::default();
        let added = add_robot(&mut chart, &description, &links, 1000.0).unwrap();
        let by_name = |n: &str| chart.actuators.iter().find(|a| a.name == n).unwrap();
        let j1 = by_name("j1");
        let j2 = by_name("j2");
        assert_eq!(
            (j1.kind, j1.axis, j1.reversed),
            (ActuatorKind::RotaryTable, Axis::Y, false)
        );
        assert_eq!(j1.limits, Some([-1f64.to_degrees(), 1f64.to_degrees()]));
        assert_eq!(j1.parts, vec![11]);
        assert_eq!(j2.mounted_on, Some(j1.id));
        assert_eq!(j2.pivot, [0.0, 200.0, 0.0]);
        // 回り続けられる関節は可動範囲なし
        assert_eq!(j2.limits, None);
        // 固定でつながった手は肩と一緒に動く
        assert_eq!(j2.parts, vec![12, 13]);
        let left = by_name("left");
        assert_eq!(
            (left.kind, left.axis, left.reversed),
            (ActuatorKind::LinearAxis, Axis::X, true)
        );
        // メートル → モデル単位 (mm)
        assert_eq!(left.limits, Some([-1000.0, 1000.0]));
        assert_eq!(left.mounted_on, Some(j2.id));
        // 枝分かれ (指) の手前の肩が先端。TCP は tool0
        let robot = chart.robot(added.robot).unwrap();
        assert_eq!(robot.tip, j2.id);
        assert_eq!(robot.name, "テストアーム");
        assert_eq!(robot.tcp, [0.0, 800.0, 0.0]);
        assert_eq!(robot.tool_axis, [0.0, 0.0, 1.0]);
        assert_eq!(robot.speed.joint, 180.0);
        assert_eq!(added.joints, vec![j1.id, j2.id]);
        assert!(added.warnings.iter().any(|w| w.contains("mimic")));
    }

    #[test]
    fn slanted_axes_use_a_custom_direction_and_ik_follows_them() {
        // 斜めの軸 (X と Y の間) まわりに回る 1 軸 + Z 軸の 1 軸
        let (mut description, mut links) = arm();
        description
            .joints
            .retain(|j| !j.name.starts_with("left") && j.name != "right");
        let tilt = DQuat::from_rotation_z(0.3);
        links.get_mut("l1").unwrap().frame =
            DMat4::from_rotation_translation(tilt, DVec3::new(0.0, 100.0, 0.0));
        let mut chart = TimingChart::default();
        let added = add_robot(&mut chart, &description, &links, 1000.0).unwrap();
        let j1 = chart.actuator(added.joints[0]).unwrap();
        let axis = DVec3::from_array(j1.custom_axis.expect("斜めの軸"));
        assert!((axis - tilt * DVec3::Y).length() < 1e-12);
        // 順運動学でも斜めの軸で回る
        let robot = chart.robot(added.robot).unwrap().clone();
        let values = HashMap::from([(added.joints[0], 30.0)]);
        let tcp = kinematics::tcp_for(&chart, &robot, &values);
        let expected = DVec3::new(0.0, 100.0, 0.0)
            + DQuat::from_axis_angle(tilt * DVec3::Y, 30f64.to_radians())
                * DVec3::new(0.0, 700.0, 0.0);
        assert!((tcp.position - expected).length() < 1e-9, "{tcp:?}");
    }

    #[test]
    fn descriptions_without_moving_joints_are_rejected() {
        let description = RobotDescription {
            name: "台".into(),
            links: vec!["a".into(), "b".into()],
            joints: vec![joint("f", JointKind::Fixed, "a", "b")],
        };
        let mut chart = TimingChart::default();
        assert!(add_robot(&mut chart, &description, &HashMap::new(), 1.0).is_err());
        assert!(chart.actuators.is_empty());
    }
}
