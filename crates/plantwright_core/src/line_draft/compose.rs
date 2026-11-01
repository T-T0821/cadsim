//! ユースケース: 言葉でラインを組む
//!
//! 文 (「コンベア2本とロボット1台で箱詰め」など) から、ラインの下書き
//! ([`LineDraft`]) と設備の並べ方を作る。
//!
//! 1. 生成 AI のポート [`LineDraftAi`] があれば、それに下書きを作らせる。
//! 2. AI がない・失敗したときは、文のキーワードから作る ([`rules::draft_from_text`])。
//! 3. どちらの下書きも [`check::normalize`] で整え、[`layout::layout`] で並べる。
//!
//! 下書きをモデルとタイミングチャートに置くのはアプリ側 (UI 層) の仕事。

use crate::line_draft::layout::{Placed, layout};
use crate::line_draft::{LineDraft, check, rules};

/// 生成 AI で下書きを作るポート。`line_draft::json` の `LlmLineDrafter` が実装する。
pub trait LineDraftAi {
    /// 文から下書きを作る。失敗したら理由の文を返す。
    fn draft(&self, request: &str) -> Result<LineDraft, String>;
}

/// 下書きを誰が作ったか
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DraftOrigin {
    /// 生成 AI
    Ai,
    /// 文のキーワード
    Rules,
}

impl DraftOrigin {
    pub fn label(self) -> &'static str {
        match self {
            DraftOrigin::Ai => "生成 AI",
            DraftOrigin::Rules => "キーワード",
        }
    }
}

/// 作った下書き
#[derive(Clone, Debug, PartialEq)]
pub struct ComposedLine {
    /// 元の文
    pub request: String,
    pub draft: LineDraft,
    /// `draft.equipment` と同じ順の並べ方
    pub placed: Vec<Placed>,
    pub origin: DraftOrigin,
    /// 直したところ・AI が使えなかった理由など
    pub warnings: Vec<String>,
}

/// 文から下書きを作る。AI でもキーワードでも作れなければ Err。
pub fn compose_line(request: &str, ai: Option<&dyn LineDraftAi>) -> Result<ComposedLine, String> {
    let request = request.trim();
    if request.is_empty() {
        return Err("作りたいラインを文で書いてください".into());
    }
    let mut warnings = Vec::new();
    let ai_result = ai.map(|ai| ai.draft(request).and_then(check::normalize));
    let (checked, origin) = match ai_result {
        Some(Ok(checked)) => (checked, DraftOrigin::Ai),
        Some(Err(e)) => {
            warnings.push(format!(
                "生成 AI で作れなかったので、キーワードから作りました ({e})"
            ));
            (rules_draft(request)?, DraftOrigin::Rules)
        }
        None => (rules_draft(request)?, DraftOrigin::Rules),
    };
    warnings.extend(checked.warnings);
    let placed = layout(&checked.draft);
    Ok(ComposedLine {
        request: request.to_string(),
        draft: checked.draft,
        placed,
        origin,
        warnings,
    })
}

fn rules_draft(request: &str) -> Result<check::Checked, String> {
    rules::draft_from_text(request).and_then(check::normalize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_draft::{Equipment, EquipmentKind};

    struct Fixed(Result<LineDraft, String>);

    impl LineDraftAi for Fixed {
        fn draft(&self, _: &str) -> Result<LineDraft, String> {
            self.0.clone()
        }
    }

    #[test]
    fn without_ai_uses_keywords() {
        let c = compose_line("コンベア2本とロボット1台で箱詰め", None).unwrap();
        assert_eq!(c.origin, DraftOrigin::Rules);
        assert_eq!(c.draft.equipment.len(), 3);
        assert_eq!(c.placed.len(), 3);
        assert!(c.warnings.is_empty());
    }

    #[test]
    fn uses_ai_draft_when_it_works() {
        let draft = LineDraft {
            title: "AI".into(),
            equipment: vec![Equipment::new("A", EquipmentKind::Table)],
            ..Default::default()
        };
        let ai = Fixed(Ok(draft));
        let c = compose_line("何か", Some(&ai)).unwrap();
        assert_eq!(c.origin, DraftOrigin::Ai);
        assert_eq!(c.draft.title, "AI");
    }

    #[test]
    fn falls_back_when_ai_fails_or_returns_nothing() {
        let ai = Fixed(Err("通信エラー".into()));
        let c = compose_line("ロボット1台と作業台2つ", Some(&ai)).unwrap();
        assert_eq!(c.origin, DraftOrigin::Rules);
        assert!(c.warnings[0].contains("通信エラー"));

        let ai = Fixed(Ok(LineDraft::default()));
        let c = compose_line("ロボット1台と作業台2つ", Some(&ai)).unwrap();
        assert_eq!(c.origin, DraftOrigin::Rules);
    }

    #[test]
    fn empty_or_unreadable_request_is_an_error() {
        assert!(compose_line("  ", None).is_err());
        assert!(compose_line("よろしく", None).is_err());
    }
}
