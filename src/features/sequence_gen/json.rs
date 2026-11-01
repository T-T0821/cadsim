//! 動作の下書き ([`SequenceSpec`]) と JSON の変換 (アダプター)
//!
//! 生成 AI には [`SCHEMA`] の形の JSON で答えさせ、[`parse`] で下書きに戻す。
//! 今のチャートを生成 AI に伝えるときも同じ形 ([`render`]) にする。

use serde::{Deserialize, Serialize};

use super::spec::{ActionSpec, ActuatorSpec, InterlockSpec, SensorSpec, SequenceSpec, StepSpec};
use plantwright_core::motion::chart::ActuatorKind;
use plantwright_core::motion::chart::SensorKind;

/// 生成 AI に渡す JSON Schema (構造化出力の決まりに合わせ、すべての項目を必須・余分な項目なしにする)
pub const SCHEMA: &str = r#"{
  "type": "object",
  "additionalProperties": false,
  "required": ["actuators", "sensors", "steps", "notes"],
  "properties": {
    "actuators": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["name", "kind", "stroke", "default_duration"],
        "properties": {
          "name": {"type": "string"},
          "kind": {"type": "string", "enum": ["cylinder", "linear_axis", "rotary_table"]},
          "stroke": {"type": "number"},
          "default_duration": {"type": "number"}
        }
      }
    },
    "sensors": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["name", "kind"],
        "properties": {
          "name": {"type": "string"},
          "kind": {"type": "string", "enum": ["photoelectric", "proximity"]}
        }
      }
    },
    "steps": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["comment", "interlocks", "actions", "dwell"],
        "properties": {
          "comment": {"type": "string"},
          "interlocks": {
            "type": "array",
            "items": {
              "type": "object",
              "additionalProperties": false,
              "required": ["sensor", "on"],
              "properties": {
                "sensor": {"type": "string"},
                "on": {"type": "boolean"}
              }
            }
          },
          "actions": {
            "type": "array",
            "items": {
              "type": "object",
              "additionalProperties": false,
              "required": ["actuator", "target", "duration"],
              "properties": {
                "actuator": {"type": "string"},
                "target": {"type": "number"},
                "duration": {"type": "number"}
              }
            }
          },
          "dwell": {"type": "number"}
        }
      }
    },
    "notes": {"type": "string"}
  }
}"#;

#[derive(Serialize, Deserialize)]
struct SpecRecord {
    #[serde(default)]
    actuators: Vec<ActuatorRecord>,
    #[serde(default)]
    sensors: Vec<SensorRecord>,
    #[serde(default)]
    steps: Vec<StepRecord>,
    #[serde(default)]
    notes: String,
}

#[derive(Serialize, Deserialize)]
struct ActuatorRecord {
    name: String,
    kind: String,
    #[serde(default)]
    stroke: f64,
    #[serde(default)]
    default_duration: f64,
}

#[derive(Serialize, Deserialize)]
struct SensorRecord {
    name: String,
    #[serde(default)]
    kind: String,
}

#[derive(Serialize, Deserialize)]
struct StepRecord {
    #[serde(default)]
    comment: String,
    #[serde(default)]
    interlocks: Vec<InterlockRecord>,
    #[serde(default)]
    actions: Vec<ActionRecord>,
    #[serde(default)]
    dwell: f64,
}

#[derive(Serialize, Deserialize)]
struct InterlockRecord {
    sensor: String,
    on: bool,
}

#[derive(Serialize, Deserialize)]
struct ActionRecord {
    actuator: String,
    target: f64,
    #[serde(default)]
    duration: f64,
}

fn kind_name(kind: ActuatorKind) -> &'static str {
    match kind {
        ActuatorKind::Cylinder => "cylinder",
        ActuatorKind::LinearAxis => "linear_axis",
        ActuatorKind::RotaryTable => "rotary_table",
    }
}

fn parse_kind(name: &str) -> Option<ActuatorKind> {
    match name {
        "cylinder" => Some(ActuatorKind::Cylinder),
        "linear_axis" => Some(ActuatorKind::LinearAxis),
        "rotary_table" => Some(ActuatorKind::RotaryTable),
        _ => None,
    }
}

/// 生成 AI の返事 (JSON) を下書きにする。前後に文章やコードブロックが付いていてもよい。
pub fn parse(text: &str) -> Result<SequenceSpec, String> {
    let json = plantwright_core::assistant::llm::extract_json(text)
        .ok_or_else(|| "返事に JSON がありませんでした".to_string())?;
    let record: SpecRecord =
        serde_json::from_str(json).map_err(|e| format!("JSON の形が違います: {e}"))?;
    let actuators = record
        .actuators
        .into_iter()
        .map(|a| {
            let kind = parse_kind(&a.kind)
                .ok_or_else(|| format!("装置「{}」の種類「{}」が分かりません", a.name, a.kind))?;
            Ok(ActuatorSpec {
                name: a.name,
                kind,
                stroke: a.stroke,
                default_duration: a.default_duration,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(SequenceSpec {
        actuators,
        sensors: record
            .sensors
            .into_iter()
            .map(|s| SensorSpec {
                name: s.name,
                kind: if s.kind == "proximity" {
                    SensorKind::Proximity
                } else {
                    SensorKind::Photoelectric
                },
            })
            .collect(),
        steps: record
            .steps
            .into_iter()
            .map(|s| StepSpec {
                comment: s.comment,
                interlocks: s
                    .interlocks
                    .into_iter()
                    .map(|i| InterlockSpec {
                        sensor: i.sensor,
                        on: i.on,
                    })
                    .collect(),
                actions: s
                    .actions
                    .into_iter()
                    .map(|a| ActionSpec {
                        actuator: a.actuator,
                        target: a.target,
                        duration: a.duration,
                    })
                    .collect(),
                dwell: s.dwell,
            })
            .collect(),
        notes: record.notes,
    })
}

/// 下書きを JSON にする (今のチャートを生成 AI に伝える用)
pub fn render(spec: &SequenceSpec) -> String {
    let record = SpecRecord {
        actuators: spec
            .actuators
            .iter()
            .map(|a| ActuatorRecord {
                name: a.name.clone(),
                kind: kind_name(a.kind).into(),
                stroke: a.stroke,
                default_duration: a.default_duration,
            })
            .collect(),
        sensors: spec
            .sensors
            .iter()
            .map(|s| SensorRecord {
                name: s.name.clone(),
                kind: match s.kind {
                    SensorKind::Photoelectric => "photoelectric",
                    SensorKind::Proximity => "proximity",
                }
                .into(),
            })
            .collect(),
        steps: spec
            .steps
            .iter()
            .map(|s| StepRecord {
                comment: s.comment.clone(),
                interlocks: s
                    .interlocks
                    .iter()
                    .map(|i| InterlockRecord {
                        sensor: i.sensor.clone(),
                        on: i.on,
                    })
                    .collect(),
                actions: s
                    .actions
                    .iter()
                    .map(|a| ActionRecord {
                        actuator: a.actuator.clone(),
                        target: a.target,
                        duration: a.duration,
                    })
                    .collect(),
                dwell: s.dwell,
            })
            .collect(),
        notes: spec.notes.clone(),
    };
    serde_json::to_string_pretty(&record).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::sequence_gen::spec::tests::sample_spec;

    #[test]
    fn schema_is_valid_json() {
        let value: serde_json::Value = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(value["required"][2], "steps");
    }

    #[test]
    fn render_and_parse_round_trip() {
        let spec = sample_spec();
        let back = parse(&format!("```json\n{}\n```", render(&spec))).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn parse_reports_problems() {
        assert!(parse("ごめんなさい").is_err());
        assert!(parse(r#"{"steps": 3}"#).is_err());
        let err = parse(r#"{"actuators":[{"name":"A","kind":"robot"}]}"#).unwrap_err();
        assert!(err.contains("robot"));
        // 足りない項目は空で読む
        let spec = parse(r#"{"steps":[{"comment":"待つ","dwell":1}]}"#).unwrap();
        assert_eq!(spec.steps[0].dwell, 1.0);
    }
}
