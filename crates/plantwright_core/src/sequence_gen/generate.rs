//! ユースケース: 説明文からタイミングチャートの下書きを作る / チャート・ラダー・ログから動作を説明させる
//!
//! 生成 AI はポート [`LlmClient`] で受け取る。Bevy・egui に依存しない。

use crate::assistant::llm::{Effort, LlmClient, LlmError, LlmRequest};

use super::json;
use super::log_events::LogSummary;
use super::spec::{SequenceSpec, describe};

/// 一度に送る材料の上限 (文字数)。超えたら黙って切らず、短くしてもらう。
pub const MAX_INPUT_CHARS: usize = 400_000;

const GENERATE_SYSTEM: &str = "\
あなたは工場の自動機 (FA) のシーケンス設計者です。ユーザーの動作の説明から、plantwright のタイミングチャートの下書きを JSON で作ります。

決まり:
- 装置の種類は cylinder (エアシリンダ。target は 1 = 出端、0 = 戻端)、linear_axis (直動軸。target は原点からの移動量で単位は mm)、rotary_table (回転テーブル。target は角度で単位は度) の3つです。ロボットは扱いません。
- 今のチャートにある装置・センサーは、同じ名前のまま使ってください。新しく要る装置・センサーだけを足し、名前は日本語で短く (例: リフタ、クランプ、ワーク有無)。
- ステップは上から順に実行されます。同じステップの動作は同時に始まり、すべて終わってから dwell 秒待って次のステップへ進みます。
- interlocks はそのステップの開始条件です (on が true ならセンサーが ON、false なら OFF)。ワーク有無・安全扉・前の工程の完了など、説明に条件があれば必ずセンサーと条件にしてください。
- duration は動作時間 (秒)、stroke はシリンダのストローク (mm)、default_duration は装置の標準の動作時間 (秒) です。説明から分からなければ 0 にします (plantwright の標準値が入ります)。
- 1サイクルの終わりには、装置を始めの位置に戻してください。
- comment はステップの内容を日本語で短く (例: ワーク把持)。
- 説明から決められなかったこと、置いた前提は notes に日本語で短く書いてください。";

const EXPLAIN_SYSTEM: &str = "\
あなたは PLC とシーケンス制御に詳しい技術者です。渡された材料から、設備が何をしているかを現場の人向けに日本語で説明します。

次の順で書いてください。Markdown の見出し記号 (#) は使わず、番号と箇条書きにします。
1. 概要 (2〜3文)
2. 動作の流れ (ステップごとに1行ずつ)
3. インターロックと異常のときの動き
4. 気になる点 (装置の戻し忘れ、条件のない動作、止まりそうな所、デバイスの重複など。なければ「特になし」)

材料に書かれていないことは、推測であると分かるように書いてください。";

/// 説明文から下書きを作る問い合わせ
pub fn generate_request(description: &str, current: &SequenceSpec) -> LlmRequest {
    let user = format!(
        "今のタイミングチャート:\n```json\n{}\n```\n\n動作の説明:\n{}",
        json::render(current),
        description.trim()
    );
    LlmRequest::new(GENERATE_SYSTEM, user)
        .with_json_schema(json::SCHEMA)
        .with_effort(Effort::High)
}

/// 説明文から下書きを作る
pub fn generate(
    llm: &dyn LlmClient,
    description: &str,
    current: &SequenceSpec,
) -> Result<SequenceSpec, LlmError> {
    if description.trim().is_empty() {
        return Err(LlmError::InvalidResponse(
            "動作の説明を入力してください".into(),
        ));
    }
    check_size(description)?;
    let reply = llm.complete(&generate_request(description, current))?;
    json::parse(&reply).map_err(LlmError::InvalidResponse)
}

/// 説明させる材料
#[derive(Clone, Debug, PartialEq)]
pub enum ExplainSource {
    /// 今のタイミングチャート
    Chart(SequenceSpec),
    /// ラダー (命令リストやテキストに書き出したもの)
    Ladder(String),
    /// PLC のログから取り出した変化の一覧
    Log(LogSummary),
}

/// 説明させる問い合わせ
pub fn explain_request(source: &ExplainSource) -> LlmRequest {
    let user = match source {
        ExplainSource::Chart(spec) => {
            format!(
                "plantwright のタイミングチャートです。\n\n{}",
                describe(spec)
            )
        }
        ExplainSource::Ladder(text) => format!(
            "PLC のラダー (命令リストなど) です。デバイスのコメントがあれば参考にしてください。\n```\n{}\n```",
            text.trim()
        ),
        ExplainSource::Log(log) => {
            let mut head = format!(
                "PLC のログから、信号が変わった所だけを取り出したものです ({} 行、{} 信号)。",
                log.rows, log.signals
            );
            if log.omitted > 0 {
                head += &format!(
                    "長いため、後半の {} 件の変化は省いています。省いた部分には触れず、分かる範囲で説明してください。",
                    log.omitted
                );
            }
            format!("{head}\n```\n{}```", log.text)
        }
    };
    LlmRequest::new(EXPLAIN_SYSTEM, user)
}

/// 動作を説明させる
pub fn explain(llm: &dyn LlmClient, source: &ExplainSource) -> Result<String, LlmError> {
    match source {
        ExplainSource::Chart(spec) if spec.steps.is_empty() => {
            return Err(LlmError::InvalidResponse(
                "タイミングチャートにステップがありません".into(),
            ));
        }
        ExplainSource::Ladder(text) if text.trim().is_empty() => {
            return Err(LlmError::InvalidResponse(
                "ラダーを貼り付けるか、ファイルを開いてください".into(),
            ));
        }
        ExplainSource::Ladder(text) => check_size(text)?,
        ExplainSource::Log(log) => check_size(&log.text)?,
        ExplainSource::Chart(_) => {}
    }
    llm.complete(&explain_request(source))
}

fn check_size(text: &str) -> Result<(), LlmError> {
    let chars = text.chars().count();
    if chars > MAX_INPUT_CHARS {
        return Err(LlmError::InvalidResponse(format!(
            "材料が長すぎます ({chars} 文字、上限 {MAX_INPUT_CHARS} 文字)。分けて送ってください"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::llm::ScriptedLlm;
    use crate::sequence_gen::spec::tests::sample_spec;

    #[test]
    fn generate_sends_current_chart_and_schema_and_parses_reply() {
        let llm = ScriptedLlm::reply(json::render(&sample_spec()));
        let current = SequenceSpec {
            actuators: sample_spec().actuators,
            ..Default::default()
        };
        let spec = generate(&llm, "ワークが来たらリフタを上げる", &current).unwrap();
        assert_eq!(spec, sample_spec());
        let req = &llm.requests()[0];
        assert!(req.json_schema.is_some());
        assert!(req.messages[0].text.contains("\"リフタ\""));
        assert!(
            req.messages[0]
                .text
                .contains("ワークが来たらリフタを上げる")
        );
    }

    #[test]
    fn generate_rejects_empty_and_bad_replies() {
        let llm = ScriptedLlm::reply("できません");
        assert!(generate(&llm, "  ", &SequenceSpec::default()).is_err());
        assert!(llm.requests().is_empty(), "空の説明は送らない");
        assert!(matches!(
            generate(&llm, "上げる", &SequenceSpec::default()),
            Err(LlmError::InvalidResponse(_))
        ));
        let long = "あ".repeat(MAX_INPUT_CHARS + 1);
        assert!(generate(&llm, &long, &SequenceSpec::default()).is_err());
    }

    #[test]
    fn explain_builds_prompt_for_each_source() {
        let llm = ScriptedLlm::new([Ok("説明1".into()), Ok("説明2".into()), Ok("説明3".into())]);
        assert_eq!(
            explain(&llm, &ExplainSource::Chart(sample_spec())),
            Ok("説明1".into())
        );
        explain(&llm, &ExplainSource::Ladder("LD X0\nOUT Y20".into())).unwrap();
        let log = LogSummary {
            text: "初期状態: Y10=0\n".into(),
            rows: 10,
            signals: 1,
            events: 0,
            omitted: 5,
        };
        explain(&llm, &ExplainSource::Log(log)).unwrap();
        let reqs = llm.requests();
        assert!(reqs[0].messages[0].text.contains("S1 ワーク到着待ちで上昇"));
        assert!(reqs[1].messages[0].text.contains("LD X0"));
        assert!(
            reqs[2].messages[0]
                .text
                .contains("5 件の変化は省いています")
        );
        assert!(reqs.iter().all(|r| r.json_schema.is_none()));
    }

    #[test]
    fn explain_rejects_empty_sources() {
        let llm = ScriptedLlm::default();
        assert!(explain(&llm, &ExplainSource::Chart(SequenceSpec::default())).is_err());
        assert!(explain(&llm, &ExplainSource::Ladder(" ".into())).is_err());
        assert!(llm.requests().is_empty());
    }
}
