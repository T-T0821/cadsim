//! 画面の自動確認の読み書き: 写真 (PNG)、結果 (JSON)、報告書 (HTML)
//!
//! 出力フォルダの中身:
//!
//! ```text
//! <出力>/index.html                全部の手順書の一覧
//! <出力>/<手順書>/report.html       写真ごとに 基準 / 今回 / 差分 を並べたもの
//! <出力>/<手順書>/result.json       結果 (一覧を作るときに読む)
//! <出力>/<手順書>/actual/<名前>.png  今回の写真
//! <出力>/<手順書>/expected/<名前>.png 基準画像の写し (違ったときだけ)
//! <出力>/<手順書>/diff/<名前>.png    差分画像 (違う画素が赤、比べない領域が青)
//! <出力>/<手順書>/failure.png       手順が失敗したときの画面
//! ```

use std::path::Path;

use cadsim_core::ui_check::compare::Image;
use cadsim_core::ui_check::report::{ScenarioResult, ShotResult, ShotStatus, status_text};
use cadsim_core::ui_check::runner::Failure;
use serde::{Deserialize, Serialize};

/// 手順が失敗したときの画面の写真のファイル名
pub const FAILURE_PNG: &str = "failure.png";

pub fn read_png(path: &Path) -> Result<Image, String> {
    let img = image::open(path)
        .map_err(|e| format!("{} を読めません: {e}", path.display()))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    Ok(Image::new(w, h, img.into_raw()))
}

/// PNG に書く。基準画像はリポジトリに入れるので、時間をかけてでも小さく圧縮する
pub fn write_png(path: &Path, img: &Image) -> Result<(), String> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    }
    let file =
        std::fs::File::create(path).map_err(|e| format!("{} に書けません: {e}", path.display()))?;
    PngEncoder::new_with_quality(
        std::io::BufWriter::new(file),
        CompressionType::Best,
        FilterType::Adaptive,
    )
    .write_image(
        &img.rgba,
        img.width,
        img.height,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| format!("{} に書けません: {e}", path.display()))
}

#[derive(Serialize, Deserialize)]
struct ResultRecord {
    name: String,
    failure: Option<FailureRecord>,
    shots: Vec<ShotRecord>,
}

#[derive(Serialize, Deserialize)]
struct FailureRecord {
    line: usize,
    message: String,
}

#[derive(Serialize, Deserialize)]
struct ShotRecord {
    name: String,
    /// same / different / size / new / updated
    status: String,
    #[serde(default)]
    ratio: f64,
    #[serde(default)]
    expected: (u32, u32),
    #[serde(default)]
    actual: (u32, u32),
}

pub fn result_to_json(result: &ScenarioResult) -> String {
    let record = ResultRecord {
        name: result.name.clone(),
        failure: result.failure.as_ref().map(|f| FailureRecord {
            line: f.line,
            message: f.message.clone(),
        }),
        shots: result
            .shots
            .iter()
            .map(|s| {
                let mut r = ShotRecord {
                    name: s.name.clone(),
                    status: String::new(),
                    ratio: 0.0,
                    expected: (0, 0),
                    actual: (0, 0),
                };
                r.status = match s.status {
                    ShotStatus::Same { ratio } => {
                        r.ratio = ratio;
                        "same"
                    }
                    ShotStatus::Different { ratio } => {
                        r.ratio = ratio;
                        "different"
                    }
                    ShotStatus::SizeMismatch { expected, actual } => {
                        r.expected = expected;
                        r.actual = actual;
                        "size"
                    }
                    ShotStatus::NewBaseline => "new",
                    ShotStatus::Updated => "updated",
                }
                .into();
                r
            })
            .collect(),
    };
    serde_json::to_string_pretty(&record).unwrap_or_default()
}

pub fn result_from_json(json: &str) -> Result<ScenarioResult, String> {
    let r: ResultRecord = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let shots = r
        .shots
        .into_iter()
        .map(|s| {
            let status = match s.status.as_str() {
                "same" => ShotStatus::Same { ratio: s.ratio },
                "different" => ShotStatus::Different { ratio: s.ratio },
                "size" => ShotStatus::SizeMismatch {
                    expected: s.expected,
                    actual: s.actual,
                },
                "new" => ShotStatus::NewBaseline,
                "updated" => ShotStatus::Updated,
                other => return Err(format!("知らない判定「{other}」")),
            };
            Ok(ShotResult {
                name: s.name,
                status,
            })
        })
        .collect::<Result<_, _>>()?;
    Ok(ScenarioResult {
        name: r.name,
        failure: r.failure.map(|f| Failure {
            line: f.line,
            message: f.message,
        }),
        shots,
    })
}

const STYLE: &str = "body{font-family:sans-serif;margin:16px;background:#1d1f23;color:#ddd}\
a{color:#8cf}h1,h2{font-weight:600}.ok{color:#6c6}.ng{color:#f66}\
.shot{margin:16px 0 32px}.row{display:flex;gap:8px;flex-wrap:wrap}\
.row figure{margin:0;flex:1 1 30%;min-width:280px}.row img{width:100%;border:1px solid #444}\
figcaption{font-size:12px;color:#aaa}table{border-collapse:collapse}\
td,th{padding:4px 10px;border-bottom:1px solid #333;text-align:left}";

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn mark(passed: bool) -> &'static str {
    if passed {
        "<span class=ok>OK</span>"
    } else {
        "<span class=ng>NG</span>"
    }
}

/// 手順書 1 つの報告書 (写真ごとに 基準 / 今回 / 差分 を並べる)
pub fn scenario_html(result: &ScenarioResult) -> String {
    let mut html = format!(
        "<!doctype html><meta charset=utf-8><title>{name}</title><style>{STYLE}</style>\
         <p><a href=../index.html>← 一覧</a></p><h1>{m} {name}</h1>",
        name = escape(&result.name),
        m = mark(result.passed()),
    );
    if let Some(f) = &result.failure {
        html.push_str(&format!(
            "<p class=ng>手順 {} 行目で止まりました: {}</p>",
            f.line,
            escape(&f.message)
        ));
        // アプリが落ちたときは写真がないので、読めなければ消す
        html.push_str(&format!(
            "<div class=row><figure><img src=\"{FAILURE_PNG}\" onerror=\"this.parentNode.remove()\">\
             <figcaption>止まったときの画面</figcaption></figure></div>"
        ));
    }
    for shot in &result.shots {
        let name = escape(&shot.name);
        html.push_str(&format!(
            "<div class=shot><h2>{} {name}</h2><p>{}</p><div class=row>",
            mark(shot.status.passed()),
            escape(&status_text(&shot.status))
        ));
        let compared = matches!(
            shot.status,
            ShotStatus::Same { .. } | ShotStatus::Different { .. }
        );
        if !shot.status.passed() {
            html.push_str(&format!(
                "<figure><img src=\"expected/{name}.png\"><figcaption>基準</figcaption></figure>"
            ));
        }
        html.push_str(&format!(
            "<figure><img src=\"actual/{name}.png\"><figcaption>今回</figcaption></figure>"
        ));
        if compared {
            html.push_str(&format!(
                "<figure><img src=\"diff/{name}.png\"><figcaption>差分 (赤: 違う画素、青: 比べない領域)</figcaption></figure>"
            ));
        }
        html.push_str("</div></div>");
    }
    html
}

/// 全部の手順書の一覧
pub fn index_html(results: &[ScenarioResult]) -> String {
    let passed = results.iter().filter(|r| r.passed()).count();
    let mut html = format!(
        "<!doctype html><meta charset=utf-8><title>画面の自動確認</title><style>{STYLE}</style>\
         <h1>画面の自動確認 {} / {} 件 OK</h1><table><tr><th></th><th>手順書</th><th>写真</th><th>内容</th></tr>",
        passed,
        results.len()
    );
    for r in results {
        let name = escape(&r.name);
        let note = match &r.failure {
            Some(f) => format!("{} 行目で失敗: {}", f.line, escape(&f.message)),
            None => {
                let bad: Vec<String> = r
                    .shots
                    .iter()
                    .filter(|s| !s.status.passed())
                    .map(|s| escape(&s.name))
                    .collect();
                if bad.is_empty() {
                    String::new()
                } else {
                    format!("違う写真: {}", bad.join(", "))
                }
            }
        };
        html.push_str(&format!(
            "<tr><td>{}</td><td><a href=\"{name}/report.html\">{name}</a></td><td>{}</td><td>{note}</td></tr>",
            mark(r.passed()),
            r.shots.len()
        ));
    }
    html.push_str("</table>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ScenarioResult {
        ScenarioResult {
            name: "01_開く".into(),
            failure: Some(Failure {
                line: 7,
                message: "「保存」が<見つかりません>".into(),
            }),
            shots: vec![
                ShotResult {
                    name: "a".into(),
                    status: ShotStatus::Different { ratio: 0.25 },
                },
                ShotResult {
                    name: "b".into(),
                    status: ShotStatus::SizeMismatch {
                        expected: (10, 20),
                        actual: (30, 40),
                    },
                },
                ShotResult {
                    name: "c".into(),
                    status: ShotStatus::NewBaseline,
                },
            ],
        }
    }

    #[test]
    fn 結果を_json_で往復できる() {
        let r = sample();
        assert_eq!(result_from_json(&result_to_json(&r)).unwrap(), r);
    }

    #[test]
    fn 報告書は文字を逃がし_違う写真に基準と差分を付ける() {
        let html = scenario_html(&sample());
        assert!(html.contains("&lt;見つかりません&gt;"));
        assert!(html.contains("expected/a.png"));
        assert!(html.contains("diff/a.png"));
        // 基準画像がなかった写真には差分がない
        assert!(!html.contains("diff/c.png"));
        let index = index_html(&[sample()]);
        assert!(index.contains("0 / 1 件 OK"));
        assert!(index.contains("01_開く/report.html"));
    }

    #[test]
    fn png_を往復できる() {
        let dir = std::env::temp_dir().join(format!("cadsim_ui_check_{}", std::process::id()));
        let path = dir.join("x.png");
        let img = Image::new(2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255]);
        write_png(&path, &img).unwrap();
        assert_eq!(read_png(&path).unwrap(), img);
        let _ = std::fs::remove_dir_all(dir);
    }
}
