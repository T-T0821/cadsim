//! 部品の役割のデータの型

use crate::conveyor::{Conveyor, Gripper, Sink, Source};
use crate::math::Aabb;
use crate::model::NodeId;
use crate::motion::chart::{Actuator, ActuatorKind, Sensor, SensorKind};

/// 部品に付けられる役割の種類 (プロパティに並ぶボタン)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum RoleKind {
    Cylinder,
    LinearAxis,
    RotaryTable,
    Conveyor,
    Source,
    Sink,
    Gripper,
    Photoelectric,
    Proximity,
}

impl RoleKind {
    pub const ALL: [RoleKind; 9] = [
        RoleKind::Cylinder,
        RoleKind::LinearAxis,
        RoleKind::RotaryTable,
        RoleKind::Conveyor,
        RoleKind::Source,
        RoleKind::Sink,
        RoleKind::Gripper,
        RoleKind::Photoelectric,
        RoleKind::Proximity,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RoleKind::Cylinder => "シリンダ",
            RoleKind::LinearAxis => "直動軸",
            RoleKind::RotaryTable => "回転テーブル",
            RoleKind::Conveyor => "コンベア",
            RoleKind::Source => "投入口",
            RoleKind::Sink => "排出口",
            RoleKind::Gripper => "グリッパ",
            RoleKind::Photoelectric => "光電センサー",
            RoleKind::Proximity => "近接センサー",
        }
    }

    /// ボタンにかざしたときの説明
    pub fn help(self) -> &'static str {
        match self {
            RoleKind::Cylinder => "出 / 戻の 2 位置で動く。矢印の先をドラッグでストローク",
            RoleKind::LinearAxis => "決めた範囲を動く軸。矢印の先をドラッグで動ける長さ",
            RoleKind::RotaryTable => "部品の中心で回る。面をクリックで回転軸",
            RoleKind::Conveyor => "上に載ったワークを運ぶ。緑の矢印の向きで流れ、長さで速さ",
            RoleKind::Source => "決まった間隔でワークを出す。点をコンベアまでドラッグでつなぐ",
            RoleKind::Sink => "触れたワークを消して数える",
            RoleKind::Gripper => {
                "この部品を動かすシリンダで開閉する。青い球で掴む位置、外の点で掴める距離"
            }
            RoleKind::Photoelectric => "光軸を遮ると ON。面をクリックで向き、先をドラッグで長さ",
            RoleKind::Proximity => "近づくと ON。面をクリックで向き、先をドラッグで検出距離",
        }
    }

    pub(crate) fn actuator_kind(self) -> Option<ActuatorKind> {
        match self {
            RoleKind::Cylinder => Some(ActuatorKind::Cylinder),
            RoleKind::LinearAxis => Some(ActuatorKind::LinearAxis),
            RoleKind::RotaryTable => Some(ActuatorKind::RotaryTable),
            _ => None,
        }
    }

    pub(crate) fn sensor_kind(self) -> Option<SensorKind> {
        match self {
            RoleKind::Photoelectric => Some(SensorKind::Photoelectric),
            RoleKind::Proximity => Some(SensorKind::Proximity),
            _ => None,
        }
    }
}

/// 付いている役割 (タイミングチャートかワーク搬送の設定の、どの行か)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum RoleRef {
    Actuator(u32),
    Sensor(u32),
    Conveyor(u32),
    Source(u32),
    Sink(u32),
    Gripper(u32),
}

/// 編集中の役割の中身 (確定するまで設定には書かない写し)
#[derive(Clone, PartialEq, Debug)]
pub enum RoleBody {
    Actuator(Actuator),
    Sensor(Sensor),
    Conveyor(Conveyor),
    Source(Source),
    Sink(Sink),
    Gripper(Gripper),
}

impl RoleBody {
    pub fn kind(&self) -> RoleKind {
        match self {
            RoleBody::Actuator(a) => match a.kind {
                ActuatorKind::Cylinder => RoleKind::Cylinder,
                ActuatorKind::LinearAxis => RoleKind::LinearAxis,
                ActuatorKind::RotaryTable => RoleKind::RotaryTable,
            },
            RoleBody::Sensor(s) => match s.kind {
                SensorKind::Photoelectric => RoleKind::Photoelectric,
                SensorKind::Proximity => RoleKind::Proximity,
            },
            RoleBody::Conveyor(_) => RoleKind::Conveyor,
            RoleBody::Source(_) => RoleKind::Source,
            RoleBody::Sink(_) => RoleKind::Sink,
            RoleBody::Gripper(_) => RoleKind::Gripper,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            RoleBody::Actuator(a) => &a.name,
            RoleBody::Sensor(s) => &s.name,
            RoleBody::Conveyor(c) => &c.name,
            RoleBody::Source(s) => &s.name,
            RoleBody::Sink(s) => &s.name,
            RoleBody::Gripper(g) => &g.name,
        }
    }
}

/// 置きかけ (または調整中) の役割
#[derive(Clone, PartialEq, Debug)]
pub struct RoleDraft {
    /// 役割を付ける部品
    pub part: NodeId,
    /// 部品の外接箱 (ワールド座標、設計位置)
    pub bounds: Aabb,
    /// モデルの単位で 1 mm がいくつか
    pub units_per_mm: f64,
    pub body: RoleBody,
    /// すでにある役割を調整している (None なら新しく足す)
    pub existing: Option<RoleRef>,
}
