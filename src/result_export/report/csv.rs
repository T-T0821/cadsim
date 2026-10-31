//! 小さな CSV 書き出し (RFC 4180 形式)
//!
//! Excel (日本語版) でそのまま開けるよう、先頭に BOM を付けた UTF-8 で、改行は CRLF にする。

/// 1行ずつ積んで、最後に文字列にする CSV
#[derive(Default)]
pub struct Csv {
    text: String,
}

impl Csv {
    /// 見出し行を付けて作る
    pub fn with_header(header: &[&str]) -> Self {
        let mut csv = Csv {
            text: String::from('\u{feff}'),
        };
        csv.row(header.iter().map(|s| s.to_string()));
        csv
    }

    /// 1行足す。カンマ・ダブルクォート・改行を含む値はクォートで囲む。
    pub fn row(&mut self, fields: impl IntoIterator<Item = String>) {
        for (i, field) in fields.into_iter().enumerate() {
            if i > 0 {
                self.text.push(',');
            }
            self.text.push_str(&escape(&field));
        }
        self.text.push_str("\r\n");
    }

    pub fn into_string(self) -> String {
        self.text
    }
}

fn escape(field: &str) -> String {
    if field.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// 秒をミリ秒単位までの文字列にする
pub fn seconds(value: f64) -> String {
    format!("{value:.3}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_fields_that_need_it() {
        let mut csv = Csv::with_header(&["名前", "値"]);
        csv.row(["箱 1".to_string(), "1.5".to_string()]);
        csv.row(["a,b".to_string(), "say \"hi\"".to_string()]);
        assert_eq!(
            csv.into_string(),
            "\u{feff}名前,値\r\n箱 1,1.5\r\n\"a,b\",\"say \"\"hi\"\"\"\r\n"
        );
    }
}
