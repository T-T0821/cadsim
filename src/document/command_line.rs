//! コマンドライン引数 (入力)
//!
//! `cargo run -- 部品.step --stress 5000` のような引数を読む。何を開くかは input::file_dialog、
//! ベンチマークは bench、物理演算のサンプルは physics、画面の自動確認は ui_check が使う。

use std::path::PathBuf;

/// コマンドライン引数
#[derive(Default, Debug, PartialEq)]
pub struct CommandLine {
    /// 開くファイル
    pub file: Option<PathBuf>,
    /// `--stress N`: 部品 N 個の負荷テスト用シーンを開く
    pub stress: Option<usize>,
    /// `--bench 秒`: カメラを回しながら FPS を測り、結果を出して終了する
    pub bench_secs: Option<f32>,
    /// `--physics`: 物理演算のサンプルライン (ワーク 100 個) を開き、物理演算を始める
    pub physics: bool,
    /// `--ui-check-run 手順書 --out 出力 --baseline 基準画像 [--update]`:
    /// 手順書どおりに操作して画面を撮り、基準画像と比べて終了する (ui_check が使う)
    pub ui_check: Option<UiCheckRun>,
}

/// 画面の自動確認を1つ流すときの指定
#[derive(Default, Debug, PartialEq, Clone)]
pub struct UiCheckRun {
    pub scenario: PathBuf,
    pub out: PathBuf,
    pub baseline: PathBuf,
    pub update: bool,
}

impl CommandLine {
    pub fn parse() -> Self {
        Self::from_args(std::env::args_os().skip(1).map(PathBuf::from))
    }

    fn from_args(args: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut out = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--stress") => out.stress = Some(parse_next(args.next()).unwrap_or(5000)),
                Some("--bench") => out.bench_secs = Some(parse_next(args.next()).unwrap_or(10.0)),
                Some("--physics") => out.physics = true,
                Some("--ui-check-run") => {
                    out.ui_check.get_or_insert_default().scenario = args.next().unwrap_or_default();
                }
                Some("--out") => {
                    out.ui_check.get_or_insert_default().out = args.next().unwrap_or_default()
                }
                Some("--baseline") => {
                    out.ui_check.get_or_insert_default().baseline = args.next().unwrap_or_default();
                }
                Some("--update") => out.ui_check.get_or_insert_default().update = true,
                _ => out.file = Some(arg),
            }
        }
        out
    }
}

fn parse_next<T: std::str::FromStr>(next: Option<PathBuf>) -> Option<T> {
    next.and_then(|n| n.to_str()?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_line() {
        let parse = |args: &[&str]| CommandLine::from_args(args.iter().map(PathBuf::from));
        assert_eq!(parse(&[]), CommandLine::default());
        assert_eq!(parse(&["a.step"]).file, Some(PathBuf::from("a.step")));
        let bench = parse(&["--stress", "2000", "--bench", "5"]);
        assert_eq!(bench.stress, Some(2000));
        assert_eq!(bench.bench_secs, Some(5.0));
        assert!(parse(&["--physics", "--bench", "20"]).physics);
        let check = parse(&[
            "--ui-check-run",
            "a.scenario",
            "--out",
            "o",
            "--baseline",
            "b",
            "--update",
        ]);
        assert_eq!(check.file, None);
        assert_eq!(
            check.ui_check,
            Some(UiCheckRun {
                scenario: "a.scenario".into(),
                out: "o".into(),
                baseline: "b".into(),
                update: true,
            })
        );
    }
}
