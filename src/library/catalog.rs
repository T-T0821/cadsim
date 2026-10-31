//! 標準部品のカタログ (Bevy 非依存のデータ)
//!
//! 部品ごとに次を宣言する。
//! - 寸法パラメータ (mm): 値を変えると形状を作り直す
//! - I/O 点: PLC から見た入力 (センサー → PLC) と出力 (PLC → ソレノイドなど)
//! - アクチュエータ動作: どの本体 (body) を、どの向きに、どれだけ動かすか
//! - センサーの検出範囲: 光軸の始点・向き・検出距離
//!
//! 座標は部品ローカル (取付け面の中心が原点、Y が上) の mm で持つ。
//! モデルの単位 (mm 以外のこともある) への換算は [`units_per_mm`] で行う。

/// モデルの長さの単位 ("mm" など) で 1 mm がいくつになるか
pub fn units_per_mm(unit: &str) -> f64 {
    match unit {
        "cm" => 0.1,
        "m" => 0.001,
        "inch" | "in" => 1.0 / 25.4,
        _ => 1.0,
    }
}

/// パレットでの分類
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Actuator,
    Conveying,
    Sensor,
    Control,
    Operator,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Actuator,
        Category::Conveying,
        Category::Sensor,
        Category::Control,
        Category::Operator,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Actuator => "アクチュエータ",
            Category::Conveying => "搬送",
            Category::Sensor => "センサー",
            Category::Control => "制御盤",
            Category::Operator => "操作・表示",
        }
    }
}

/// 部品の種類
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PartKind {
    AirCylinder,
    Gripper,
    Motor,
    BeltConveyor,
    PhotoSensor,
    ProximitySensor,
    Plc,
    RemoteIo,
    PowerSupply,
    Relay,
    ControlPanel,
    PushButton,
    SignalTower,
}

impl PartKind {
    pub const ALL: [PartKind; 13] = [
        PartKind::AirCylinder,
        PartKind::Gripper,
        PartKind::Motor,
        PartKind::BeltConveyor,
        PartKind::PhotoSensor,
        PartKind::ProximitySensor,
        PartKind::Plc,
        PartKind::RemoteIo,
        PartKind::PowerSupply,
        PartKind::Relay,
        PartKind::ControlPanel,
        PartKind::PushButton,
        PartKind::SignalTower,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PartKind::AirCylinder => "エアシリンダ",
            PartKind::Gripper => "エアチャック",
            PartKind::Motor => "モーター",
            PartKind::BeltConveyor => "ベルトコンベア",
            PartKind::PhotoSensor => "光電センサー",
            PartKind::ProximitySensor => "近接センサー",
            PartKind::Plc => "シーケンサー (PLC)",
            PartKind::RemoteIo => "リモート I/O",
            PartKind::PowerSupply => "電源 24V",
            PartKind::Relay => "リレー",
            PartKind::ControlPanel => "制御盤 (筐体)",
            PartKind::PushButton => "押しボタン",
            PartKind::SignalTower => "シグナルタワー",
        }
    }

    /// プロジェクトファイルに書く名前 (表示名を変えてもファイルは読めるように別に持つ)
    pub fn key(self) -> &'static str {
        match self {
            PartKind::AirCylinder => "air_cylinder",
            PartKind::Gripper => "gripper",
            PartKind::Motor => "motor",
            PartKind::BeltConveyor => "belt_conveyor",
            PartKind::PhotoSensor => "photo_sensor",
            PartKind::ProximitySensor => "proximity_sensor",
            PartKind::Plc => "plc",
            PartKind::RemoteIo => "remote_io",
            PartKind::PowerSupply => "power_supply",
            PartKind::Relay => "relay",
            PartKind::ControlPanel => "control_panel",
            PartKind::PushButton => "push_button",
            PartKind::SignalTower => "signal_tower",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key() == key)
    }

    pub fn category(self) -> Category {
        match self {
            PartKind::AirCylinder | PartKind::Gripper | PartKind::Motor => Category::Actuator,
            PartKind::BeltConveyor => Category::Conveying,
            PartKind::PhotoSensor | PartKind::ProximitySensor => Category::Sensor,
            PartKind::Plc
            | PartKind::RemoteIo
            | PartKind::PowerSupply
            | PartKind::Relay
            | PartKind::ControlPanel => Category::Control,
            PartKind::PushButton | PartKind::SignalTower => Category::Operator,
        }
    }

    /// 寸法パラメータの定義。並び順が [`Params`] の添字になる。
    pub fn param_specs(self) -> &'static [ParamSpec] {
        const fn p(
            key: &'static str,
            label: &'static str,
            default: f64,
            min: f64,
            max: f64,
        ) -> ParamSpec {
            ParamSpec {
                key,
                label,
                default,
                min,
                max,
            }
        }
        macro_rules! specs {
            ($($e:expr),* $(,)?) => {{
                const S: &[ParamSpec] = &[$($e),*];
                S
            }};
        }
        match self {
            PartKind::AirCylinder => specs![
                p("bore", "チューブ内径 mm", 32.0, 6.0, 200.0),
                p("stroke", "ストローク mm", 100.0, 5.0, 1000.0),
            ],
            PartKind::Gripper => specs![
                p("width", "本体幅 mm", 60.0, 20.0, 300.0),
                p("jaw_stroke", "片側ストローク mm", 6.0, 1.0, 50.0),
            ],
            PartKind::Motor => specs![
                p("frame", "フランジ角 mm", 60.0, 20.0, 200.0),
                p("length", "本体長 mm", 100.0, 30.0, 400.0),
                p("shaft", "軸径 mm", 10.0, 3.0, 50.0),
            ],
            PartKind::BeltConveyor => specs![
                p("length", "機長 mm", 1000.0, 200.0, 10000.0),
                p("width", "ベルト幅 mm", 300.0, 50.0, 2000.0),
                p("height", "搬送面高さ mm", 700.0, 100.0, 2000.0),
                p("speed", "速度 mm/s", 200.0, 1.0, 3000.0),
            ],
            PartKind::PhotoSensor => specs![p("range", "検出距離 mm", 500.0, 10.0, 10000.0)],
            PartKind::ProximitySensor => specs![
                p("diameter", "ねじ径 mm", 12.0, 5.0, 30.0),
                p("range", "検出距離 mm", 4.0, 0.5, 40.0),
            ],
            PartKind::Plc => specs![
                p("inputs", "入力点数", 16.0, 8.0, 64.0),
                p("outputs", "出力点数", 16.0, 8.0, 64.0),
            ],
            PartKind::RemoteIo => specs![
                p("inputs", "入力点数", 16.0, 0.0, 32.0),
                p("outputs", "出力点数", 16.0, 0.0, 32.0),
            ],
            PartKind::PowerSupply => specs![p("watts", "容量 W", 120.0, 15.0, 960.0)],
            PartKind::Relay => specs![],
            PartKind::ControlPanel => specs![
                p("width", "幅 mm", 600.0, 200.0, 2000.0),
                p("height", "高さ mm", 800.0, 200.0, 2200.0),
                p("depth", "奥行 mm", 250.0, 100.0, 800.0),
            ],
            PartKind::PushButton => specs![],
            PartKind::SignalTower => specs![p("tiers", "段数", 3.0, 1.0, 5.0)],
        }
    }

    pub fn default_params(self) -> Params {
        Params(self.param_specs().iter().map(|s| s.default).collect())
    }
}

/// 数値パラメータ1つ分の定義
#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub default: f64,
    pub min: f64,
    pub max: f64,
}

/// パラメータの値。並びは [`PartKind::param_specs`] と同じ。
#[derive(Clone, Debug, PartialEq)]
pub struct Params(pub Vec<f64>);

/// 部品1つ分の設計データ (種類 + パラメータ)
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub kind: PartKind,
    pub params: Params,
}

impl Spec {
    pub fn new(kind: PartKind) -> Self {
        Self {
            kind,
            params: kind.default_params(),
        }
    }

    /// キーでパラメータを引く。定義にないキーは定義側の誤りなので panic する。
    pub fn get(&self, key: &str) -> f64 {
        let specs = self.kind.param_specs();
        let index = specs
            .iter()
            .position(|s| s.key == key)
            .unwrap_or_else(|| panic!("{:?} にパラメータ {key} はない", self.kind));
        self.params
            .0
            .get(index)
            .copied()
            .unwrap_or(specs[index].default)
    }

    /// 点数のパラメータを 8 点単位に丸めて返す
    fn points(&self, key: &str) -> usize {
        let n = self.get(key).max(0.0).round() as usize;
        n.div_ceil(8) * 8
    }

    /// 範囲外の値を定義の範囲に収める
    pub fn clamp_params(&mut self) {
        let specs = self.kind.param_specs();
        self.params.0.resize(specs.len(), 0.0);
        for (value, spec) in self.params.0.iter_mut().zip(specs) {
            *value = value.clamp(spec.min, spec.max);
        }
    }

    /// PLC から見た I/O 点
    pub fn io_points(&self) -> Vec<IoPoint> {
        use IoDir::{Input, Output};
        let fixed = |list: &[(&str, IoDir)]| {
            list.iter()
                .map(|&(name, dir)| IoPoint::new(name, dir))
                .collect::<Vec<_>>()
        };
        match self.kind {
            PartKind::AirCylinder => {
                fixed(&[("前進 SOL", Output), ("後退端", Input), ("前進端", Input)])
            }
            PartKind::Gripper => fixed(&[("閉 SOL", Output), ("開端", Input), ("閉端", Input)]),
            PartKind::Motor => fixed(&[("運転", Output), ("運転中", Input)]),
            PartKind::BeltConveyor => fixed(&[("運転", Output), ("逆転", Output)]),
            PartKind::PhotoSensor | PartKind::ProximitySensor => fixed(&[("検出", Input)]),
            PartKind::Relay => fixed(&[("コイル", Output)]),
            PartKind::PowerSupply => fixed(&[("電源異常", Input)]),
            PartKind::PushButton => fixed(&[("押下", Input)]),
            PartKind::SignalTower => ["赤", "黄", "緑", "青", "白"]
                .into_iter()
                .take(self.get("tiers").round() as usize)
                .map(|c| IoPoint::new(format!("{c} 点灯"), Output))
                .collect(),
            // PLC 本体の端子は三菱 Q / iQ-R 流の 16 進 (X0..XF, X10..)
            PartKind::Plc => {
                let inputs =
                    (0..self.points("inputs")).map(|i| IoPoint::new(format!("X{i:X}"), Input));
                let outputs =
                    (0..self.points("outputs")).map(|i| IoPoint::new(format!("Y{i:X}"), Output));
                inputs.chain(outputs).collect()
            }
            PartKind::RemoteIo => {
                let inputs =
                    (0..self.points("inputs")).map(|i| IoPoint::new(format!("IN{i}"), Input));
                let outputs =
                    (0..self.points("outputs")).map(|i| IoPoint::new(format!("OUT{i}"), Output));
                inputs.chain(outputs).collect()
            }
            PartKind::ControlPanel => Vec::new(),
        }
    }

    /// PLC 本体やリモート I/O のように、他の部品の I/O をつなぐ側か
    pub fn is_io_host(&self) -> bool {
        matches!(self.kind, PartKind::Plc | PartKind::RemoteIo)
    }

    /// アクチュエータ動作。値 0 が原位置 (後退端・開・停止)、1 が動作端。
    pub fn actuators(&self) -> Vec<ActuatorSpec> {
        match self.kind {
            PartKind::AirCylinder => vec![ActuatorSpec {
                name: "ロッド",
                drive: "前進 SOL",
                motion: Motion::Linear {
                    direction: [1.0, 0.0, 0.0],
                    travel: self.get("stroke"),
                },
                bodies: vec![body::CYL_ROD, body::CYL_ROD_END],
                end_inputs: Some(("後退端", "前進端")),
            }],
            // 左右の爪が逆向きに閉じる
            PartKind::Gripper => {
                let travel = self.get("jaw_stroke");
                vec![
                    ActuatorSpec {
                        name: "爪 (左)",
                        drive: "閉 SOL",
                        motion: Motion::Linear {
                            direction: [1.0, 0.0, 0.0],
                            travel,
                        },
                        bodies: vec![body::GRIP_JAW_L],
                        end_inputs: Some(("開端", "閉端")),
                    },
                    ActuatorSpec {
                        name: "爪 (右)",
                        drive: "閉 SOL",
                        motion: Motion::Linear {
                            direction: [-1.0, 0.0, 0.0],
                            travel,
                        },
                        bodies: vec![body::GRIP_JAW_R],
                        end_inputs: None,
                    },
                ]
            }
            PartKind::Motor => {
                let frame = self.get("frame");
                vec![ActuatorSpec {
                    name: "出力軸",
                    drive: "運転",
                    motion: Motion::Rotary {
                        axis: [1.0, 0.0, 0.0],
                        pivot: [0.0, frame / 2.0, 0.0],
                        angle_deg: 360.0,
                    },
                    bodies: vec![body::MOTOR_SHAFT, body::MOTOR_KEY],
                    end_inputs: None,
                }]
            }
            PartKind::BeltConveyor => vec![ActuatorSpec {
                name: "ベルト",
                drive: "運転",
                motion: Motion::Belt {
                    direction: [1.0, 0.0, 0.0],
                    speed: self.get("speed"),
                },
                bodies: vec![body::CONV_BELT],
                end_inputs: None,
            }],
            _ => Vec::new(),
        }
    }

    /// センサーの検出範囲 (部品ローカル mm)
    pub fn sensing(&self) -> Option<SensingSpec> {
        match self.kind {
            PartKind::PhotoSensor => Some(SensingSpec {
                input: "検出",
                origin: [15.0, 25.0, 0.0],
                direction: [1.0, 0.0, 0.0],
                range: self.get("range"),
            }),
            PartKind::ProximitySensor => Some(SensingSpec {
                input: "検出",
                origin: [50.0, self.get("diameter") / 2.0 + 3.0, 0.0],
                direction: [1.0, 0.0, 0.0],
                range: self.get("range"),
            }),
            _ => None,
        }
    }

    /// コンベアの搬送面 (部品ローカル mm): 中心、搬送方向、長さ、幅
    pub fn conveying_surface(&self) -> Option<ConveyingSurface> {
        (self.kind == PartKind::BeltConveyor).then(|| ConveyingSurface {
            center: [0.0, self.get("height"), 0.0],
            direction: [1.0, 0.0, 0.0],
            length: self.get("length"),
            width: self.get("width"),
            speed: self.get("speed"),
        })
    }
}

/// [`geometry::bodies`](super::geometry::bodies) が返す本体の並び順。
/// アクチュエータが動かす本体をこの番号で指す。
pub mod body {
    pub const CYL_ROD: usize = 2;
    pub const CYL_ROD_END: usize = 3;
    pub const GRIP_JAW_L: usize = 1;
    pub const GRIP_JAW_R: usize = 2;
    pub const MOTOR_SHAFT: usize = 2;
    pub const MOTOR_KEY: usize = 3;
    pub const CONV_BELT: usize = 0;
}

/// I/O の向き (PLC から見て)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IoDir {
    /// センサー・スイッチ → PLC
    Input,
    /// PLC → ソレノイド・モーター・表示灯
    Output,
}

impl IoDir {
    pub fn label(self) -> &'static str {
        match self {
            IoDir::Input => "入力",
            IoDir::Output => "出力",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IoPoint {
    pub name: String,
    pub dir: IoDir,
}

impl IoPoint {
    fn new(name: impl Into<String>, dir: IoDir) -> Self {
        Self {
            name: name.into(),
            dir,
        }
    }
}

/// アクチュエータ1軸分の動き
#[derive(Clone, Debug, PartialEq)]
pub struct ActuatorSpec {
    pub name: &'static str,
    /// この動作を起こす出力 I/O の名前
    pub drive: &'static str,
    pub motion: Motion,
    /// 動く本体の番号
    pub bodies: Vec<usize>,
    /// 原位置 / 動作端を知らせる入力 I/O の名前
    pub end_inputs: Option<(&'static str, &'static str)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Motion {
    /// 直動。値 0..1 で 0..travel mm 動く
    Linear { direction: [f64; 3], travel: f64 },
    /// 回転。値 0..1 で 0..angle_deg 回る
    Rotary {
        axis: [f64; 3],
        pivot: [f64; 3],
        angle_deg: f64,
    },
    /// 搬送ベルト。本体は動かず、搬送面上のワークが speed mm/s で流れる
    Belt { direction: [f64; 3], speed: f64 },
}

impl Motion {
    /// 値 `value` (0..1) のときの本体の変位 (mm の平行移動, 回転角 deg)
    pub fn displacement(&self, value: f64) -> ([f64; 3], f64) {
        let v = value.clamp(0.0, 1.0);
        match *self {
            Motion::Linear { direction, travel } => (direction.map(|d| d * travel * v), 0.0),
            Motion::Rotary { angle_deg, .. } => ([0.0; 3], angle_deg * v),
            Motion::Belt { .. } => ([0.0; 3], 0.0),
        }
    }
}

/// センサーの検出範囲 (光軸)
#[derive(Clone, Debug, PartialEq)]
pub struct SensingSpec {
    pub input: &'static str,
    pub origin: [f64; 3],
    pub direction: [f64; 3],
    pub range: f64,
}

/// コンベアの搬送面
#[derive(Clone, Debug, PartialEq)]
pub struct ConveyingSurface {
    pub center: [f64; 3],
    pub direction: [f64; 3],
    pub length: f64,
    pub width: f64,
    pub speed: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_valid_defaults() {
        for kind in PartKind::ALL {
            let spec = Spec::new(kind);
            for p in kind.param_specs() {
                assert!(
                    p.min <= p.default && p.default <= p.max,
                    "{kind:?} {}",
                    p.key
                );
                assert_eq!(spec.get(p.key), p.default);
            }
        }
    }

    #[test]
    fn actuator_drives_and_end_inputs_are_declared_io_points() {
        for kind in PartKind::ALL {
            let spec = Spec::new(kind);
            let io = spec.io_points();
            let has = |name: &str, dir| io.iter().any(|p| p.name == name && p.dir == dir);
            for act in spec.actuators() {
                assert!(has(act.drive, IoDir::Output), "{kind:?} {}", act.drive);
                if let Some((home, end)) = act.end_inputs {
                    assert!(has(home, IoDir::Input) && has(end, IoDir::Input));
                }
            }
            if let Some(sensing) = spec.sensing() {
                assert!(has(sensing.input, IoDir::Input));
            }
        }
    }

    #[test]
    fn keys_round_trip() {
        for kind in PartKind::ALL {
            assert_eq!(PartKind::from_key(kind.key()), Some(kind));
        }
        assert_eq!(PartKind::from_key("unknown"), None);
    }

    #[test]
    fn plc_points_round_up_to_eight_and_use_hex() {
        let mut spec = Spec::new(PartKind::Plc);
        spec.params.0 = vec![20.0, 8.0];
        let io = spec.io_points();
        let inputs: Vec<_> = io.iter().filter(|p| p.dir == IoDir::Input).collect();
        assert_eq!(inputs.len(), 24);
        assert_eq!(inputs[15].name, "XF");
        assert_eq!(inputs[16].name, "X10");
        assert_eq!(io.iter().filter(|p| p.dir == IoDir::Output).count(), 8);
    }

    #[test]
    fn signal_tower_outputs_follow_tiers() {
        let mut spec = Spec::new(PartKind::SignalTower);
        spec.params.0 = vec![2.0];
        let names: Vec<_> = spec.io_points().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["赤 点灯", "黄 点灯"]);
    }

    #[test]
    fn clamp_params_restores_length_and_range() {
        let mut spec = Spec::new(PartKind::AirCylinder);
        spec.params.0 = vec![1.0];
        spec.clamp_params();
        assert_eq!(spec.params.0, vec![6.0, 0.0_f64.clamp(5.0, 1000.0)]);
    }

    #[test]
    fn cylinder_displacement_scales_with_stroke() {
        let spec = Spec::new(PartKind::AirCylinder);
        let act = &spec.actuators()[0];
        assert_eq!(act.motion.displacement(0.5).0, [50.0, 0.0, 0.0]);
        assert_eq!(act.motion.displacement(2.0).0, [100.0, 0.0, 0.0]);
    }
}
