//! ワーク搬送の設定 (ユーザーが編集するデータ)
//!
//! - ワーク種類: 名前・寸法・色
//! - 投入口: ワークが現れる場所 (コンベアの始端か、部品の上)
//! - 排出口: ワークが消える場所 (部品)。消えたワークを数える
//! - コンベア: ベルトにする部品の上面をワークが流れる
//! - グリッパ: チャックの開閉でワークを掴む・離す

use glam::DVec3;

/// ワークの種類
#[derive(Clone, Debug, PartialEq)]
pub struct WorkKind {
    pub id: u32,
    pub name: String,
    /// 幅 / 高さ / 奥行
    pub size: [f64; 3],
    /// 表示色 (sRGB 0..1)
    pub color: [f32; 3],
}

/// 種類を追加したときに順に使う色
const KIND_COLORS: [[f32; 3]; 6] = [
    [0.95, 0.75, 0.25],
    [0.30, 0.70, 0.95],
    [0.55, 0.85, 0.40],
    [0.90, 0.45, 0.70],
    [0.95, 0.50, 0.25],
    [0.65, 0.55, 0.95],
];

/// コンベアの終端の作り
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConveyorEnd {
    /// ストッパで止める。ワークは終端から詰まって並ぶ。
    Stopper,
    /// 終端から次の工程へ流れ出る (排出として数えて消す)
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

/// ワークが流れる向き (水平の X か Z)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BeltAxis {
    X,
    Z,
}

impl BeltAxis {
    pub const ALL: [BeltAxis; 2] = [BeltAxis::X, BeltAxis::Z];

    pub fn label(self) -> &'static str {
        match self {
            BeltAxis::X => "X",
            BeltAxis::Z => "Z",
        }
    }

    pub fn unit_vector(self) -> DVec3 {
        match self {
            BeltAxis::X => DVec3::X,
            BeltAxis::Z => DVec3::Z,
        }
    }
}

/// ベルトコンベア。ベルトにする部品の上面を、X か Z の向きにワークが流れる。
#[derive(Clone, Debug, PartialEq)]
pub struct Conveyor {
    pub id: u32,
    pub name: String,
    /// ベルトにする部品 (アセンブリでもよい) の id。設計位置での外接箱を使う。
    pub part: Option<u32>,
    pub axis: BeltAxis,
    /// true なら軸のマイナス方向に流れる
    pub reversed: bool,
    /// ベルト速度 (モデル単位 / 秒)
    pub speed: f64,
    pub end: ConveyorEnd,
}

impl Conveyor {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            name: format!("コンベア {id}"),
            part: None,
            axis: BeltAxis::X,
            reversed: false,
            speed: 50.0,
            end: ConveyorEnd::Stopper,
        }
    }

    /// 流れる向きの単位ベクトル
    pub fn direction(&self) -> DVec3 {
        let v = self.axis.unit_vector();
        if self.reversed { -v } else { v }
    }

    /// 寸法のうち、流れる向きの長さ
    pub fn along(&self, size: DVec3) -> f64 {
        size.dot(self.axis.unit_vector()).abs()
    }
}

/// ワークを出す先
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SourceOutput {
    /// コンベアの始端に置く
    Conveyor(u32),
    /// 投入口の部品の上面に置く
    OnPart,
}

/// 投入口。決まった間隔でワークを出す。
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub id: u32,
    pub name: String,
    /// 投入口の部品 (表示用。`SourceOutput::OnPart` ならこの上面にワークが現れる)
    pub part: Option<u32>,
    pub output: SourceOutput,
    /// 出す間隔 (秒)。最初の1個は時刻 0 に出る。
    pub interval: f64,
    /// 出すワーク種類。複数あれば順番に出す。空なら最初の種類。
    pub kinds: Vec<u32>,
    /// 出す個数の上限。None なら無制限。
    pub limit: Option<u32>,
}

impl Source {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            name: format!("投入口 {id}"),
            part: None,
            output: SourceOutput::OnPart,
            interval: 3.0,
            kinds: Vec::new(),
            limit: None,
        }
    }
}

/// 排出口。部品に触れたワークを消して数える。
#[derive(Clone, Debug, PartialEq)]
pub struct Sink {
    pub id: u32,
    pub name: String,
    pub part: Option<u32>,
}

impl Sink {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            name: format!("排出口 {id}"),
            part: None,
        }
    }
}

/// チャック付きのグリッパ。チャックの開閉と、掴んだワークを運ぶ動きは
/// [`MachineMotion`](super::MachineMotion) が教える (アプリではタイミングチャートのシリンダ)。
#[derive(Clone, Debug, PartialEq)]
pub struct Gripper {
    pub id: u32,
    pub name: String,
    /// 開閉に使うアクチュエータの id
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
            reach: 20.0,
        }
    }
}

/// ワーク搬送の設定全体
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConveyorConfig {
    pub kinds: Vec<WorkKind>,
    pub sources: Vec<Source>,
    pub sinks: Vec<Sink>,
    pub conveyors: Vec<Conveyor>,
    pub grippers: Vec<Gripper>,
    pub next_id: u32,
}

impl ConveyorConfig {
    fn take_id(&mut self) -> u32 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn add_kind(&mut self) -> &mut WorkKind {
        let id = self.take_id();
        let color = KIND_COLORS[self.kinds.len() % KIND_COLORS.len()];
        self.kinds.push(WorkKind {
            id,
            name: format!("ワーク {}", self.kinds.len() + 1),
            size: [30.0, 20.0, 30.0],
            color,
        });
        self.kinds.last_mut().unwrap()
    }

    /// 投入口を足す。種類がまだなければ1つ作る。
    pub fn add_source(&mut self) -> &mut Source {
        if self.kinds.is_empty() {
            self.add_kind();
        }
        let id = self.take_id();
        self.sources.push(Source::new(id));
        self.sources.last_mut().unwrap()
    }

    pub fn add_sink(&mut self) -> &mut Sink {
        let id = self.take_id();
        self.sinks.push(Sink::new(id));
        self.sinks.last_mut().unwrap()
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

    pub fn kind(&self, id: u32) -> Option<&WorkKind> {
        self.kinds.iter().find(|k| k.id == id)
    }

    /// 投入口が `index` 個目 (0 始まり) に出すワーク種類
    pub fn kind_for(&self, source: &Source, index: u32) -> Option<&WorkKind> {
        if source.kinds.is_empty() {
            return self.kinds.first();
        }
        let id = source.kinds[index as usize % source.kinds.len()];
        self.kind(id)
    }

    /// 種類を消す。投入口の指定からも外す。
    pub fn remove_kind(&mut self, id: u32) {
        self.kinds.retain(|k| k.id != id);
        for source in &mut self.sources {
            source.kinds.retain(|k| *k != id);
        }
    }

    /// コンベアを消す。そこへ出していた投入口は部品の上に出す。
    pub fn remove_conveyor(&mut self, id: u32) {
        self.conveyors.retain(|c| c.id != id);
        for source in &mut self.sources {
            if source.output == SourceOutput::Conveyor(id) {
                source.output = SourceOutput::OnPart;
            }
        }
    }

    /// 投入口・排出口・コンベアに使っている部品 (外接箱が要るもの)
    pub fn parts_in_use(&self) -> Vec<u32> {
        let sources = self.sources.iter().filter_map(|s| s.part);
        let sinks = self.sinks.iter().filter_map(|s| s.part);
        let belts = self.conveyors.iter().filter_map(|c| c.part);
        sources.chain(sinks).chain(belts).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
            && self.sinks.is_empty()
            && self.conveyors.is_empty()
            && self.grippers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_cycle_through_their_kinds() {
        let mut config = ConveyorConfig::default();
        let a = config.add_kind().id;
        let b = config.add_kind().id;
        let source = config.add_source().clone();
        // 指定がなければ最初の種類
        assert_eq!(config.kind_for(&source, 5).unwrap().id, a);
        let mut mixed = source;
        mixed.kinds = vec![a, b, b];
        let order: Vec<u32> = (0..4)
            .map(|i| config.kind_for(&mixed, i).unwrap().id)
            .collect();
        assert_eq!(order, vec![a, b, b, a]);
    }

    #[test]
    fn removing_a_conveyor_redirects_its_sources() {
        let mut config = ConveyorConfig::default();
        let conveyor = config.add_conveyor().id;
        config.add_source().output = SourceOutput::Conveyor(conveyor);
        config.remove_conveyor(conveyor);
        assert_eq!(config.sources[0].output, SourceOutput::OnPart);
    }
}
