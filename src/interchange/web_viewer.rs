//! Web ビューア (.html): cadsim が無い人がブラウザ・スマホ・VR ヘッドセットで見るための1ファイル
//!
//! シーンを GLB にして Base64 で HTML に埋め込み、WebGL のビューア
//! (`assets/web_viewer/viewer.html`) と一緒に1つのファイルにする。
//! 外部のスクリプトやネットワークは使わないので、メールで送ってもオフラインで開ける。
//! ビューアの中身は docs/web-viewer.md を参照。

use std::path::Path;

use super::{ExchangeResult, Scene, UnitOptions, gltf::to_glb_bytes, io_error};

/// ビューアの HTML。`__CADSIM_TITLE__` と `__CADSIM_MODEL__` を差し替えて使う
const TEMPLATE: &str = include_str!("../../assets/web_viewer/viewer.html");

pub fn export(scene: &Scene, path: &Path, _units: &UnitOptions) -> ExchangeResult<()> {
    let title = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cadsim".to_owned());
    std::fs::write(path, to_html(scene, &title)).map_err(|e| io_error(path, e))
}

/// シーンを埋め込んだビューアの HTML
pub fn to_html(scene: &Scene, title: &str) -> String {
    TEMPLATE
        .replace("__CADSIM_TITLE__", &escape_html(title))
        .replace("__CADSIM_MODEL__", &base64(&to_glb_bytes(scene)))
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// 標準の Base64 (RFC 4648、パディングあり)
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::test_util::sample_scene;
    use super::*;

    fn decode_base64(text: &str) -> Vec<u8> {
        let value = |c: u8| match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            _ => 63,
        };
        let mut out = Vec::new();
        for chunk in text.as_bytes().chunks(4) {
            let pad = chunk.iter().filter(|&&c| c == b'=').count();
            let n = chunk
                .iter()
                .map(|&c| if c == b'=' { 0 } else { u32::from(value(c)) })
                .fold(0, |n, v| (n << 6) | v);
            out.extend_from_slice(&n.to_be_bytes()[1..4 - pad]);
        }
        out
    }

    #[test]
    fn base64_matches_rfc_examples() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }

    #[test]
    fn html_embeds_the_glb_and_escapes_the_title() {
        let scene = sample_scene();
        let html = to_html(&scene, "<ライン & 1>");
        assert!(html.contains("<title>&lt;ライン &amp; 1&gt;"));
        assert!(!html.contains("__CADSIM_"));

        // 埋め込んだ GLB を取り出して読み直すと、元のシーンに戻る
        let start = html.find("id=\"cadsim-model\"").unwrap();
        let body = &html[start..];
        let body = &body[body.find('>').unwrap() + 1..];
        let encoded = body[..body.find('<').unwrap()].trim();
        let glb = decode_base64(encoded);
        assert_eq!(glb, to_glb_bytes(&scene));

        let dir = std::env::temp_dir().join(format!("cadsim_web_viewer_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("line.glb");
        std::fs::write(&path, &glb).unwrap();
        let back = super::super::gltf::import(&path, &UnitOptions::default()).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(back.nodes.len(), scene.nodes.len());
        assert_eq!(back.animations.len(), scene.animations.len());
    }

    #[test]
    fn export_writes_a_single_file() {
        let dir = std::env::temp_dir().join(format!("cadsim_web_export_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ライン1.html");
        export(&sample_scene(), &path, &UnitOptions::default()).unwrap();
        let html = std::fs::read_to_string(&path).unwrap();
        let files = std::fs::read_dir(&dir).unwrap().count();
        std::fs::remove_dir_all(&dir).ok();
        assert!(html.contains("<title>ライン1"));
        assert_eq!(files, 1);
    }
}
