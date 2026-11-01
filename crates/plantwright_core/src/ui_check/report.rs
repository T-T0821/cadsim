//! 写真ごとの判定と、手順書ごとの結果

use super::Rect;
use super::compare::{Image, Tolerance, compare};
use super::runner::Failure;

/// 写真 1 枚の判定
#[derive(Clone, Debug, PartialEq)]
pub enum ShotStatus {
    /// 基準画像と同じ (違う画素の割合が許容以内)
    Same { ratio: f64 },
    /// 基準画像と違う
    Different { ratio: f64 },
    /// 画像の大きさが違う
    SizeMismatch {
        expected: (u32, u32),
        actual: (u32, u32),
    },
    /// 基準画像がなかったので、今回の写真を基準にした
    NewBaseline,
    /// 基準画像を今回の写真で置き換えた (`--update`)
    Updated,
}

impl ShotStatus {
    pub fn passed(&self) -> bool {
        !matches!(
            self,
            ShotStatus::Different { .. } | ShotStatus::SizeMismatch { .. }
        )
    }
}

/// 判定と、その結果どうするか
#[derive(Debug)]
pub struct Judgement {
    pub status: ShotStatus,
    /// 差分画像 (基準画像と比べたときだけ)
    pub diff: Option<Image>,
    /// 今回の写真を基準画像として保存する
    pub save_as_baseline: bool,
}

/// 写真を基準画像と比べて判定する。`update` なら比べずに基準画像を置き換える
pub fn judge(
    baseline: Option<&Image>,
    actual: &Image,
    tol: Tolerance,
    masks: &[Rect],
    update: bool,
) -> Judgement {
    let Some(baseline) = baseline else {
        return Judgement {
            status: ShotStatus::NewBaseline,
            diff: None,
            save_as_baseline: true,
        };
    };
    if update {
        return Judgement {
            status: ShotStatus::Updated,
            diff: None,
            save_as_baseline: true,
        };
    }
    let result = compare(baseline, actual, tol, masks);
    let status = if let Some((expected, actual)) = result.size_mismatch {
        ShotStatus::SizeMismatch { expected, actual }
    } else if result.passed {
        ShotStatus::Same {
            ratio: result.ratio(),
        }
    } else {
        ShotStatus::Different {
            ratio: result.ratio(),
        }
    };
    Judgement {
        status,
        diff: Some(result.diff),
        save_as_baseline: false,
    }
}

/// 写真 1 枚の結果
#[derive(Clone, Debug, PartialEq)]
pub struct ShotResult {
    pub name: String,
    pub status: ShotStatus,
}

/// 手順書 1 つの結果
#[derive(Clone, Debug, PartialEq)]
pub struct ScenarioResult {
    pub name: String,
    /// 手順が途中で止まった理由
    pub failure: Option<Failure>,
    pub shots: Vec<ShotResult>,
}

impl ScenarioResult {
    pub fn passed(&self) -> bool {
        self.failure.is_none() && self.shots.iter().all(|s| s.status.passed())
    }

    /// 端末に出す数行の要約
    pub fn summary(&self) -> String {
        let mark = if self.passed() { "OK  " } else { "NG  " };
        let mut out = format!("{mark}{}\n", self.name);
        if let Some(f) = &self.failure {
            out.push_str(&format!(
                "      手順 {} 行目で失敗: {}\n",
                f.line, f.message
            ));
        }
        for shot in &self.shots {
            out.push_str(&format!(
                "      {:<28} {}\n",
                shot.name,
                status_text(&shot.status)
            ));
        }
        out
    }
}

/// 判定を短い文にする
pub fn status_text(status: &ShotStatus) -> String {
    match status {
        ShotStatus::Same { ratio } => format!("同じ (違い {:.3}%)", ratio * 100.0),
        ShotStatus::Different { ratio } => format!("違う (違い {:.3}%)", ratio * 100.0),
        ShotStatus::SizeMismatch { expected, actual } => format!(
            "大きさが違う (基準 {}×{}、今回 {}×{})",
            expected.0, expected.1, actual.0, actual.1
        ),
        ShotStatus::NewBaseline => "基準画像がないので今回の写真を基準にした".into(),
        ShotStatus::Updated => "基準画像を今回の写真で置き換えた".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(v: u8) -> Image {
        Image::new(4, 4, vec![v; 64])
    }

    #[test]
    fn 基準画像がなければ今回を基準にする() {
        let j = judge(None, &img(0), Tolerance::default(), &[], false);
        assert_eq!(j.status, ShotStatus::NewBaseline);
        assert!(j.save_as_baseline);
        assert!(j.status.passed());
    }

    #[test]
    fn 置き換えでは比べない() {
        let j = judge(Some(&img(0)), &img(255), Tolerance::default(), &[], true);
        assert_eq!(j.status, ShotStatus::Updated);
        assert!(j.save_as_baseline);
    }

    #[test]
    fn 違えば不合格で差分を返す() {
        let j = judge(Some(&img(0)), &img(255), Tolerance::default(), &[], false);
        assert_eq!(j.status, ShotStatus::Different { ratio: 1.0 });
        assert!(!j.save_as_baseline);
        assert!(j.diff.is_some());

        let result = ScenarioResult {
            name: "s".into(),
            failure: None,
            shots: vec![ShotResult {
                name: "a".into(),
                status: j.status,
            }],
        };
        assert!(!result.passed());
        assert!(result.summary().starts_with("NG"));
    }

    #[test]
    fn 手順の失敗も不合格() {
        let result = ScenarioResult {
            name: "s".into(),
            failure: Some(Failure {
                line: 4,
                message: "x".into(),
            }),
            shots: vec![],
        };
        assert!(!result.passed());
        assert!(result.summary().contains("4 行目"));
    }
}
