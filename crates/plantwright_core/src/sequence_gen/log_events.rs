//! PLC のログ (CSV) から「いつ、どの信号が、どう変わったか」の一覧を作る
//!
//! 生成 AI にログをそのまま渡すと、変化のない行ばかりで長くなる。変化した所だけを
//! 時刻つきの文にして渡す。I/O 表との突き合わせはせず、見出しの列名をそのまま使うので、
//! 割り付けを済ませていないログでも説明させられる。
//!
//! Bevy・egui に依存しない。

/// 時刻の列とみなす見出し (大文字・小文字は区別しない)
const TIME_HEADERS: [&str; 8] = [
    "time",
    "date",
    "datetime",
    "timestamp",
    "時刻",
    "時間",
    "日付",
    "日時",
];

/// ログから取り出した変化の一覧
#[derive(Clone, Debug, PartialEq)]
pub struct LogSummary {
    /// 生成 AI に渡す文 (初期状態と変化の一覧)
    pub text: String,
    /// データの行数
    pub rows: usize,
    /// 信号の列数
    pub signals: usize,
    /// 一覧に入れた変化の数
    pub events: usize,
    /// 上限を超えたため一覧に入れなかった変化の数
    pub omitted: usize,
}

fn split(line: &str) -> Vec<String> {
    let sep = if line.contains('\t') { '\t' } else { ',' };
    line.split(sep)
        .map(|f| f.trim().trim_matches('"').trim().to_string())
        .collect()
}

fn is_number(s: &str) -> bool {
    s.parse::<f64>().is_ok()
}

/// CSV を読み、変化を最大 `max_events` 件まで並べる
pub fn summarize(csv: &str, max_events: usize) -> Result<LogSummary, String> {
    let rows: Vec<Vec<String>> = csv
        .lines()
        .map(|l| split(l.trim_start_matches('\u{feff}')))
        .filter(|r| r.iter().any(|f| !f.is_empty()))
        .collect();
    // 見出し行: 数値でない欄が 2 つ以上ある最初の行
    let header_at = rows
        .iter()
        .position(|r| r.iter().filter(|f| !f.is_empty() && !is_number(f)).count() >= 2)
        .ok_or("見出しの行が見つかりませんでした")?;
    let header = &rows[header_at];
    let is_time = |name: &str| {
        let lower = name.to_lowercase();
        TIME_HEADERS.iter().any(|t| lower == *t)
    };
    let time_cols: Vec<usize> = (0..header.len()).filter(|&i| is_time(&header[i])).collect();
    let signal_cols: Vec<usize> = (0..header.len())
        .filter(|&i| !header[i].is_empty() && !is_time(&header[i]))
        .collect();
    if signal_cols.is_empty() {
        return Err("信号の列がありませんでした".into());
    }
    let data = &rows[header_at + 1..];
    if data.is_empty() {
        return Err("データの行がありませんでした".into());
    }
    let cell = |row: &[String], i: usize| row.get(i).cloned().unwrap_or_default();
    let time_of = |row: &[String], index: usize| {
        let t: Vec<String> = time_cols
            .iter()
            .map(|&i| cell(row, i))
            .filter(|s| !s.is_empty())
            .collect();
        if t.is_empty() {
            format!("{}行目", index + 1)
        } else {
            t.join(" ")
        }
    };

    let mut text = format!("初期状態 ({}):", time_of(&data[0], 0));
    for &c in &signal_cols {
        text += &format!(" {}={}", header[c], cell(&data[0], c));
    }
    text += "\n";
    let mut events = 0;
    let mut omitted = 0;
    for (index, pair) in data.windows(2).enumerate() {
        let (prev, row) = (&pair[0], &pair[1]);
        let changes: Vec<String> = signal_cols
            .iter()
            .filter(|&&c| cell(prev, c) != cell(row, c))
            .map(|&c| format!("{} {}→{}", header[c], cell(prev, c), cell(row, c)))
            .collect();
        if changes.is_empty() {
            continue;
        }
        if events + changes.len() > max_events {
            omitted += changes.len();
            continue;
        }
        events += changes.len();
        text += &format!("{}: {}\n", time_of(row, index + 1), changes.join(", "));
    }
    Ok(LogSummary {
        text,
        rows: data.len(),
        signals: signal_cols.len(),
        events,
        omitted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\u{feff}Date,Time,Y10,X3,D100\n\
        2026/10/31,12:00:00.000,0,0,0\n\
        2026/10/31,12:00:00.100,1,0,0\n\
        2026/10/31,12:00:00.200,1,0,0\n\
        2026/10/31,12:00:00.600,1,1,1250\n";

    #[test]
    fn lists_only_changes_with_time() {
        let s = summarize(LOG, 100).unwrap();
        assert_eq!((s.rows, s.signals, s.events, s.omitted), (4, 3, 3, 0));
        assert!(
            s.text
                .starts_with("初期状態 (2026/10/31 12:00:00.000): Y10=0 X3=0 D100=0")
        );
        assert!(s.text.contains("2026/10/31 12:00:00.100: Y10 0→1\n"));
        assert!(s.text.contains("12:00:00.600: X3 0→1, D100 0→1250"));
        assert!(!s.text.contains("12:00:00.200"));
    }

    #[test]
    fn counts_omitted_events_over_limit() {
        let s = summarize(LOG, 1).unwrap();
        assert_eq!((s.events, s.omitted), (1, 2));
    }

    #[test]
    fn works_without_time_column_and_rejects_bad_files() {
        let s = summarize("A\tB\n0\t0\n1\t0\n", 10).unwrap();
        assert!(s.text.contains("2行目: A 0→1"));
        assert!(summarize("1,2\n3,4\n", 10).is_err());
        assert!(summarize("Time,Y0\n", 10).is_err());
    }
}
