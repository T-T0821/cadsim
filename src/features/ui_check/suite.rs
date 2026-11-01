//! 手順書をまとめて流す: `cargo run -- --ui-check [手順書かフォルダ] [--update] [--only 名前]`
//!
//! 手順書ごとにアプリを起動し直す (前の手順書の状態を持ち越さないため)。
//! 起動したアプリは `--ui-check-run` で 1 つの手順書を流し、結果を `result.json` に書いて終わる。
//! 落ちたり固まったりしても、ここで終了コードとログの終わりを結果に入れる。
//! 最後に一覧 (`index.html`) を書き、全部合格なら 0、そうでなければ 1 を返す。
//!
//! Bevy は使わない (ウィンドウを開く前に main から呼ぶ)。

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use plantwright_core::preferences::CONFIG_DIR_ENV;
use plantwright_core::ui_check::files;
use plantwright_core::ui_check::report::ScenarioResult;
use plantwright_core::ui_check::runner::Failure;

use super::scenario_name;

/// 手順書の拡張子
pub const EXTENSION: &str = "scenario";

/// まとめて流すときの指定
#[derive(Debug, PartialEq)]
pub struct SuiteArgs {
    /// 手順書か、手順書の入ったフォルダ
    pub target: PathBuf,
    pub out: PathBuf,
    pub baseline: PathBuf,
    /// 基準画像を今回の写真で置き換える
    pub update: bool,
    /// 名前にこれらのどれかを含む手順書だけを流す (空なら全部)
    pub only: Vec<String>,
    /// 手順書 1 つの制限時間
    pub timeout: Duration,
}

impl SuiteArgs {
    /// `--ui-check` がなければ None
    pub fn from_args(args: impl IntoIterator<Item = OsString>) -> Option<Self> {
        let args: Vec<String> = args
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let start = args.iter().position(|a| a == "--ui-check")?;
        let mut out = SuiteArgs {
            target: PathBuf::from("tests/ui"),
            out: PathBuf::from("target/ui-check"),
            baseline: PathBuf::from("tests/ui/baseline").join(std::env::consts::OS),
            update: false,
            only: Vec::new(),
            timeout: Duration::from_secs(15 * 60),
        };
        let mut rest = args[start + 1..].iter();
        while let Some(arg) = rest.next() {
            match arg.as_str() {
                "--update" => out.update = true,
                "--out" => out.out = rest.next()?.into(),
                "--baseline" => out.baseline = rest.next()?.into(),
                "--only" => out.only.push(rest.next()?.clone()),
                "--timeout" => out.timeout = Duration::from_secs(rest.next()?.parse().ok()?),
                other => out.target = other.into(),
            }
        }
        Some(out)
    }
}

/// `--ui-check` が付いていれば流して終了コードを返す。付いていなければ None (ふつうに起動する)
pub fn run_if_requested() -> Option<i32> {
    let args = SuiteArgs::from_args(std::env::args_os().skip(1))?;
    Some(match run(&args) {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(message) => {
            eprintln!("{message}");
            2
        }
    })
}

/// 流す手順書の一覧 (名前順)
pub fn scenarios(target: &Path, only: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut list = if target.is_dir() {
        std::fs::read_dir(target)
            .map_err(|e| format!("{} を読めません: {e}", target.display()))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == EXTENSION))
            .collect()
    } else if target.is_file() {
        vec![target.to_path_buf()]
    } else {
        return Err(format!("{} がありません", target.display()));
    };
    list.retain(|p| only.is_empty() || only.iter().any(|o| scenario_name(p).contains(o.as_str())));
    list.sort();
    if list.is_empty() {
        return Err(format!(
            "{} に手順書 (*.{EXTENSION}) がありません",
            target.display()
        ));
    }
    Ok(list)
}

fn run(args: &SuiteArgs) -> Result<bool, String> {
    let list = scenarios(&args.target, &args.only)?;
    let exe = std::env::current_exe().map_err(|e| format!("アプリの場所が分かりません: {e}"))?;
    println!(
        "画面の自動確認: 手順書 {} 件 (基準画像: {})",
        list.len(),
        args.baseline.display()
    );
    let mut results = Vec::new();
    for path in &list {
        let result = run_one(&exe, path, args);
        print!("{}", result.summary());
        results.push(result);
    }
    // 一覧には、今回流さなかった手順書の前回の結果も載せる
    let mut all = results.clone();
    if let Ok(entries) = std::fs::read_dir(&args.out) {
        for entry in entries.flatten() {
            let json = entry.path().join("result.json");
            let Ok(text) = std::fs::read_to_string(&json) else {
                continue;
            };
            if let Ok(old) = files::result_from_json(&text)
                && !all.iter().any(|r| r.name == old.name)
            {
                all.push(old);
            }
        }
    }
    all.sort_by(|a, b| a.name.cmp(&b.name));
    std::fs::create_dir_all(&args.out)
        .and_then(|_| std::fs::write(args.out.join("index.html"), files::index_html(&all)))
        .map_err(|e| format!("{} に一覧を書けません: {e}", args.out.display()))?;
    let passed = results.iter().filter(|r| r.passed()).count();
    println!(
        "{passed} / {} 件 OK。報告書: {}",
        results.len(),
        args.out.join("index.html").display()
    );
    Ok(passed == results.len())
}

/// 手順書 1 つ分、アプリを起動して流す
fn run_one(exe: &Path, scenario: &Path, args: &SuiteArgs) -> ScenarioResult {
    let name = scenario_name(scenario);
    let dir = args.out.join(&name);
    let failed = |message: String| ScenarioResult {
        name: name.clone(),
        failure: Some(Failure { line: 0, message }),
        shots: Vec::new(),
    };
    let _ = std::fs::remove_dir_all(&dir);
    // 設定ファイルは空のフォルダから始める (使う人の見た目やミッションの進みに左右されない)
    let config = dir.join("config");
    if let Err(e) = std::fs::create_dir_all(&config) {
        return failed(format!("{} を作れません: {e}", config.display()));
    }
    let log_path = dir.join("log.txt");
    let log = match std::fs::File::create(&log_path) {
        Ok(f) => f,
        Err(e) => return failed(format!("{} を作れません: {e}", log_path.display())),
    };
    let mut command = Command::new(exe);
    command
        .arg("--ui-check-run")
        .arg(scenario)
        .arg("--out")
        .arg(&args.out)
        .arg("--baseline")
        .arg(&args.baseline)
        .env(CONFIG_DIR_ENV, &config)
        .env_remove("ANTHROPIC_API_KEY")
        .stdin(Stdio::null())
        .stdout(
            log.try_clone()
                .map(Stdio::from)
                .unwrap_or_else(|_| Stdio::null()),
        )
        .stderr(Stdio::from(log));
    if args.update {
        command.arg("--update");
    }
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => return failed(format!("アプリを起動できません: {e}")),
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() > args.timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => return failed(format!("アプリの終了を待てません: {e}")),
        }
    };
    let result = std::fs::read_to_string(dir.join("result.json"))
        .ok()
        .and_then(|json| files::result_from_json(&json).ok());
    match (result, status) {
        (Some(result), _) => result,
        (None, status) => {
            let why = match status {
                None => format!("{} 秒たっても終わりません", args.timeout.as_secs()),
                Some(s) => format!("アプリが途中で終了しました ({s})"),
            };
            let tail = log_tail(&log_path, 8);
            let result = failed(format!("{why}。ログの終わり:\n{tail}"));
            // 一覧に載せるため、ここで結果と報告書を書いておく
            let _ = std::fs::write(dir.join("result.json"), files::result_to_json(&result));
            let _ = std::fs::write(dir.join("report.html"), files::scenario_html(&result));
            result
        }
    }
}

/// ログの手がかり。パニックしていればその場所と理由、なければ最後の数行
fn log_tail(path: &Path, lines: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    excerpt(&text, lines)
}

fn excerpt(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    if let Some(i) = all.iter().position(|l| l.contains("panicked at")) {
        let end = all[i..]
            .iter()
            .position(|l| l.starts_with("stack backtrace:") || l.starts_with("note:"))
            .map_or(all.len(), |n| i + n);
        return all[i..end.min(i + lines)].join("\n");
    }
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Option<SuiteArgs> {
        SuiteArgs::from_args(args.iter().map(OsString::from))
    }

    #[test]
    fn 引数を読む() {
        assert_eq!(parse(&["a.step"]), None);
        let args = parse(&["--ui-check"]).unwrap();
        assert_eq!(args.target, PathBuf::from("tests/ui"));
        assert!(!args.update);
        assert!(args.baseline.ends_with(std::env::consts::OS));

        let args = parse(&[
            "--ui-check",
            "tests/ui/01.scenario",
            "--update",
            "--only",
            "chart",
            "--out",
            "o",
            "--timeout",
            "60",
        ])
        .unwrap();
        assert_eq!(args.target, PathBuf::from("tests/ui/01.scenario"));
        assert!(args.update);
        assert_eq!(args.only, vec!["chart".to_string()]);
        assert_eq!(args.out, PathBuf::from("o"));
        assert_eq!(args.timeout, Duration::from_secs(60));
        // 値が足りなければ読めない
        assert_eq!(parse(&["--ui-check", "--out"]), None);
    }

    #[test]
    fn ログからパニックの理由を抜き出す() {
        let log = "INFO a\nthread 'main' panicked at src/x.rs:1:2:\nこわれた\nstack backtrace:\n 0: foo\n";
        assert_eq!(
            excerpt(log, 8),
            "thread 'main' panicked at src/x.rs:1:2:\nこわれた"
        );
        assert_eq!(excerpt("a\nb\nc", 2), "b\nc");
    }

    #[test]
    fn リポジトリの手順書は全部読める() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ui");
        let list = scenarios(&dir, &[]).unwrap();
        assert!(!list.is_empty());
        for path in list {
            let text = std::fs::read_to_string(&path).unwrap();
            if let Err(e) = plantwright_core::ui_check::scenario::parse(&text) {
                panic!("{}: {e}", path.display());
            }
        }
    }
}
