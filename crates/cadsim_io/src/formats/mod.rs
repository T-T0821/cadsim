//! ファイル形式の読み書き (アダプター層)
//!
//! 平面図 (DXF / PDF / 画像)・点群 (PLY / LAS / LAZ / E57) の読み込みと、
//! 図面 (DXF / PDF)・部品表 (CSV / Excel)・動線のヒートマップ (PNG) の書き出し。
//! Bevy にも egui にも依存しない。

pub mod bom_out;
pub mod camera_path_record;
pub mod chart_record;
pub mod conveyor_record;
pub mod drawing_out;
pub mod dxf;
pub mod heatmap_png;
pub mod pdf_plan;
pub mod pointcloud_read;
pub mod site_record;

use std::path::Path;

use glam::DVec2;

use cadsim_core::site::underlay::{PlanContent, PlanImage};

/// 下敷きにできる平面図の拡張子
pub const PLAN_EXTENSIONS: &[&str] = &["dxf", "pdf", "png", "jpg", "jpeg"];

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// 平面図として読むファイルか
pub fn is_plan_file(path: &Path) -> bool {
    PLAN_EXTENSIONS.contains(&extension(path).as_str())
}

/// 平面図のファイルを読む (拡張子で形式を選ぶ)
pub fn read_plan_file(path: &Path) -> Result<PlanContent, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("{} を開けませんでした: {e}", path.display()))?;
    match extension(path).as_str() {
        "dxf" => dxf::read_plan(&String::from_utf8_lossy(&bytes)),
        "pdf" => pdf_plan::read_plan(&bytes),
        _ => read_image_plan(&bytes),
    }
}

/// 画像を、1画素 = 1 の大きさで敷く平面図にする (縮尺は後で合わせる)
pub fn read_image_plan(bytes: &[u8]) -> Result<PlanContent, String> {
    let image = image::load_from_memory(bytes)
        .map_err(|e| format!("画像を読めませんでした: {e}"))?
        .to_rgba8();
    let (width, height) = image.dimensions();
    Ok(PlanContent {
        polylines: Vec::new(),
        image: Some(PlanImage {
            width,
            height,
            rgba: image.into_raw(),
            min: DVec2::ZERO,
            max: DVec2::new(width as f64, height as f64),
        }),
        unit_mm: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_becomes_image_plan() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 2, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let plan = read_image_plan(&png).unwrap();
        let image = plan.image.unwrap();
        assert_eq!((image.width, image.height), (4, 2));
        assert_eq!(image.max, DVec2::new(4.0, 2.0));
        assert_eq!(&image.rgba[..4], &[10, 20, 30, 255]);
        assert!(is_plan_file(Path::new("a/B.JPG")));
        assert!(!is_plan_file(Path::new("a/b.step")));
    }
}
