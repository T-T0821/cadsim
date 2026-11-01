//! 動線のヒートマップの画像 (PNG) の書き出し

use std::path::Path;

use cadsim_core::floor::heatmap::HeatImage;

/// ヒートマップの画像を PNG のバイト列にする
pub fn encode(img: &HeatImage) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    use image::codecs::png::PngEncoder;

    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(
            &img.rgba,
            img.width,
            img.height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("PNG にできませんでした: {e}"))?;
    Ok(bytes)
}

/// ヒートマップの画像を PNG に書く
pub fn write(path: &Path, img: &HeatImage) -> Result<(), String> {
    let bytes = encode(img)?;
    std::fs::write(path, bytes).map_err(|e| format!("{} に書けませんでした: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec2;

    #[test]
    fn round_trips_through_png() {
        let img = HeatImage {
            width: 3,
            height: 2,
            rgba: (0..24).map(|i| (i * 10) as u8).collect(),
            min: DVec2::ZERO,
            max: DVec2::new(3.0, 2.0),
        };
        let bytes = encode(&img).unwrap();
        let back = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(back.dimensions(), (3, 2));
        assert_eq!(back.into_raw(), img.rgba);
    }
}
