//! PDF の平面図を下敷き用に読む
//!
//! CAD から書き出した PDF は線 (パス) で描かれているので、1ページ目の描画命令をたどって
//! 線を折れ線として取り出す。座標変換 (cm, q / Q) とフォーム XObject の入れ子に対応する。
//! スキャンした図面のように画像だけのページは、いちばん大きな画像 (JPEG か 8bit の RGB / グレー)
//! を下敷きの画像にする。座標の単位はポイント (1/72 インチ)。

use std::collections::HashSet;

use glam::{DAffine2, DVec2};
use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::site::underlay::{PlanContent, PlanImage};

/// 1 ポイントは何 mm か
const POINT_MM: f64 = 25.4 / 72.0;
/// ベジェ曲線を折れ線にするときの分割数
const BEZIER_STEPS: usize = 12;
/// フォームの入れ子をたどる深さの上限
const MAX_FORM_DEPTH: usize = 8;

pub fn read_plan(bytes: &[u8]) -> Result<PlanContent, String> {
    let doc = Document::load_mem(bytes).map_err(|e| format!("PDF を読めませんでした: {e}"))?;
    let page = *doc
        .get_pages()
        .values()
        .next()
        .ok_or("PDF にページがありません")?;
    let content = doc.get_page_content(page);
    let resources = page_resources(&doc, page);

    let mut walker = Walker {
        doc: &doc,
        polylines: Vec::new(),
        image: None,
        visited: HashSet::new(),
    };
    walker.run(&content, DAffine2::IDENTITY, resources.as_ref(), 0);
    if walker.polylines.is_empty() && walker.image.is_none() {
        return Err("PDF の1ページ目に線も画像も見つかりませんでした".into());
    }
    Ok(PlanContent {
        polylines: walker.polylines,
        image: walker.image.map(|(_, image)| image),
        unit_mm: Some(POINT_MM),
    })
}

/// ページの Resources (親のページツリーから引き継ぐものも含む)
fn page_resources(doc: &Document, page: ObjectId) -> Option<Dictionary> {
    let (direct, inherited) = doc.get_page_resources(page).ok()?;
    if let Some(dict) = direct {
        return Some(dict.clone());
    }
    inherited
        .into_iter()
        .find_map(|id| match doc.get_object(id) {
            Ok(Object::Dictionary(d)) => Some(d.clone()),
            _ => None,
        })
}

fn deref<'a>(doc: &'a Document, object: &'a Object) -> &'a Object {
    match object {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(object),
        _ => object,
    }
}

fn number(object: &Object) -> f64 {
    match object {
        Object::Integer(i) => *i as f64,
        Object::Real(r) => *r as f64,
        _ => 0.0,
    }
}

fn name(object: &Object) -> Option<&[u8]> {
    match object {
        Object::Name(n) => Some(n),
        _ => None,
    }
}

struct Walker<'a> {
    doc: &'a Document,
    polylines: Vec<Vec<DVec2>>,
    /// (ページ上の面積, 画像)。いちばん大きなものだけ残す
    image: Option<(f64, PlanImage)>,
    /// たどったフォーム (循環参照で止まらないように)
    visited: HashSet<ObjectId>,
}

impl Walker<'_> {
    fn run(
        &mut self,
        content: &[u8],
        base: DAffine2,
        resources: Option<&Dictionary>,
        depth: usize,
    ) {
        let Ok(content) = Content::decode(content) else {
            return;
        };
        let mut ctm = base;
        let mut stack = Vec::new();
        // 組み立て中のパス (サブパスの列)。描画命令 (S, f など) で確定する
        let mut path: Vec<Vec<DVec2>> = Vec::new();
        for op in &content.operations {
            let n: Vec<f64> = op.operands.iter().map(number).collect();
            let at = |i: usize| n.get(i).copied().unwrap_or(0.0);
            let pt = |i: usize| ctm.transform_point2(DVec2::new(at(i), at(i + 1)));
            match op.operator.as_str() {
                "q" => stack.push(ctm),
                "Q" => ctm = stack.pop().unwrap_or(base),
                "cm" => {
                    ctm *= DAffine2::from_cols_array(&[at(0), at(1), at(2), at(3), at(4), at(5)])
                }
                "m" => path.push(vec![pt(0)]),
                "l" => {
                    let p = pt(0);
                    match path.last_mut() {
                        Some(sub) => sub.push(p),
                        None => path.push(vec![p]),
                    }
                }
                "c" | "v" | "y" => {
                    let Some(sub) = path.last_mut() else { continue };
                    let Some(&p0) = sub.last() else { continue };
                    let (p1, p2, p3) = match op.operator.as_str() {
                        "c" => (pt(0), pt(2), pt(4)),
                        "v" => (p0, pt(0), pt(2)),
                        _ => (pt(0), pt(2), pt(2)),
                    };
                    for k in 1..=BEZIER_STEPS {
                        let t = k as f64 / BEZIER_STEPS as f64;
                        let s = 1.0 - t;
                        sub.push(
                            p0 * (s * s * s)
                                + p1 * (3.0 * s * s * t)
                                + p2 * (3.0 * s * t * t)
                                + p3 * (t * t * t),
                        );
                    }
                }
                "h" => {
                    if let Some(sub) = path.last_mut()
                        && let Some(&first) = sub.first()
                    {
                        sub.push(first);
                    }
                }
                "re" => {
                    let (x, y, w, h) = (at(0), at(1), at(2), at(3));
                    let corners = [(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)];
                    path.push(
                        corners
                            .iter()
                            .map(|&(px, py)| ctm.transform_point2(DVec2::new(px, py)))
                            .collect(),
                    );
                }
                "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                    let close = matches!(op.operator.as_str(), "s" | "b" | "b*");
                    for mut sub in path.drain(..) {
                        if close && let Some(&first) = sub.first() {
                            sub.push(first);
                        }
                        if sub.len() >= 2 {
                            self.polylines.push(sub);
                        }
                    }
                }
                "n" => path.clear(),
                "Do" if depth < MAX_FORM_DEPTH => {
                    if let Some(name) = op.operands.first().and_then(name) {
                        self.draw_xobject(name, ctm, resources, depth);
                    }
                }
                _ => {}
            }
        }
    }

    fn draw_xobject(
        &mut self,
        name: &[u8],
        ctm: DAffine2,
        resources: Option<&Dictionary>,
        depth: usize,
    ) {
        let doc = self.doc;
        let Some(xobjects) = resources.and_then(|r| r.get(b"XObject").ok()) else {
            return;
        };
        let Object::Dictionary(xobjects) = deref(doc, xobjects) else {
            return;
        };
        let Ok(entry) = xobjects.get(name) else {
            return;
        };
        if let Object::Reference(id) = entry
            && !self.visited.insert(*id)
            && depth > 0
        {
            return;
        }
        let Object::Stream(stream) = deref(doc, entry) else {
            return;
        };
        match stream.dict.get(b"Subtype").ok().and_then(name_of) {
            Some(b"Form") => {
                let matrix = match stream.dict.get(b"Matrix") {
                    Ok(Object::Array(a)) if a.len() == 6 => {
                        let v: Vec<f64> = a.iter().map(number).collect();
                        DAffine2::from_cols_array(&[v[0], v[1], v[2], v[3], v[4], v[5]])
                    }
                    _ => DAffine2::IDENTITY,
                };
                let own = match stream.dict.get(b"Resources").map(|r| deref(doc, r)) {
                    Ok(Object::Dictionary(d)) => Some(d.clone()),
                    _ => resources.cloned(),
                };
                let data = stream
                    .decompressed_content()
                    .unwrap_or_else(|_| stream.content.clone());
                self.run(&data, ctm * matrix, own.as_ref(), depth + 1);
            }
            Some(b"Image") => {
                let corners =
                    [DVec2::ZERO, DVec2::X, DVec2::ONE, DVec2::Y].map(|c| ctm.transform_point2(c));
                let min = corners
                    .iter()
                    .copied()
                    .reduce(DVec2::min)
                    .unwrap_or_default();
                let max = corners
                    .iter()
                    .copied()
                    .reduce(DVec2::max)
                    .unwrap_or_default();
                let area = (max - min).x * (max - min).y;
                if self.image.as_ref().is_some_and(|(a, _)| *a >= area) {
                    return;
                }
                if let Some((width, height, rgba)) = decode_image(doc, stream) {
                    self.image = Some((
                        area,
                        PlanImage {
                            width,
                            height,
                            rgba,
                            min,
                            max,
                        },
                    ));
                }
            }
            _ => {}
        }
    }
}

fn name_of(object: &Object) -> Option<&[u8]> {
    name(object)
}

/// 画像 XObject を RGBA にする。JPEG と、8bit の RGB / グレーだけ
fn decode_image(doc: &Document, stream: &lopdf::Stream) -> Option<(u32, u32, Vec<u8>)> {
    let filters: Vec<Vec<u8>> = match stream.dict.get(b"Filter").ok().map(|f| deref(doc, f)) {
        Some(Object::Name(n)) => vec![n.clone()],
        Some(Object::Array(a)) => a.iter().filter_map(name).map(<[u8]>::to_vec).collect(),
        _ => Vec::new(),
    };
    if filters.iter().any(|f| f == b"DCTDecode") {
        let image = image::load_from_memory_with_format(&stream.content, image::ImageFormat::Jpeg)
            .ok()?
            .to_rgba8();
        return Some((image.width(), image.height(), image.into_raw()));
    }
    let width = stream.dict.get(b"Width").map(number).ok()? as u32;
    let height = stream.dict.get(b"Height").map(number).ok()? as u32;
    let bits = stream
        .dict
        .get(b"BitsPerComponent")
        .map(number)
        .unwrap_or(8.0);
    if bits != 8.0 {
        return None;
    }
    let channels = match stream.dict.get(b"ColorSpace").ok().map(|c| deref(doc, c)) {
        Some(Object::Name(n)) if n == b"DeviceRGB" => 3,
        Some(Object::Name(n)) if n == b"DeviceGray" => 1,
        Some(Object::Array(a)) if a.first().and_then(name) == Some(b"ICCBased") => {
            match a.get(1).map(|s| deref(doc, s)) {
                Some(Object::Stream(s)) => s.dict.get(b"N").map(number).unwrap_or(3.0) as usize,
                _ => 3,
            }
        }
        _ => return None,
    };
    let raw = if filters.is_empty() {
        stream.content.clone()
    } else {
        stream.decompressed_content().ok()?
    };
    let pixels = (width as usize) * (height as usize);
    if raw.len() < pixels * channels || !(channels == 1 || channels == 3) {
        return None;
    }
    let mut rgba = Vec::with_capacity(pixels * 4);
    for px in raw.chunks_exact(channels).take(pixels) {
        match channels {
            1 => rgba.extend_from_slice(&[px[0], px[0], px[0], 255]),
            _ => rgba.extend_from_slice(&[px[0], px[1], px[2], 255]),
        }
    }
    Some((width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Stream, dictionary};

    /// 線だけの1ページの PDF を作る
    fn pdf_with(content: &str) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.as_bytes().to_vec()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn reads_paths_with_transform() {
        let bytes = pdf_with("q 2 0 0 2 10 20 cm 0 0 m 100 0 l S Q 0 0 50 25 re f");
        let plan = read_plan(&bytes).unwrap();
        assert_eq!(plan.polylines.len(), 2);
        assert_eq!(
            plan.polylines[0],
            vec![DVec2::new(10.0, 20.0), DVec2::new(210.0, 20.0)]
        );
        assert_eq!(plan.polylines[1].len(), 5);
        assert!((plan.unit_mm.unwrap() - 0.352777).abs() < 1e-5);
    }

    #[test]
    fn curves_are_flattened() {
        let bytes = pdf_with("0 0 m 0 10 10 10 10 0 c S");
        let plan = read_plan(&bytes).unwrap();
        let line = &plan.polylines[0];
        assert_eq!(line.len(), BEZIER_STEPS + 1);
        assert_eq!(*line.last().unwrap(), DVec2::new(10.0, 0.0));
        // 曲線の真ん中は 7.5 の高さ
        assert!((line[BEZIER_STEPS / 2].y - 7.5).abs() < 1e-9);
    }

    #[test]
    fn empty_page_is_an_error() {
        assert!(read_plan(&pdf_with("")).is_err());
        assert!(read_plan(b"not a pdf").is_err());
    }
}
