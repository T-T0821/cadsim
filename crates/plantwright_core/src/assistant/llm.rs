//! 生成 AI (大規模言語モデル) を呼ぶためのポート
//!
//! ユースケースは [`LlmClient`] トレイトだけを知っていて、Claude API などの具体的な呼び出し方は
//! `crate::assistant::claude` が実装する。テストでは [`ScriptedLlm`] で決まった返事を返す。
//!
//! 返事を JSON で受け取りたいときは [`LlmRequest::json_schema`] に JSON Schema を入れる。
//! アダプターが構造化出力に対応していれば形が保証され、対応していなくても
//! [`extract_json`] で返事の中から JSON の部分を取り出せる。

use std::collections::VecDeque;
use std::fmt;
use std::sync::Mutex;

/// 会話の話し手
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// 会話の1発言
#[derive(Clone, Debug, PartialEq)]
pub struct LlmMessage {
    pub role: Role,
    pub text: String,
}

impl LlmMessage {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            text: text.into(),
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            text: text.into(),
        }
    }
}

/// どれだけ考えさせるか。深いほど良い答えになりやすいが、時間と料金がかかる。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Effort {
    Low,
    #[default]
    Medium,
    High,
}

/// 1回の問い合わせ
#[derive(Clone, Debug, PartialEq)]
pub struct LlmRequest {
    /// 役割や出力の決まり (システムプロンプト)
    pub system: String,
    /// 会話。最初と最後はユーザーの発言にする。
    pub messages: Vec<LlmMessage>,
    /// 返事の長さの上限 (トークン)
    pub max_tokens: u32,
    pub effort: Effort,
    /// 返事を JSON にさせるときの JSON Schema (JSON の文字列)。None なら自由な文章。
    pub json_schema: Option<String>,
}

impl LlmRequest {
    /// ユーザーの発言1つだけの問い合わせ
    pub fn new(system: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            messages: vec![LlmMessage::user(user)],
            max_tokens: 16_000,
            effort: Effort::default(),
            json_schema: None,
        }
    }

    pub fn with_json_schema(mut self, schema: impl Into<String>) -> Self {
        self.json_schema = Some(schema.into());
        self
    }

    pub fn with_effort(mut self, effort: Effort) -> Self {
        self.effort = effort;
        self
    }
}

/// 問い合わせの失敗
#[derive(Clone, Debug, PartialEq)]
pub enum LlmError {
    /// API キーが設定されていない
    MissingApiKey,
    /// つながらない・時間切れ
    Network(String),
    /// API がエラーを返した (キーが違う、混んでいる、など)
    Api { status: u16, message: String },
    /// 安全上の理由で答えが断られた
    Refused(String),
    /// 長さの上限で返事が途中で切れた
    Truncated,
    /// 返事の形が読めなかった
    InvalidResponse(String),
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::MissingApiKey => {
                write!(f, "API キーが設定されていません (ツール → AI の設定)")
            }
            LlmError::Network(e) => write!(f, "接続できませんでした: {e}"),
            LlmError::Api { status: 401, .. } => {
                write!(f, "API キーが正しくありません (401)")
            }
            LlmError::Api { status: 429, .. } => {
                write!(
                    f,
                    "混み合っているか利用上限に達しました。しばらく待ってからやり直してください (429)"
                )
            }
            LlmError::Api { status, message } => write!(f, "API エラー ({status}): {message}"),
            LlmError::Refused(why) if why.is_empty() => write!(f, "AI が回答を断りました"),
            LlmError::Refused(why) => write!(f, "AI が回答を断りました: {why}"),
            LlmError::Truncated => write!(
                f,
                "返事が長すぎて途中で切れました。説明を短く分けてやり直してください"
            ),
            LlmError::InvalidResponse(e) => write!(f, "返事を読み取れませんでした: {e}"),
        }
    }
}

impl std::error::Error for LlmError {}

/// 生成 AI への問い合わせ口 (トレイト)。時間がかかるので、UI からは別スレッドで呼ぶ。
pub trait LlmClient: Send + Sync {
    /// 問い合わせて、返事の本文 (文章、または JSON の文字列) を返す
    fn complete(&self, request: &LlmRequest) -> Result<String, LlmError>;
}

/// テスト用: 決まった返事を順に返し、受け取った問い合わせを記録する
#[derive(Default)]
pub struct ScriptedLlm {
    replies: Mutex<VecDeque<Result<String, LlmError>>>,
    requests: Mutex<Vec<LlmRequest>>,
}

impl ScriptedLlm {
    pub fn new(replies: impl IntoIterator<Item = Result<String, LlmError>>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
            requests: Mutex::default(),
        }
    }

    /// 1つの返事だけを返す
    pub fn reply(text: impl Into<String>) -> Self {
        Self::new([Ok(text.into())])
    }

    /// これまでに受け取った問い合わせ
    pub fn requests(&self) -> Vec<LlmRequest> {
        self.requests.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

impl LlmClient for ScriptedLlm {
    fn complete(&self, request: &LlmRequest) -> Result<String, LlmError> {
        if let Ok(mut requests) = self.requests.lock() {
            requests.push(request.clone());
        }
        self.replies
            .lock()
            .ok()
            .and_then(|mut r| r.pop_front())
            .unwrap_or_else(|| Err(LlmError::InvalidResponse("返事の用意がありません".into())))
    }
}

/// 返事から JSON の部分 (最初の `{` から対応する `}` まで) を取り出す。
/// コードブロック (```json … ```) や前後の文章が付いていてもよい。
pub fn extract_json(text: &str) -> Option<&str> {
    let start = text.find(['{', '['])?;
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_from_code_block() {
        let text = "はい。\n```json\n{\"a\": {\"b\": \"}{\"}, \"c\": [1, 2]}\n```\n以上です";
        assert_eq!(
            extract_json(text),
            Some("{\"a\": {\"b\": \"}{\"}, \"c\": [1, 2]}")
        );
    }

    #[test]
    fn extract_json_handles_escaped_quotes_and_unclosed() {
        assert_eq!(extract_json(r#"{"a": "x\"}"}"#), Some(r#"{"a": "x\"}"}"#));
        assert_eq!(extract_json("{\"a\": 1"), None);
        assert_eq!(extract_json("JSON はありません"), None);
    }

    #[test]
    fn scripted_llm_replies_in_order_and_records_requests() {
        let llm = ScriptedLlm::new([Ok("1".into()), Err(LlmError::Truncated)]);
        let req = LlmRequest::new("sys", "hello").with_effort(Effort::High);
        assert_eq!(llm.complete(&req), Ok("1".into()));
        assert_eq!(llm.complete(&req), Err(LlmError::Truncated));
        assert!(llm.complete(&req).is_err());
        assert_eq!(llm.requests().len(), 3);
        assert_eq!(llm.requests()[0].messages[0].text, "hello");
    }

    #[test]
    fn errors_read_as_guidance() {
        assert!(LlmError::MissingApiKey.to_string().contains("AI の設定"));
        let e = LlmError::Api {
            status: 401,
            message: String::new(),
        };
        assert!(e.to_string().contains("API キー"));
    }
}
