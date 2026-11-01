//! 物理演算のサンプルライン: ベルトコンベア → ストッパ → シュート → ベルトコンベア → 回収箱
//!
//! ```text
//!   ワーク 100 個                ストッパ (シリンダで上下)
//!   ▣▣▣▣▣▣▣▣▣▣▣▣ → → → → → → ┃
//!  ════════ コンベア1 ═══════════╗
//!                                 ╲ シュート
//!                                  ╲
//!                                   ════ コンベア2 ════ → ┌回収箱┐
//! ```
//!
//! 寸法は mm。ワークは「物理で動く」、コンベアとシュートは「動かない」(コンベアはベルト付き)、
//! ストッパは「機構で動く」。ストッパの上下はタイミングチャートで繰り返す。
//! ワークはベルトに少しめり込んで載るので、干渉チェックの自動実行は切っておく。

use bevy::math::DMat4;

use plantwright_core::model::BoxPart;
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::model::NodeKind;
use plantwright_core::motion::chart::Action;
use plantwright_core::motion::chart::ActuatorKind;
use plantwright_core::motion::chart::Axis;
use plantwright_core::motion::chart::Step;
use plantwright_core::motion::chart::TimingChart;
use plantwright_core::physics::cable::{Cable, CableAnchor, CableKind};
use plantwright_core::physics::settings::{BeltDrive, BodyMode, LocalAxis, PartPhysics};

/// サンプルのワークの数
pub const WORK_COUNT: usize = 100;
/// コンベアの角度 (度)
const CHUTE_ANGLE: f64 = 25.0;

/// モデルとタイミングチャートをサンプルラインに置き換える
pub fn load_sample_line(model: &mut Model, chart: &mut TimingChart) {
    model.clear();
    model.unit = "mm".into();
    *chart = TimingChart::default();

    let part = |model: &mut Model,
                parent: Option<NodeId>,
                name: &str,
                size: [f64; 3],
                position: [f64; 3],
                rotation_deg: [f64; 3],
                physics: PartPhysics,
                color: [f32; 3]| {
        let kind = NodeKind::Box(BoxPart {
            size,
            position,
            rotation_deg,
        });
        let id = model.add_node(parent, name.into(), DMat4::IDENTITY, kind);
        if let Some(node) = model.get_mut(id) {
            node.physics = physics;
            node.color = Some(color);
        }
        id
    };
    let fixed = PartPhysics {
        mode: BodyMode::Fixed,
        ..Default::default()
    };
    let belt = |speed: f64| PartPhysics {
        belt: Some(BeltDrive {
            axis: LocalAxis::X,
            reversed: false,
            speed,
        }),
        friction: 0.8,
        ..fixed
    };
    let frame = [0.55, 0.58, 0.62];
    let belt_color = [0.18, 0.20, 0.22];
    let guide = [0.75, 0.78, 0.80];

    let line = model.add_node(
        None,
        "搬送ライン".into(),
        DMat4::IDENTITY,
        NodeKind::Assembly { product: None },
    );
    let p = Some(line);
    part(
        model,
        p,
        "床",
        [7600.0, 20.0, 2000.0],
        [1300.0, -10.0, 0.0],
        [0.0; 3],
        fixed,
        [0.35, 0.37, 0.40],
    );

    // コンベア1: 上面 y = 730、x = -1200..1200
    part(
        model,
        p,
        "コンベア1",
        [2400.0, 60.0, 400.0],
        [0.0, 700.0, 0.0],
        [0.0; 3],
        belt(300.0),
        belt_color,
    );
    for z in [-207.5, 207.5] {
        part(
            model,
            p,
            "コンベア1 ガイド",
            [2400.0, 120.0, 15.0],
            [0.0, 790.0, z],
            [0.0; 3],
            fixed,
            guide,
        );
    }
    part(
        model,
        p,
        "コンベア1 架台",
        [2300.0, 670.0, 300.0],
        [0.0, 335.0, 0.0],
        [0.0; 3],
        fixed,
        frame,
    );

    // ストッパ: 上がっていればワークを止め、70 mm 下がるとベルト面より下に隠れる
    let stopper_physics = PartPhysics {
        mode: BodyMode::Mechanism,
        ..Default::default()
    };
    let stopper = part(
        model,
        p,
        "ストッパ",
        [20.0, 60.0, 400.0],
        [900.0, 760.0, 0.0],
        [0.0; 3],
        stopper_physics,
        [0.95, 0.55, 0.15],
    );

    // シュート: コンベア1 の端 (x = 1200, y = 725) から 25° で下る
    let (sin, cos) = CHUTE_ANGLE.to_radians().sin_cos();
    let length = 900.0;
    let surface_center = [1200.0 + cos * length / 2.0, 725.0 - sin * length / 2.0];
    let along_normal = |d: f64| [surface_center[0] + sin * d, surface_center[1] + cos * d];
    let center = along_normal(-10.0);
    let tilt = [0.0, 0.0, -CHUTE_ANGLE];
    let chute = PartPhysics {
        friction: 0.15,
        ..fixed
    };
    part(
        model,
        p,
        "シュート",
        [length, 20.0, 400.0],
        [center[0], center[1], 0.0],
        tilt,
        chute,
        guide,
    );
    let rail = along_normal(40.0);
    for z in [-207.5, 207.5] {
        part(
            model,
            p,
            "シュート ガイド",
            [length, 80.0, 15.0],
            [rail[0], rail[1], z],
            tilt,
            chute,
            guide,
        );
    }

    // コンベア2: 上面 y = 340、x = 2000..3500
    part(
        model,
        p,
        "コンベア2",
        [1500.0, 60.0, 400.0],
        [2750.0, 310.0, 0.0],
        [0.0; 3],
        belt(400.0),
        belt_color,
    );
    for z in [-207.5, 207.5] {
        part(
            model,
            p,
            "コンベア2 ガイド",
            [1500.0, 120.0, 15.0],
            [2750.0, 400.0, z],
            [0.0; 3],
            fixed,
            guide,
        );
    }
    part(
        model,
        p,
        "コンベア2 架台",
        [1400.0, 280.0, 300.0],
        [2750.0, 140.0, 0.0],
        [0.0; 3],
        fixed,
        frame,
    );

    // 回収箱
    let bin = [0.30, 0.55, 0.35];
    part(
        model,
        p,
        "回収箱 底",
        [1200.0, 20.0, 900.0],
        [4150.0, 10.0, 0.0],
        [0.0; 3],
        fixed,
        bin,
    );
    for x in [3560.0, 4740.0] {
        part(
            model,
            p,
            "回収箱 壁",
            [20.0, 250.0, 900.0],
            [x, 145.0, 0.0],
            [0.0; 3],
            fixed,
            bin,
        );
    }
    for z in [-440.0, 440.0] {
        part(
            model,
            p,
            "回収箱 壁",
            [1200.0, 250.0, 20.0],
            [4150.0, 145.0, z],
            [0.0; 3],
            fixed,
            bin,
        );
    }

    // ワーク: コンベア1 の始端の上に 12 × 3 列、3 段で並べ、落として流す
    let works = model.add_node(
        None,
        "ワーク".into(),
        DMat4::IDENTITY,
        NodeKind::Assembly { product: None },
    );
    let dynamic = PartPhysics {
        mode: BodyMode::Dynamic,
        friction: 0.6,
        ..Default::default()
    };
    for i in 0..WORK_COUNT {
        let (layer, rest) = (i / 36, i % 36);
        let (row, column) = (rest / 3, rest % 3);
        let position = [
            -1100.0 + row as f64 * 90.0,
            770.0 + layer as f64 * 70.0,
            -120.0 + column as f64 * 120.0,
        ];
        let name = format!("ワーク {}", i + 1);
        part(
            model,
            Some(works),
            &name,
            [60.0, 50.0, 60.0],
            position,
            [0.0; 3],
            dynamic,
            [0.95, 0.75, 0.25],
        );
    }

    // ストッパへのエアホース (ストッパが下がると引かれる)
    model.cables.push(Cable {
        name: "ストッパのエアホース".into(),
        kind: CableKind::Hose,
        start: CableAnchor {
            node: Some(stopper),
            local: [900.0, 790.0, 0.0].into(),
        },
        end: CableAnchor {
            node: None,
            local: [1100.0, 1300.0, 0.0].into(),
        },
        length: 700.0,
        diameter: 8.0,
        // φ8 のウレタンチューブの最小曲げ半径の目安
        min_bend_radius: 20.0,
    });

    // ストッパを 1.2 秒下げてワークを通し、3 秒上げて溜める (サイクルの終わりは上げた原点)
    let cylinder = chart.add_actuator(ActuatorKind::Cylinder);
    if let Some(a) = chart.actuator_mut(cylinder) {
        a.name = "ストッパ".into();
        a.axis = Axis::Y;
        a.reversed = true;
        a.stroke = 70.0;
        a.parts = vec![stopper];
    }
    let step = |comment: &str, target: f64, dwell: f64| Step {
        comment: comment.into(),
        actions: vec![Action {
            actuator: cylinder,
            target,
            duration: 0.3,
        }],
        dwell,
        ..Default::default()
    };
    chart.steps = vec![
        step("ストッパ下げ (ワークを通す)", 1.0, 1.2),
        step("ストッパ上げ (ワークを溜める)", 0.0, 3.0),
    ];
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::physics::engine::{RapierEngine, headless_app};
    use bevy::app::App;
    use bevy::math::DVec3;
    use plantwright_core::math::euler_deg_to_quat;
    use plantwright_core::physics::settings::{gravity, meters_per_unit};
    use plantwright_core::physics::world::{
        BodyKey, Iso, PartGeometry, PartInput, PhysicsEngine, PhysicsRun, part_body,
    };

    /// サンプルラインの部品を、アプリと同じ手順で剛体にする
    fn sample_world() -> (Model, App, PhysicsRun, Vec<NodeId>) {
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        load_sample_line(&mut model, &mut chart);
        let mut app = headless_app(gravity(&model.unit), meters_per_unit(&model.unit));
        let mut engine = RapierEngine::new(app.world_mut());
        let mut works = Vec::new();
        for id in super::super::all_parts(&model) {
            let node = model.get(id).unwrap();
            let NodeKind::Box(part) = &node.kind else {
                unreachable!()
            };
            let input = PartInput {
                settings: node.physics,
                geometry: PartGeometry::Box {
                    size: DVec3::from(part.size),
                    offset: Iso::new(
                        euler_deg_to_quat(part.rotation_deg),
                        DVec3::from(part.position),
                    ),
                },
                pose: Iso::IDENTITY,
                meters_per_unit: 0.001,
            };
            let desc = part_body(&input).unwrap();
            assert!(engine.add_body(BodyKey::Part(id), &desc));
            if node.physics.mode == BodyMode::Dynamic {
                works.push(id);
            }
        }
        (model, app, PhysicsRun::new(), works)
    }

    /// ワークの中心 (箱の位置 + 剛体の姿勢)
    fn work_center(model: &Model, app: &mut App, id: NodeId) -> DVec3 {
        let NodeKind::Box(part) = &model.get(id).unwrap().kind else {
            unreachable!()
        };
        RapierEngine::new(app.world_mut())
            .pose(BodyKey::Part(id))
            .unwrap()
            .apply(DVec3::from(part.position))
    }

    #[test]
    fn sample_has_a_hundred_works_and_a_stopper_cycle() {
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        load_sample_line(&mut model, &mut chart);
        let works = super::super::all_parts(&model)
            .into_iter()
            .filter(|id| model.get(*id).unwrap().physics.mode == BodyMode::Dynamic)
            .count();
        assert_eq!(works, WORK_COUNT);
        assert_eq!(chart.steps.len(), 2);
        assert_eq!(model.cables.len(), 1);
    }

    /// ストッパを下げたまま 40 秒流すと、ワークはシュートとコンベア2 を通って回収箱に入る。
    /// 100 個を 1 フレーム (60 fps = 16.7 ms) の中で十分速く計算できることも確かめる。
    #[test]
    fn works_flow_into_the_bin() {
        let (model, mut app, mut run, works) = sample_world();
        let stopper = super::super::all_parts(&model)
            .into_iter()
            .find(|id| model.get(*id).unwrap().name == "ストッパ")
            .unwrap();
        let down = Iso::from_translation(DVec3::new(0.0, -70.0, 0.0));
        let frames = 40 * 60;
        let started = Instant::now();
        for _ in 0..frames {
            run.advance(
                &mut RapierEngine::new(app.world_mut()),
                1.0 / 60.0,
                &[(BodyKey::Part(stopper), down)],
            );
        }
        let per_frame_ms = started.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
        println!(
            "ワーク {} 個: 1 フレームあたり {per_frame_ms:.2} ms",
            works.len()
        );

        let mut in_bin = 0;
        for &id in &works {
            let c = work_center(&model, &mut app, id);
            assert!(c.y > -100.0, "ワークが床をすり抜けた: {c}");
            if c.x > 3560.0 && c.x < 4740.0 && c.z.abs() < 440.0 && c.y < 400.0 {
                in_bin += 1;
            }
        }
        println!("回収箱に入ったワーク: {in_bin} 個");
        assert!(
            in_bin >= WORK_COUNT * 9 / 10,
            "回収箱に入ったのは {in_bin} 個"
        );
        // デバッグビルドでも 1 フレームの物理は 60 fps の予算 (16.7 ms) に収まる
        if !cfg!(debug_assertions) {
            assert!(per_frame_ms < 16.7);
        }
    }

    /// ストッパが上がっていれば、ワークはストッパの手前で止まる
    #[test]
    fn the_raised_stopper_holds_the_works() {
        let (model, mut app, mut run, works) = sample_world();
        for _ in 0..(10 * 60) {
            run.advance(&mut RapierEngine::new(app.world_mut()), 1.0 / 60.0, &[]);
        }
        for &id in &works {
            let c = work_center(&model, &mut app, id);
            assert!(c.x < 900.0, "ストッパを越えた: {c}");
        }
        let near_stopper = works
            .iter()
            .filter(|&&id| work_center(&model, &mut app, id).x > 700.0)
            .count();
        println!("ストッパの前に溜まったワーク: {near_stopper} 個");
        assert!(
            near_stopper >= 3,
            "ストッパの前に溜まったのは {near_stopper} 個"
        );
    }

    /// ストッパのエアホースは、ストッパが上がっていても下がっていても無理なく垂れる
    #[test]
    fn the_hose_is_fine_in_both_stopper_positions() {
        use plantwright_core::physics::cable::{CableState, Obstacle};
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        load_sample_line(&mut model, &mut chart);
        let hose = model.cables[0].clone();
        let stopper = hose.start.node.unwrap();
        for drop in [0.0, -70.0] {
            let moved = Iso::from_translation(DVec3::new(0.0, drop, 0.0));
            let obstacles: Vec<Obstacle> = super::super::all_parts(&model)
                .into_iter()
                .map(|id| {
                    let NodeKind::Box(part) = &model.get(id).unwrap().kind else {
                        unreachable!()
                    };
                    let shift = if id == stopper { drop } else { 0.0 };
                    Obstacle {
                        node: id,
                        pose: Iso::new(
                            euler_deg_to_quat(part.rotation_deg),
                            DVec3::from(part.position) + DVec3::new(0.0, shift, 0.0),
                        ),
                        half: DVec3::from(part.size) / 2.0,
                    }
                })
                .collect();
            let start = moved.apply(hose.start.local);
            let end = hose.end.local;
            let mut state = CableState::new(&hose, start, end);
            for _ in 0..600 {
                state.step(&hose, start, end, &obstacles, gravity("mm"), 1.0 / 120.0);
            }
            let report = state.report(&hose, &obstacles);
            assert!(report.is_ok(), "ストッパ {drop}: {report:?}");
        }
    }
}
