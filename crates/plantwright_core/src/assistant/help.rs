//! 使い方を AI に聞く
//!
//! 「AI に聞く」ウィンドウで、操作に迷ったときに質問する。アプリの道具の一覧・メニュー・
//! キー操作と、いま画面で何をしているか ([`HelpContext`]) をシステムプロンプトに入れて、
//! このアプリでの具体的な手順を答えてもらう。
//!
//! 返事に `[[open:道具の名前]]` と書いてもらい、[`parse_answer`] で道具を取り出す。
//! 画面ではその道具を開くボタンにするので、答えを読んですぐ操作に移れる。
//! API キーがないときは [`suggest_tools`] で質問の言葉から道具を探して見せる。

use super::llm::{Effort, LlmClient, LlmError, LlmMessage, LlmRequest};
use crate::workspace::{Tool, ToolGroup};

/// キー操作の一覧 (ヘルプメニューと AI への説明で共通)
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("左クリック", "部品を選択"),
    ("Alt + 左ドラッグ", "回転 (オービット)"),
    ("中ドラッグ", "平行移動 (パン)"),
    ("ホイール", "ズーム"),
    ("右ドラッグ + WASD/QE", "フライ移動"),
    ("W / E / R", "移動 / 回転 / 拡大縮小ギズモ"),
    ("F / Home", "選択を拡大 / 全体を表示"),
    (
        "1 / 3 / 7 / 0",
        "正面 / 右側面 / 上面 / 等角 (Ctrl で反対側)",
    ),
    ("5", "透視投影 / 平行投影を切り替え"),
    ("M", "距離を測る"),
    ("H / Alt+H", "選択を隠す / すべて表示"),
    ("Ctrl+N / Ctrl+O", "新規プロジェクト / 開く"),
    ("Ctrl+S / Ctrl+Shift+S", "保存 / 名前を付けて保存"),
    ("Esc", "選択を解除"),
    ("Delete", "選択を削除"),
    ("Ctrl+Z", "元に戻す"),
    ("Ctrl+Y / Ctrl+Shift+Z", "やり直し"),
    ("F1", "AI に聞く"),
];

/// 質問したときの画面の様子
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HelpContext {
    /// 部品の数
    pub part_count: usize,
    /// 選んでいる部品の場所 (「アセンブリ / 部品」)
    pub selected: Option<String>,
    /// 開いている道具
    pub open_tools: Vec<Tool>,
    /// タイミングチャートのステップの数
    pub chart_steps: usize,
    /// 挑戦中のミッション
    pub mission: Option<String>,
}

impl HelpContext {
    fn describe(&self) -> String {
        let mut lines = vec![format!("- 部品の数: {}", self.part_count)];
        lines.push(match &self.selected {
            Some(path) => format!("- 選択中: {path}"),
            None => "- 選択中: なし".into(),
        });
        lines.push(if self.open_tools.is_empty() {
            "- 開いている道具: なし".into()
        } else {
            let names: Vec<&str> = self.open_tools.iter().map(|t| t.title()).collect();
            format!("- 開いている道具: {}", names.join("、"))
        });
        lines.push(format!(
            "- タイミングチャートのステップ: {}",
            self.chart_steps
        ));
        if let Some(mission) = &self.mission {
            lines.push(format!("- 挑戦中のミッション: {mission}"));
        }
        lines.join("\n")
    }
}

/// アプリの画面と道具の説明 (システムプロンプトの中身)
pub fn system_prompt(context: &HelpContext) -> String {
    let mut tools = String::new();
    for group in ToolGroup::ALL {
        tools.push_str(&format!("【{}】\n", group.label()));
        for tool in Tool::ALL.iter().filter(|t| t.group() == group) {
            tools.push_str(&format!(
                "- {} (レールの「{}」): {}\n",
                tool.title(),
                tool.short_label(),
                tool.hint()
            ));
        }
    }
    let shortcuts: String = SHORTCUTS
        .iter()
        .map(|(key, action)| format!("- {key}: {action}\n"))
        .collect();
    format!(
        "あなたは、工場の設備やラインを 3D で組んで動かすデスクトップアプリ「plantwright」の操作案内役です。\n\
         初めて使う人にも分かるよう、このアプリでの具体的な手順を日本語で短く答えてください。\n\
         \n\
         # 答え方\n\
         - 手順は番号付きで 3〜6 個まで。画面のどこを押すか (メニュー名・ボタン名・レールの名前) をはっきり書く。\n\
         - 道具を使うときは、答えの最後に [[open:道具の名前]] を1行ずつ書く (道具の名前は下の一覧の名前そのまま)。\n\
           画面ではそれが「開く」ボタンになる。\n\
         - アプリにない機能は「この機能はありません」と正直に言い、近い方法があれば案内する。\n\
         - Markdown の見出しや表は使わない。\n\
         \n\
         # 画面の配置\n\
         - 上: メニュー (ファイル・編集・表示・作成・ツール・ヘルプ) とツールバー (開く・保存・元に戻す・ギズモ・投影・ビュー・全体表示)。\n\
         - 左: 道具を開け閉めするツールレール (アイコンのボタンの列) と、アセンブリツリー (部品の階層)。\n\
         - 真ん中: 3D ビュー。右上にビューキューブ、左下にミッションカード。\n\
         - 右: 選んだ部品のプロパティ (名前・位置・回転・寸法・標準部品の設定・物理・スクリプト)。\n\
         - 下: タイミングチャート (アクチュエータの動きをステップで並べて再生する) とステータスバー。\n\
         - ファイルはウィンドウへドラッグ&ドロップでも開ける。サンプルは「ファイル → サンプル」。\n\
         \n\
         # 道具 (ツールレールとメニューの「ツール」から開く)\n\
         {tools}\n\
         # キー操作\n\
         {shortcuts}\n\
         # いまの画面\n\
         {}\n",
        context.describe()
    )
}

/// 会話の続きとして質問する問い合わせを作る
pub fn request(history: &[LlmMessage], question: &str, context: &HelpContext) -> LlmRequest {
    let mut messages = history.to_vec();
    messages.push(LlmMessage::user(question));
    LlmRequest {
        system: system_prompt(context),
        messages,
        max_tokens: 2_000,
        effort: Effort::Low,
        json_schema: None,
    }
}

/// AI の答え。本文から道具の印を抜き、開くボタンにする道具を分けたもの
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub text: String,
    pub tools: Vec<Tool>,
}

/// 質問して答えを受け取る
pub fn ask(
    client: &dyn LlmClient,
    history: &[LlmMessage],
    question: &str,
    context: &HelpContext,
) -> Result<Answer, LlmError> {
    let reply = client.complete(&request(history, question, context))?;
    Ok(parse_answer(&reply))
}

/// 返事から `[[open:道具の名前]]` を取り出す。知らない名前の印は消すだけにする。
pub fn parse_answer(reply: &str) -> Answer {
    let mut text = String::new();
    let mut tools = Vec::new();
    let mut rest = reply;
    while let Some(start) = rest.find("[[open:") {
        text.push_str(&rest[..start]);
        let after = &rest[start + "[[open:".len()..];
        let Some(end) = after.find("]]") else {
            rest = after;
            break;
        };
        let name = after[..end].trim();
        if let Some(tool) = find_tool(name)
            && !tools.contains(&tool)
        {
            tools.push(tool);
        }
        rest = &after[end + 2..];
    }
    text.push_str(rest);
    // 印を抜いたあとの空行を詰める
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let mut cleaned: Vec<&str> = Vec::new();
    for line in lines {
        if line.is_empty() && cleaned.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        cleaned.push(line);
    }
    while cleaned.last().is_some_and(|l| l.is_empty()) {
        cleaned.pop();
    }
    Answer {
        text: cleaned.join("\n"),
        tools,
    }
}

fn find_tool(name: &str) -> Option<Tool> {
    Tool::ALL
        .into_iter()
        .find(|t| t.title() == name || t.short_label() == name)
}

/// 質問の言葉に関係しそうな道具 (API キーがないときの手がかり)。関係の強い順に最大 3 つ。
pub fn suggest_tools(question: &str) -> Vec<Tool> {
    let mut scored: Vec<(usize, Tool)> = Tool::ALL
        .into_iter()
        .filter_map(|tool| {
            let score: usize = keywords(tool)
                .iter()
                .filter(|k| question.contains(*k))
                .map(|k| k.chars().count())
                .sum();
            (score > 0).then_some((score, tool))
        })
        .collect();
    scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
    scored.into_iter().take(3).map(|(_, t)| t).collect()
}

/// 道具を探すときの言葉
fn keywords(tool: Tool) -> &'static [&'static str] {
    match tool {
        Tool::Library => &[
            "部品",
            "ライブラリ",
            "シリンダ",
            "コンベア",
            "センサ",
            "置",
            "配置",
        ],
        Tool::Modeling => &["円柱", "押し出し", "穴", "ブーリアン", "形", "モデリング"],
        Tool::Interference => &["干渉", "ぶつか", "衝突", "接触", "距離", "測"],
        Tool::Annotation => &["寸法", "注記", "メモ", "長さ"],
        Tool::Physics => &["物理", "重力", "落", "ケーブル", "ホース", "摩擦"],
        Tool::Review => &["レビュー", "複数人", "一緒に", "共有", "離れた", "リモート"],
        Tool::Conveyor => &["搬送", "ワーク", "流", "グリッパ", "つかむ"],
        Tool::Floor => &["作業者", "AGV", "人", "動線", "台車"],
        Tool::Safety => &["安全", "ライトカーテン", "非常停止", "柵", "エリアセンサ"],
        Tool::Plc => &["PLC", "ラダー", "Modbus", "シーケンサ", "実機"],
        Tool::Flow => &["工程", "稼働率", "スループット", "ボトルネック", "人員"],
        Tool::LineLink => &["連携", "同じ時計", "ライン全体"],
        Tool::Site => &["平面図", "点群", "DXF", "図面を敷", "縮尺"],
        Tool::Compose => &["言葉", "文", "ラインを組", "自動で配置"],
        Tool::SequenceGen => &["チャート", "ラダー", "I/O", "自動生成", "説明"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::llm::ScriptedLlm;

    #[test]
    fn prompt_lists_every_tool_and_the_current_screen() {
        let context = HelpContext {
            part_count: 7,
            selected: Some("SampleVise / Shaft_1".into()),
            open_tools: vec![Tool::Safety],
            chart_steps: 3,
            mission: Some("部品をえらぼう".into()),
        };
        let prompt = system_prompt(&context);
        for tool in Tool::ALL {
            assert!(prompt.contains(tool.title()), "{tool:?}");
        }
        assert!(prompt.contains("SampleVise / Shaft_1"));
        assert!(prompt.contains("開いている道具: 安全"));
        assert!(prompt.contains("部品をえらぼう"));
        assert!(prompt.contains("Ctrl+Z"));
    }

    #[test]
    fn request_continues_the_conversation() {
        let history = vec![
            LlmMessage::user("保存は?"),
            LlmMessage::assistant("Ctrl+S です"),
        ];
        let req = request(&history, "開くのは?", &HelpContext::default());
        assert_eq!(req.messages.len(), 3);
        assert_eq!(req.messages[2], LlmMessage::user("開くのは?"));
        assert_eq!(req.effort, Effort::Low);
    }

    #[test]
    fn markers_become_tools() {
        let answer = parse_answer(
            "1. 左のレールで「干渉」を押す\n2. 「今すぐチェック」を押す\n\n[[open:干渉チェック・計測]]\n[[open:存在しない道具]]\n[[open:安全]]\n",
        );
        assert_eq!(answer.tools, vec![Tool::Interference, Tool::Safety]);
        assert_eq!(
            answer.text,
            "1. 左のレールで「干渉」を押す\n2. 「今すぐチェック」を押す"
        );
    }

    #[test]
    fn unterminated_marker_is_kept_as_text() {
        let answer = parse_answer("手順です [[open:安全");
        assert!(answer.tools.is_empty());
        assert!(answer.text.starts_with("手順です"));
    }

    #[test]
    fn ask_uses_the_client() {
        let llm = ScriptedLlm::reply("レールの「部品」を押します\n[[open:部品ライブラリ]]");
        let answer = ask(&llm, &[], "シリンダを置きたい", &HelpContext::default()).unwrap();
        assert_eq!(answer.tools, vec![Tool::Library]);
        assert_eq!(llm.requests()[0].messages[0].text, "シリンダを置きたい");
    }

    #[test]
    fn offline_suggestions_follow_the_words() {
        assert_eq!(
            suggest_tools("部品がぶつかっていないか調べたい")[0],
            Tool::Interference
        );
        assert!(suggest_tools("非常停止を置きたい").contains(&Tool::Safety));
        assert!(suggest_tools("こんにちは").is_empty());
        assert!(suggest_tools("部品 干渉 安全 PLC 工程").len() <= 3);
    }
}
