//! 2D 図面 ([`Sheet`]) を DXF / PDF に書き出す
//!
//! DXF は実寸 (モデルの単位) のまま書く。CAD で開くと 1:1 の図形と、尺度に合わせた大きさの文字になる。
//! PDF は A3 横の用紙に尺度を掛けて書く。文字は埋め込みの日本語フォント (Noto Sans JP) を使う。

use std::sync::Arc;

use glam::DVec2;
use krilla::Document;
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Transform};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, LineCap, Stroke, StrokeDash};
use krilla::text::{Font, TextDirection};

use super::dxf::{DxfDrawing, insunits_for, write_drawing};
use cadsim_core::drawing::{Layer, PAPER_MM, Sheet};

const FONT_BYTES: &[u8] = include_bytes!("../../../../assets/fonts/NotoSansJP-Subset.otf");
/// 1 mm は何ポイントか
const PT_PER_MM: f64 = 72.0 / 25.4;

pub fn write_dxf(sheet: &Sheet, unit: &str) -> String {
    let drawing = DxfDrawing {
        lines: sheet
            .lines
            .iter()
            .map(|(layer, a, b)| (layer.dxf_name(), *a, *b))
            .collect(),
        texts: sheet
            .texts
            .iter()
            .map(|t| {
                (
                    t.layer.dxf_name(),
                    t.at,
                    t.height,
                    t.angle_deg,
                    t.text.clone(),
                )
            })
            .collect(),
        insunits: insunits_for(unit),
    };
    write_drawing(&drawing)
}

/// 線の太さ (mm) と破線の有無
fn pen(layer: Layer) -> (f64, bool) {
    match layer {
        Layer::Outline => (0.5, false),
        Layer::Hidden => (0.3, true),
        Layer::Dim | Layer::Text => (0.18, false),
        Layer::Frame => (0.7, false),
    }
}

pub fn write_pdf(sheet: &Sheet) -> Result<Vec<u8>, String> {
    let width = PAPER_MM[0] * PT_PER_MM;
    let height = PAPER_MM[1] * PT_PER_MM;
    // シートの実寸 → 用紙のポイント (PDF は上が y = 0)
    let to_page = |p: DVec2| {
        let mm = (p - sheet.frame_min) * sheet.unit_mm / sheet.scale;
        Point::from_xy(
            (mm.x * PT_PER_MM) as f32,
            (height - mm.y * PT_PER_MM) as f32,
        )
    };

    let mut document = Document::new();
    let settings =
        PageSettings::from_wh(width as f32, height as f32).ok_or("用紙の大きさが不正です")?;
    let mut page = document.start_page_with(settings);
    let mut surface = page.surface();

    for layer in [Layer::Frame, Layer::Hidden, Layer::Outline, Layer::Dim] {
        let mut pb = PathBuilder::new();
        let mut any = false;
        for (_, a, b) in sheet.lines.iter().filter(|l| l.0 == layer) {
            let (a, b) = (to_page(*a), to_page(*b));
            pb.move_to(a.x, a.y);
            pb.line_to(b.x, b.y);
            any = true;
        }
        let Some(path) = pb.finish().filter(|_| any) else {
            continue;
        };
        let (width_mm, dashed) = pen(layer);
        surface.set_fill(None);
        surface.set_stroke(Some(Stroke {
            paint: rgb::Color::new(0, 0, 0).into(),
            width: (width_mm * PT_PER_MM) as f32,
            line_cap: LineCap::Round,
            dash: dashed.then(|| StrokeDash {
                array: vec![(3.0 * PT_PER_MM) as f32, (1.5 * PT_PER_MM) as f32],
                offset: 0.0,
            }),
            ..Default::default()
        }));
        surface.draw_path(&path);
    }

    let font =
        Font::new(Arc::new(FONT_BYTES.to_vec()).into(), 0).ok_or("フォントを読めませんでした")?;
    surface.set_stroke(None);
    surface.set_fill(Some(Fill {
        paint: rgb::Color::new(0, 0, 0).into(),
        opacity: NormalizedF32::ONE,
        rule: Default::default(),
    }));
    for text in &sheet.texts {
        let at = to_page(text.at);
        let size = (sheet.to_paper_mm(text.height) * PT_PER_MM) as f32;
        let angle = (-text.angle_deg).to_radians() as f32;
        let (sin, cos) = angle.sin_cos();
        surface.push_transform(&Transform::from_row(cos, sin, -sin, cos, at.x, at.y));
        surface.draw_text(
            Point::from_xy(0.0, 0.0),
            font.clone(),
            size,
            &text.text,
            false,
            TextDirection::Auto,
        );
        surface.pop();
    }
    surface.finish();
    page.finish();
    document
        .finish()
        .map_err(|e| format!("PDF を作れませんでした: {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadsim_core::drawing::{DrawingInput, DrawingItem, DrawingOptions, make_sheet};
    use glam::DVec3;

    fn sheet() -> Sheet {
        let input = DrawingInput {
            title: "テスト図面".into(),
            unit: "mm".into(),
            unit_mm: 1.0,
            parts: Vec::new(),
            items: vec![DrawingItem {
                name: "箱".into(),
                min: DVec3::ZERO,
                max: DVec3::new(500.0, 300.0, 200.0),
            }],
            date: "2026-10-31".into(),
        };
        make_sheet(&input, &DrawingOptions::default())
    }

    #[test]
    fn pdf_is_written() {
        let bytes = write_pdf(&sheet()).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        // 書いた PDF を平面図として読み直すと、線が入っている
        let plan = super::super::pdf_plan::read_plan(&bytes).unwrap();
        assert!(!plan.polylines.is_empty());
    }

    #[test]
    fn dxf_has_dimensions_and_title() {
        let text = write_dxf(&sheet(), "mm");
        assert!(text.contains("DIM"));
        assert!(
            text.contains("\n500\n") || text.contains("\n  1\n500\n"),
            "幅の寸法"
        );
        let plan = super::super::dxf::read_plan(&text).unwrap();
        assert!(plan.polylines.len() > 10);
    }
}
