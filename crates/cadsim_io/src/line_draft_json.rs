//! 生成 AI とやり取りするラインの下書きの JSON
//!
//! - [`SYSTEM_PROMPT`] … 下書きを JSON で返してもらうための指示
//! - [`SCHEMA`] … 返事の形 (構造化出力の JSON Schema)。省略できる項目は空文字・0 で表す
//! - [`parse`] … 返ってきた JSON (前後に文があってもよい) を [`LineDraft`] にする
//! - [`LlmLineDrafter`] … 共通の LLM クライアントで下書きを作る、ポート [`LineDraftAi`] の実装
//!
//! 形が多少崩れていても読める分だけ読む。中身の正しさは
//! `cadsim_core::line_draft::check::normalize` が後で整える。

use serde_json::Value;

use cadsim_core::line_draft::{DraftAction, DraftStep, Equipment, EquipmentKind, LineDraft, Verb};
use cadsim_core::usecase::compose_line::LineDraftAi;
use cadsim_core::usecase::llm::{LlmClient, LlmRequest, extract_json};

/// 生成 AI への指示 (システムプロンプト)
pub const SYSTEM_PROMPT: &str = r#"あなたは工場の自動化ラインの設計者です。
ユーザーが書いたラインの説明から、3D の設備配置とタイミングチャートの下書きを作ります。
次の形の JSON だけを返してください。説明の文は notes に入れます。

{
  "title": "ラインの名前 (例: 箱詰めライン)",
  "equipment": [
    {"name": "設備の名前 (ライン内で一意)", "kind": "conveyor | robot | cylinder | sensor | table",
     "length": 0}
  ],
  "steps": [
    {"comment": "ステップの説明 (例: 製品を取る)",
     "wait_sensor": "開始前に待つセンサーの名前 (なければ空文字)", "wait_on": true,
     "dwell": 0,
     "actions": [
       {"equipment": "動かす設備の名前", "verb": "run | extend | retract | pick | place",
        "target": "pick / place の場所 (コンベアか作業台の名前。それ以外は空文字)", "seconds": 1.5}
     ]}
  ],
  "notes": ["前提や、決めきれなかったこと"]
}

決まり:
- 単位は mm と秒。設備の位置は自動で並べる。length はコンベアの長さで、指定がなければ 0 (標準の 2000 mm)。
- conveyor は run (ワークを送る)、cylinder は extend / retract (ストッパ・プッシャ・位置決め)、robot は pick / place だけができる。
- robot の pick / place の target は conveyor か table の名前にする。
- sensor はワークの有無を見る光電センサー。ワークが来るのを待つステップに wait_sensor で使う。
- 同じステップの動作は同時に始まる。1つのステップで同じ設備は1回だけ動かす。
- ユーザーが台数を書いた設備はその台数にする。書いていない設備は、必要なら最小限だけ足し、notes にそう書く。
- 動作時間の目安: コンベアの搬送 2〜5 秒、シリンダ 0.3〜1 秒、ロボットの取る・置く 1〜2 秒、検査は dwell で 1〜3 秒。
- 名前と説明は日本語にする。"#;

/// 返事の形 (構造化出力の JSON Schema)
pub const SCHEMA: &str = r#"{
  "type": "object",
  "additionalProperties": false,
  "required": ["title", "equipment", "steps", "notes"],
  "properties": {
    "title": {"type": "string"},
    "equipment": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["name", "kind", "length"],
        "properties": {
          "name": {"type": "string"},
          "kind": {"type": "string", "enum": ["conveyor", "robot", "cylinder", "sensor", "table"]},
          "length": {"type": "number"}
        }
      }
    },
    "steps": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["comment", "wait_sensor", "wait_on", "dwell", "actions"],
        "properties": {
          "comment": {"type": "string"},
          "wait_sensor": {"type": "string"},
          "wait_on": {"type": "boolean"},
          "dwell": {"type": "number"},
          "actions": {
            "type": "array",
            "items": {
              "type": "object",
              "additionalProperties": false,
              "required": ["equipment", "verb", "target", "seconds"],
              "properties": {
                "equipment": {"type": "string"},
                "verb": {"type": "string", "enum": ["run", "extend", "retract", "pick", "place"]},
                "target": {"type": "string"},
                "seconds": {"type": "number"}
              }
            }
          }
        }
      }
    },
    "notes": {"type": "array", "items": {"type": "string"}}
  }
}"#;

/// 共通の LLM クライアント ([`LlmClient`]) で下書きを作る
pub struct LlmLineDrafter<'a> {
    pub client: &'a dyn LlmClient,
}

impl LineDraftAi for LlmLineDrafter<'_> {
    fn draft(&self, request: &str) -> Result<LineDraft, String> {
        let request = LlmRequest::new(SYSTEM_PROMPT, request).with_json_schema(SCHEMA);
        let reply = self.client.complete(&request).map_err(|e| e.to_string())?;
        parse(&reply)
    }
}

/// 返ってきた文から JSON を取り出して下書きにする
pub fn parse(text: &str) -> Result<LineDraft, String> {
    let json = extract_json(text).ok_or("返答に JSON が見つかりませんでした")?;
    let value: Value =
        serde_json::from_str(json).map_err(|e| format!("返答の JSON を読めませんでした: {e}"))?;
    from_value(&value)
}

fn from_value(v: &Value) -> Result<LineDraft, String> {
    let obj = v
        .as_object()
        .ok_or("返答の JSON がオブジェクトではありません")?;
    let text = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").to_string();
    let number = |v: Option<&Value>| v.and_then(Value::as_f64);
    let list = |key: &str| {
        obj.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };

    let mut skipped = Vec::new();
    let equipment = list("equipment")
        .iter()
        .filter_map(|e| {
            let kind_text = text(e.get("kind"));
            let Some(kind) = EquipmentKind::parse(&kind_text) else {
                skipped.push(format!("知らない設備の種類「{kind_text}」を外しました"));
                return None;
            };
            let position = match (number(e.get("x")), number(e.get("z"))) {
                (Some(x), Some(z)) => Some([x, z]),
                _ => None,
            };
            Some(Equipment {
                name: text(e.get("name")),
                kind,
                position,
                length: number(e.get("length")),
            })
        })
        .collect();

    let steps = list("steps")
        .iter()
        .map(|s| {
            let actions = s
                .get("actions")
                .and_then(Value::as_array)
                .map(|a| a.as_slice())
                .unwrap_or_default()
                .iter()
                .filter_map(|a| {
                    let verb_text = text(a.get("verb"));
                    let Some(verb) = Verb::parse(&verb_text) else {
                        skipped.push(format!("知らない動作「{verb_text}」を外しました"));
                        return None;
                    };
                    let target = text(a.get("target"));
                    Some(DraftAction {
                        equipment: text(a.get("equipment")),
                        verb,
                        target: (!target.is_empty()).then_some(target),
                        seconds: number(a.get("seconds")).unwrap_or(f64::NAN),
                    })
                })
                .collect();
            let sensor = text(s.get("wait_sensor"));
            let on = s.get("wait_on").and_then(Value::as_bool).unwrap_or(true);
            DraftStep {
                comment: text(s.get("comment")),
                actions,
                wait_for: (!sensor.is_empty()).then_some((sensor, on)),
                dwell: number(s.get("dwell")).unwrap_or(0.0),
            }
        })
        .collect();

    let mut notes: Vec<String> = list("notes")
        .iter()
        .filter_map(|n| n.as_str().map(String::from))
        .collect();
    notes.extend(skipped);

    Ok(LineDraft {
        title: text(obj.get("title")),
        equipment,
        steps,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"はい、下書きです。
```json
{
  "title": "箱詰めライン",
  "equipment": [
    {"name": "製品コンベア", "kind": "conveyor", "length": 2500},
    {"name": "箱コンベア", "kind": "conveyor", "x": 0, "z": 1600},
    {"name": "ロボット1", "kind": "robot"},
    {"name": "ワーク検出", "kind": "sensor"},
    {"name": "謎の装置", "kind": "teleporter"}
  ],
  "steps": [
    {"comment": "製品を送る", "actions": [{"equipment": "製品コンベア", "verb": "run", "seconds": 3}]},
    {"comment": "製品を取る {注意}", "wait_sensor": "ワーク検出", "wait_on": true,
     "actions": [{"equipment": "ロボット1", "verb": "pick", "target": "製品コンベア", "seconds": 1.5},
                 {"equipment": "ロボット1", "verb": "fly"}]}
  ],
  "notes": ["箱の供給は手作業とした"]
}
```
以上です。"#;

    #[test]
    fn parses_a_fenced_reply() {
        let d = parse(SAMPLE).unwrap();
        assert_eq!(d.title, "箱詰めライン");
        assert_eq!(d.equipment.len(), 4);
        assert_eq!(d.equipment[0].length, Some(2500.0));
        assert_eq!(d.equipment[0].position, None);
        assert_eq!(d.equipment[1].position, Some([0.0, 1600.0]));
        assert_eq!(d.steps.len(), 2);
        assert_eq!(d.steps[1].comment, "製品を取る {注意}");
        assert_eq!(d.steps[1].wait_for, Some(("ワーク検出".into(), true)));
        assert_eq!(d.steps[1].actions.len(), 1);
        assert_eq!(
            d.steps[1].actions[0].target.as_deref(),
            Some("製品コンベア")
        );
        // 知らない種類・動作はメモに残す
        assert_eq!(d.notes.len(), 3);
    }

    #[test]
    fn parsed_draft_normalizes_cleanly() {
        let d = parse(SAMPLE).unwrap();
        let c = cadsim_core::line_draft::check::normalize(d).unwrap();
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }

    #[test]
    fn schema_is_valid_json() {
        let v: Value = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(v["required"][1], "equipment");
    }

    #[test]
    fn drafter_sends_prompt_and_schema_and_parses_reply() {
        use cadsim_core::usecase::llm::{LlmError, ScriptedLlm};
        let llm = ScriptedLlm::reply(SAMPLE);
        let d = LlmLineDrafter { client: &llm }.draft("箱詰め").unwrap();
        assert_eq!(d.title, "箱詰めライン");
        let sent = llm.requests();
        assert_eq!(sent[0].json_schema.as_deref(), Some(SCHEMA));
        assert_eq!(sent[0].messages[0].text, "箱詰め");

        let llm = ScriptedLlm::new([Err(LlmError::MissingApiKey)]);
        let e = LlmLineDrafter { client: &llm }.draft("x").unwrap_err();
        assert!(e.contains("API キー"));
    }

    #[test]
    fn empty_strings_and_zero_length_mean_unset() {
        let d = parse(
            r#"{"title":"t","equipment":[{"name":"c","kind":"conveyor","length":0}],
                "steps":[{"comment":"s","wait_sensor":"","wait_on":true,"dwell":0,
                          "actions":[{"equipment":"c","verb":"run","target":"","seconds":2}]}],
                "notes":[]}"#,
        )
        .unwrap();
        assert_eq!(d.steps[0].wait_for, None);
        assert_eq!(d.steps[0].actions[0].target, None);
        let c = cadsim_core::line_draft::check::normalize(d).unwrap();
        assert_eq!(c.draft.equipment[0].length, None);
    }

    #[test]
    fn rejects_replies_without_json() {
        assert!(parse("ごめんなさい、作れません").is_err());
        assert!(parse("{ \"title\": ").is_err());
        assert!(parse("[1, 2]").is_err());
    }
}
