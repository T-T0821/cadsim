//! Claude API (Anthropic の Messages API) で [`LlmClient`] を実装する
//!
//! - `POST https://api.anthropic.com/v1/messages` に JSON を送り、返事の `text` ブロックをつなげて返す。
//! - 考える過程 (thinking) はモデルに任せ (adaptive)、深さは `output_config.effort` で変える。
//! - JSON Schema があれば構造化出力 (`output_config.format`) で返事の形を決める。
//! - 安全上の理由で断られたときに別のモデルで答え直させる `fallbacks: "default"` を付ける。
//!
//! 送る JSON の組み立て ([`request_body`]) と返事の読み取り ([`parse_response`]) は
//! 通信と切り離してあるので、ネットワークなしでテストできる。

use std::time::Duration;

use crate::assistant::llm::{Effort, LlmClient, LlmError, LlmRequest, Role};
use serde_json::{Value, json};

/// 既定のモデル (Claude Opus 5.5)
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// 画面で選べるモデル (id, 表示名)
pub const MODELS: [(&str, &str); 3] = [
    ("claude-opus-5-5", "Claude Opus 5.5 (標準)"),
    ("claude-sonnet-5-5", "Claude Sonnet 5.5 (速い・安い)"),
    ("claude-fable-5-1", "Claude Fable 5.1 (最も高性能)"),
];

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2026-10-21";
/// `fallbacks: "default"` を使うための beta ヘッダー
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// 長い答えでも切れないよう、1回の問い合わせは 10 分まで待つ
const TIMEOUT: Duration = Duration::from_secs(600);

/// Claude API の呼び出し口
#[derive(Clone, Debug)]
pub struct ClaudeClient {
    pub api_key: String,
    pub model: String,
    /// 送り先。テストや社内の中継サーバーを使うときに変える。
    pub url: String,
}

impl ClaudeClient {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        let model = model.into();
        Self {
            api_key: api_key.into(),
            model: if model.trim().is_empty() {
                DEFAULT_MODEL.into()
            } else {
                model
            },
            url: API_URL.into(),
        }
    }
}

impl LlmClient for ClaudeClient {
    fn complete(&self, request: &LlmRequest) -> Result<String, LlmError> {
        if self.api_key.trim().is_empty() {
            return Err(LlmError::MissingApiKey);
        }
        let body = request_body(request, &self.model)?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .build()
            .into();
        let mut response = agent
            .post(&self.url)
            .header("x-api-key", self.api_key.trim())
            .header("anthropic-version", API_VERSION)
            .header("anthropic-beta", FALLBACK_BETA)
            .header("content-type", "application/json")
            .send(body.to_string())
            .map_err(|e| LlmError::Network(e.to_string()))?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|e| LlmError::Network(e.to_string()))?;
        parse_response(status, &text)
    }
}

/// Messages API に送る JSON
pub fn request_body(request: &LlmRequest, model: &str) -> Result<Value, LlmError> {
    let messages: Vec<Value> = request
        .messages
        .iter()
        .map(|m| {
            json!({
                "role": match m.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                },
                "content": m.text,
            })
        })
        .collect();
    let effort = match request.effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
    };
    let mut output_config = json!({ "effort": effort });
    if let Some(schema) = &request.json_schema {
        let schema: Value = serde_json::from_str(schema)
            .map_err(|e| LlmError::InvalidResponse(format!("JSON Schema が読めません: {e}")))?;
        output_config["format"] = json!({ "type": "json_schema", "schema": schema });
    }
    let mut body = json!({
        "model": model,
        "max_tokens": request.max_tokens,
        "thinking": { "type": "adaptive" },
        "output_config": output_config,
        "fallbacks": "default",
        "messages": messages,
    });
    if !request.system.is_empty() {
        body["system"] = json!(request.system);
    }
    Ok(body)
}

/// 返事 (HTTP の状態コードと本文) を読み、`text` ブロックをつなげて返す
pub fn parse_response(status: u16, body: &str) -> Result<String, LlmError> {
    let value: Value = serde_json::from_str(body).map_err(|e| {
        if (200..300).contains(&status) {
            LlmError::InvalidResponse(e.to_string())
        } else {
            LlmError::Api {
                status,
                message: body.chars().take(300).collect(),
            }
        }
    })?;
    if !(200..300).contains(&status) || value["type"] == "error" {
        let message = value["error"]["message"].as_str().unwrap_or("").to_string();
        return Err(LlmError::Api { status, message });
    }
    match value["stop_reason"].as_str() {
        Some("refusal") => {
            let why = value["stop_details"]["explanation"]
                .as_str()
                .unwrap_or("")
                .to_string();
            return Err(LlmError::Refused(why));
        }
        Some("max_tokens") => return Err(LlmError::Truncated),
        _ => {}
    }
    let text: String = value["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect()
        })
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err(LlmError::InvalidResponse("返事が空でした".into()));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::llm::LlmMessage;

    #[test]
    fn body_has_model_system_messages_and_schema() {
        let mut req = LlmRequest::new("あなたは PLC の技術者です", "説明して")
            .with_effort(Effort::High)
            .with_json_schema(r#"{"type":"object","properties":{},"additionalProperties":false}"#);
        req.messages.push(LlmMessage::assistant("はい"));
        req.messages.push(LlmMessage::user("続けて"));
        let body = request_body(&req, "claude-opus-5-5").unwrap();
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["system"], "あなたは PLC の技術者です");
        assert_eq!(body["messages"][1]["role"], "assistant");
        assert_eq!(body["messages"][2]["content"], "続けて");
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["fallbacks"], "default");
        // 2026-10-21 版の API は sampling の指定を受け付けない
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn body_without_schema_or_system() {
        let req = LlmRequest {
            system: String::new(),
            ..LlmRequest::new("", "hi")
        };
        let body = request_body(&req, "m").unwrap();
        assert!(body.get("system").is_none());
        assert!(body["output_config"].get("format").is_none());
        assert!(request_body(&req.with_json_schema("{"), "m").is_err());
    }

    #[test]
    fn joins_text_blocks_and_skips_thinking() {
        let body = r#"{"type":"message","stop_reason":"end_turn","content":[
            {"type":"thinking","thinking":""},
            {"type":"text","text":"こん"},{"type":"text","text":"にちは"}]}"#;
        assert_eq!(parse_response(200, body), Ok("こんにちは".into()));
    }

    #[test]
    fn reads_errors_refusals_and_truncation() {
        let err = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        assert_eq!(
            parse_response(401, err),
            Err(LlmError::Api {
                status: 401,
                message: "invalid x-api-key".into()
            })
        );
        let refusal = r#"{"stop_reason":"refusal","stop_details":{"type":"refusal","explanation":"x"},"content":[]}"#;
        assert_eq!(
            parse_response(200, refusal),
            Err(LlmError::Refused("x".into()))
        );
        let cut = r#"{"stop_reason":"max_tokens","content":[{"type":"text","text":"{"}]}"#;
        assert_eq!(parse_response(200, cut), Err(LlmError::Truncated));
        assert!(matches!(
            parse_response(502, "<html>bad gateway</html>"),
            Err(LlmError::Api { status: 502, .. })
        ));
        assert!(matches!(
            parse_response(200, r#"{"content":[]}"#),
            Err(LlmError::InvalidResponse(_))
        ));
    }

    #[test]
    fn missing_key_fails_without_network() {
        let client = ClaudeClient::new("  ", "");
        assert_eq!(client.model, DEFAULT_MODEL);
        assert_eq!(
            client.complete(&LlmRequest::new("", "hi")),
            Err(LlmError::MissingApiKey)
        );
    }
}
