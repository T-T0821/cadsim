//! 2D 図面 (ドメイン): 上面図・正面図を寸法付きで描く
//!
//! 部品の稜線と、曲面の輪郭 (シルエット) を平行投影し、手前の面に隠れる部分は
//! かくれ線 (破線) に分ける。全体寸法と、並んだ設備どうしの間隔寸法 (直列寸法) を付け、
//! A3 横の図枠と表題欄に収まる尺度を選ぶ。配置は第三角法 (正面図の上に上面図)。
//!
//! 出来上がる [`Sheet`] は実寸 (モデルの単位) の線と文字の集まりで、
//! DXF / PDF への書き出しは `formats` が受け持つ。Bevy にもファイル形式にも依存しない。

use glam::{DMat4, DVec2, DVec3};

use crate::simulation::mesh::TriMesh;

/// 図面に描く部品1つ
pub struct DrawingPart {
    /// 部品の座標系の三角形を、面ごとに (シルエットを面の中で探すため)
    pub faces: Vec<Vec<[DVec3; 3]>>,
    /// 部品の座標系の稜線
    pub edges: Vec<Vec<DVec3>>,
    /// 部品 → ワールド
    pub world: DMat4,
}

/// 寸法や名前を付けるまとまり (ライン上の設備1台など)
pub struct DrawingItem {
    pub name: String,
    /// ワールドの外接箱
    pub min: DVec3,
    pub max: DVec3,
}

pub struct DrawingInput {
    pub title: String,
    /// モデルの単位 ("mm" など)
    pub unit: String,
    /// モデルの単位 1 が何 mm か
    pub unit_mm: f64,
    pub parts: Vec<DrawingPart>,
    pub items: Vec<DrawingItem>,
    /// 表題欄の日付
    pub date: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewKind {
    Top,
    Front,
}

impl ViewKind {
    pub fn label(self) -> &'static str {
        match self {
            ViewKind::Top => "上面図",
            ViewKind::Front => "正面図",
        }
    }

    /// 見る人の方へ向かう向き
    fn toward_viewer(self) -> DVec3 {
        match self {
            ViewKind::Top => DVec3::Y,
            ViewKind::Front => DVec3::Z,
        }
    }

    /// ワールド → 図の2次元座標。上面図は右が +X・上が -Z、正面図は右が +X・上が +Y
    pub fn project(self, p: DVec3) -> DVec2 {
        match self {
            ViewKind::Top => DVec2::new(p.x, -p.z),
            ViewKind::Front => DVec2::new(p.x, p.y),
        }
    }
}

#[derive(Clone, Copy)]
pub struct DrawingOptions {
    pub top: bool,
    pub front: bool,
    pub hidden_lines: bool,
    pub overall_dims: bool,
    pub chain_dims: bool,
    pub labels: bool,
}

impl Default for DrawingOptions {
    fn default() -> Self {
        Self {
            top: true,
            front: true,
            hidden_lines: true,
            overall_dims: true,
            chain_dims: true,
            labels: true,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layer {
    Outline,
    Hidden,
    Dim,
    Text,
    Frame,
}

impl Layer {
    pub fn dxf_name(self) -> &'static str {
        match self {
            Layer::Outline => "OUTLINE",
            Layer::Hidden => "HIDDEN",
            Layer::Dim => "DIM",
            Layer::Text => "TEXT",
            Layer::Frame => "FRAME",
        }
    }
}

pub struct SheetText {
    pub layer: Layer,
    /// 文字の左下
    pub at: DVec2,
    /// 文字の高さ (実寸)
    pub height: f64,
    /// 反時計回りの角度 (度)。縦の寸法の数字は 90
    pub angle_deg: f64,
    pub text: String,
}

/// 実寸 (モデルの単位) で描いた図面1枚
pub struct Sheet {
    pub lines: Vec<(Layer, DVec2, DVec2)>,
    pub texts: Vec<SheetText>,
    /// 尺度 1 : scale
    pub scale: f64,
    /// 図枠の左下と右上 (実寸)
    pub frame_min: DVec2,
    pub frame_max: DVec2,
    pub unit_mm: f64,
}

impl Sheet {
    /// 実寸 → 用紙の mm
    pub fn to_paper_mm(&self, length: f64) -> f64 {
        length * self.unit_mm / self.scale
    }
}

/// 用紙 (A3 横) の大きさ (mm)
pub const PAPER_MM: [f64; 2] = [420.0, 297.0];
/// 図枠の余白 (mm)
const BORDER_MM: f64 = 10.0;
/// 尺度の候補 (1 : N)
const SCALES: &[f64] = &[
    1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0, 2000.0, 5000.0,
    10000.0,
];
/// 文字の高さ (用紙の mm)
const TEXT_MM: f64 = 3.0;
const SMALL_TEXT_MM: f64 = 2.5;
/// 寸法線どうしの間隔 (用紙の mm)
const DIM_STEP_MM: f64 = 8.0;
/// 間隔寸法の数の上限 (これより多いと読めないので描かない)
const MAX_CHAIN: usize = 24;

/// 図面を描く
pub fn make_sheet(input: &DrawingInput, options: &DrawingOptions) -> Sheet {
    let mut views: Vec<ViewKind> = Vec::new();
    if options.front {
        views.push(ViewKind::Front);
    }
    if options.top || views.is_empty() {
        views.push(ViewKind::Top);
    }

    // ビューごとの範囲 (図の2次元座標)
    let extents: Vec<(DVec2, DVec2)> = views
        .iter()
        .map(|&v| view_extent(input, v).unwrap_or((DVec2::ZERO, DVec2::ONE)))
        .collect();
    let dim_rows = (options.overall_dims as usize + options.chain_dims as usize) as f64;
    let margin_mm = 14.0 + DIM_STEP_MM * dim_rows;
    let width = extents
        .iter()
        .map(|(min, max)| max.x - min.x)
        .fold(0.0, f64::max);
    let height: f64 = extents.iter().map(|(min, max)| max.y - min.y).sum();
    let scale = choose_scale(
        width * input.unit_mm,
        height * input.unit_mm,
        views.len(),
        margin_mm,
    );
    let mm = |paper: f64| paper * scale / input.unit_mm;

    let mut sheet = Sheet {
        lines: Vec::new(),
        texts: Vec::new(),
        scale,
        frame_min: DVec2::ZERO,
        frame_max: DVec2::ZERO,
        unit_mm: input.unit_mm,
    };

    // 正面図を下、上面図をその上に、X をそろえて並べる
    let mut cursor_y = 0.0;
    let left = extents
        .iter()
        .map(|(min, _)| min.x)
        .fold(f64::MAX, f64::min);
    let mut placed = Vec::new();
    for (&view, &(min, max)) in views.iter().zip(&extents) {
        let shift = DVec2::new(-left, cursor_y - min.y);
        placed.push((view, shift, min + shift, max + shift));
        cursor_y += (max.y - min.y) + mm(margin_mm + 6.0);
    }

    let meshes: Vec<TriMesh> = input
        .parts
        .iter()
        .map(|part| {
            TriMesh::new(
                part.faces
                    .iter()
                    .flatten()
                    .map(|t| t.map(|p| part.world.transform_point3(p)))
                    .collect(),
            )
        })
        .collect();
    let scene_size = input
        .items
        .iter()
        .map(|i| (i.max - i.min).length())
        .fold(0.0, f64::max)
        .max(1e-9);

    for &(view, shift, min, max) in &placed {
        draw_view(&mut sheet, input, &meshes, view, shift, options, scene_size);
        drop_covered_hidden_lines(&mut sheet, scene_size);
        let label_h = mm(TEXT_MM);
        sheet.texts.push(SheetText {
            layer: Layer::Text,
            at: DVec2::new(min.x, max.y + mm(3.0)),
            height: label_h,
            angle_deg: 0.0,
            text: view.label().into(),
        });
        annotate_view(&mut sheet, input, view, shift, min, max, options, &mm);
    }

    // 図枠: 図を真ん中に置いた A3 の枠
    let content_min = placed
        .iter()
        .map(|p| p.2)
        .reduce(DVec2::min)
        .unwrap_or_default()
        - DVec2::splat(mm(margin_mm));
    let content_max = placed
        .iter()
        .map(|p| p.3)
        .reduce(DVec2::max)
        .unwrap_or_default()
        + DVec2::new(mm(6.0), mm(10.0));
    let paper = DVec2::new(mm(PAPER_MM[0]), mm(PAPER_MM[1]));
    let title_h = mm(TITLE_HEIGHT_MM);
    let center = (content_min + content_max) / 2.0 - DVec2::new(0.0, title_h / 2.0);
    let center = DVec2::new(center.x, center.y);
    let outer_min = center - paper / 2.0;
    let frame_min = outer_min + DVec2::splat(mm(BORDER_MM));
    let frame_max = outer_min + paper - DVec2::splat(mm(BORDER_MM));
    // 図が表題欄にかからないよう、下にずれていたら枠を下げる
    let drop = (frame_min.y + title_h - content_min.y).max(0.0);
    let (outer_min, frame_min, frame_max) = (
        outer_min - DVec2::new(0.0, drop),
        frame_min - DVec2::new(0.0, drop),
        frame_max - DVec2::new(0.0, drop),
    );
    rect(&mut sheet, Layer::Frame, frame_min, frame_max);
    title_block(&mut sheet, input, frame_min, frame_max, &mm);
    sheet.frame_min = outer_min;
    sheet.frame_max = outer_min + paper;
    sheet
}

/// ビューの範囲 (表示する設備の外接箱を投影したもの)
fn view_extent(input: &DrawingInput, view: ViewKind) -> Option<(DVec2, DVec2)> {
    input
        .items
        .iter()
        .flat_map(|i| box_corners(i.min, i.max))
        .map(|p| view.project(p))
        .fold(None, |acc, p| match acc {
            None => Some((p, p)),
            Some((a, b)) => Some((a.min(p), b.max(p))),
        })
}

fn box_corners(min: DVec3, max: DVec3) -> [DVec3; 8] {
    std::array::from_fn(|i| {
        DVec3::new(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        )
    })
}

/// 図が用紙に収まる、いちばん大きく描ける尺度
pub fn choose_scale(width_mm: f64, height_mm: f64, views: usize, margin_mm: f64) -> f64 {
    let usable_w = PAPER_MM[0] - 2.0 * BORDER_MM - 2.0 * margin_mm;
    let usable_h = PAPER_MM[1]
        - 2.0 * BORDER_MM
        - TITLE_HEIGHT_MM
        - views as f64 * (margin_mm + 6.0)
        - margin_mm;
    let usable_w = usable_w.max(50.0);
    let usable_h = usable_h.max(30.0);
    SCALES
        .iter()
        .copied()
        .find(|&n| width_mm / n <= usable_w && height_mm / n <= usable_h)
        .unwrap_or_else(|| {
            let n = (width_mm / usable_w).max(height_mm / usable_h);
            10f64.powf(n.log10().ceil())
        })
}

/// 稜線とシルエットを投影し、見える部分は外形線、隠れる部分はかくれ線にする
fn draw_view(
    sheet: &mut Sheet,
    input: &DrawingInput,
    meshes: &[TriMesh],
    view: ViewKind,
    shift: DVec2,
    options: &DrawingOptions,
    scene_size: f64,
) {
    let toward = view.toward_viewer();
    let step = scene_size / 150.0;
    let eps = scene_size * 1e-7;
    let hidden = |p: DVec3| {
        let origin = p + toward * eps * 10.0;
        meshes.iter().any(|m| m.ray_blocked(origin, toward, eps))
    };
    for part in &input.parts {
        let mut polylines: Vec<Vec<DVec3>> = sharp_edges(part, toward)
            .map(|e| e.iter().map(|&p| part.world.transform_point3(p)).collect())
            .collect();
        for face in &part.faces {
            polylines.extend(silhouette(face, part.world, toward));
        }
        for line in polylines {
            for pair in line.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let (pa, pb) = (view.project(a) + shift, view.project(b) + shift);
                if pa.distance(pb) < eps {
                    continue;
                }
                // 長い線分は区切って、区間ごとに見える・隠れるを調べる
                let pieces = ((a.distance(b) / step).ceil() as usize).clamp(1, 64);
                let mut run_start = 0usize;
                let mut run_hidden = hidden(a.lerp(b, 0.5 / pieces as f64));
                for k in 1..=pieces {
                    let state = if k < pieces {
                        hidden(a.lerp(b, (k as f64 + 0.5) / pieces as f64))
                    } else {
                        !run_hidden
                    };
                    if state != run_hidden {
                        let t0 = run_start as f64 / pieces as f64;
                        let t1 = k as f64 / pieces as f64;
                        let layer = if run_hidden {
                            Layer::Hidden
                        } else {
                            Layer::Outline
                        };
                        if layer == Layer::Outline || options.hidden_lines {
                            sheet.lines.push((layer, pa.lerp(pb, t0), pa.lerp(pb, t1)));
                        }
                        run_start = k;
                        run_hidden = state;
                    }
                }
            }
        }
    }
}

/// 外形線と重なるかくれ線を消す (箱の底の辺が上面図で上の辺と重なる場合など)
fn drop_covered_hidden_lines(sheet: &mut Sheet, scene_size: f64) {
    use std::collections::HashMap;
    let cell = scene_size / 50.0;
    let eps = scene_size * 1e-6;
    let key = |p: DVec2| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<(DVec2, DVec2)>> = HashMap::new();
    for (layer, a, b) in &sheet.lines {
        if *layer != Layer::Outline {
            continue;
        }
        let (lo, hi) = (key(a.min(*b)), key(a.max(*b)));
        if (hi.0 - lo.0 + 1) * (hi.1 - lo.1 + 1) > 10_000 {
            continue;
        }
        for x in lo.0..=hi.0 {
            for y in lo.1..=hi.1 {
                grid.entry((x, y)).or_default().push((*a, *b));
            }
        }
    }
    let on_outline = |p: DVec2| {
        grid.get(&key(p)).is_some_and(|segments| {
            segments.iter().any(|&(a, b)| {
                let ab = b - a;
                let t = ((p - a).dot(ab) / ab.length_squared().max(1e-300)).clamp(0.0, 1.0);
                (a + ab * t).distance(p) < eps
            })
        })
    };
    sheet.lines.retain(|(layer, a, b)| {
        *layer != Layer::Hidden || ![*a, a.lerp(*b, 0.5), *b].into_iter().all(on_outline)
    });
}

/// 面どうしが滑らかにつながる稜線 (円柱の継ぎ目など) を除いた稜線。
/// 稜線の真ん中の線分を共有する2つの三角形の向きの差で見分ける。
/// 滑らかでも、`toward` から見て輪郭になる (片側が表、片側が裏を向く) 稜線は残す。
fn sharp_edges(part: &DrawingPart, toward: DVec3) -> impl Iterator<Item = &Vec<DVec3>> {
    use std::collections::HashMap;
    const SMOOTH_COS: f64 = 0.94; // 約 20°
    let key = |p: DVec3| (p * 1e6).round().as_i64vec3().to_array();
    let segment = move |a: DVec3, b: DVec3| {
        let (ka, kb) = (key(a), key(b));
        if ka <= kb { (ka, kb) } else { (kb, ka) }
    };
    let mut normals: HashMap<_, Vec<DVec3>> = HashMap::new();
    for t in part.faces.iter().flatten() {
        let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
        for i in 0..3 {
            normals
                .entry(segment(t[i], t[(i + 1) % 3]))
                .or_default()
                .push(n);
        }
    }
    part.edges.iter().filter(move |edge| {
        if edge.len() < 2 {
            return false;
        }
        let mid = (edge.len() - 1) / 2;
        match normals
            .get(&segment(edge[mid], edge[mid + 1]))
            .map(Vec::as_slice)
        {
            Some([n1, n2]) => n1.dot(*n2) < SMOOTH_COS || n1.dot(toward) * n2.dot(toward) <= 0.0,
            _ => true,
        }
    })
}

/// 曲面の輪郭: 面のメッシュの中で、表を向いた三角形と裏を向いた三角形の境の辺
fn silhouette(face: &[[DVec3; 3]], world: DMat4, toward: DVec3) -> Vec<Vec<DVec3>> {
    use std::collections::HashMap;
    let key = |p: DVec3| (p * 1e6).round().as_i64vec3().to_array();
    // 辺 (端点の丸めた座標) → 端点と、その辺を持つ三角形の向き
    type EdgeKey = ([i64; 3], [i64; 3]);
    let mut edges: HashMap<EdgeKey, (DVec3, DVec3, f64)> = HashMap::new();
    let mut out = Vec::new();
    for t in face {
        let w = t.map(|p| world.transform_point3(p));
        let facing = (w[1] - w[0])
            .cross(w[2] - w[0])
            .normalize_or_zero()
            .dot(toward);
        for (a, b) in [(w[0], w[1]), (w[1], w[2]), (w[2], w[0])] {
            let (ka, kb) = (key(a), key(b));
            let k = if ka <= kb { (ka, kb) } else { (kb, ka) };
            match edges.remove(&k) {
                Some((pa, pb, other)) => {
                    if (other > 1e-9 && facing < -1e-9) || (other < -1e-9 && facing > 1e-9) {
                        out.push(vec![pa, pb]);
                    }
                }
                None => {
                    edges.insert(k, (a, b, facing));
                }
            }
        }
    }
    out
}

/// 寸法と名前を付ける
#[allow(clippy::too_many_arguments)]
fn annotate_view(
    sheet: &mut Sheet,
    input: &DrawingInput,
    view: ViewKind,
    shift: DVec2,
    min: DVec2,
    max: DVec2,
    options: &DrawingOptions,
    mm: &dyn Fn(f64) -> f64,
) {
    let text_h = mm(TEXT_MM);
    let item_boxes: Vec<(DVec2, DVec2)> = input
        .items
        .iter()
        .map(|i| {
            let pts = box_corners(i.min, i.max).map(|p| view.project(p) + shift);
            let lo = pts.iter().copied().reduce(DVec2::min).unwrap_or_default();
            let hi = pts.iter().copied().reduce(DVec2::max).unwrap_or_default();
            (lo, hi)
        })
        .collect();

    let mut row = 1.0;
    if options.chain_dims && item_boxes.len() >= 2 {
        let xs = chain_positions(
            item_boxes.iter().flat_map(|(a, b)| [a.x, b.x]),
            max.x - min.x,
        );
        let ys = chain_positions(
            item_boxes.iter().flat_map(|(a, b)| [a.y, b.y]),
            max.y - min.y,
        );
        if xs.len() > 2 && xs.len() <= MAX_CHAIN {
            let y = min.y - mm(DIM_STEP_MM) * row;
            for w in xs.windows(2) {
                dimension_h(sheet, w[0], w[1], min.y, y, text_h, input);
            }
        }
        if ys.len() > 2 && ys.len() <= MAX_CHAIN {
            let x = min.x - mm(DIM_STEP_MM) * row;
            for w in ys.windows(2) {
                dimension_v(sheet, w[0], w[1], min.x, x, text_h, input);
            }
        }
        row += 1.0;
    }
    if options.overall_dims {
        let y = min.y - mm(DIM_STEP_MM) * row;
        dimension_h(sheet, min.x, max.x, min.y, y, text_h, input);
        let x = min.x - mm(DIM_STEP_MM) * row;
        dimension_v(sheet, min.y, max.y, min.x, x, text_h, input);
    }
    if options.labels && view == ViewKind::Top {
        for (item, (lo, hi)) in input.items.iter().zip(&item_boxes) {
            let h = mm(SMALL_TEXT_MM).min((hi.y - lo.y) * 0.4).max(mm(1.5));
            let center = (*lo + *hi) / 2.0;
            let width = text_width(&item.name, h);
            sheet.texts.push(SheetText {
                layer: Layer::Text,
                at: center - DVec2::new(width / 2.0, h / 2.0),
                height: h,
                angle_deg: 0.0,
                text: item.name.clone(),
            });
        }
    }
}

/// 直列寸法を置く位置。近すぎる値 (全体の 0.5% 以内) はまとめる
pub fn chain_positions(values: impl Iterator<Item = f64>, extent: f64) -> Vec<f64> {
    let mut v: Vec<f64> = values.collect();
    v.sort_by(f64::total_cmp);
    let mut out: Vec<f64> = Vec::new();
    for x in v {
        if out.last().is_none_or(|&last| x - last > extent * 0.005) {
            out.push(x);
        }
    }
    out
}

/// 文字列のおおよその幅 (全角は高さと同じ、半角はその 0.6 倍)
pub fn text_width(text: &str, height: f64) -> f64 {
    text.chars()
        .map(|c| if c.is_ascii() { 0.6 } else { 1.0 })
        .sum::<f64>()
        * height
}

/// 長さの表示 (整数なら小数なし、そうでなければ小数1桁)
pub fn format_length(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if (rounded - rounded.round()).abs() < 1e-9 {
        format!("{}", rounded.round() as i64)
    } else {
        format!("{rounded:.1}")
    }
}

/// 水平の寸法。`base_y` は測る形の下端、`y` は寸法線の高さ
fn dimension_h(
    sheet: &mut Sheet,
    x0: f64,
    x1: f64,
    base_y: f64,
    y: f64,
    text_h: f64,
    _input: &DrawingInput,
) {
    let gap = text_h * 0.3;
    sheet.lines.push((
        Layer::Dim,
        DVec2::new(x0, base_y - gap),
        DVec2::new(x0, y - gap),
    ));
    sheet.lines.push((
        Layer::Dim,
        DVec2::new(x1, base_y - gap),
        DVec2::new(x1, y - gap),
    ));
    sheet
        .lines
        .push((Layer::Dim, DVec2::new(x0, y), DVec2::new(x1, y)));
    arrow(sheet, DVec2::new(x0, y), DVec2::X, text_h);
    arrow(sheet, DVec2::new(x1, y), -DVec2::X, text_h);
    let text = format_length(x1 - x0);
    let w = text_width(&text, text_h);
    sheet.texts.push(SheetText {
        layer: Layer::Dim,
        at: DVec2::new((x0 + x1) / 2.0 - w / 2.0, y + gap),
        height: text_h,
        angle_deg: 0.0,
        text,
    });
}

/// 垂直の寸法。`base_x` は測る形の左端、`x` は寸法線の位置
fn dimension_v(
    sheet: &mut Sheet,
    y0: f64,
    y1: f64,
    base_x: f64,
    x: f64,
    text_h: f64,
    _input: &DrawingInput,
) {
    let gap = text_h * 0.3;
    sheet.lines.push((
        Layer::Dim,
        DVec2::new(base_x - gap, y0),
        DVec2::new(x - gap, y0),
    ));
    sheet.lines.push((
        Layer::Dim,
        DVec2::new(base_x - gap, y1),
        DVec2::new(x - gap, y1),
    ));
    sheet
        .lines
        .push((Layer::Dim, DVec2::new(x, y0), DVec2::new(x, y1)));
    arrow(sheet, DVec2::new(x, y0), DVec2::Y, text_h);
    arrow(sheet, DVec2::new(x, y1), -DVec2::Y, text_h);
    let text = format_length(y1 - y0);
    let w = text_width(&text, text_h);
    sheet.texts.push(SheetText {
        layer: Layer::Dim,
        at: DVec2::new(x - gap, (y0 + y1) / 2.0 - w / 2.0),
        height: text_h,
        angle_deg: 90.0,
        text,
    });
}

/// 寸法線の端の矢印 (2本の線で描く)。`dir` は矢じりが開く向き
fn arrow(sheet: &mut Sheet, tip: DVec2, dir: DVec2, text_h: f64) {
    let len = text_h * 0.9;
    let side = DVec2::new(-dir.y, dir.x) * len * 0.3;
    sheet.lines.push((Layer::Dim, tip, tip + dir * len + side));
    sheet.lines.push((Layer::Dim, tip, tip + dir * len - side));
}

fn rect(sheet: &mut Sheet, layer: Layer, min: DVec2, max: DVec2) {
    let c = [min, DVec2::new(max.x, min.y), max, DVec2::new(min.x, max.y)];
    for i in 0..4 {
        sheet.lines.push((layer, c[i], c[(i + 1) % 4]));
    }
}

/// 表題欄の高さ (mm)
const TITLE_HEIGHT_MM: f64 = 24.0;
const TITLE_WIDTH_MM: f64 = 140.0;

/// 右下の表題欄: 名称・尺度・単位・投影法・日付
fn title_block(
    sheet: &mut Sheet,
    input: &DrawingInput,
    _frame_min: DVec2,
    frame_max: DVec2,
    mm: &dyn Fn(f64) -> f64,
) {
    let frame_min = _frame_min;
    let min = DVec2::new(frame_max.x - mm(TITLE_WIDTH_MM), frame_min.y);
    let max = DVec2::new(frame_max.x, frame_min.y + mm(TITLE_HEIGHT_MM));
    rect(sheet, Layer::Frame, min, max);
    let mid_y = min.y + mm(TITLE_HEIGHT_MM / 2.0);
    sheet.lines.push((
        Layer::Frame,
        DVec2::new(min.x, mid_y),
        DVec2::new(max.x, mid_y),
    ));
    let cols = [0.0, 45.0, 80.0, 110.0];
    for c in &cols[1..] {
        let x = min.x + mm(*c);
        sheet
            .lines
            .push((Layer::Frame, DVec2::new(x, min.y), DVec2::new(x, mid_y)));
    }
    let h = mm(TEXT_MM);
    let small = mm(SMALL_TEXT_MM);
    sheet.texts.push(SheetText {
        layer: Layer::Text,
        at: DVec2::new(min.x + mm(3.0), mid_y + mm(4.0)),
        height: mm(5.0),
        angle_deg: 0.0,
        text: input.title.clone(),
    });
    let scale_text = format!("尺度 1:{}", format_length(sheet.scale));
    let cells = [
        scale_text,
        format!("単位 {}", input.unit),
        "第三角法".to_owned(),
        input.date.clone(),
    ];
    for (c, text) in cols.iter().zip(cells) {
        sheet.texts.push(SheetText {
            layer: Layer::Text,
            at: DVec2::new(min.x + mm(*c + 2.5), min.y + mm(4.0)),
            height: if text.len() > 12 { small } else { h },
            angle_deg: 0.0,
            text,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 中心 `c`、大きさ `s` の箱 (面ごとに2枚の三角形)
    fn box_part(c: DVec3, s: DVec3) -> DrawingPart {
        let h = s / 2.0;
        let v = |x: f64, y: f64, z: f64| c + DVec3::new(x * h.x, y * h.y, z * h.z);
        let quads = [
            [
                v(-1., -1., 1.),
                v(1., -1., 1.),
                v(1., 1., 1.),
                v(-1., 1., 1.),
            ],
            [
                v(1., -1., -1.),
                v(-1., -1., -1.),
                v(-1., 1., -1.),
                v(1., 1., -1.),
            ],
            [
                v(-1., 1., 1.),
                v(1., 1., 1.),
                v(1., 1., -1.),
                v(-1., 1., -1.),
            ],
            [
                v(-1., -1., -1.),
                v(1., -1., -1.),
                v(1., -1., 1.),
                v(-1., -1., 1.),
            ],
            [
                v(1., -1., 1.),
                v(1., -1., -1.),
                v(1., 1., -1.),
                v(1., 1., 1.),
            ],
            [
                v(-1., -1., -1.),
                v(-1., -1., 1.),
                v(-1., 1., 1.),
                v(-1., 1., -1.),
            ],
        ];
        let faces = quads
            .iter()
            .map(|q| vec![[q[0], q[1], q[2]], [q[0], q[2], q[3]]])
            .collect();
        let edges = quads
            .iter()
            .flat_map(|q| (0..4).map(move |i| vec![q[i], q[(i + 1) % 4]]))
            .collect();
        DrawingPart {
            faces,
            edges,
            world: DMat4::IDENTITY,
        }
    }

    fn input(parts: Vec<(DVec3, DVec3)>) -> DrawingInput {
        DrawingInput {
            title: "テスト".into(),
            unit: "mm".into(),
            unit_mm: 1.0,
            items: parts
                .iter()
                .enumerate()
                .map(|(i, (c, s))| DrawingItem {
                    name: format!("設備{i}"),
                    min: *c - *s / 2.0,
                    max: *c + *s / 2.0,
                })
                .collect(),
            parts: parts.iter().map(|(c, s)| box_part(*c, *s)).collect(),
            date: "2026-10-31".into(),
        }
    }

    #[test]
    fn scale_fits_a3() {
        // 3 m の設備は A3 に 1:10 では入らず 1:20
        assert_eq!(choose_scale(4000.0, 800.0, 2, 30.0), 20.0);
        assert_eq!(choose_scale(3000.0, 800.0, 2, 30.0), 10.0);
        assert_eq!(choose_scale(100.0, 50.0, 2, 30.0), 1.0);
        assert_eq!(
            chain_positions([0.0, 100.0, 100.2, 300.0].into_iter(), 300.0),
            vec![0.0, 100.0, 300.0]
        );
        assert_eq!(format_length(1200.0), "1200");
        assert_eq!(format_length(12.34), "12.3");
    }

    #[test]
    fn top_view_hides_lower_box() {
        // 大きな箱の下にある箱は、上面図ではかくれ線になる。
        // 大きな箱の底の辺は上の辺と重なるので、かくれ線にしない
        let data = input(vec![
            (DVec3::new(0.0, 100.0, 0.0), DVec3::new(400.0, 200.0, 400.0)),
            (DVec3::new(0.0, -50.0, 0.0), DVec3::new(100.0, 100.0, 100.0)),
        ]);
        let options = DrawingOptions {
            front: false,
            ..Default::default()
        };
        let sheet = make_sheet(&data, &options);
        let hidden = sheet.lines.iter().filter(|l| l.0 == Layer::Hidden).count();
        let outline = sheet.lines.iter().filter(|l| l.0 == Layer::Outline).count();
        assert!(hidden >= 4, "hidden {hidden}");
        assert!(outline >= 4, "outline {outline}");
        // 隠れ線は小さな箱の輪郭 (±50) の上にある
        for (_, a, b) in sheet.lines.iter().filter(|l| l.0 == Layer::Hidden) {
            let w = (*b - *a).abs();
            assert!(w.x <= 100.0 + 1e-6 && w.y <= 100.0 + 1e-6, "{a} {b}");
        }
        assert_eq!(sheet.scale, 5.0);
        // 寸法の数字に全体寸法 400 が入る
        assert!(sheet.texts.iter().any(|t| t.text == "400"));
        assert!(sheet.texts.iter().any(|t| t.text == "上面図"));
    }

    #[test]
    fn front_and_top_are_stacked_and_framed() {
        let data = input(vec![
            (DVec3::new(0.0, 50.0, 0.0), DVec3::new(100.0, 100.0, 100.0)),
            (DVec3::new(300.0, 25.0, 0.0), DVec3::new(100.0, 50.0, 100.0)),
        ]);
        let sheet = make_sheet(&data, &DrawingOptions::default());
        // 直列寸法: 100, 100 (すき間), 100
        let hundreds = sheet.texts.iter().filter(|t| t.text == "100").count();
        assert!(hundreds >= 3, "{hundreds}");
        assert!(sheet.texts.iter().any(|t| t.text == "400"));
        // すべての線が図枠の中にある
        for (_, a, b) in &sheet.lines {
            for p in [a, b] {
                assert!(p.x >= sheet.frame_min.x - 1e-6 && p.x <= sheet.frame_max.x + 1e-6);
                assert!(p.y >= sheet.frame_min.y - 1e-6 && p.y <= sheet.frame_max.y + 1e-6);
            }
        }
        let paper_w = sheet.to_paper_mm(sheet.frame_max.x - sheet.frame_min.x);
        assert!((paper_w - 420.0).abs() < 1e-6);
    }

    #[test]
    fn cylinder_silhouette_is_found() {
        // 縦の円柱を横から見ると、側面の左右の輪郭が出る
        let n = 24;
        let mut tris = Vec::new();
        for i in 0..n {
            let a0 = std::f64::consts::TAU * i as f64 / n as f64;
            let a1 = std::f64::consts::TAU * (i + 1) as f64 / n as f64;
            let p = |a: f64, y: f64| DVec3::new(a.cos(), y, -a.sin());
            tris.push([p(a0, 0.0), p(a1, 0.0), p(a1, 1.0)]);
            tris.push([p(a0, 0.0), p(a1, 1.0), p(a0, 1.0)]);
        }
        let lines = silhouette(&tris, DMat4::IDENTITY, DVec3::Z);
        assert_eq!(lines.len(), 2, "{lines:?}");
        for l in lines {
            assert!((l[0].x.abs() - 1.0).abs() < 0.02);
        }
    }

    #[test]
    fn smooth_seams_are_not_drawn() {
        // 箱の稜線はすべて残る
        let cube = box_part(DVec3::ZERO, DVec3::splat(2.0));
        assert_eq!(sharp_edges(&cube, DVec3::Z).count(), cube.edges.len());
        // 円柱の側面を2つの面に分けた継ぎ目 (奥の -Z 側) は、面が滑らかにつながるので正面からは描かない
        let n = 24;
        let p = |i: usize, y: f64| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            DVec3::new(a.cos(), y, -a.sin())
        };
        let side = |range: std::ops::Range<usize>| {
            range
                .flat_map(|i| {
                    [
                        [p(i, 0.0), p(i + 1, 0.0), p(i + 1, 1.0)],
                        [p(i, 0.0), p(i + 1, 1.0), p(i, 1.0)],
                    ]
                })
                .collect::<Vec<_>>()
        };
        let cylinder = DrawingPart {
            faces: vec![side(0..6), side(6..24)],
            edges: vec![vec![p(6, 0.0), p(6, 1.0)]],
            world: DMat4::IDENTITY,
        };
        assert_eq!(sharp_edges(&cylinder, DVec3::Z).count(), 0);
        // 継ぎ目が真横に来る向きから見ると輪郭なので残す
        assert_eq!(sharp_edges(&cylinder, DVec3::X).count(), 1);
    }
}
