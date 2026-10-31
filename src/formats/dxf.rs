//! DXF (ASCII) の読み書き
//!
//! 読み込み: 平面図の下敷き用に、線として見えるものを折れ線にする。
//! LINE / LWPOLYLINE (ふくらみ付きの円弧も) / POLYLINE / CIRCLE / ARC / ELLIPSE / SPLINE と、
//! ブロック参照 (INSERT) の展開に対応する。文字・寸法・ハッチは読み飛ばす。
//!
//! 書き出し: 2D 図面を AutoCAD R12 形式で書く。線種 (実線・かくれ線) と画層を定義し、
//! 線分と文字だけで描く (寸法も線と文字に分解して書く)。日本語は `\U+XXXX` で書く。

use std::collections::HashMap;
use std::f64::consts::TAU;
use std::fmt::Write as _;

use glam::{DAffine2, DVec2};

use crate::site::underlay::PlanContent;

/// ブロックの入れ子をたどる深さの上限 (自分自身を参照する壊れたファイルで止まらないように)
const MAX_INSERT_DEPTH: usize = 8;
/// 円や円弧を折れ線にするときの、1周あたりの分割数
const CIRCLE_SEGMENTS: f64 = 72.0;

/// グループコードと値の組
struct Pair<'a> {
    code: i32,
    value: &'a str,
}

/// 1つの図形 (ENTITIES や BLOCK の中の1要素)
struct Entity<'a> {
    kind: &'a str,
    pairs: Vec<Pair<'a>>,
}

impl Entity<'_> {
    fn f(&self, code: i32) -> Option<f64> {
        self.pairs
            .iter()
            .find(|p| p.code == code)
            .and_then(|p| p.value.trim().parse().ok())
    }

    fn f_or(&self, code: i32, default: f64) -> f64 {
        self.f(code).unwrap_or(default)
    }

    fn i(&self, code: i32) -> Option<i64> {
        self.pairs
            .iter()
            .find(|p| p.code == code)
            .and_then(|p| p.value.trim().parse().ok())
    }

    fn s(&self, code: i32) -> Option<&str> {
        self.pairs
            .iter()
            .find(|p| p.code == code)
            .map(|p| p.value.trim())
    }

    fn point(&self, x: i32, y: i32) -> DVec2 {
        DVec2::new(self.f_or(x, 0.0), self.f_or(y, 0.0))
    }
}

/// DXF の文字列を読み、平面図にする
pub fn read_plan(text: &str) -> Result<PlanContent, String> {
    if text.starts_with("AutoCAD Binary DXF") {
        return Err("バイナリ形式の DXF には対応していません。ASCII 形式で保存し直してください".into());
    }
    let pairs = parse_pairs(text)?;
    let sections = split_sections(&pairs);

    let unit_mm = sections
        .get("HEADER")
        .and_then(|h| header_value(h, "$INSUNITS"))
        .and_then(insunits_mm);

    let mut blocks: HashMap<String, Vec<Entity>> = HashMap::new();
    if let Some(block_pairs) = sections.get("BLOCKS") {
        let mut current: Option<(String, Vec<Entity>)> = None;
        for entity in group_entities(block_pairs) {
            match entity.kind {
                "BLOCK" => current = Some((entity.s(2).unwrap_or("").to_owned(), Vec::new())),
                "ENDBLK" => {
                    if let Some((name, list)) = current.take() {
                        blocks.insert(name, list);
                    }
                }
                _ => {
                    if let Some((_, list)) = &mut current {
                        list.push(entity);
                    }
                }
            }
        }
    }

    let entities = sections
        .get("ENTITIES")
        .map(|p| group_entities(p))
        .unwrap_or_default();
    let mut polylines = Vec::new();
    let reader = Reader { blocks: &blocks };
    reader.collect(&entities, DAffine2::IDENTITY, 0, &mut polylines);
    if polylines.is_empty() {
        return Err("DXF に線が見つかりませんでした".into());
    }
    Ok(PlanContent {
        polylines,
        image: None,
        unit_mm,
    })
}

fn parse_pairs(text: &str) -> Result<Vec<Pair<'_>>, String> {
    let mut lines = text.lines();
    let mut pairs = Vec::new();
    while let Some(code) = lines.next() {
        let code = code.trim();
        if code.is_empty() {
            continue;
        }
        let code: i32 = code
            .parse()
            .map_err(|_| format!("DXF のグループコードを読めませんでした: {code}"))?;
        let value = lines.next().unwrap_or("");
        pairs.push(Pair {
            code,
            value: value.trim_end_matches('\r'),
        });
    }
    Ok(pairs)
}

/// SECTION ごとに分ける (セクション名 → 中身)
fn split_sections<'a, 'b>(pairs: &'b [Pair<'a>]) -> HashMap<String, &'b [Pair<'a>]> {
    let mut out = HashMap::new();
    let mut i = 0;
    while i < pairs.len() {
        if pairs[i].code == 0 && pairs[i].value.trim() == "SECTION" && i + 1 < pairs.len() {
            let name = pairs[i + 1].value.trim().to_owned();
            let start = i + 2;
            let mut end = start;
            while end < pairs.len() && !(pairs[end].code == 0 && pairs[end].value.trim() == "ENDSEC")
            {
                end += 1;
            }
            out.insert(name, &pairs[start..end]);
            i = end;
        }
        i += 1;
    }
    out
}

fn header_value(pairs: &[Pair], name: &str) -> Option<i64> {
    let pos = pairs
        .iter()
        .position(|p| p.code == 9 && p.value.trim() == name)?;
    pairs.get(pos + 1)?.value.trim().parse().ok()
}

/// $INSUNITS の番号 → 1 単位が何 mm か
fn insunits_mm(code: i64) -> Option<f64> {
    Some(match code {
        1 => 25.4,
        2 => 304.8,
        4 => 1.0,
        5 => 10.0,
        6 => 1000.0,
        8 => 0.0000254,
        9 => 0.0254,
        10 => 914.4,
        14 => 100.0,
        _ => return None,
    })
}

/// グループコード 0 で区切って図形にする
fn group_entities<'a>(pairs: &[Pair<'a>]) -> Vec<Entity<'a>> {
    let mut out: Vec<Entity> = Vec::new();
    for pair in pairs {
        if pair.code == 0 {
            out.push(Entity {
                kind: pair.value.trim(),
                pairs: Vec::new(),
            });
        } else if let Some(last) = out.last_mut() {
            last.pairs.push(Pair {
                code: pair.code,
                value: pair.value,
            });
        }
    }
    out
}

struct Reader<'a, 'b> {
    blocks: &'b HashMap<String, Vec<Entity<'a>>>,
}

impl Reader<'_, '_> {
    fn collect(&self, entities: &[Entity], m: DAffine2, depth: usize, out: &mut Vec<Vec<DVec2>>) {
        let mut i = 0;
        while i < entities.len() {
            let e = &entities[i];
            let mut push = |points: Vec<DVec2>| {
                if points.len() >= 2 {
                    out.push(points.into_iter().map(|p| m.transform_point2(p)).collect());
                }
            };
            match e.kind {
                "LINE" => push(vec![e.point(10, 20), e.point(11, 21)]),
                "LWPOLYLINE" => push(lwpolyline(e)),
                "POLYLINE" => {
                    // 続く VERTEX を SEQEND まで集める
                    let mut vertices = Vec::new();
                    let mut j = i + 1;
                    while j < entities.len() && entities[j].kind == "VERTEX" {
                        vertices.push((entities[j].point(10, 20), entities[j].f_or(42, 0.0)));
                        j += 1;
                    }
                    let closed = e.i(70).unwrap_or(0) & 1 == 1;
                    push(with_bulges(&vertices, closed));
                    i = j;
                }
                "CIRCLE" => push(arc(e.point(10, 20), e.f_or(40, 0.0), 0.0, TAU)),
                "ARC" => {
                    let start = e.f_or(50, 0.0).to_radians();
                    let mut end = e.f_or(51, 360.0).to_radians();
                    if end <= start {
                        end += TAU;
                    }
                    push(arc(e.point(10, 20), e.f_or(40, 0.0), start, end));
                }
                "ELLIPSE" => push(ellipse(e)),
                "SPLINE" => push(spline(e)),
                "INSERT" if depth < MAX_INSERT_DEPTH => {
                    if let Some(block) = e.s(2).and_then(|name| self.blocks.get(name)) {
                        let scale = DVec2::new(e.f_or(41, 1.0), e.f_or(42, 1.0));
                        let local = DAffine2::from_scale_angle_translation(
                            scale,
                            e.f_or(50, 0.0).to_radians(),
                            e.point(10, 20),
                        );
                        self.collect(block, m * local, depth + 1, out);
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

fn lwpolyline(e: &Entity) -> Vec<DVec2> {
    // 10 / 20 / 42 が頂点の数だけ繰り返す。42 (ふくらみ) はその頂点から次の頂点への円弧
    let mut vertices: Vec<(DVec2, f64)> = Vec::new();
    for pair in &e.pairs {
        let v: f64 = pair.value.trim().parse().unwrap_or(0.0);
        match pair.code {
            10 => vertices.push((DVec2::new(v, 0.0), 0.0)),
            20 => {
                if let Some(last) = vertices.last_mut() {
                    last.0.y = v;
                }
            }
            42 => {
                if let Some(last) = vertices.last_mut() {
                    last.1 = v;
                }
            }
            _ => {}
        }
    }
    let closed = e.i(70).unwrap_or(0) & 1 == 1;
    with_bulges(&vertices, closed)
}

/// ふくらみ (bulge) 付きの頂点列を折れ線にする
fn with_bulges(vertices: &[(DVec2, f64)], closed: bool) -> Vec<DVec2> {
    let mut out = Vec::new();
    let n = vertices.len();
    let segments = if closed { n } else { n.saturating_sub(1) };
    if let Some(first) = vertices.first() {
        out.push(first.0);
    }
    for k in 0..segments {
        let (a, bulge) = vertices[k];
        let b = vertices[(k + 1) % n].0;
        if bulge.abs() > 1e-9 {
            // ふくらみ = tan(中心角 / 4)
            let angle = 4.0 * bulge.atan();
            let chord = b - a;
            let radius = chord.length() / (2.0 * (angle / 2.0).sin()).abs();
            let mid = (a + b) / 2.0;
            let normal = DVec2::new(-chord.y, chord.x).normalize_or_zero();
            let h = radius * (angle / 2.0).cos().abs();
            // 小さい弧 (|角| < π) では中心は進行方向の左 (bulge > 0) か右
            let side = if angle.abs() < std::f64::consts::PI { 1.0 } else { -1.0 };
            let center = mid + normal * h * bulge.signum() * side;
            let start = (a - center).to_angle();
            let points = arc(center, radius, start, start + angle);
            out.extend(points.into_iter().skip(1));
        } else {
            out.push(b);
        }
    }
    out
}

fn arc(center: DVec2, radius: f64, start: f64, end: f64) -> Vec<DVec2> {
    if radius <= 0.0 {
        return Vec::new();
    }
    let steps = (((end - start).abs() / TAU) * CIRCLE_SEGMENTS).ceil().max(2.0) as usize;
    (0..=steps)
        .map(|k| {
            let t = start + (end - start) * k as f64 / steps as f64;
            center + DVec2::from_angle(t) * radius
        })
        .collect()
}

fn ellipse(e: &Entity) -> Vec<DVec2> {
    let center = e.point(10, 20);
    let major = e.point(11, 21);
    let ratio = e.f_or(40, 1.0);
    let start = e.f_or(41, 0.0);
    let mut end = e.f_or(42, TAU);
    if end <= start {
        end += TAU;
    }
    let minor = DVec2::new(-major.y, major.x) * ratio;
    let steps = (((end - start) / TAU) * CIRCLE_SEGMENTS).ceil().max(2.0) as usize;
    (0..=steps)
        .map(|k| {
            let t = start + (end - start) * k as f64 / steps as f64;
            center + major * t.cos() + minor * t.sin()
        })
        .collect()
}

/// スプラインは通過点 (11/21) があればそれを、なければ制御点 (10/20) を結ぶ
fn spline(e: &Entity) -> Vec<DVec2> {
    let collect = |xc: i32, yc: i32| {
        let mut points: Vec<DVec2> = Vec::new();
        for pair in &e.pairs {
            let v: f64 = pair.value.trim().parse().unwrap_or(0.0);
            if pair.code == xc {
                points.push(DVec2::new(v, 0.0));
            } else if pair.code == yc
                && let Some(last) = points.last_mut()
            {
                last.y = v;
            }
        }
        points
    };
    let fit = collect(11, 21);
    if fit.len() >= 2 { fit } else { collect(10, 20) }
}

// ---------------------------------------------------------------------------
// 書き出し
// ---------------------------------------------------------------------------

/// DXF に書く2D の図
pub struct DxfDrawing<'a> {
    /// (画層, 始点, 終点)
    pub lines: Vec<(&'a str, DVec2, DVec2)>,
    /// (画層, 位置 (左下), 高さ, 文字)
    pub texts: Vec<(&'a str, DVec2, f64, String)>,
    /// $INSUNITS (4 = mm など)
    pub insunits: i64,
}

/// 画層の名前と色番号と線種
pub const DXF_LAYERS: &[(&str, i32, &str)] = &[
    ("OUTLINE", 7, "CONTINUOUS"),
    ("HIDDEN", 8, "HIDDEN"),
    ("DIM", 3, "CONTINUOUS"),
    ("TEXT", 7, "CONTINUOUS"),
    ("FRAME", 7, "CONTINUOUS"),
];

/// mm 単位系の DXF ($INSUNITS) の番号
pub fn insunits_for(unit: &str) -> i64 {
    match unit.to_ascii_lowercase().as_str() {
        "inch" | "in" => 1,
        "ft" | "feet" => 2,
        "cm" => 5,
        "m" => 6,
        _ => 4,
    }
}

pub fn write_drawing(drawing: &DxfDrawing) -> String {
    let mut s = String::new();
    let mut pair = |code: i32, value: &str| {
        let _ = writeln!(s, "{code:>3}\n{value}");
    };
    pair(0, "SECTION");
    pair(2, "HEADER");
    pair(9, "$ACADVER");
    pair(1, "AC1009");
    pair(9, "$INSUNITS");
    pair(70, &drawing.insunits.to_string());
    pair(0, "ENDSEC");

    pair(0, "SECTION");
    pair(2, "TABLES");
    pair(0, "TABLE");
    pair(2, "LTYPE");
    pair(70, "2");
    for (name, description, pattern) in [
        ("CONTINUOUS", "Solid line", &[][..]),
        ("HIDDEN", "Hidden __ __ __", &[3.0, -1.5][..]),
    ] {
        pair(0, "LTYPE");
        pair(2, name);
        pair(70, "0");
        pair(3, description);
        pair(72, "65");
        pair(73, &pattern.len().to_string());
        pair(40, &num(pattern.iter().map(|v| v.abs()).sum()));
        for v in pattern {
            pair(49, &num(*v));
        }
    }
    pair(0, "ENDTAB");
    pair(0, "TABLE");
    pair(2, "LAYER");
    pair(70, &DXF_LAYERS.len().to_string());
    for (name, color, ltype) in DXF_LAYERS {
        pair(0, "LAYER");
        pair(2, name);
        pair(70, "0");
        pair(62, &color.to_string());
        pair(6, ltype);
    }
    pair(0, "ENDTAB");
    pair(0, "ENDSEC");

    pair(0, "SECTION");
    pair(2, "ENTITIES");
    for (layer, a, b) in &drawing.lines {
        pair(0, "LINE");
        pair(8, layer);
        pair(10, &num(a.x));
        pair(20, &num(a.y));
        pair(30, "0");
        pair(11, &num(b.x));
        pair(21, &num(b.y));
        pair(31, "0");
    }
    for (layer, at, height, text) in &drawing.texts {
        pair(0, "TEXT");
        pair(8, layer);
        pair(10, &num(at.x));
        pair(20, &num(at.y));
        pair(30, "0");
        pair(40, &num(*height));
        pair(1, &escape_text(text));
    }
    pair(0, "ENDSEC");
    pair(0, "EOF");
    s
}

fn num(v: f64) -> String {
    let v = if v.abs() < 1e-12 { 0.0 } else { v };
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_owned()
}

/// ASCII 以外の文字を `\U+XXXX` にする (R12 の DXF は文字コードを持たないため)
fn escape_text(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii() && !c.is_ascii_control() {
            out.push(c);
        } else if (c as u32) <= 0xFFFF {
            let _ = write!(out, "\\U+{:04X}", c as u32);
        } else {
            out.push('?');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dxf(header: &str, blocks: &str, entities: &str) -> String {
        format!(
            "0\nSECTION\n2\nHEADER\n{header}0\nENDSEC\n0\nSECTION\n2\nBLOCKS\n{blocks}0\nENDSEC\n0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n"
        )
    }

    #[test]
    fn reads_lines_circles_and_units() {
        let text = dxf(
            "9\n$INSUNITS\n70\n6\n",
            "",
            "0\nLINE\n8\n0\n10\n0\n20\n0\n11\n5\n21\n0\n0\nCIRCLE\n10\n2\n20\n2\n40\n1\n",
        );
        let plan = read_plan(&text).unwrap();
        assert_eq!(plan.unit_mm, Some(1000.0));
        assert_eq!(plan.polylines.len(), 2);
        assert_eq!(plan.polylines[0], vec![DVec2::ZERO, DVec2::new(5.0, 0.0)]);
        let (min, max) = plan.bounds().unwrap();
        assert!((min - DVec2::new(0.0, 0.0)).length() < 1e-9);
        assert!((max - DVec2::new(5.0, 3.0)).length() < 1e-9);
    }

    #[test]
    fn closed_lwpolyline_with_bulge() {
        // 半円 2 つで直径 2 の円になる
        let text = dxf(
            "",
            "",
            "0\nLWPOLYLINE\n90\n2\n70\n1\n10\n0\n20\n0\n42\n1\n10\n2\n20\n0\n42\n1\n",
        );
        let plan = read_plan(&text).unwrap();
        let line = &plan.polylines[0];
        assert!(line.len() > 10);
        for p in line {
            assert!(((*p - DVec2::new(1.0, 0.0)).length() - 1.0).abs() < 1e-9, "{p}");
        }
        assert!((line.first().unwrap() - line.last().unwrap()).length() < 1e-9);
    }

    #[test]
    fn inserts_expand_blocks() {
        let text = dxf(
            "",
            "0\nBLOCK\n2\nDESK\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n0\n0\nENDBLK\n",
            "0\nINSERT\n2\nDESK\n10\n10\n20\n5\n41\n2\n42\n2\n50\n90\n",
        );
        let plan = read_plan(&text).unwrap();
        let line = &plan.polylines[0];
        assert!((line[0] - DVec2::new(10.0, 5.0)).length() < 1e-9);
        assert!((line[1] - DVec2::new(10.0, 7.0)).length() < 1e-9, "{}", line[1]);
    }

    #[test]
    fn written_drawing_reads_back() {
        let drawing = DxfDrawing {
            lines: vec![("OUTLINE", DVec2::ZERO, DVec2::new(100.0, 50.0))],
            texts: vec![("TEXT", DVec2::new(1.0, 2.0), 3.5, "上面 A-1".into())],
            insunits: 4,
        };
        let text = write_drawing(&drawing);
        assert!(text.contains("\\U+4E0A\\U+9762 A-1"), "{text}");
        let plan = read_plan(&text).unwrap();
        assert_eq!(plan.unit_mm, Some(1.0));
        assert_eq!(
            plan.polylines,
            vec![vec![DVec2::ZERO, DVec2::new(100.0, 50.0)]]
        );
    }

    #[test]
    fn rejects_binary_and_empty() {
        assert!(read_plan("AutoCAD Binary DXF\r\n").is_err());
        assert!(read_plan(&dxf("", "", "")).is_err());
    }
}
