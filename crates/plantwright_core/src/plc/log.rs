//! PLC のログ (GX LogViewer などで書き出した CSV) を読み、時刻ごとの出力を取り出す
//!
//! 実機で記録した PLC の出力 (PLC → 設備) を時刻どおりに流し、設備の動きを 3D で再現するのに使う。
//!
//! ```text
//! Date,Time,Y10,Y11,D100
//! 2026/10/31,12:00:00.000,0,0,0
//! 2026/10/31,12:00:00.100,1,0,1250
//! ```
//!
//! - 見出し行は、I/O 表の信号の「ログ列名」(空なら信号名) と一致する列を含む最初の行
//! - 時刻の列は見出しが Time / 時刻 / 時間 / Date / 日付 / 日時 などの列。
//!   「12:00:00.100」「2026/10/31 12:00:00.100」「秒数」のどれでもよく、日付と時刻が別の列でもよい
//! - ビットは 1 / 0、ON / OFF、TRUE / FALSE。ワードは PLC のデバイスの値 (整数) で、I/O 表のスケールを掛ける
//! - 使うのは出力の信号だけ。入力 (端のスイッチなど) は設備モデルが作る

use super::io::{IoTable, IoValues, Point};

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

/// 読み込んだログ
#[derive(Clone, Debug, PartialEq)]
pub struct PlcLog {
    /// 各行の時刻 (最初の行からの秒数)
    times: Vec<f64>,
    /// 信号ごとの値の列 (ビットは 0 / 1)
    columns: Vec<(Point, Vec<f64>)>,
    /// I/O 表と結びつかなかった列の見出し
    pub unused_columns: Vec<String>,
}

impl PlcLog {
    /// ログの長さ (秒)
    pub fn duration(&self) -> f64 {
        self.times.last().copied().unwrap_or(0.0)
    }

    pub fn rows(&self) -> usize {
        self.times.len()
    }

    /// 結びついた信号の数
    pub fn signals(&self) -> usize {
        self.columns.len()
    }

    /// 時刻 `t` の出力を `io` に書く (直前の行の値をそのまま保つ)
    pub fn apply_at(&self, t: f64, io: &mut IoValues) {
        // t 以下で最後の行
        let row = self
            .times
            .partition_point(|&time| time <= t)
            .saturating_sub(1);
        for (point, values) in &self.columns {
            let value = values[row];
            if point.is_word() {
                io.set_word(*point, value);
            } else {
                io.set_bit(*point, value != 0.0);
            }
        }
    }

    /// CSV を読む。`table` の出力の信号と列を結びつける。
    pub fn parse(text: &str, table: &IoTable) -> Result<Self, String> {
        let rows: Vec<Vec<String>> = text
            .lines()
            .map(|line| split(line.trim_start_matches('\u{feff}')))
            .collect();
        let outputs: Vec<_> = table
            .points
            .iter()
            .filter(|p| p.point.is_output())
            .collect();
        if outputs.is_empty() {
            return Err("I/O 表に出力の信号がありません。先にタイミングチャートでアクチュエータを作ってください".into());
        }
        let column_name = |p: &super::io::IoPoint| {
            if p.log_column.is_empty() {
                p.name.clone()
            } else {
                p.log_column.clone()
            }
        };

        let header_at = rows
            .iter()
            .position(|r| outputs.iter().any(|p| r.iter().any(|f| *f == column_name(p))))
            .ok_or("I/O 表の信号の列が見つかりません。I/O 表の「ログ列名」にログの列見出しを書いてください")?;
        let header = &rows[header_at];
        let time_columns: Vec<usize> = header
            .iter()
            .enumerate()
            .filter(|(_, h)| TIME_HEADERS.contains(&h.to_lowercase().as_str()))
            .map(|(i, _)| i)
            .collect();
        if time_columns.is_empty() {
            return Err("時刻の列 (Time / 時刻 など) が見つかりません".into());
        }
        let mut signal_columns = Vec::new();
        let mut unused_columns = Vec::new();
        for (i, h) in header.iter().enumerate() {
            if time_columns.contains(&i) || h.is_empty() {
                continue;
            }
            match outputs.iter().find(|p| column_name(p) == *h) {
                Some(p) => signal_columns.push((i, p.point, p.scale)),
                None => unused_columns.push(h.clone()),
            }
        }

        let mut times = Vec::new();
        let mut columns: Vec<(Point, Vec<f64>)> = signal_columns
            .iter()
            .map(|(_, point, _)| (*point, Vec::new()))
            .collect();
        for (index, row) in rows.iter().enumerate().skip(header_at + 1) {
            if row.iter().all(|f| f.is_empty()) {
                continue;
            }
            let line = index + 1;
            let stamp: Vec<&str> = time_columns
                .iter()
                .filter_map(|&c| row.get(c).map(String::as_str))
                .collect();
            let time = parse_time(&stamp.join(" "))
                .ok_or_else(|| format!("{line} 行目: 時刻「{}」を読めません", stamp.join(" ")))?;
            times.push(time);
            for ((col, point, scale), (_, values)) in signal_columns.iter().zip(&mut columns) {
                let cell = row.get(*col).map_or("", String::as_str);
                let value = parse_value(cell, point.is_word())
                    .ok_or_else(|| format!("{line} 行目: 値「{cell}」を読めません"))?;
                values.push(if point.is_word() {
                    value * scale
                } else {
                    value
                });
            }
        }
        if times.is_empty() {
            return Err("データの行がありません".into());
        }
        // 最初の行を 0 秒にする。日付は先に引き算して、秒の桁の精度を落とさない。
        // 日付のない時刻が戻ったら (半日以上) 日付をまたいだとみなして 1 日足す。
        let (first_day, first_second) = times[0];
        let mut extra_days = 0.0;
        let mut previous = first_second;
        let times: Vec<f64> = times
            .iter()
            .map(|&(day, second)| {
                if day == first_day && second < previous - 43_200.0 {
                    extra_days += 1.0;
                }
                previous = second;
                (day - first_day + extra_days) * 86_400.0 + second - first_second
            })
            .collect();
        if times.windows(2).any(|w| w[1] < w[0]) {
            return Err("時刻が古い順に並んでいません".into());
        }
        Ok(Self {
            times,
            columns,
            unused_columns,
        })
    }
}

/// 1行をカンマ (なければタブ) で分ける。引用符は外す。
fn split(line: &str) -> Vec<String> {
    let separator = if line.contains(',') { ',' } else { '\t' };
    line.split(separator)
        .map(|f| f.trim().trim_matches('"').trim().to_string())
        .collect()
}

/// 時刻を (通し日数, その日の秒数) にする。
/// 「秒数」「hh:mm:ss(.sss)」と、その前に日付 (yyyy/mm/dd または yyyy-mm-dd) があってもよい。
fn parse_time(text: &str) -> Option<(f64, f64)> {
    let text = text.trim();
    if let Ok(seconds) = text.parse::<f64>() {
        return Some((0.0, seconds));
    }
    let mut days = 0.0;
    let mut clock = text;
    if let Some((date, rest)) = text.split_once([' ', 'T']) {
        days = days_from_date(date)?;
        clock = rest.trim();
    }
    let parts: Vec<&str> = clock.split(':').collect();
    let [h, m, s] = parts.as_slice() else {
        return None;
    };
    let seconds =
        h.parse::<f64>().ok()? * 3600.0 + m.parse::<f64>().ok()? * 60.0 + s.parse::<f64>().ok()?;
    Some((days, seconds))
}

/// 日付を「ある日からの日数」にする (時刻の差を出すためだけに使う)
fn days_from_date(date: &str) -> Option<f64> {
    let parts: Vec<i64> = date
        .split(['/', '-'])
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let [y, m, d] = parts.as_slice() else {
        return None;
    };
    // 1年 = 365.25 日、1か月 = 平均の日数で近似すると月末をまたぐときにずれるので、
    // グレゴリオ暦の通し日数 (civil from days の逆) で数える
    let (y, m) = if *m <= 2 { (y - 1, m + 9) } else { (*y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe) as f64)
}

/// 値を読む。ビットは ON / OFF / TRUE / FALSE も受け付ける。
fn parse_value(text: &str, is_word: bool) -> Option<f64> {
    if !is_word {
        match text.to_uppercase().as_str() {
            "ON" | "TRUE" => return Some(1.0),
            "OFF" | "FALSE" | "" => return Some(0.0),
            _ => {}
        }
    }
    text.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::chart::ActuatorKind;
    use crate::motion::chart::TimingChart;
    use crate::plc::io::{AddressPlan, PointSettings};

    fn table() -> (IoTable, u32, u32) {
        let mut chart = TimingChart::default();
        let cyl = chart.add_actuator(ActuatorKind::Cylinder);
        chart.actuator_mut(cyl).unwrap().name = "リフタ".into();
        let axis = chart.add_actuator(ActuatorKind::LinearAxis);
        chart.actuator_mut(axis).unwrap().name = "X軸".into();
        let mut plan = AddressPlan::default();
        plan.custom.insert(
            Point::AxisTarget(axis),
            PointSettings {
                log_column: "D100".into(),
                ..Default::default()
            },
        );
        (IoTable::from_chart(&chart, &plan), cyl, axis)
    }

    #[test]
    fn gx_logviewer_style_csv_is_replayed() {
        let (table, cyl, axis) = table();
        let csv = "ロギング結果\n\
                   \"Date\",\"Time\",\"リフタ.出\",\"D100\",\"M0\"\n\
                   2026/10/31,23:59:59.900,OFF,0,0\n\
                   2026/11/01,00:00:00.000,ON,1250,1\n\
                   2026/11/01,00:00:00.500,OFF,4000,0\n";
        let log = PlcLog::parse(csv, &table).unwrap();
        assert_eq!(log.rows(), 3);
        assert_eq!(log.signals(), 2);
        assert_eq!(log.unused_columns, vec!["M0".to_string()]);
        assert!(
            (log.duration() - 0.6).abs() < 1e-9,
            "日付をまたいでも続けて数える"
        );

        let mut io = IoValues::default();
        log.apply_at(0.05, &mut io);
        assert!(!io.bit(Point::CylinderExtend(cyl)));
        log.apply_at(0.2, &mut io);
        assert!(io.bit(Point::CylinderExtend(cyl)));
        assert_eq!(io.word(Point::AxisTarget(axis)), 125.0);
        log.apply_at(10.0, &mut io);
        assert_eq!(io.word(Point::AxisTarget(axis)), 400.0);
    }

    #[test]
    fn seconds_column_and_tabs_work() {
        let (table, cyl, _) = table();
        let csv = "時刻\tリフタ.出\n0\t0\n1.5\t1\n";
        let log = PlcLog::parse(csv, &table).unwrap();
        assert_eq!(log.duration(), 1.5);
        let mut io = IoValues::default();
        log.apply_at(1.5, &mut io);
        assert!(io.bit(Point::CylinderExtend(cyl)));
    }

    #[test]
    fn helpful_errors() {
        let (table, _, _) = table();
        assert!(
            PlcLog::parse("Time,Y0\n0,1\n", &table).is_err(),
            "結びつく列がない"
        );
        assert!(
            PlcLog::parse("A,リフタ.出\n0,1\n", &table).is_err(),
            "時刻の列がない"
        );
        let bad = PlcLog::parse("Time,リフタ.出\n0,1\nabc,0\n", &table).unwrap_err();
        assert!(bad.starts_with("3 行目"), "{bad}");
    }

    #[test]
    fn dates_count_days_across_month_ends() {
        let a = days_from_date("2026/02/28").unwrap();
        let b = days_from_date("2026-03-01").unwrap();
        assert_eq!(b - a, 1.0);
        assert_eq!(parse_time("12:00:01.5"), Some((0.0, 43_201.5)));
    }
}
