//! 画面上の部品を表示名で探す

use super::Rect;
use super::scenario::WidgetQuery;

/// 画面に出ている部品 1 つ (ボタン・メニュー・ラベル・入力欄など)
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    /// 表示名 (ボタンの文字など)
    pub label: String,
    /// 種類 (Button, Label, TextInput など。報告や一覧で見せるだけ)
    pub role: String,
    /// 見えている範囲 (スクロールで隠れた部分は除く。まったく見えなければ大きさ 0)
    pub rect: Rect,
    /// ポップアップ (開いたメニューなど) の中にある。ほかの画面の同じ表示名のボタンと見分けるのに使う
    pub popup: bool,
}

/// アクセシビリティ情報から集めたままの一覧を、探しやすい形に整える。
///
/// - 文字の断片 (`TextRun`) や入れ物は除く (ラベルと同じ表示名が重なるため)
/// - 窓 (`Window`) は、表示名を窓のタイトルにして、位置を「たたむ / 開く」の三角に置き換える。
///   `click "部品ライブラリ"` で、たたまれた窓を開いたり、開いた窓をたたんだりできる
pub fn normalize(raw: Vec<Widget>) -> Vec<Widget> {
    let toggles: Vec<Widget> = raw
        .iter()
        .filter(|w| w.role == "Window")
        .filter_map(|window| {
            let r = window.rect;
            let toggle = raw.iter().find(|b| {
                b.role == "Button"
                    && matches!(b.label.as_str(), "Hide" | "Show")
                    && r.contains(b.rect.x, b.rect.y)
                    && b.rect.y < r.y + 40.0
            })?;
            Some(Widget {
                label: window.label.clone(),
                role: "WindowTitle".into(),
                rect: toggle.rect,
                popup: window.popup,
            })
        })
        .collect();
    raw.into_iter()
        .filter(|w| {
            !w.label.is_empty()
                && !matches!(w.role.as_str(), "TextRun" | "GenericContainer" | "Window")
        })
        .chain(toggles)
        .collect()
}

/// 探した結果
#[derive(Debug, PartialEq)]
pub enum Found<'a> {
    Widget(&'a Widget),
    /// 見つからない。似た表示名をいくつか添える
    Missing {
        similar: Vec<String>,
    },
}

/// 表示名で探す。表示名が完全に同じものを優先し、なければ表示名を含むものを探す。
/// 同じ条件に合うものが複数あれば、上から (同じ高さなら左から) 数えて `nth` 番目。
pub fn find<'a>(widgets: &'a [Widget], query: &WidgetQuery) -> Found<'a> {
    let visible = || widgets.iter().filter(|w| w.rect.w > 0.0 && w.rect.h > 0.0);
    let mut exact: Vec<&Widget> = visible().filter(|w| w.label == query.label).collect();
    if exact.is_empty() {
        exact = visible()
            .filter(|w| w.label.contains(&query.label))
            .collect();
    }
    exact.sort_by(|a, b| {
        a.rect
            .y
            .total_cmp(&b.rect.y)
            .then(a.rect.x.total_cmp(&b.rect.x))
    });
    match exact.get(query.nth - 1) {
        Some(w) => Found::Widget(w),
        None => Found::Missing {
            similar: similar_labels(widgets, &query.label),
        },
    }
}

/// 表示名が画面にあるか (`expect` 用。含むものも数える)
pub fn exists(widgets: &[Widget], label: &str) -> bool {
    matches!(
        find(
            widgets,
            &WidgetQuery {
                label: label.into(),
                nth: 1
            }
        ),
        Found::Widget(_)
    )
}

/// 共通の文字が多い表示名を最大 5 つ (打ち間違いを見つけやすくする)
fn similar_labels(widgets: &[Widget], label: &str) -> Vec<String> {
    let wanted: Vec<char> = label.chars().collect();
    let mut scored: Vec<(usize, &str)> = widgets
        .iter()
        .filter(|w| !w.label.is_empty())
        .map(|w| {
            let common = w.label.chars().filter(|c| wanted.contains(c)).count();
            (common, w.label.as_str())
        })
        .filter(|&(common, _)| common > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    let mut out: Vec<String> = Vec::new();
    for (_, l) in scored {
        if !out.iter().any(|o| o == l) {
            out.push(l.to_string());
        }
        if out.len() == 5 {
            break;
        }
    }
    out
}

/// 一覧を文字にする (`dump_widgets` 用)。上から順に「種類  x y 幅 高さ  表示名」
pub fn dump(widgets: &[Widget]) -> String {
    let mut sorted: Vec<&Widget> = widgets.iter().filter(|w| !w.label.is_empty()).collect();
    sorted.sort_by(|a, b| {
        a.rect
            .y
            .total_cmp(&b.rect.y)
            .then(a.rect.x.total_cmp(&b.rect.x))
    });
    let mut out = String::new();
    for w in sorted {
        out.push_str(&format!(
            "{:<12} {:>5.0} {:>5.0} {:>5.0} {:>5.0}  {:?}\n",
            w.role, w.rect.x, w.rect.y, w.rect.w, w.rect.h, w.label
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(label: &str, x: f32, y: f32) -> Widget {
        Widget {
            label: label.into(),
            role: "Button".into(),
            rect: Rect::new(x, y, 50.0, 20.0),
            popup: false,
        }
    }

    fn q(label: &str, nth: usize) -> WidgetQuery {
        WidgetQuery {
            label: label.into(),
            nth,
        }
    }

    #[test]
    fn 完全に同じ名前を優先して上から数える() {
        let widgets = vec![
            w("保存  (Ctrl+S)", 0.0, 0.0),
            w("保存", 10.0, 300.0),
            w("保存", 10.0, 100.0),
        ];
        assert_eq!(find(&widgets, &q("保存", 1)), Found::Widget(&widgets[2]));
        assert_eq!(find(&widgets, &q("保存", 2)), Found::Widget(&widgets[1]));
        // 完全に同じものがなければ含むもの
        assert_eq!(find(&widgets, &q("Ctrl+S", 1)), Found::Widget(&widgets[0]));
    }

    #[test]
    fn 見つからなければ似た名前を返す() {
        let mut hidden = w("開く", 0.0, 0.0);
        hidden.rect.w = 0.0;
        let widgets = vec![hidden, w("開く…", 0.0, 20.0), w("閉じる", 0.0, 40.0)];
        assert_eq!(
            find(&widgets, &q("開く…", 2)),
            Found::Missing {
                similar: vec!["開く…".into(), "開く".into()]
            }
        );
        assert!(exists(&widgets, "閉じ"));
        // 大きさのない (見えていない) ものは数えない
        assert_eq!(find(&widgets, &q("開く", 1)), Found::Widget(&widgets[1]));
    }

    #[test]
    fn 窓のタイトルはたたむ三角の位置にする() {
        let raw = vec![
            Widget {
                label: "部品ライブラリ".into(),
                role: "Window".into(),
                rect: Rect::new(100.0, 100.0, 300.0, 400.0),
                popup: false,
            },
            w("Hide", 10.0, 10.0),
            w("Show", 110.0, 108.0),
            Widget {
                label: "部品".into(),
                role: "TextRun".into(),
                rect: Rect::new(0.0, 0.0, 1.0, 1.0),
                popup: false,
            },
        ];
        let out = normalize(raw);
        let title = out.iter().find(|w| w.label == "部品ライブラリ").unwrap();
        assert_eq!(title.role, "WindowTitle");
        assert_eq!(title.rect, Rect::new(110.0, 108.0, 50.0, 20.0));
        assert!(
            out.iter()
                .all(|w| w.role != "TextRun" && w.role != "Window")
        );
    }

    #[test]
    fn 一覧は上から並べる() {
        let widgets = vec![w("下", 0.0, 50.0), w("上", 0.0, 10.0), w("", 0.0, 0.0)];
        let text = dump(&widgets);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("\"上\""));
    }
}
