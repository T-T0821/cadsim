//! 生産計画の表を読む: Excel (xlsx / xlsm / xls / ods) の最初のシート、または CSV / TSV
//! (Bevy に依存しない)
//!
//! 文字の表 (行 × 列) にして返すだけで、どの列が何かの解釈はドメインの
//! `cadsim_core::flow::plan::plan_from_rows` がする。

use std::path::Path;

use calamine::{Data, Reader, open_workbook_auto};

/// ファイルを読んで文字の表にする。拡張子で Excel か CSV / TSV かを決める。
pub fn read_table(path: &Path) -> Result<Vec<Vec<String>>, String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => read_workbook(path),
        _ => {
            let bytes = std::fs::read(path).map_err(|e| format!("読めません: {e}"))?;
            Ok(parse_text(&decode(&bytes)))
        }
    }
}

fn read_workbook(path: &Path) -> Result<Vec<Vec<String>>, String> {
    let mut book = open_workbook_auto(path).map_err(|e| format!("Excel として読めません: {e}"))?;
    let name = book
        .sheet_names()
        .first()
        .cloned()
        .ok_or("シートがありません")?;
    let range = book
        .worksheet_range(&name)
        .map_err(|e| format!("シート「{name}」が読めません: {e}"))?;
    Ok(range
        .rows()
        .map(|row| row.iter().map(cell_text).collect())
        .collect())
}

fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{f:.0}"),
        other => other.to_string(),
    }
}

/// UTF-8 (BOM 付きも) でなければ Shift_JIS とみなす
fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => encoding_rs::SHIFT_JIS.decode(bytes).0.into_owned(),
    }
}

/// CSV / TSV の文字を表にする。区切りは 1 行目にタブがあればタブ、なければカンマ。
/// `"…"` で囲んだ欄の中の区切りと改行、`""` (引用符そのもの) を扱う。
pub fn parse_text(text: &str) -> Vec<Vec<String>> {
    let first = text.lines().next().unwrap_or("");
    let sep = if first.contains('\t') { '\t' } else { ',' };
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == sep && !quoted => row.push(std::mem::take(&mut cell)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            c => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_with_quotes_and_crlf() {
        let rows = parse_text("品種,数量,納期\r\n\"標準品, 青\",\"1,200\",8\r\n特注品,50,4\r\n");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1], ["標準品, 青", "1,200", "8"]);
        assert_eq!(rows[2], ["特注品", "50", "4"]);
    }

    #[test]
    fn tab_separated_text_from_excel() {
        let rows = parse_text("品種\t数量\t納期\nA\t10\t2");
        assert_eq!(rows, [vec!["品種", "数量", "納期"], vec!["A", "10", "2"]]);
    }

    #[test]
    fn shift_jis_is_decoded() {
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode("品種,数量,納期\n標準品,1,2\n");
        let rows = parse_text(&decode(&bytes));
        assert_eq!(rows[1][0], "標準品");
    }

    #[test]
    fn an_xlsx_written_by_us_can_be_read_back() {
        let mut book = rust_xlsxwriter::Workbook::new();
        let sheet = book.add_worksheet();
        for (col, title) in ["注文", "品種", "数量", "納期"].iter().enumerate() {
            sheet.write_string(0, col as u16, *title).unwrap();
        }
        sheet.write_string(1, 0, "No.1").unwrap();
        sheet.write_string(1, 1, "標準品").unwrap();
        sheet.write_number(1, 2, 120).unwrap();
        sheet.write_number(1, 3, 4.5).unwrap();
        let dir = std::env::temp_dir().join(format!("cadsim_plan_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plan.xlsx");
        book.save(&path).unwrap();
        let rows = read_table(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(rows[0], ["注文", "品種", "数量", "納期"]);
        assert_eq!(rows[1], ["No.1", "標準品", "120", "4.5"]);
    }
}
