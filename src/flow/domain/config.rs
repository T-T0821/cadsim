//! コンベアとグリッパの設定 (ユーザーが編集するデータ)

use glam::DVec3;

use crate::motion::Axis;

/// コンベアの終端の作り
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConveyorEnd {
    /// ストッパで止める。ワークは終端から詰まって並ぶ。
    Stopper,
    /// 終端から次の工程へ流れ出る (シミュレーション上は消える)
    Discharge,
}

impl ConveyorEnd {
    pub const ALL: [ConveyorEnd; 2] = [ConveyorEnd::Stopper, ConveyorEnd::Discharge];

    pub fn label(self) -> &'static str {
        match self {
            ConveyorEnd::Stopper => "ストッパ",
            ConveyorEnd::Discharge => "排出",
        }
    }
}

/// ベルトコンベア。ベルトにする部品 (箱) の上面を、X か Z の向きにワークが流れる。
#[derive(Clone, Debug, PartialEq)]
pub struct Conveyor {
    pub id: u32,
    pub name: String,
    /// ベルトにする部品の id。部品の回転は無視し、寸法と位置だけを使う。
    pub part: Option<u32>,
    /// 流れる向き (X か Z)
    pub axis: Axis,
    /// true なら軸のマイナス方向に流れる
    pub reversed: bool,
    /// ベルト速度 (モデル単位 / 秒)
    pub speed: f64,
    /// 始端にワークを供給する間隔 (秒)。None なら供給しない (グリッパで置かれたものだけ流す)。
    pub feed_interval: Option<f64>,
    pub end: ConveyorEnd,
    /// 供給するワークの寸法 (幅 / 高さ / 奥行)
    pub work_size: [f64; 3],
}

impl Conveyor {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            name: format!("コンベア {id}"),
            part: None,
            axis: Axis::X,
            reversed: false,
            speed: 0.5,
            feed_interval: Some(3.0),
            end: ConveyorEnd::Stopper,
            work_size: [0.3, 0.2, 0.3],
        }
    }

    /// 流れる向きの単位ベクトル
    pub fn direction(&self) -> DVec3 {
        let v = DVec3::from_array(self.axis.unit_vector());
        if self.reversed { -v } else { v }
    }

    /// 寸法のうち、流れる向きの長さ
    pub fn along(&self, size: DVec3) -> f64 {
        size.dot(DVec3::from_array(self.axis.unit_vector())).abs()
    }
}

/// チャック付きのグリッパ。チャックはタイミングチャートのシリンダで、1 (出端) = 閉。
#[derive(Clone, Debug, PartialEq)]
pub struct Gripper {
    pub id: u32,
    pub name: String,
    /// 開閉に使うシリンダのアクチュエータ id。
    /// 掴んだワークは、このシリンダが載っているアクチュエータと一緒に動く。
    pub chuck: Option<u32>,
    /// 掴む位置 (TCP)。全アクチュエータが原点にあるときのワールド座標。
    pub tcp: [f64; 3],
    /// TCP からワーク中心までがこの距離以内なら掴める
    pub reach: f64,
}

impl Gripper {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            name: format!("グリッパ {id}"),
            chuck: None,
            tcp: [0.0; 3],
            reach: 0.2,
        }
    }
}

/// ワーク搬送の設定全体
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlowConfig {
    pub conveyors: Vec<Conveyor>,
    pub grippers: Vec<Gripper>,
    pub next_id: u32,
}

impl FlowConfig {
    fn take_id(&mut self) -> u32 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn add_conveyor(&mut self) -> &mut Conveyor {
        let id = self.take_id();
        self.conveyors.push(Conveyor::new(id));
        self.conveyors.last_mut().unwrap()
    }

    pub fn add_gripper(&mut self) -> &mut Gripper {
        let id = self.take_id();
        self.grippers.push(Gripper::new(id));
        self.grippers.last_mut().unwrap()
    }
}
