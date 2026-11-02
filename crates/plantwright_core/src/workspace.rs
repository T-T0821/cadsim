//! 画面の道具 (ツール) の一覧と、どれを開いているか
//!
//! 3D ビューの上に浮かぶパネル (干渉チェック・寸法・安全・PLC 連携など) は、
//! 画面左端のツールレールとメニューの「ツール」から開け閉めする。
//! ここには道具の名前・分類・並び順と、開いた順番 (新しいパネルを少しずらして重ねるため) を置く。
//! Bevy・egui に依存しない。アプリ側はここを見てボタンとパネルを描くだけにする。

/// 道具の分類。ツールレールではこの順に区切って並べる。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolGroup {
    /// 部品を置く・形を作る
    Build,
    /// 干渉・寸法・物理で確かめる
    Check,
    /// 搬送・作業者・安全・PLC・工程などライン全体
    Line,
    /// 生成 AI
    Ai,
}

impl ToolGroup {
    pub const ALL: [ToolGroup; 4] = [
        ToolGroup::Build,
        ToolGroup::Check,
        ToolGroup::Line,
        ToolGroup::Ai,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ToolGroup::Build => "つくる",
            ToolGroup::Check => "たしかめる",
            ToolGroup::Line => "ライン",
            ToolGroup::Ai => "AI",
        }
    }
}

/// 3D ビューの上に開く道具
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tool {
    Library,
    Modeling,
    Interference,
    Annotation,
    Physics,
    Review,
    Conveyor,
    Floor,
    Safety,
    Plc,
    Flow,
    LineLink,
    Site,
    Compose,
    SequenceGen,
}

impl Tool {
    /// ツールレールとメニューに並べる順
    pub const ALL: [Tool; 15] = [
        Tool::Library,
        Tool::Modeling,
        Tool::Interference,
        Tool::Annotation,
        Tool::Physics,
        Tool::Review,
        Tool::Conveyor,
        Tool::Floor,
        Tool::Safety,
        Tool::Plc,
        Tool::Flow,
        Tool::LineLink,
        Tool::Site,
        Tool::Compose,
        Tool::SequenceGen,
    ];

    pub fn group(self) -> ToolGroup {
        match self {
            Tool::Library | Tool::Modeling => ToolGroup::Build,
            Tool::Interference | Tool::Annotation | Tool::Physics | Tool::Review => {
                ToolGroup::Check
            }
            Tool::Conveyor
            | Tool::Floor
            | Tool::Safety
            | Tool::Plc
            | Tool::Flow
            | Tool::LineLink
            | Tool::Site => ToolGroup::Line,
            Tool::Compose | Tool::SequenceGen => ToolGroup::Ai,
        }
    }

    /// パネルの題名とメニューの項目名
    pub fn title(self) -> &'static str {
        match self {
            Tool::Library => "部品ライブラリ",
            Tool::Modeling => "モデリング",
            Tool::Interference => "干渉チェック・計測",
            Tool::Annotation => "寸法・注記",
            Tool::Physics => "物理演算",
            Tool::Review => "複数人でレビュー",
            Tool::Conveyor => "ワーク搬送",
            Tool::Floor => "作業者・AGV",
            Tool::Safety => "安全",
            Tool::Plc => "PLC連携",
            Tool::Flow => "工程シミュレーション",
            Tool::LineLink => "ライン連携",
            Tool::Site => "平面図・点群",
            Tool::Compose => "言葉でラインを組む",
            Tool::SequenceGen => "チャート・ラダーの自動生成",
        }
    }

    /// ツールレールのボタンの下に出す短い名前 (全角 4 文字まで)
    pub fn short_label(self) -> &'static str {
        match self {
            Tool::Library => "部品",
            Tool::Modeling => "モデル",
            Tool::Interference => "干渉",
            Tool::Annotation => "寸法",
            Tool::Physics => "物理",
            Tool::Review => "レビュー",
            Tool::Conveyor => "搬送",
            Tool::Floor => "作業者",
            Tool::Safety => "安全",
            Tool::Plc => "PLC",
            Tool::Flow => "工程",
            Tool::LineLink => "連携",
            Tool::Site => "平面図",
            Tool::Compose => "言葉で",
            Tool::SequenceGen => "ラダー",
        }
    }

    /// ツールチップに出す説明
    pub fn hint(self) -> &'static str {
        match self {
            Tool::Library => {
                "シリンダ・コンベア・センサーなどの標準部品を、ビューへドラッグして置く"
            }
            Tool::Modeling => "円柱・押し出し・穴あけ・ブーリアンで形を作る",
            Tool::Interference => "部品どうしの干渉・接触を調べ、2点の距離を測る",
            Tool::Annotation => "3D 上に寸法と注記を置く",
            Tool::Physics => "重力・接触・摩擦で部品を動かす。ケーブルやホースのたるみも見る",
            Tool::Review => {
                "離れた人と同じ工場に入り、互いの視点と指した場所を見ながらレビューする"
            }
            Tool::Conveyor => "コンベアでワークを流し、グリッパでつかんで放す",
            Tool::Floor => "作業者の動線と AGV の走行を床の上で動かす",
            Tool::Safety => "ライトカーテン・エリアセンサ・安全柵・非常停止を置き、停止を確かめる",
            Tool::Plc => {
                "動きを PLC のラダーで動かす。シミュレーションと実機 (Modbus など) を選べる"
            }
            Tool::Flow => "工程とバッファを並べてラインを早回しし、稼働率やスループットを集計する",
            Tool::LineLink => "工程・AGV・作業者・タイミングチャートを同じ時計で動かす",
            Tool::Site => "床に敷いた平面図と点群の縮尺合わせ・表示",
            Tool::Compose => {
                "「コンベア2本とロボット1台で箱詰め」のような文から、設備の配置とチャートの下書きを作る"
            }
            Tool::SequenceGen => {
                "説明文からタイミングチャートを、チャートからラダーと I/O 表を作る"
            }
        }
    }

    /// ビューに浮かべる小さなパネルか (大きな作業用のウィンドウは真ん中寄りに開く)
    pub fn is_panel(self) -> bool {
        !matches!(
            self,
            Tool::Flow | Tool::LineLink | Tool::Compose | Tool::SequenceGen | Tool::Modeling
        )
    }
}

/// どの道具を開いているか。開いた順に覚える。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Workspace {
    open: Vec<Tool>,
}

/// 新しく開いたパネルを前のパネルからずらす量 (画素)
pub const CASCADE_STEP: f32 = 28.0;
/// ずらすのはこの枚数まで。それ以上は最初の位置に戻って重ねる。
pub const CASCADE_LIMIT: usize = 6;

impl Workspace {
    pub fn is_open(&self, tool: Tool) -> bool {
        self.open.contains(&tool)
    }

    pub fn set_open(&mut self, tool: Tool, open: bool) {
        if open {
            if !self.is_open(tool) {
                self.open.push(tool);
            }
        } else {
            self.open.retain(|&t| t != tool);
        }
    }

    pub fn toggle(&mut self, tool: Tool) {
        let open = !self.is_open(tool);
        self.set_open(tool, open);
    }

    /// すべて閉じる
    pub fn close_all(&mut self) {
        self.open.clear();
    }

    /// 開いている道具 (開いた順)
    pub fn open_tools(&self) -> &[Tool] {
        &self.open
    }

    /// 開いている小さなパネルのうち、`tool` が何枚目に開いたか (0 から)。
    /// パネルを最初に出す位置を、この数だけ右下へずらす。
    pub fn cascade_index(&self, tool: Tool) -> usize {
        self.open
            .iter()
            .filter(|t| t.is_panel())
            .position(|&t| t == tool)
            .unwrap_or(0)
            % CASCADE_LIMIT
    }

    /// 小さなパネルを最初に出す位置。`origin` は 3D ビューの左上 (余白込み)。
    pub fn panel_position(&self, tool: Tool, origin: [f32; 2]) -> [f32; 2] {
        let step = self.cascade_index(tool) as f32 * CASCADE_STEP;
        [origin[0] + step, origin[1] + step]
    }
}

/// カーソルが乗っていないパネルの地の濃さ (0 = 透明, 1 = 不透明)
pub const PANEL_SEE_THROUGH: f32 = 0.72;
/// 3D ビューをドラッグしている間のパネルの地の濃さ
pub const PANEL_FADED: f32 = 0.15;

const _: () = assert!(PANEL_FADED < PANEL_SEE_THROUGH && PANEL_SEE_THROUGH < 1.0);

/// 3D ビューに浮かべたパネルの地をどれだけ濃く塗るかを決める材料
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PanelSight {
    /// パネルを透かす設定 (表示メニュー)
    pub see_through: bool,
    /// カーソルがパネルの上にある
    pub hovered: bool,
    /// 入力欄に文字を打っている
    pub typing: bool,
    /// 別のパネルの上に重なっている (透かすと下のパネルの字と混ざって読めない)
    pub over_other_panel: bool,
    /// 3D ビューの上でドラッグしている (視点を回す・部品を動かす)
    pub view_dragging: bool,
}

impl PanelSight {
    /// パネルの地の濃さ (0〜1)。
    ///
    /// - 透かさない設定なら、いつも不透明 (以前と同じ見た目)。
    /// - 3D ビューをドラッグしている間は、後ろのモデルが見えるよう大きく薄くする
    ///   (ドラッグの途中でカーソルがパネルを横切っても薄いまま)。
    /// - カーソルを乗せているとき・文字を打っているとき・別のパネルに重なっているときは、
    ///   読みやすいよう不透明にする。
    /// - それ以外は少し透かして、後ろの 3D ビューが透けて見えるようにする。
    pub fn opacity(self) -> f32 {
        if !self.see_through {
            1.0
        } else if self.view_dragging {
            PANEL_FADED
        } else if self.hovered || self.typing || self.over_other_panel {
            1.0
        } else {
            PANEL_SEE_THROUGH
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_is_opaque_unless_see_through_is_on() {
        let off = PanelSight {
            see_through: false,
            view_dragging: true,
            ..Default::default()
        };
        assert_eq!(off.opacity(), 1.0);
    }

    #[test]
    fn panel_fades_while_dragging_the_view_and_solidifies_on_hover() {
        let idle = PanelSight {
            see_through: true,
            ..Default::default()
        };
        assert_eq!(idle.opacity(), PANEL_SEE_THROUGH);
        let hovered = PanelSight {
            hovered: true,
            ..idle
        };
        assert_eq!(hovered.opacity(), 1.0);
        let typing = PanelSight {
            typing: true,
            ..idle
        };
        assert_eq!(typing.opacity(), 1.0);
        let stacked = PanelSight {
            over_other_panel: true,
            ..idle
        };
        assert_eq!(stacked.opacity(), 1.0);
        // 視点を回す途中でパネルを横切っても、薄いまま
        let dragging = PanelSight {
            hovered: true,
            view_dragging: true,
            ..idle
        };
        assert_eq!(dragging.opacity(), PANEL_FADED);
    }

    #[test]
    fn every_tool_is_listed_once_in_group_order() {
        let mut seen = std::collections::HashSet::new();
        for tool in Tool::ALL {
            assert!(seen.insert(tool), "{tool:?} が2回ある");
        }
        // 分類ごとにまとまって並んでいる (レールで区切りを1回ずつ入れられる)
        let groups: Vec<ToolGroup> = Tool::ALL.iter().map(|t| t.group()).collect();
        let mut order = groups.clone();
        order.dedup();
        assert_eq!(order, ToolGroup::ALL.to_vec());
    }

    #[test]
    fn short_labels_fit_the_rail() {
        for tool in Tool::ALL {
            assert!(tool.short_label().chars().count() <= 4, "{tool:?}");
            assert!(!tool.hint().is_empty());
        }
    }

    #[test]
    fn open_close_and_toggle() {
        let mut ws = Workspace::default();
        assert!(!ws.is_open(Tool::Safety));
        ws.toggle(Tool::Safety);
        assert!(ws.is_open(Tool::Safety));
        ws.set_open(Tool::Safety, true);
        assert_eq!(ws.open_tools(), &[Tool::Safety]);
        ws.toggle(Tool::Safety);
        assert!(!ws.is_open(Tool::Safety));
        ws.set_open(Tool::Plc, true);
        ws.set_open(Tool::Flow, true);
        ws.close_all();
        assert!(ws.open_tools().is_empty());
    }

    #[test]
    fn panels_cascade_in_opening_order() {
        let mut ws = Workspace::default();
        ws.set_open(Tool::Interference, true);
        // 大きなウィンドウはずらす数に入れない
        ws.set_open(Tool::Flow, true);
        ws.set_open(Tool::Safety, true);
        assert_eq!(ws.cascade_index(Tool::Interference), 0);
        assert_eq!(ws.cascade_index(Tool::Safety), 1);
        assert_eq!(
            ws.panel_position(Tool::Safety, [100.0, 50.0]),
            [100.0 + CASCADE_STEP, 50.0 + CASCADE_STEP]
        );
        // 前のパネルを閉じたら、次に開くパネルは空いた位置に詰める
        ws.set_open(Tool::Interference, false);
        assert_eq!(ws.cascade_index(Tool::Safety), 0);
    }

    #[test]
    fn cascade_wraps_around() {
        let mut ws = Workspace::default();
        let panels: Vec<Tool> = Tool::ALL.into_iter().filter(|t| t.is_panel()).collect();
        for &tool in &panels {
            ws.set_open(tool, true);
        }
        assert!(panels.len() > CASCADE_LIMIT);
        assert_eq!(ws.cascade_index(panels[CASCADE_LIMIT]), 0);
    }
}
