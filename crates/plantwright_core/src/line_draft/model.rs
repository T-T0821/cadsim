//! ラインの下書きのデータ

/// 下書きで使える設備の種類 (標準部品)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum EquipmentKind {
    /// ベルトコンベア
    Conveyor,
    /// ロボット (旋回と上下でワークを取って置く)
    Robot,
    /// エアシリンダ (ストッパ・プッシャ・位置決め)
    Cylinder,
    /// 光電センサー (ワークの有無を見る)
    Sensor,
    /// 作業台・置き台
    Table,
}

impl EquipmentKind {
    pub const ALL: [EquipmentKind; 5] = [
        EquipmentKind::Conveyor,
        EquipmentKind::Robot,
        EquipmentKind::Cylinder,
        EquipmentKind::Sensor,
        EquipmentKind::Table,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EquipmentKind::Conveyor => "コンベア",
            EquipmentKind::Robot => "ロボット",
            EquipmentKind::Cylinder => "シリンダ",
            EquipmentKind::Sensor => "センサー",
            EquipmentKind::Table => "作業台",
        }
    }

    /// 英語の識別子 (生成 AI とのやり取りに使う)
    pub fn key(self) -> &'static str {
        match self {
            EquipmentKind::Conveyor => "conveyor",
            EquipmentKind::Robot => "robot",
            EquipmentKind::Cylinder => "cylinder",
            EquipmentKind::Sensor => "sensor",
            EquipmentKind::Table => "table",
        }
    }

    /// 識別子か日本語の名前から種類を探す
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim().to_lowercase();
        Self::ALL
            .into_iter()
            .find(|k| t == k.key() || t == k.label())
    }
}

/// 下書きの設備1台
#[derive(Clone, Debug, PartialEq)]
pub struct Equipment {
    /// ラインの中で一意な名前 ("製品コンベア" など)
    pub name: String,
    pub kind: EquipmentKind,
    /// 床の上の位置 [x, z] (mm)。None なら [`super::layout`] が決める。
    pub position: Option<[f64; 2]>,
    /// コンベアの長さ (mm)。None なら標準の長さ。
    pub length: Option<f64>,
}

impl Equipment {
    pub fn new(name: impl Into<String>, kind: EquipmentKind) -> Self {
        Self {
            name: name.into(),
            kind,
            position: None,
            length: None,
        }
    }
}

/// 設備にさせる動き
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verb {
    /// コンベアを回してワークを送る
    Run,
    /// シリンダを出す
    Extend,
    /// シリンダを戻す
    Retract,
    /// ロボットが `target` の上でワークを取る
    Pick,
    /// ロボットが `target` の上にワークを置く
    Place,
}

impl Verb {
    pub const ALL: [Verb; 5] = [
        Verb::Run,
        Verb::Extend,
        Verb::Retract,
        Verb::Pick,
        Verb::Place,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Verb::Run => "run",
            Verb::Extend => "extend",
            Verb::Retract => "retract",
            Verb::Pick => "pick",
            Verb::Place => "place",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Verb::Run => "搬送",
            Verb::Extend => "出",
            Verb::Retract => "戻",
            Verb::Pick => "取る",
            Verb::Place => "置く",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim().to_lowercase();
        Self::ALL.into_iter().find(|v| t == v.key())
    }

    /// この動きができる設備の種類
    pub fn applies_to(self) -> EquipmentKind {
        match self {
            Verb::Run => EquipmentKind::Conveyor,
            Verb::Extend | Verb::Retract => EquipmentKind::Cylinder,
            Verb::Pick | Verb::Place => EquipmentKind::Robot,
        }
    }

    /// 秒数の指定がないときの動作時間
    pub fn default_seconds(self) -> f64 {
        match self {
            Verb::Run => 3.0,
            Verb::Extend | Verb::Retract => 0.5,
            Verb::Pick | Verb::Place => 1.5,
        }
    }

    /// 取る・置く場所 (`target`) が要る動きか
    pub fn needs_target(self) -> bool {
        matches!(self, Verb::Pick | Verb::Place)
    }
}

/// ステップの中の動作1つ
#[derive(Clone, Debug, PartialEq)]
pub struct DraftAction {
    /// 動かす設備の名前
    pub equipment: String,
    pub verb: Verb,
    /// 取る・置く場所の設備の名前 (Pick / Place のとき)
    pub target: Option<String>,
    /// 動作時間 (秒)
    pub seconds: f64,
}

impl DraftAction {
    pub fn new(equipment: impl Into<String>, verb: Verb) -> Self {
        Self {
            equipment: equipment.into(),
            verb,
            target: None,
            seconds: verb.default_seconds(),
        }
    }

    pub fn at(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }
}

/// 下書きのステップ。同じステップの動作は同時に始まる。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DraftStep {
    /// ステップの説明 ("製品を取る" など)
    pub comment: String,
    pub actions: Vec<DraftAction>,
    /// 開始条件: (センサーの名前, ON を待つなら true)
    pub wait_for: Option<(String, bool)>,
    /// 動作のあとに待つ時間 (秒)。検査や作業者の作業に使う。
    pub dwell: f64,
}

/// ラインの下書き全体
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineDraft {
    /// ラインの名前 ("箱詰めライン" など)
    pub title: String,
    pub equipment: Vec<Equipment>,
    pub steps: Vec<DraftStep>,
    /// 前提や、決めきれなかったことのメモ
    pub notes: Vec<String>,
}

impl LineDraft {
    pub fn find(&self, name: &str) -> Option<&Equipment> {
        self.equipment.iter().find(|e| e.name == name)
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.equipment.iter().position(|e| e.name == name)
    }

    pub fn count(&self, kind: EquipmentKind) -> usize {
        self.equipment.iter().filter(|e| e.kind == kind).count()
    }

    /// 1サイクルのおおよその時間 (秒): 各ステップの最も長い動作 + 待ち時間の合計
    pub fn cycle_seconds(&self) -> f64 {
        self.steps
            .iter()
            .map(|s| s.actions.iter().map(|a| a.seconds).fold(0.0, f64::max) + s.dwell)
            .sum()
    }
}
