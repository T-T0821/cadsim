//! 標準部品の形状 (B-rep)
//!
//! 部品は複数の本体 (body) からできている。アクチュエータが動かすのは本体単位なので、
//! シリンダのロッドやチャックの爪は別の本体にしてある。並び順は [`catalog::body`] と合わせる。

use std::f64::consts::FRAC_PI_2;

use glam::{DMat4, DVec3};

use crate::kernel::Brep;

use super::catalog::{PartKind, Spec};

/// 本体1つ分の形状と色
pub struct Body {
    pub name: &'static str,
    /// 部品ローカル座標 (mm) のソリッド
    pub solid: Brep,
    /// 表示色 (sRGB)
    pub color: [f32; 3],
}

const ALUMINUM: [f32; 3] = [0.78, 0.80, 0.83];
const STEEL: [f32; 3] = [0.62, 0.64, 0.68];
const DARK: [f32; 3] = [0.22, 0.23, 0.25];
const BLACK: [f32; 3] = [0.10, 0.10, 0.11];
const PANEL_BEIGE: [f32; 3] = [0.86, 0.85, 0.80];
const PLATE_ORANGE: [f32; 3] = [0.93, 0.62, 0.30];
const SENSOR_ORANGE: [f32; 3] = [0.95, 0.55, 0.12];
const BELT_GREEN: [f32; 3] = [0.20, 0.38, 0.28];
const MODULE_GRAY: [f32; 3] = [0.55, 0.57, 0.60];
const TOWER: [[f32; 3]; 5] = [
    [0.90, 0.15, 0.12],
    [0.98, 0.80, 0.10],
    [0.15, 0.75, 0.25],
    [0.15, 0.40, 0.90],
    [0.95, 0.95, 0.95],
];

/// 軸の向き
#[derive(Clone, Copy)]
enum Dir {
    X,
    Y,
    Z,
}

/// mm で指定した直方体 (min..max)
fn cuboid(min: [f64; 3], max: [f64; 3]) -> Brep {
    let (min, max) = (DVec3::from_array(min), DVec3::from_array(max));
    Brep::cuboid(max - min)
        .and_then(|b| b.transformed(DMat4::from_translation(min)))
        .expect("直方体は作れる")
}

/// mm で指定した円柱。`start` は底面の中心、`dir` の向きに `length` 伸ばす。
fn cylinder(start: [f64; 3], dir: Dir, radius: f64, length: f64) -> Brep {
    // 円柱は +Z 向きに作るので、`dir` に向ける
    let rotation = match dir {
        Dir::Z => DMat4::IDENTITY,
        Dir::X => DMat4::from_rotation_y(FRAC_PI_2),
        Dir::Y => DMat4::from_rotation_x(-FRAC_PI_2),
    };
    Brep::cylinder_z(radius, length)
        .and_then(|b| b.transformed(DMat4::from_translation(DVec3::from_array(start)) * rotation))
        .expect("円柱は作れる")
}

fn body(name: &'static str, solid: Brep, color: [f32; 3]) -> Body {
    Body { name, solid, color }
}

/// 部品の本体をすべて作る (部品ローカル座標、原点は取付け面の中心)
pub fn bodies(spec: &Spec) -> Vec<Body> {
    let g = |key| spec.get(key);
    match spec.kind {
        PartKind::AirCylinder => {
            let bore = g("bore");
            let tube = bore + 12.0;
            let len = 60.0 + bore;
            let foot = 10.0;
            let cy = foot + tube / 2.0;
            let rod_r = (bore * 0.19).max(3.0);
            let rod_out = 25.0;
            vec![
                body(
                    "チューブ",
                    cuboid([0.0, foot, -tube / 2.0], [len, foot + tube, tube / 2.0]),
                    ALUMINUM,
                ),
                body(
                    "フート",
                    cuboid(
                        [-5.0, 0.0, -tube / 2.0 - 10.0],
                        [len + 5.0, foot, tube / 2.0 + 10.0],
                    ),
                    STEEL,
                ),
                body(
                    "ロッド",
                    cylinder([len - 5.0, cy, 0.0], Dir::X, rod_r, rod_out + 5.0),
                    STEEL,
                ),
                body(
                    "ロッド先端",
                    cuboid(
                        [len + rod_out, cy - rod_r * 1.5, -rod_r * 1.5],
                        [len + rod_out + rod_r * 2.5, cy + rod_r * 1.5, rod_r * 1.5],
                    ),
                    DARK,
                ),
            ]
        }
        PartKind::Gripper => {
            let w = g("width");
            let h = w * 0.6;
            let d = w * 0.6;
            let jaw_w = w * 0.18;
            let jaw_h = w * 0.5;
            let jaw = |x0: f64| cuboid([x0, h, -d / 4.0], [x0 + jaw_w, h + jaw_h, d / 4.0]);
            vec![
                body(
                    "本体",
                    cuboid([-w / 2.0, 0.0, -d / 2.0], [w / 2.0, h, d / 2.0]),
                    ALUMINUM,
                ),
                body("爪 (左)", jaw(-w / 2.0 + 2.0), STEEL),
                body("爪 (右)", jaw(w / 2.0 - 2.0 - jaw_w), STEEL),
            ]
        }
        PartKind::Motor => {
            let f = g("frame");
            let l = g("length");
            let r = g("shaft") / 2.0;
            let shaft_len = r * 6.0;
            let x0 = l + 8.0;
            vec![
                body("本体", cuboid([0.0, 0.0, -f / 2.0], [l, f, f / 2.0]), DARK),
                body(
                    "フランジ",
                    cuboid([l, 0.0, -f / 2.0], [x0, f, f / 2.0]),
                    ALUMINUM,
                ),
                body(
                    "出力軸",
                    cylinder([x0, f / 2.0, 0.0], Dir::X, r, shaft_len),
                    STEEL,
                ),
                body(
                    "キー",
                    cuboid(
                        [x0 + r, f / 2.0 + r * 0.6, -r * 0.3],
                        [x0 + shaft_len - r * 0.5, f / 2.0 + r * 1.2, r * 0.3],
                    ),
                    PLATE_ORANGE,
                ),
            ]
        }
        PartKind::BeltConveyor => {
            let (l, w, h) = (g("length"), g("width"), g("height"));
            let (hl, hw) = (l / 2.0, w / 2.0);
            let roller_r = 25.0;
            let mut list = vec![
                body(
                    "ベルト",
                    cuboid([-hl, h - 2.0 * roller_r, -hw], [hl, h, hw]),
                    BELT_GREEN,
                ),
                body(
                    "フレーム (奥)",
                    cuboid([-hl, h - 80.0, -hw - 30.0], [hl, h + 15.0, -hw]),
                    ALUMINUM,
                ),
                body(
                    "フレーム (手前)",
                    cuboid([-hl, h - 80.0, hw], [hl, h + 15.0, hw + 30.0]),
                    ALUMINUM,
                ),
                body(
                    "駆動プーリ",
                    cylinder([hl, h - roller_r, -hw], Dir::Z, roller_r, w),
                    STEEL,
                ),
                body(
                    "従動プーリ",
                    cylinder([-hl, h - roller_r, -hw], Dir::Z, roller_r, w),
                    STEEL,
                ),
            ];
            let leg_x = (hl - 60.0).max(20.0);
            for x in [-leg_x, leg_x] {
                for z in [-hw - 15.0, hw + 15.0] {
                    list.push(body(
                        "脚",
                        cuboid([x - 20.0, 0.0, z - 20.0], [x + 20.0, h - 80.0, z + 20.0]),
                        ALUMINUM,
                    ));
                }
            }
            list
        }
        PartKind::PhotoSensor => vec![
            body(
                "本体",
                cuboid([-15.0, 0.0, -10.0], [15.0, 50.0, 10.0]),
                SENSOR_ORANGE,
            ),
            body(
                "レンズ",
                cylinder([15.0, 25.0, 0.0], Dir::X, 6.0, 3.0),
                BLACK,
            ),
        ],
        PartKind::ProximitySensor => {
            let r = g("diameter") / 2.0;
            let cy = r + 3.0;
            vec![
                body("ねじ部", cylinder([0.0, cy, 0.0], Dir::X, r, 50.0), STEEL),
                body(
                    "ブラケット",
                    cuboid([15.0, 0.0, -r - 6.0], [20.0, 2.0 * r + 6.0, r + 6.0]),
                    ALUMINUM,
                ),
                body(
                    "ケーブル",
                    cylinder([-20.0, cy, 0.0], Dir::X, r * 0.35, 20.0),
                    BLACK,
                ),
            ]
        }
        PartKind::Plc => {
            let modules = |key| (spec.get(key).max(0.0) / 16.0).ceil() as usize;
            let io_modules = modules("inputs") + modules("outputs");
            let cpu_w = 60.0;
            let total = cpu_w + 28.0 * io_modules as f64;
            let mut list = vec![
                body(
                    "DIN レール",
                    cuboid([-10.0, 30.0, -7.5], [total + 10.0, 65.0, 0.0]),
                    STEEL,
                ),
                body(
                    "CPU",
                    cuboid([0.0, 0.0, 0.0], [cpu_w, 98.0, 90.0]),
                    PANEL_BEIGE,
                ),
            ];
            for i in 0..io_modules {
                let x = cpu_w + 28.0 * i as f64;
                list.push(body(
                    "I/O ユニット",
                    cuboid([x + 0.5, 0.0, 0.0], [x + 27.5, 98.0, 85.0]),
                    MODULE_GRAY,
                ));
            }
            list
        }
        PartKind::RemoteIo => vec![
            body(
                "本体",
                cuboid([-50.0, 0.0, -25.0], [50.0, 60.0, 25.0]),
                MODULE_GRAY,
            ),
            body(
                "端子台",
                cuboid([-46.0, 60.0, -18.0], [46.0, 75.0, 18.0]),
                BLACK,
            ),
        ],
        PartKind::PowerSupply => {
            let w = (30.0 + g("watts") / 8.0).min(150.0);
            vec![
                body(
                    "本体",
                    cuboid([-w / 2.0, 0.0, 0.0], [w / 2.0, 125.0, 100.0]),
                    STEEL,
                ),
                body(
                    "端子台",
                    cuboid([-w / 2.0 + 3.0, 10.0, 100.0], [w / 2.0 - 3.0, 30.0, 110.0]),
                    BLACK,
                ),
            ]
        }
        PartKind::Relay => vec![
            body(
                "ソケット",
                cuboid([-15.0, 0.0, -20.0], [15.0, 15.0, 20.0]),
                DARK,
            ),
            body(
                "リレー",
                cuboid([-11.0, 15.0, -14.0], [11.0, 50.0, 14.0]),
                [0.70, 0.78, 0.86],
            ),
        ],
        PartKind::ControlPanel => {
            let (w, h, d) = (g("width"), g("height"), g("depth"));
            let (hw, hd, t) = (w / 2.0, d / 2.0, 3.0);
            vec![
                body(
                    "背板",
                    cuboid([-hw, 0.0, -hd], [hw, h, -hd + t]),
                    PANEL_BEIGE,
                ),
                body(
                    "側板 (左)",
                    cuboid([-hw, 0.0, -hd], [-hw + t, h, hd]),
                    PANEL_BEIGE,
                ),
                body(
                    "側板 (右)",
                    cuboid([hw - t, 0.0, -hd], [hw, h, hd]),
                    PANEL_BEIGE,
                ),
                body("天板", cuboid([-hw, h - t, -hd], [hw, h, hd]), PANEL_BEIGE),
                body("底板", cuboid([-hw, 0.0, -hd], [hw, t, hd]), PANEL_BEIGE),
                body(
                    "取付板",
                    cuboid(
                        [-hw + 30.0, 30.0, -hd + t],
                        [hw - 30.0, h - 30.0, -hd + t + 3.0],
                    ),
                    PLATE_ORANGE,
                ),
            ]
        }
        PartKind::PushButton => vec![
            body(
                "ボックス",
                cuboid([-35.0, 0.0, -35.0], [35.0, 70.0, 35.0]),
                [0.85, 0.80, 0.20],
            ),
            body(
                "ボタン",
                cylinder([0.0, 35.0, 35.0], Dir::Z, 15.0, 12.0),
                TOWER[2],
            ),
        ],
        PartKind::SignalTower => {
            let tiers = (g("tiers").round() as usize).clamp(1, TOWER.len());
            let mut list = vec![
                body("台座", cylinder([0.0, 0.0, 0.0], Dir::Y, 30.0, 20.0), BLACK),
                body(
                    "ポール",
                    cylinder([0.0, 20.0, 0.0], Dir::Y, 10.0, 120.0),
                    STEEL,
                ),
            ];
            for (i, color) in TOWER.iter().take(tiers).enumerate() {
                let y = 140.0 + 45.0 * i as f64;
                list.push(body(
                    "表示灯",
                    cylinder([0.0, y, 0.0], Dir::Y, 30.0, 44.0),
                    *color,
                ));
            }
            let top = 140.0 + 45.0 * tiers as f64;
            list.push(body(
                "キャップ",
                cylinder([0.0, top, 0.0], Dir::Y, 30.0, 10.0),
                DARK,
            ));
            list
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::catalog::body as idx;

    fn bbox(solid: &Brep) -> ([f64; 3], [f64; 3]) {
        let b = solid.bounds().unwrap();
        (b.min.to_array(), b.max.to_array())
    }

    #[test]
    fn every_kind_builds_closed_solids_above_the_floor() {
        for kind in PartKind::ALL {
            let spec = Spec::new(kind);
            let list = bodies(&spec);
            assert!(!list.is_empty(), "{kind:?}");
            for b in &list {
                assert!(b.solid.is_solid(), "{kind:?} {}", b.name);
                assert!(bbox(&b.solid).0[1] >= -1e-9, "{kind:?} {}", b.name);
            }
        }
    }

    #[test]
    fn actuator_bodies_exist() {
        for kind in PartKind::ALL {
            let spec = Spec::new(kind);
            let n = bodies(&spec).len();
            for act in spec.actuators() {
                assert!(act.bodies.iter().all(|&b| b < n), "{kind:?}");
            }
        }
    }

    #[test]
    fn named_body_indices_match() {
        let name = |kind: PartKind, i: usize| bodies(&Spec::new(kind))[i].name;
        assert_eq!(name(PartKind::AirCylinder, idx::CYL_ROD), "ロッド");
        assert_eq!(name(PartKind::AirCylinder, idx::CYL_ROD_END), "ロッド先端");
        assert_eq!(name(PartKind::Gripper, idx::GRIP_JAW_L), "爪 (左)");
        assert_eq!(name(PartKind::Gripper, idx::GRIP_JAW_R), "爪 (右)");
        assert_eq!(name(PartKind::Motor, idx::MOTOR_SHAFT), "出力軸");
        assert_eq!(name(PartKind::Motor, idx::MOTOR_KEY), "キー");
        assert_eq!(name(PartKind::BeltConveyor, idx::CONV_BELT), "ベルト");
    }

    #[test]
    fn conveyor_top_matches_height_param() {
        let mut spec = Spec::new(PartKind::BeltConveyor);
        spec.params.0[2] = 850.0;
        let belt = &bodies(&spec)[idx::CONV_BELT];
        assert!((bbox(&belt.solid).1[1] - 850.0).abs() < 1e-9);
        assert_eq!(spec.conveying_surface().unwrap().center[1], 850.0);
    }

    #[test]
    fn plc_grows_with_io_points() {
        let mut spec = Spec::new(PartKind::Plc);
        let small = bodies(&spec).len();
        spec.params.0 = vec![64.0, 64.0];
        assert_eq!(bodies(&spec).len(), small + 6);
    }
}
