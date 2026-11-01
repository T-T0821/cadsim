//! 結果の CSV を 1 行ずつ積む入れ物
//!
//! 文字への変換 (BOM 付き UTF-8・CRLF・必要な欄だけ引用符) は [`plantwright_core::formats::csv_text::write`] がする。

/// 1行ずつ積んで、最後に文字列にする CSV
#[derive(Default)]
pub struct Csv {
    rows: Vec<Vec<String>>,
}

impl Csv {
    /// 見出し行を付けて作る
    pub fn with_header(header: &[&str]) -> Self {
        let mut csv = Csv::default();
        csv.row(header.iter().map(|s| s.to_string()));
        csv
    }

    /// 1行足す
    pub fn row(&mut self, fields: impl IntoIterator<Item = String>) {
        self.rows.push(fields.into_iter().collect());
    }

    pub fn into_string(self) -> String {
        plantwright_core::formats::csv_text::write(self.rows)
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
