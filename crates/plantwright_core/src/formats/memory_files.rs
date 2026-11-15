//! 埋め込みのファイルを、ディスクのファイルと同じパスで読めるようにする
//!
//! ブラウザ版 (wasm32) にはファイルシステムが無いので、サンプルの glTF やプロジェクトファイルを
//! ここに置き、[`read`] で読む。置いていないパスはディスクから読む (デスクトップ版はいつもこちら)。

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

fn table() -> &'static Mutex<HashMap<PathBuf, &'static [u8]>> {
    static TABLE: OnceLock<Mutex<HashMap<PathBuf, &'static [u8]>>> = OnceLock::new();
    TABLE.get_or_init(Default::default)
}

/// `path` を読んだら `bytes` が返るようにする
pub fn insert(path: impl Into<PathBuf>, bytes: &'static [u8]) {
    table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(path.into(), bytes);
}

/// 置いたファイルがあればそれを、無ければディスクのファイルを読む
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    let found = table()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(path)
        .copied();
    match found {
        Some(bytes) => Ok(bytes.to_vec()),
        None => std::fs::read(path),
    }
}

/// [`read`] して UTF-8 の文字列にする
pub fn read_to_string(path: &Path) -> io::Result<String> {
    String::from_utf8(read(path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserted_file_wins_over_disk() {
        let path = Path::new("/no/such/dir/memory_files_test.txt");
        assert!(read(path).is_err());
        insert(path, b"hello");
        assert_eq!(read_to_string(path).unwrap(), "hello");
    }
}
