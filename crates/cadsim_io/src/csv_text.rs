//! CSV / TSV の文字と表 (行 × 欄の文字) の変換
//!
//! 引用符・区切り・改行の扱いは `csv` クレートに任せる。
//!
//! - 書き出しは Excel (日本語版) でそのまま開けるよう、先頭に BOM を付けた UTF-8、改行は CRLF。
//!   引用符で囲むのは、カンマ・引用符・改行を含む欄だけ。
//! - 読み込みは先頭の BOM を外し、区切りは 1 行目にタブがあればタブ、なければカンマ。
//!   行ごとに欄の数が違ってもよい。空の行は飛ばす。

/// Excel が UTF-8 と判断できるよう先頭に付ける印 (BOM)
pub const BOM: char = '\u{feff}';

/// 表を CSV の文字にする (BOM 付き、改行 CRLF)
pub fn write<R, F>(rows: R) -> String
where
    R: IntoIterator,
    R::Item: IntoIterator<Item = F>,
    F: AsRef<str>,
{
    let mut writer = csv::WriterBuilder::new()
        .flexible(true)
        .terminator(csv::Terminator::CRLF)
        .from_writer(Vec::new());
    for row in rows {
        let fields: Vec<F> = row.into_iter().collect();
        // 欄が 1 つもない行は、空の行として書く (csv クレートは空の記録を書かない)
        if fields.is_empty() {
            writer
                .write_record([""])
                .expect("メモリへの書き込みは失敗しない");
        } else {
            writer
                .write_record(fields.iter().map(|f| f.as_ref()))
                .expect("メモリへの書き込みは失敗しない");
        }
    }
    let bytes = writer.into_inner().expect("メモリへの書き込みは失敗しない");
    let mut text = String::from(BOM);
    text += &String::from_utf8(bytes).expect("UTF-8 の文字から作ったので UTF-8");
    text
}

/// CSV / TSV の文字を表にする
pub fn parse(text: &str) -> Vec<Vec<String>> {
    let text = text.trim_start_matches(BOM);
    let first = text.lines().next().unwrap_or("");
    let delimiter = if first.contains('\t') { b'\t' } else { b',' };
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(text.as_bytes());
    reader
        .records()
        // 文字列から読むので入出力の失敗はなく、UTF-8 の誤りも起きない。壊れた行だけ飛ばす
        .filter_map(Result::ok)
        .map(|record| record.iter().map(str::to_owned).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_bom_crlf_and_quotes_only_what_needs_it() {
        let text = write([
            vec!["名前", "値"],
            vec!["箱 1", "1.5"],
            vec!["a,b", "say \"hi\""],
            vec!["改\n行", ""],
        ]);
        assert_eq!(
            text,
            "\u{feff}名前,値\r\n箱 1,1.5\r\n\"a,b\",\"say \"\"hi\"\"\"\r\n\"改\n行\",\r\n"
        );
    }

    #[test]
    fn reads_quotes_crlf_and_bom() {
        let rows = parse(
            "\u{feff}品種,数量\r\n\"標準品, 青\",\"1,200\"\r\n\"say \"\"hi\"\"\",\"改\r\n行\"\r\n",
        );
        assert_eq!(
            rows,
            [
                vec!["品種", "数量"],
                vec!["標準品, 青", "1,200"],
                vec!["say \"hi\"", "改\r\n行"],
            ]
        );
    }

    #[test]
    fn reads_tab_separated_and_ragged_rows() {
        let rows = parse("A\tB\tC\n1\t2\n\n3\t4\t5\t6\n");
        assert_eq!(
            rows,
            [
                vec!["A", "B", "C"],
                vec!["1", "2"],
                vec!["3", "4", "5", "6"]
            ]
        );
    }

    #[test]
    fn what_is_written_reads_back() {
        let table = vec![
            vec!["信号".to_string(), "メモ".to_string()],
            vec!["リフタ.出".to_string(), "「,」と\"と\r\nを含む".to_string()],
        ];
        assert_eq!(parse(&write(&table)), table);
    }
}
