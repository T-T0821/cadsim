//! 部品から役割を探す・既定値で作る・設定に書き込む・外す・足りない設定を見つける
//!
//! 役割の実体はタイミングチャート ([`TimingChart`]) とワーク搬送の設定 ([`ConveyorConfig`]) の行。
//! ここではその行と部品を結びつける。

use glam::DVec3;

use super::model::{RoleBody, RoleDraft, RoleKind, RoleRef};
use crate::conveyor::{BeltAxis, ConveyorConfig, Gripper, SourceOutput};
use crate::math::Aabb;
use crate::model::NodeId;
use crate::motion::chart::{Action, Actuator, ActuatorKind, Axis, Sensor, Step, TimingChart};

/// コンベアの既定の速さ (mm/s)。ベルトが短ければ 1 秒で長さの半分まで
const CONVEYOR_SPEED_MM: f64 = 200.0;
/// 光電センサーの既定の光軸の長さ (mm)
const PHOTO_RANGE_MM: f64 = 300.0;
/// 近接センサーの既定の検出距離 (mm)
const PROXIMITY_RANGE_MM: f64 = 20.0;
/// グリッパの既定の掴める距離 (mm)
const GRIPPER_REACH_MM: f64 = 20.0;

/// 部品 `part` に付いている役割
pub fn roles_of(part: NodeId, chart: &TimingChart, conveyors: &ConveyorConfig) -> Vec<RoleRef> {
    let mut roles = Vec::new();
    for a in &chart.actuators {
        if a.parts.first() == Some(&part) {
            roles.push(RoleRef::Actuator(a.id));
        }
    }
    for s in &chart.sensors {
        if s.part == Some(part) {
            roles.push(RoleRef::Sensor(s.id));
        }
    }
    for c in &conveyors.conveyors {
        if c.part == Some(part) {
            roles.push(RoleRef::Conveyor(c.id));
        }
    }
    for s in &conveyors.sources {
        if s.part == Some(part) {
            roles.push(RoleRef::Source(s.id));
        }
    }
    for s in &conveyors.sinks {
        if s.part == Some(part) {
            roles.push(RoleRef::Sink(s.id));
        }
    }
    for g in &conveyors.grippers {
        if gripper_part(g, chart) == Some(part) {
            roles.push(RoleRef::Gripper(g.id));
        }
    }
    roles
}

/// グリッパの部品 (開閉するシリンダが動かす最初の部品)
fn gripper_part(gripper: &Gripper, chart: &TimingChart) -> Option<NodeId> {
    let chuck = chart.actuator(gripper.chuck?)?;
    chuck.parts.first().copied()
}

/// 部品を動かしているアクチュエータ (グリッパの開閉に使う)
pub fn moving_actuator(part: NodeId, chart: &TimingChart) -> Option<u32> {
    chart
        .actuators
        .iter()
        .find(|a| a.parts.contains(&part))
        .map(|a| a.id)
}

/// 役割の今の中身 (調整を始めるときの写し)
pub fn body_of(role: RoleRef, chart: &TimingChart, conveyors: &ConveyorConfig) -> Option<RoleBody> {
    Some(match role {
        RoleRef::Actuator(id) => RoleBody::Actuator(chart.actuator(id)?.clone()),
        RoleRef::Sensor(id) => RoleBody::Sensor(chart.sensor(id)?.clone()),
        RoleRef::Conveyor(id) => {
            RoleBody::Conveyor(conveyors.conveyors.iter().find(|c| c.id == id)?.clone())
        }
        RoleRef::Source(id) => {
            RoleBody::Source(conveyors.sources.iter().find(|s| s.id == id)?.clone())
        }
        RoleRef::Sink(id) => RoleBody::Sink(conveyors.sinks.iter().find(|s| s.id == id)?.clone()),
        RoleRef::Gripper(id) => {
            RoleBody::Gripper(conveyors.grippers.iter().find(|g| g.id == id)?.clone())
        }
    })
}

/// 部品の寸法から決めた既定値で、新しい役割の下書きを作る。
/// グリッパは部品を動かすアクチュエータがないと作れない (None)
pub fn new_draft(
    kind: RoleKind,
    part: NodeId,
    part_name: &str,
    bounds: Aabb,
    units_per_mm: f64,
    chart: &TimingChart,
) -> Option<RoleDraft> {
    let size = bounds.size();
    let center = bounds.center();
    let body = if let Some(actuator_kind) = kind.actuator_kind() {
        let mut a = Actuator::new(0, actuator_kind);
        a.name = format!("{part_name} ({})", kind.label());
        a.parts = vec![part];
        match actuator_kind {
            ActuatorKind::Cylinder => {
                // 最も長い向きに、部品の長さの半分
                a.axis = longest_axis(size);
                a.stroke = axis_len(size, a.axis) * 0.5;
            }
            ActuatorKind::LinearAxis => {
                a.axis = longest_axis(size);
                a.limits = Some([0.0, axis_len(size, a.axis)]);
            }
            ActuatorKind::RotaryTable => {
                a.axis = Axis::Y;
                a.pivot = center.to_array();
                a.limits = None;
            }
        }
        RoleBody::Actuator(a)
    } else if let Some(sensor_kind) = kind.sensor_kind() {
        let mut s = Sensor::new(0, sensor_kind);
        s.name = format!("{part_name} ({})", kind.label());
        s.part = Some(part);
        // 部品の前 (+Z の面の中心) から前へ
        s.axis = Axis::Z;
        s.reversed = false;
        s.position = [center.x, center.y, bounds.max.z];
        s.range = units_per_mm
            * match sensor_kind {
                crate::motion::chart::SensorKind::Photoelectric => PHOTO_RANGE_MM,
                crate::motion::chart::SensorKind::Proximity => PROXIMITY_RANGE_MM,
            };
        s.mounted_on = moving_actuator(part, chart);
        RoleBody::Sensor(s)
    } else {
        match kind {
            RoleKind::Conveyor => {
                let mut c = crate::conveyor::Conveyor::new(0);
                c.name = format!("{part_name} (コンベア)");
                c.part = Some(part);
                // 水平の長い辺の向きに流す
                c.axis = if size.x >= size.z {
                    BeltAxis::X
                } else {
                    BeltAxis::Z
                };
                // 0.2 m/s。小さな部品では、1 秒で長さの半分まで
                let along = c.along(size);
                c.speed = (CONVEYOR_SPEED_MM * units_per_mm).min(along * 0.5);
                RoleBody::Conveyor(c)
            }
            RoleKind::Source => {
                let mut s = crate::conveyor::Source::new(0);
                s.name = format!("{part_name} (投入口)");
                s.part = Some(part);
                RoleBody::Source(s)
            }
            RoleKind::Sink => {
                let mut s = crate::conveyor::Sink::new(0);
                s.name = format!("{part_name} (排出口)");
                s.part = Some(part);
                RoleBody::Sink(s)
            }
            RoleKind::Gripper => {
                let mut g = Gripper::new(0);
                g.name = format!("{part_name} (グリッパ)");
                g.chuck = Some(moving_actuator(part, chart)?);
                // 部品の下面の中心で掴む
                g.tcp = [center.x, bounds.min.y, center.z];
                g.reach = GRIPPER_REACH_MM * units_per_mm;
                RoleBody::Gripper(g)
            }
            _ => unreachable!("アクチュエータとセンサーは上で扱う"),
        }
    };
    Some(RoleDraft {
        part,
        bounds,
        units_per_mm,
        body,
        existing: None,
    })
}

/// 下書きを設定に書き込む。新しいシリンダには「出 → 戻」の 2 ステップも足す。書いた行を返す
pub fn apply(
    draft: &RoleDraft,
    chart: &mut TimingChart,
    conveyors: &mut ConveyorConfig,
) -> RoleRef {
    if let Some(existing) = draft.existing {
        write_existing(existing, &draft.body, chart, conveyors);
        return existing;
    }
    match &draft.body {
        RoleBody::Actuator(a) => {
            let id = chart.add_actuator(a.kind);
            if let Some(slot) = chart.actuator_mut(id) {
                *slot = Actuator { id, ..a.clone() };
            }
            if a.kind == ActuatorKind::Cylinder {
                for (target, word) in [(1.0, "出"), (0.0, "戻")] {
                    chart.steps.push(Step {
                        comment: format!("{} {word}", a.name),
                        actions: vec![Action {
                            actuator: id,
                            target,
                            duration: a.default_duration,
                        }],
                        ..Step::default()
                    });
                }
            }
            RoleRef::Actuator(id)
        }
        RoleBody::Sensor(s) => {
            let id = chart.add_sensor(s.kind);
            if let Some(slot) = chart.sensor_mut(id) {
                *slot = Sensor { id, ..s.clone() };
            }
            RoleRef::Sensor(id)
        }
        RoleBody::Conveyor(c) => {
            let slot = conveyors.add_conveyor();
            let id = slot.id;
            *slot = crate::conveyor::Conveyor { id, ..c.clone() };
            RoleRef::Conveyor(id)
        }
        RoleBody::Source(s) => {
            let slot = conveyors.add_source();
            let id = slot.id;
            *slot = crate::conveyor::Source { id, ..s.clone() };
            RoleRef::Source(id)
        }
        RoleBody::Sink(s) => {
            let slot = conveyors.add_sink();
            let id = slot.id;
            *slot = crate::conveyor::Sink { id, ..s.clone() };
            RoleRef::Sink(id)
        }
        RoleBody::Gripper(g) => {
            let slot = conveyors.add_gripper();
            let id = slot.id;
            *slot = Gripper { id, ..g.clone() };
            RoleRef::Gripper(id)
        }
    }
}

fn write_existing(
    role: RoleRef,
    body: &RoleBody,
    chart: &mut TimingChart,
    conveyors: &mut ConveyorConfig,
) {
    match (role, body) {
        (RoleRef::Actuator(id), RoleBody::Actuator(a)) => {
            if let Some(slot) = chart.actuator_mut(id) {
                *slot = Actuator { id, ..a.clone() };
            }
        }
        (RoleRef::Sensor(id), RoleBody::Sensor(s)) => {
            if let Some(slot) = chart.sensor_mut(id) {
                *slot = Sensor { id, ..s.clone() };
            }
        }
        (RoleRef::Conveyor(id), RoleBody::Conveyor(c)) => {
            if let Some(slot) = conveyors.conveyors.iter_mut().find(|x| x.id == id) {
                *slot = crate::conveyor::Conveyor { id, ..c.clone() };
            }
        }
        (RoleRef::Source(id), RoleBody::Source(s)) => {
            if let Some(slot) = conveyors.sources.iter_mut().find(|x| x.id == id) {
                *slot = crate::conveyor::Source { id, ..s.clone() };
            }
        }
        (RoleRef::Sink(id), RoleBody::Sink(s)) => {
            if let Some(slot) = conveyors.sinks.iter_mut().find(|x| x.id == id) {
                *slot = crate::conveyor::Sink { id, ..s.clone() };
            }
        }
        (RoleRef::Gripper(id), RoleBody::Gripper(g)) => {
            if let Some(slot) = conveyors.grippers.iter_mut().find(|x| x.id == id) {
                *slot = Gripper { id, ..g.clone() };
            }
        }
        _ => {}
    }
}

/// 役割を外す (行を消す。その行を使っていた動作・条件・つなぎも外れる)
pub fn remove(role: RoleRef, chart: &mut TimingChart, conveyors: &mut ConveyorConfig) {
    match role {
        RoleRef::Actuator(id) => {
            chart.remove_actuator(id);
            // 動作がなくなったステップは消す (シリンダの「出 → 戻」など)
            chart
                .steps
                .retain(|s| !s.actions.is_empty() || !s.moves.is_empty() || s.dwell > 0.0);
            for g in &mut conveyors.grippers {
                if g.chuck == Some(id) {
                    g.chuck = None;
                }
            }
        }
        RoleRef::Sensor(id) => chart.remove_sensor(id),
        RoleRef::Conveyor(id) => conveyors.remove_conveyor(id),
        RoleRef::Source(id) => conveyors.sources.retain(|s| s.id != id),
        RoleRef::Sink(id) => conveyors.sinks.retain(|s| s.id != id),
        RoleRef::Gripper(id) => conveyors.grippers.retain(|g| g.id != id),
    }
}

/// 役割の一行の説明 (プロパティに出す)
pub fn summary(body: &RoleBody, unit: &str) -> String {
    match body {
        RoleBody::Actuator(a) => {
            let dir = direction_label(a.axis, a.reversed);
            match a.kind {
                ActuatorKind::Cylinder => format!("{dir} 向きにストローク {} {unit}", r3(a.stroke)),
                ActuatorKind::LinearAxis => match a.limits {
                    Some([lo, hi]) => format!("{dir} 向きに {}〜{} {unit}", r3(lo), r3(hi)),
                    None => format!("{dir} 向き"),
                },
                ActuatorKind::RotaryTable => format!("{} 軸まわり", a.axis.label()),
            }
        }
        RoleBody::Sensor(s) => format!(
            "{} 向きに {} {unit}",
            direction_label(s.axis, s.reversed),
            r3(s.range)
        ),
        RoleBody::Conveyor(c) => {
            let axis = match c.axis {
                BeltAxis::X => Axis::X,
                BeltAxis::Z => Axis::Z,
            };
            format!(
                "{} 向きに {} {unit}/s",
                direction_label(axis, c.reversed),
                r3(c.speed)
            )
        }
        RoleBody::Source(s) => match s.output {
            SourceOutput::Conveyor(_) => format!("{} 秒ごとにコンベアへ", r3(s.interval)),
            SourceOutput::OnPart => format!("{} 秒ごとに部品の上へ", r3(s.interval)),
        },
        RoleBody::Sink(_) => "触れたワークを数える".into(),
        RoleBody::Gripper(g) => format!("掴める距離 {} {unit}", r3(g.reach)),
    }
}

/// 足りない設定・使われていない設定 (3D の部品に「!」を出す理由)
pub fn issues(role: RoleRef, chart: &TimingChart, conveyors: &ConveyorConfig) -> Vec<String> {
    let mut out = Vec::new();
    match role {
        RoleRef::Actuator(id) => {
            let Some(a) = chart.actuator(id) else {
                return out;
            };
            if a.kind == ActuatorKind::Cylinder && a.stroke <= 0.0 {
                out.push("ストロークが 0 です".into());
            }
            let used = chart.steps.iter().any(|s| s.action(id).is_some())
                || chart
                    .robots
                    .iter()
                    .any(|r| chart.robot_actuators(r).contains(&id));
            if !used {
                out.push("タイミングチャートで動かしていません".into());
            }
        }
        RoleRef::Sensor(id) => {
            if chart.sensor(id).is_some_and(|s| s.range <= 0.0) {
                out.push("検出範囲が 0 です".into());
            }
        }
        RoleRef::Conveyor(id) => {
            if conveyors
                .conveyors
                .iter()
                .any(|c| c.id == id && c.speed <= 0.0)
            {
                out.push("速さが 0 です".into());
            }
        }
        RoleRef::Source(id) => {
            if conveyors
                .sources
                .iter()
                .any(|s| s.id == id && s.interval <= 0.0)
            {
                out.push("出す間隔が 0 です".into());
            }
        }
        RoleRef::Sink(_) => {}
        RoleRef::Gripper(id) => {
            if conveyors
                .grippers
                .iter()
                .any(|g| g.id == id && g.chuck.is_none())
            {
                out.push("開閉するシリンダがありません".into());
            }
        }
    }
    out
}

/// 役割を付けた部品のうち、足りない設定があるもの (部品ごとに理由をまとめる)
pub fn parts_with_issues(
    chart: &TimingChart,
    conveyors: &ConveyorConfig,
) -> Vec<(NodeId, Vec<String>)> {
    let mut rows: Vec<(NodeId, RoleRef)> = Vec::new();
    rows.extend(
        chart
            .actuators
            .iter()
            .filter_map(|a| Some((*a.parts.first()?, RoleRef::Actuator(a.id)))),
    );
    rows.extend(
        chart
            .sensors
            .iter()
            .filter_map(|s| Some((s.part?, RoleRef::Sensor(s.id)))),
    );
    rows.extend(
        conveyors
            .conveyors
            .iter()
            .filter_map(|c| Some((c.part?, RoleRef::Conveyor(c.id)))),
    );
    rows.extend(
        conveyors
            .sources
            .iter()
            .filter_map(|s| Some((s.part?, RoleRef::Source(s.id)))),
    );
    rows.extend(
        conveyors
            .grippers
            .iter()
            .filter_map(|g| Some((gripper_part(g, chart)?, RoleRef::Gripper(g.id)))),
    );
    let mut out: Vec<(NodeId, Vec<String>)> = Vec::new();
    for (part, role) in rows {
        let found = issues(role, chart, conveyors);
        if found.is_empty() {
            continue;
        }
        match out.iter_mut().find(|(p, _)| *p == part) {
            Some((_, list)) => list.extend(found),
            None => out.push((part, found)),
        }
    }
    out
}

/// "+X" "-Z" のような向きの文字
pub fn direction_label(axis: Axis, reversed: bool) -> String {
    format!("{}{}", if reversed { "-" } else { "+" }, axis.label())
}

pub(crate) fn longest_axis(size: DVec3) -> Axis {
    if size.x >= size.y && size.x >= size.z {
        Axis::X
    } else if size.y >= size.z {
        Axis::Y
    } else {
        Axis::Z
    }
}

pub(crate) fn axis_len(size: DVec3, axis: Axis) -> f64 {
    size.dot(DVec3::from_array(axis.unit_vector())).abs()
}

pub(crate) fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Aabb {
        Aabb {
            min: DVec3::new(0.0, 0.0, 0.0),
            max: DVec3::new(400.0, 50.0, 100.0),
        }
    }

    fn draft(kind: RoleKind, chart: &TimingChart) -> Option<RoleDraft> {
        new_draft(kind, 7, "台", bounds(), 1.0, chart)
    }

    #[test]
    fn cylinder_defaults_to_half_of_longest_side_and_adds_out_and_back() {
        let mut chart = TimingChart::default();
        let mut conveyors = ConveyorConfig::default();
        let d = draft(RoleKind::Cylinder, &chart).unwrap();
        let RoleBody::Actuator(a) = &d.body else {
            panic!()
        };
        assert_eq!(a.axis, Axis::X);
        assert_eq!(a.stroke, 200.0);
        assert_eq!(a.parts, vec![7]);

        let role = apply(&d, &mut chart, &mut conveyors);
        let RoleRef::Actuator(id) = role else {
            panic!()
        };
        assert_eq!(chart.steps.len(), 2);
        assert_eq!(chart.steps[0].action(id).unwrap().target, 1.0);
        assert_eq!(chart.steps[1].action(id).unwrap().target, 0.0);
        assert_eq!(roles_of(7, &chart, &conveyors), vec![role]);
        assert!(issues(role, &chart, &conveyors).is_empty());

        // 外すと、動作がなくなったステップも消える
        remove(role, &mut chart, &mut conveyors);
        assert!(chart.actuators.is_empty());
        assert!(chart.steps.is_empty());
        assert!(roles_of(7, &chart, &conveyors).is_empty());
    }

    #[test]
    fn conveyor_runs_along_longer_horizontal_side_at_200mm_per_s() {
        let chart = TimingChart::default();
        let d = new_draft(RoleKind::Conveyor, 7, "台", bounds(), 0.001, &chart).unwrap();
        let RoleBody::Conveyor(c) = &d.body else {
            panic!()
        };
        assert_eq!(c.axis, BeltAxis::X);
        assert!((c.speed - 0.2).abs() < 1e-12, "m の図面なら 0.2 m/s");
    }

    #[test]
    fn sensor_points_forward_from_front_face() {
        let chart = TimingChart::default();
        let d = draft(RoleKind::Photoelectric, &chart).unwrap();
        let RoleBody::Sensor(s) = &d.body else {
            panic!()
        };
        assert_eq!(s.position, [200.0, 25.0, 100.0]);
        assert_eq!((s.axis, s.reversed, s.range), (Axis::Z, false, 300.0));
        assert_eq!(s.part, Some(7));
    }

    #[test]
    fn gripper_needs_an_actuator_moving_the_part() {
        let mut chart = TimingChart::default();
        let mut conveyors = ConveyorConfig::default();
        assert!(draft(RoleKind::Gripper, &chart).is_none());
        let cyl = apply(
            &draft(RoleKind::Cylinder, &chart).unwrap(),
            &mut chart,
            &mut conveyors,
        );
        let g = draft(RoleKind::Gripper, &chart).unwrap();
        let role = apply(&g, &mut chart, &mut conveyors);
        assert_eq!(roles_of(7, &chart, &conveyors), vec![cyl, role]);

        // 開閉のシリンダを外すと、グリッパに「!」が出る
        remove(cyl, &mut chart, &mut conveyors);
        assert_eq!(
            issues(role, &chart, &conveyors),
            vec!["開閉するシリンダがありません".to_string()]
        );
    }

    #[test]
    fn adjusting_existing_role_overwrites_only_that_row() {
        let mut chart = TimingChart::default();
        let mut conveyors = ConveyorConfig::default();
        let role = apply(
            &draft(RoleKind::Conveyor, &chart).unwrap(),
            &mut chart,
            &mut conveyors,
        );
        let mut d = draft(RoleKind::Conveyor, &chart).unwrap();
        d.existing = Some(role);
        d.body = body_of(role, &chart, &conveyors).unwrap();
        if let RoleBody::Conveyor(c) = &mut d.body {
            c.speed = 123.0;
        }
        assert_eq!(apply(&d, &mut chart, &mut conveyors), role);
        assert_eq!(conveyors.conveyors.len(), 1);
        assert_eq!(conveyors.conveyors[0].speed, 123.0);
        assert_eq!(summary(&d.body, "mm"), "+X 向きに 123 mm/s");
    }

    #[test]
    fn unused_cylinder_is_flagged() {
        let mut chart = TimingChart::default();
        let mut conveyors = ConveyorConfig::default();
        let role = apply(
            &draft(RoleKind::Cylinder, &chart).unwrap(),
            &mut chart,
            &mut conveyors,
        );
        assert!(parts_with_issues(&chart, &conveyors).is_empty());
        chart.steps.clear();
        assert_eq!(
            issues(role, &chart, &conveyors),
            vec!["タイミングチャートで動かしていません".to_string()]
        );
        assert_eq!(
            parts_with_issues(&chart, &conveyors),
            vec![(7, vec!["タイミングチャートで動かしていません".to_string()])]
        );
    }
}
