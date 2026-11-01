//! 部品表を CSV / Excel (.xlsx) に書き出す
//!
//! CSV は Excel でそのまま開けるよう、UTF-8 の BOM 付き・改行 CRLF で書く。
//! Excel は「部品表」と「構成」の2シートにする。

use rust_xlsxwriter::{Format, Workbook};

use crate::bom::Bom;
use crate::drawing::format_length;

fn bom_header(unit: &str) -> Vec<String> {
    [
        "No.",
        "品名",
        "種類",
        "取り込み元",
        &format!("幅 ({unit})"),
        &format!("高さ ({unit})"),
        &format!("奥行 ({unit})"),
        "数量",
        &format!("体積 ({unit}³)"),
        "使用箇所",
    ]
    .map(String::from)
    .to_vec()
}

const TREE_HEADER: [&str; 7] = ["階層", "名前", "種類", "部品表 No.", "X", "Y", "Z"];

pub fn write_csv(bom: &Bom) -> String {
    let rows = bom.rows.iter().map(|row| {
        vec![
            row.number.to_string(),
            row.name.clone(),
            row.kind.clone(),
            row.source.clone(),
            format_length(row.size[0]),
            format_length(row.size[1]),
            format_length(row.size[2]),
            row.quantity.to_string(),
            format_length(row.volume),
            row.used_in.join(" / "),
        ]
    });
    crate::formats::csv_text::write(std::iter::once(bom_header(&bom.unit)).chain(rows))
}

pub fn write_xlsx(bom: &Bom) -> Result<Vec<u8>, String> {
    let err = |e: rust_xlsxwriter::XlsxError| format!("Excel ファイルを作れませんでした: {e}");
    let mut workbook = Workbook::new();
    let bold = Format::new().set_bold().set_background_color("#DDE6F0");

    let sheet = workbook.add_worksheet();
    sheet.set_name("部品表").map_err(err)?;
    for (col, title) in bom_header(&bom.unit).iter().enumerate() {
        sheet
            .write_string_with_format(0, col as u16, title, &bold)
            .map_err(err)?;
    }
    for (i, row) in bom.rows.iter().enumerate() {
        let r = i as u32 + 1;
        sheet.write_number(r, 0, row.number as f64).map_err(err)?;
        sheet.write_string(r, 1, &row.name).map_err(err)?;
        sheet.write_string(r, 2, &row.kind).map_err(err)?;
        sheet.write_string(r, 3, &row.source).map_err(err)?;
        for (c, v) in row.size.iter().enumerate() {
            sheet.write_number(r, 4 + c as u16, *v).map_err(err)?;
        }
        sheet.write_number(r, 7, row.quantity as f64).map_err(err)?;
        sheet.write_number(r, 8, row.volume).map_err(err)?;
        sheet
            .write_string(r, 9, row.used_in.join(" / "))
            .map_err(err)?;
    }
    let total_row = bom.rows.len() as u32 + 1;
    sheet
        .write_string_with_format(total_row, 6, "合計", &bold)
        .map_err(err)?;
    sheet
        .write_number(total_row, 7, bom.total_parts() as f64)
        .map_err(err)?;
    for (col, width) in [6, 28, 8, 22, 10, 10, 10, 6, 14, 30]
        .into_iter()
        .enumerate()
    {
        sheet.set_column_width(col as u16, width).map_err(err)?;
    }

    let tree = workbook.add_worksheet();
    tree.set_name("構成").map_err(err)?;
    for (col, title) in TREE_HEADER.iter().enumerate() {
        tree.write_string_with_format(0, col as u16, *title, &bold)
            .map_err(err)?;
    }
    for (i, row) in bom.tree.iter().enumerate() {
        let r = i as u32 + 1;
        tree.write_number(r, 0, row.level as f64).map_err(err)?;
        // 階層が分かるよう、深さの分だけ字下げする
        let name = format!("{}{}", "　".repeat(row.level), row.name);
        tree.write_string(r, 1, name).map_err(err)?;
        tree.write_string(r, 2, &row.kind).map_err(err)?;
        if let Some(n) = row.bom_number {
            tree.write_number(r, 3, n as f64).map_err(err)?;
        }
        for (c, v) in row.position.iter().enumerate() {
            tree.write_number(r, 4 + c as u16, *v).map_err(err)?;
        }
    }
    for (col, width) in [6, 36, 12, 10, 12, 12, 12].into_iter().enumerate() {
        tree.set_column_width(col as u16, width).map_err(err)?;
    }
    workbook.save_to_buffer().map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bom::make_bom;
    use crate::model::Model;

    fn bom() -> Bom {
        let mut model = Model::default();
        model.add_box([100.0, 50.0, 20.0], [0.0; 3]);
        model.add_box([100.0, 50.0, 20.0], [300.0, 0.0, 0.0]);
        make_bom(&model)
    }

    #[test]
    fn csv_has_bom_header_and_rows() {
        let csv = write_csv(&bom());
        assert!(csv.starts_with('\u{FEFF}'));
        let lines: Vec<&str> = csv.trim_start_matches('\u{FEFF}').split("\r\n").collect();
        assert!(lines[0].starts_with("No.,品名,種類"));
        assert_eq!(lines[1], "1,箱,箱,,100,50,20,2,100000,(最上位)");
    }

    #[test]
    fn xlsx_is_a_zip() {
        let bytes = write_xlsx(&bom()).unwrap();
        assert!(bytes.starts_with(b"PK"));
    }
}
