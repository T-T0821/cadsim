//! I/O 対応表の CSV 書き出しと読み戻し
//!
//! 書き出した CSV を Excel などで編集し (番号・スケール・OPC UA ノード・ログの列名を PLC 側に合わせる)、
//! 読み込むと各信号の個別設定 ([`PointSettings`]) になる。行は「信号」列の名前で突き合わせる。
//!
//! ```text
//! 信号,向き,種類,番号,スケール,OPC UAノード,ログ列名
//! リフタ.出,出力,コイル,0,,ns=2;s=Line.Lifter.Out,Y10
//! X軸.指令位置,出力,保持レジスタ,0,0.1,,D100
//! ```
//!
//! 番号・スケール・ノード・ログ列名が空の欄は自動の値に戻す。古い CSV (ログ列名の列がない) も読める。向きと種類は読むときには使わない (確認用)。

use super::io::{AddressPlan, IoTable, PointSettings};

const HEADER: [&str; 7] = [
    "信号",
    "向き",
    "種類",
    "番号",
    "スケール",
    "OPC UAノード",
    "ログ列名",
];
/// Excel が UTF-8 と判断できるよう先頭に付ける印 (BOM)
const BOM: char = '\u{feff}';

/// I/O 一覧を CSV にする
pub fn to_csv(table: &IoTable) -> String {
    let mut text = String::from(BOM);
    text += &HEADER.join(",");
    text += "\r\n";
    for p in &table.points {
        let direction = if p.point.is_output() {
            "出力"
        } else {
            "入力"
        };
        let (kind, scale) = if p.point.is_word() {
            ("保持レジスタ", p.scale.to_string())
        } else {
            ("コイル", String::new())
        };
        let row = [
            p.name.as_str(),
            direction,
            kind,
            &p.address.to_string(),
            &scale,
            &p.node,
            &p.log_column,
        ];
        let fields: Vec<String> = row.iter().map(|f| quote(f)).collect();
        text += &fields.join(",");
        text += "\r\n";
    }
    text
}

/// 読み込みの結果
#[derive(Debug, Default, PartialEq)]
pub struct CsvReport {
    /// 個別設定を反映した信号の数
    pub applied: usize,
    /// 今の I/O 一覧にない信号名 (アクチュエータの名前が変わったときなど)
    pub unknown: Vec<String>,
    /// 読めなかった行 ("3 行目: 番号が数字ではありません" など)
    pub errors: Vec<String>,
}

/// CSV を読み、`plan` の個別設定を書き換える。表にない信号や読めない行は飛ばして報告する。
pub fn apply_csv(text: &str, table: &IoTable, plan: &mut AddressPlan) -> Result<CsvReport, String> {
    let rows = parse(text.trim_start_matches(BOM));
    let header_at = rows
        .iter()
        .position(|r| r.first().is_some_and(|f| f.trim() == HEADER[0]))
        .ok_or("見出し行 (「信号」で始まる行) が見つかりません")?;
    let column = |name: &str| rows[header_at].iter().position(|f| f.trim() == name);
    let (Some(name_col), number_col, scale_col, node_col, log_col) = (
        column(HEADER[0]),
        column(HEADER[3]),
        column(HEADER[4]),
        column(HEADER[5]),
        column(HEADER[6]),
    ) else {
        unreachable!("見出し行は「信号」列を含む");
    };

    let mut report = CsvReport::default();
    for (index, row) in rows.iter().enumerate().skip(header_at + 1) {
        let line = index + 1;
        let cell =
            |col: Option<usize>| col.and_then(|c| row.get(c)).map(|f| f.trim()).unwrap_or("");
        let name = cell(Some(name_col));
        if name.is_empty() {
            continue;
        }
        let Some(io_point) = table.points.iter().find(|p| p.name == name) else {
            report.unknown.push(name.to_string());
            continue;
        };
        let address = match cell(number_col) {
            "" => None,
            n => match n.parse::<u16>() {
                Ok(n) => Some(n),
                Err(_) => {
                    report.errors.push(format!(
                        "{line} 行目: 番号「{n}」は 0〜65535 の数字にしてください"
                    ));
                    continue;
                }
            },
        };
        let scale = match cell(scale_col) {
            "" => None,
            s => match s.parse::<f64>() {
                Ok(s) if s > 0.0 => Some(s),
                _ => {
                    report.errors.push(format!(
                        "{line} 行目: スケール「{s}」は正の数にしてください"
                    ));
                    continue;
                }
            },
        };
        let settings = PointSettings {
            address,
            // ビットにスケールはない
            scale: scale.filter(|_| io_point.point.is_word()),
            node: cell(node_col).to_string(),
            log_column: cell(log_col).to_string(),
        };
        if settings == PointSettings::default() {
            plan.custom.remove(&io_point.point);
        } else {
            plan.custom.insert(io_point.point, settings);
        }
        report.applied += 1;
    }
    Ok(report)
}

/// カンマ・引用符・改行を含む欄だけ引用符で囲む
fn quote(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// CSV を行と欄に分ける (引用符で囲んだ欄の中のカンマ・改行・"" に対応)
fn parse(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, in_quotes) {
            ('"', true) if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            ('"', _) => in_quotes = !in_quotes,
            (',', false) => row.push(std::mem::take(&mut field)),
            ('\r', false) => {}
            ('\n', false) => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            (c, _) => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::{ActuatorKind, TimingChart};
    use crate::plc::domain::io::Point;

    fn chart() -> (TimingChart, u32, u32) {
        let mut chart = TimingChart::default();
        let cyl = chart.add_actuator(ActuatorKind::Cylinder);
        chart.actuator_mut(cyl).unwrap().name = "リフタ".into();
        let axis = chart.add_actuator(ActuatorKind::LinearAxis);
        chart.actuator_mut(axis).unwrap().name = "X軸".into();
        (chart, cyl, axis)
    }

    #[test]
    fn exported_csv_reads_back_without_changes() {
        let (chart, _, _) = chart();
        let mut plan = AddressPlan::default();
        let table = IoTable::from_chart(&chart, &plan);
        let report = apply_csv(&to_csv(&table), &table, &mut plan).unwrap();
        assert_eq!(report.applied, table.points.len());
        assert!(report.errors.is_empty() && report.unknown.is_empty());
        assert_eq!(IoTable::from_chart(&chart, &plan), table);
    }

    #[test]
    fn edited_rows_become_custom_settings() {
        let (chart, cyl, axis) = chart();
        let mut plan = AddressPlan::default();
        let table = IoTable::from_chart(&chart, &plan);
        let csv = "メモ: PLC 側の割り付け\n\
                   信号,向き,種類,番号,スケール,OPC UAノード\n\
                   リフタ.出端,入力,コイル,200,,\"ns=2;s=Lifter,Ext\"\n\
                   X軸.指令位置,出力,保持レジスタ,10,0.01,\n\
                   旧シリンダ.出,出力,コイル,5,,\n\
                   リフタ.戻端,入力,コイル,abc,,\n";
        let report = apply_csv(csv, &table, &mut plan).unwrap();
        assert_eq!(report.applied, 2);
        assert_eq!(report.unknown, vec!["旧シリンダ.出".to_string()]);
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].starts_with("6 行目"));

        let table = IoTable::from_chart(&chart, &plan);
        let ext = table.get(Point::CylinderExtended(cyl)).unwrap();
        assert_eq!((ext.address, ext.node.as_str()), (200, "ns=2;s=Lifter,Ext"));
        let target = table.get(Point::AxisTarget(axis)).unwrap();
        assert_eq!((target.address, target.scale), (10, 0.01));
    }

    #[test]
    fn missing_header_is_an_error() {
        let (chart, _, _) = chart();
        let mut plan = AddressPlan::default();
        let table = IoTable::from_chart(&chart, &plan);
        assert!(apply_csv("a,b,c\n", &table, &mut plan).is_err());
    }
}
