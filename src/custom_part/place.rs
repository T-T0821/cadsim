//! 自作部品をモデルに置く
//!
//! 部品の木を最上位に加え、アクチュエータ・センサーを新しい id でタイミングチャートに足す。
//! 名前には「<置いた部品の名前>/」を頭に付けて、同じ部品を何個置いてもぶつからないようにする。

use std::collections::HashMap;

use bevy::math::{DMat4, DVec3};

use super::file::CustomPart;
use super::param::TargetOnChart;
use crate::project::LoadedProject;
use plantwright_core::custom_part::{CustomInstance, PartParam};
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::motion::chart::TimingChart;
use plantwright_core::motion::record::chart_from_record;

/// 置いた結果
pub struct Placed {
    pub root: NodeId,
    pub warnings: Vec<String>,
}

/// 同じ自作部品がいくつ置いてあるか数えて、「<部品名> <番号>」の名前を作る
pub fn instance_name(model: &Model, part: &str) -> String {
    let count = model
        .all_nodes()
        .into_iter()
        .filter(|&id| {
            model
                .get(id)
                .and_then(|n| n.custom_part.as_ref())
                .is_some_and(|c| c.part == part)
        })
        .count();
    format!("{part} {}", count + 1)
}

/// `loaded` (読み込み済みの `part.project`) の部品を `at` に置く
pub fn place(
    model: &mut Model,
    chart: &mut TimingChart,
    part: &CustomPart,
    loaded: &LoadedProject,
    at: DVec3,
) -> Option<Placed> {
    let name = instance_name(model, &part.name);
    let prefix = format!("{name}/");
    let (roots, mut warnings) = loaded.add_nodes(model);
    let root = *roots.first()?;
    if part.project.unit != model.unit {
        warnings.push(format!(
            "自作部品「{}」は {} で作られています (いまのモデルは {})。大きさが合わないかもしれません",
            part.name, part.project.unit, model.unit
        ));
    }
    if let Some(node) = model.get_mut(root) {
        node.name = name.clone();
        node.placement = DMat4::from_translation(at) * node.placement;
    }

    // アクチュエータとセンサーを、新しい id と名前でチャートに足す
    let mut actuator_ids = HashMap::new();
    let mut sensor_ids = HashMap::new();
    if let Some(record) = &part.project.chart {
        let find = |path: &[usize]| {
            let (&first, rest) = path.split_first()?;
            if first != 0 {
                return None;
            }
            let mut id = root;
            for &i in rest {
                id = *model.get(id)?.children.get(i)?;
            }
            Some(id)
        };
        let (sub, chart_warnings) = chart_from_record(record, find);
        warnings.extend(chart_warnings);
        for a in &sub.actuators {
            let id = chart.add_actuator(a.kind);
            actuator_ids.insert(a.id, id);
        }
        for a in sub.actuators {
            let id = actuator_ids[&a.id];
            if let Some(slot) = chart.actuator_mut(id) {
                *slot = plantwright_core::motion::chart::Actuator {
                    id,
                    name: format!("{prefix}{}", a.name),
                    pivot: (DVec3::from(a.pivot) + at).to_array(),
                    mounted_on: a.mounted_on.and_then(|m| actuator_ids.get(&m).copied()),
                    ..a
                };
            }
        }
        for s in &sub.sensors {
            let id = chart.add_sensor(s.kind);
            sensor_ids.insert(s.id, id);
        }
        for s in sub.sensors {
            let id = sensor_ids[&s.id];
            if let Some(slot) = chart.sensors.iter_mut().find(|x| x.id == id) {
                *slot = plantwright_core::motion::chart::Sensor {
                    id,
                    name: format!("{prefix}{}", s.name),
                    position: (DVec3::from(s.position) + at).to_array(),
                    mounted_on: s.mounted_on.and_then(|m| actuator_ids.get(&m).copied()),
                    ..s
                };
            }
        }
    }

    let params: Vec<PartParam> = part
        .params
        .iter()
        .filter_map(|p| {
            let target = p.target.remap(&actuator_ids, &sensor_ids)?;
            target.write(chart, p.value);
            Some(PartParam {
                label: p.label.clone(),
                target,
                value: p.value,
            })
        })
        .collect();
    // 中に入れ子になった自作部品の名前の頭と、パラメータの id を付け替える
    for id in model.subtree(root).into_iter().skip(1) {
        let Some(inner) = model.get_mut(id).and_then(|n| n.custom_part.as_mut()) else {
            continue;
        };
        inner.prefix = format!("{prefix}{}", inner.prefix);
        inner.params = std::mem::take(&mut inner.params)
            .into_iter()
            .filter_map(|p| {
                let target = p.target.remap(&actuator_ids, &sensor_ids)?;
                Some(PartParam { target, ..p })
            })
            .collect();
    }
    if let Some(node) = model.get_mut(root) {
        node.custom_part = Some(CustomInstance {
            part: part.name.clone(),
            prefix,
            params,
        });
    }
    Some(Placed { root, warnings })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::custom_part::file::{SaveRequest, build};
    use crate::custom_part::param::InstanceOnChart;
    use crate::project::ProjectFile;
    use plantwright_core::custom_part::ParamTarget;
    use plantwright_core::model::BoxPart;
    use plantwright_core::model::NodeKind;
    use plantwright_core::motion::chart::ActuatorKind;
    use plantwright_core::motion::chart::SensorKind;

    fn cube(model: &mut Model, parent: NodeId, name: &str, x: f64) -> NodeId {
        let part = BoxPart {
            size: [10.0; 3],
            position: [0.0; 3],
            rotation_deg: [0.0; 3],
        };
        model.add_node(
            Some(parent),
            name.into(),
            DMat4::from_translation(DVec3::X * x),
            NodeKind::Box(part),
        )
    }

    /// (100, 0, 50) に置いたプッシャー (本体 + ロッド) と、ロッドを動かすシリンダ・センサー。
    /// 関係ない箱とシリンダも1つずつある。
    fn sample() -> (Model, TimingChart, NodeId, u32) {
        let mut model = Model::default();
        let other = model.add_box([10.0; 3], [-200.0, 0.0, 0.0]);
        let root = model.add_node(
            None,
            "プッシャー".into(),
            DMat4::from_translation(DVec3::new(100.0, 0.0, 50.0)),
            NodeKind::Assembly { product: None },
        );
        cube(&mut model, root, "本体", 0.0);
        let rod = cube(&mut model, root, "ロッド", 20.0);
        model.get_mut(root).unwrap().script = Some(r#"move_joint("シリンダ", 1);"#.into());

        let mut chart = TimingChart::default();
        let cyl = chart.add_actuator(ActuatorKind::Cylinder);
        let a = chart.actuator_mut(cyl).unwrap();
        a.name = "シリンダ".into();
        a.parts = vec![rod];
        a.pivot = [120.0, 0.0, 50.0];
        a.stroke = 30.0;
        let unrelated = chart.add_actuator(ActuatorKind::Cylinder);
        chart.actuator_mut(unrelated).unwrap().parts = vec![other];
        let sensor = chart.add_sensor(SensorKind::Proximity);
        let s = chart.sensors.iter_mut().find(|s| s.id == sensor).unwrap();
        s.name = "出端".into();
        s.mounted_on = Some(cyl);
        s.position = [150.0, 0.0, 50.0];
        (model, chart, root, cyl)
    }

    fn save_sample() -> CustomPart {
        let (model, chart, root, cyl) = sample();
        let request = SaveRequest {
            name: " プッシャー ".into(),
            category: "治具".into(),
            description: "押し出し用".into(),
            params: vec![("ストローク".into(), ParamTarget::Stroke(cyl))],
        };
        let part = build(&model, &chart, root, &request).unwrap();
        // JSON にしても同じものに戻る
        let back = CustomPart::from_json(&part.to_json()).unwrap();
        assert_eq!(back, part);
        back
    }

    #[test]
    fn saved_part_keeps_only_its_own_actuators_relative_to_its_origin() {
        let part = save_sample();
        assert_eq!(part.name, "プッシャー");
        assert_eq!(part.params.len(), 1);
        assert_eq!(part.params[0].value, 30.0);
        let loaded = LoadedProject::from_file(part.project.clone(), Path::new("."));
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        let placed = place(&mut model, &mut chart, &part, &loaded, DVec3::ZERO).unwrap();
        assert_eq!(chart.actuators.len(), 1);
        assert_eq!(chart.actuators[0].pivot, [20.0, 0.0, 0.0]);
        assert_eq!(chart.sensors[0].position, [50.0, 0.0, 0.0]);
        assert_eq!(model.get(placed.root).unwrap().children.len(), 2);
        assert!(model.get(placed.root).unwrap().script.is_some());
    }

    #[test]
    fn placing_twice_gives_separate_names_ids_and_params() {
        let part = save_sample();
        let loaded = LoadedProject::from_file(part.project.clone(), Path::new("."));
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        let at = DVec3::new(0.0, 0.0, 500.0);
        let first = place(&mut model, &mut chart, &part, &loaded, DVec3::ZERO).unwrap();
        let second = place(&mut model, &mut chart, &part, &loaded, at).unwrap();

        let names: Vec<_> = chart.actuators.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["プッシャー 1/シリンダ", "プッシャー 2/シリンダ"]);
        assert_eq!(model.get(second.root).unwrap().name, "プッシャー 2");
        let (a1, a2) = (chart.actuators[0].id, chart.actuators[1].id);
        assert_ne!(a1, a2);
        assert_eq!(chart.actuators[1].pivot, [20.0, 0.0, 500.0]);
        // 動かす部品は、それぞれ自分の木のロッド
        let rod_of = |root: NodeId| model.get(root).unwrap().children[1];
        assert_eq!(chart.actuators[0].parts, vec![rod_of(first.root)]);
        assert_eq!(chart.actuators[1].parts, vec![rod_of(second.root)]);
        assert_eq!(chart.sensors[1].mounted_on, Some(a2));
        assert_eq!(chart.sensors[1].name, "プッシャー 2/出端");

        // パラメータは自分のシリンダだけを変える
        let mut instance = model.get(second.root).unwrap().custom_part.clone().unwrap();
        assert_eq!(instance.prefix, "プッシャー 2/");
        assert_eq!(instance.params[0].target, ParamTarget::Stroke(a2));
        assert!(instance.set(&mut chart, 0, 80.0));
        assert_eq!(chart.actuator(a1).unwrap().stroke, 30.0);
        assert_eq!(chart.actuator(a2).unwrap().stroke, 80.0);
        assert_eq!(instance.params[0].value, 80.0);

        // スクリプトは自分の部品の名前の頭で動く
        let scopes: Vec<_> = crate::scripting::scripts_of(&model)
            .into_iter()
            .map(|s| s.scope)
            .collect();
        assert_eq!(scopes, ["プッシャー 1/", "プッシャー 2/"]);
    }

    #[test]
    fn resaving_an_instance_drops_its_name_prefix() {
        let part = save_sample();
        let loaded = LoadedProject::from_file(part.project.clone(), Path::new("."));
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        let placed = place(&mut model, &mut chart, &part, &loaded, DVec3::ZERO).unwrap();
        let request = SaveRequest {
            name: "プッシャー改".into(),
            category: String::new(),
            description: String::new(),
            params: Vec::new(),
        };
        let again = build(&model, &chart, placed.root, &request).unwrap();
        let record = again.project.chart.as_ref().unwrap();
        let (sub, _) = chart_from_record(record, |_| Some(0));
        assert_eq!(sub.actuators[0].name, "シリンダ");
        assert_eq!(sub.sensors[0].name, "出端");
    }

    #[test]
    fn instance_survives_project_save() {
        let part = save_sample();
        let loaded = LoadedProject::from_file(part.project.clone(), Path::new("."));
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        let placed = place(&mut model, &mut chart, &part, &loaded, DVec3::ZERO).unwrap();
        let before = model.get(placed.root).unwrap().custom_part.clone();

        let json = ProjectFile::from_model(&model, None).to_json();
        let file = ProjectFile::from_json(&json).unwrap();
        let mut restored = Model::default();
        LoadedProject::from_file(file, Path::new(".")).apply_to(&mut restored);
        let root = restored.roots[0];
        assert_eq!(restored.get(root).unwrap().custom_part, before);
    }

    #[test]
    fn saved_parts_are_listed_by_category_and_name() {
        let dir = std::env::temp_dir().join(format!("plantwright_parts_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut part = save_sample();
        crate::custom_part::file::save(&part, &dir).unwrap();
        part.name = "A/B".into();
        part.category = "あ".into();
        let path = crate::custom_part::file::save(&part, &dir).unwrap();
        assert!(path.ends_with("A_B/part.cadpart"));
        let list = crate::custom_part::file::scan(&dir);
        let names: Vec<_> = list
            .iter()
            .map(|e| (e.category.as_str(), e.name.as_str()))
            .collect();
        assert_eq!(names, [("あ", "A/B"), ("治具", "プッシャー")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
