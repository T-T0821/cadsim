//! 読み込んだ URDF のロボットに、タイミングチャートの関節とロボットを付ける
//!
//! ファイルを読むのは `file_io` (形状はモデルへ、関節の木は [`ImportJobs::robots`] へ)。
//! ここでは、モデルに置いたリンクの位置を集めて、ドメインの [`robot_import::add_robot`] を呼ぶだけ。

use std::collections::HashMap;

use bevy::prelude::*;

use super::TimingChart;
use super::ui::ChartUiState;
use crate::file_io::{ImportJobs, ImportedRobot};
use crate::interchange::model_io::meters_per_unit;
use crate::model::Model;
use cadsim_core::motion::robot_import::{self, AddedRobot, PlacedLink};

/// 読み込んだばかりのロボットの関節をチャートに足し、ロボットの欄で選ぶ
pub fn attach_imported_robots(
    mut jobs: ResMut<ImportJobs>,
    model: Res<Model>,
    mut chart: ResMut<TimingChart>,
    mut chart_state: ResMut<ChartUiState>,
) {
    if jobs.robots.is_empty() {
        return;
    }
    for imported in std::mem::take(&mut jobs.robots) {
        let name = imported.description.name.clone();
        match attach(&model, &mut chart, &imported) {
            Ok(added) => {
                chart_state.selected_robot = Some(added.robot);
                jobs.messages.push(format!(
                    "ロボット「{name}」の関節 {} 軸をタイミングチャートに加えました",
                    added.joints.len()
                ));
                for warning in added.warnings {
                    jobs.messages.push(format!("{name}: {warning}"));
                }
            }
            Err(e) => jobs.messages.push(format!("{name}: {e}")),
        }
    }
}

/// モデルに置いたロボット `imported` の関節をチャートに足す
pub fn attach(
    model: &Model,
    chart: &mut TimingChart,
    imported: &ImportedRobot,
) -> Result<AddedRobot, String> {
    let links = placed_links(model, imported.root);
    robot_import::add_robot(
        chart,
        &imported.description,
        &links,
        1.0 / meters_per_unit(&model.unit),
    )
}

/// ロボットのいちばん上のノードの子 (リンクごとのノード) の名前 → 置いた位置
fn placed_links(model: &Model, root: u32) -> HashMap<String, PlacedLink> {
    let Some(root) = model.get(root) else {
        return HashMap::new();
    };
    root.children
        .iter()
        .filter_map(|&id| {
            let node = model.get(id)?;
            Some((
                node.name.clone(),
                PlacedLink {
                    // 形状のないリンク (tool0 など) は部品として動かさない
                    node: (!node.children.is_empty()).then_some(id),
                    frame: model.world_matrix(id),
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use glam::DVec3;

    use super::*;
    use crate::file_io::load_source;
    use crate::interchange::UnitOptions;
    use crate::model::StepSource;
    use cadsim_core::motion::kinematics;

    /// 同梱の UR5e の URDF をモデルに置き、関節を付ける
    fn ur5e() -> (Model, TimingChart, AddedRobot) {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/real_robot/ur5e_urdf/ur5e.urdf");
        let source = StepSource::File(path);
        let loaded = load_source(&source, &UnitOptions::default()).unwrap();
        let description = loaded.robot().unwrap().clone();
        let file = loaded.into_imported("ur5e.urdf", "mm");
        assert!(file.warnings.is_empty(), "{:?}", file.warnings);
        let mut model = Model::default();
        let root = model.add_imported(file.root, file.unit, source);
        let mut chart = TimingChart::default();
        let added = attach(&model, &mut chart, &ImportedRobot { root, description }).unwrap();
        (model, chart, added)
    }

    #[test]
    fn ur5e_urdf_becomes_a_six_axis_robot() {
        let (model, chart, added) = ur5e();
        assert!(added.warnings.is_empty(), "{:?}", added.warnings);
        assert_eq!(added.joints.len(), 6);
        // どの関節もリンクの形状を動かす
        for id in &added.joints {
            let a = chart.actuator(*id).unwrap();
            assert!(!a.parts.is_empty(), "{}", a.name);
            assert!(a.parts.iter().all(|p| model.get(*p).is_some()));
        }
        let robot = chart.robot(added.robot).unwrap();
        assert_eq!(robot.name, "UR5e");
        assert_eq!(robot.speed.joint, 180.0);
        // 全関節 0 は腕が水平に伸びた姿勢。tool0 は ROS の base_link で (0.8172, 0.2329, 0.0628) m
        // (x = 0.425 + 0.3922、y = 0.1333 + 0.0996、z = 0.1625 - 0.0997)。アプリでは Y が上なので (x, z, -y)
        let tcp = DVec3::from_array(robot.tcp);
        let expected = DVec3::new(817.2, 62.8, -232.9);
        assert!((tcp - expected).length() < 0.5, "{tcp}");
    }

    #[test]
    fn imported_ur5e_reaches_points_with_ik() {
        let (_, chart, added) = ur5e();
        let robot = chart.robot(added.robot).unwrap().clone();
        // 床の上 400 mm、ベースから 500 mm 先へ、工具を真下に向けて
        let target = DVec3::new(500.0, 400.0, 100.0);
        let solution =
            kinematics::solve_ik_to(&chart, &robot, target, Some(DVec3::NEG_Y), &HashMap::new());
        assert!(solution.reached, "{solution:?}");
        let tcp = kinematics::tcp_for(&chart, &robot, &solution.values);
        assert!((tcp.position - target).length() < 1.0);
        // 腕の届かない遠くの点は届かないと分かる
        let far = kinematics::solve_ik_to(
            &chart,
            &robot,
            DVec3::new(3000.0, 400.0, 0.0),
            None,
            &HashMap::new(),
        );
        assert!(!far.reached);
    }
}
