//! 2枚の画像を見比べる
//!
//! GPU やドライバが違うと、輪郭のなめらかさなどで数段階の色の違いが出る。
//! そこで「色の差が `tolerance` 以下の画素は同じ」とみなし、違う画素の割合が
//! `max_ratio` 以下なら合格にする。比べない領域 (マスク) の画素は数えない。

use super::Rect;

/// RGBA 8bit の画像 (左上から行ごと)
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        assert_eq!(
            rgba.len(),
            (width * height * 4) as usize,
            "画素の数が合わない"
        );
        Self {
            width,
            height,
            rgba,
        }
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// 比べ方
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerance {
    /// R・G・B のどれかの差がこれを超えたら「違う画素」(0〜255)
    pub channel: u8,
    /// 違う画素の割合がこれ以下なら合格 (0.0〜1.0)
    pub max_ratio: f64,
}

impl Default for Tolerance {
    fn default() -> Self {
        Self {
            channel: 24,
            max_ratio: 0.002,
        }
    }
}

/// 見比べた結果
#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    /// 違う画素の数
    pub differing: u64,
    /// 比べた画素の数 (マスクを除く)
    pub compared: u64,
    /// 大きさが違った (このときは画素を比べない)
    pub size_mismatch: Option<((u32, u32), (u32, u32))>,
    pub passed: bool,
    /// 差分画像: 今回の画像を薄い灰色にし、違う画素を赤、マスクを青で塗ったもの
    pub diff: Image,
}

impl Comparison {
    pub fn ratio(&self) -> f64 {
        if self.compared == 0 {
            0.0
        } else {
            self.differing as f64 / self.compared as f64
        }
    }
}

/// 基準画像 `expected` と今回の画像 `actual` を比べる
pub fn compare(expected: &Image, actual: &Image, tol: Tolerance, masks: &[Rect]) -> Comparison {
    if (expected.width, expected.height) != (actual.width, actual.height) {
        return Comparison {
            differing: 0,
            compared: 0,
            size_mismatch: Some((
                (expected.width, expected.height),
                (actual.width, actual.height),
            )),
            passed: false,
            diff: actual.clone(),
        };
    }
    let (w, h) = (actual.width, actual.height);
    let mut diff = Vec::with_capacity(actual.rgba.len());
    let mut differing = 0u64;
    let mut compared = 0u64;
    for y in 0..h {
        for x in 0..w {
            let a = actual.pixel(x, y);
            let masked = masks
                .iter()
                .any(|m| m.contains(x as f32 + 0.5, y as f32 + 0.5));
            if masked {
                diff.extend_from_slice(&[60, 90, 200, 255]);
                continue;
            }
            compared += 1;
            let e = expected.pixel(x, y);
            let off = (0..3).any(|c| a[c].abs_diff(e[c]) > tol.channel);
            if off {
                differing += 1;
                diff.extend_from_slice(&[255, 0, 0, 255]);
            } else {
                // 明るさだけを残して薄くする (どこが違うかを見つけやすく)
                let luma = (a[0] as u32 * 3 + a[1] as u32 * 6 + a[2] as u32) / 10;
                let g = (160 + luma * 95 / 255) as u8;
                diff.extend_from_slice(&[g, g, g, 255]);
            }
        }
    }
    let ratio = if compared == 0 {
        0.0
    } else {
        differing as f64 / compared as f64
    };
    Comparison {
        differing,
        compared,
        size_mismatch: None,
        passed: ratio <= tol.max_ratio,
        diff: Image::new(w, h, diff),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, c: [u8; 4]) -> Image {
        Image::new(
            w,
            h,
            c.iter()
                .copied()
                .cycle()
                .take((w * h * 4) as usize)
                .collect(),
        )
    }

    fn set(img: &mut Image, x: u32, y: u32, c: [u8; 4]) {
        let i = ((y * img.width + x) * 4) as usize;
        img.rgba[i..i + 4].copy_from_slice(&c);
    }

    #[test]
    fn 少しの色の違いは同じとみなす() {
        let a = flat(10, 10, [100, 100, 100, 255]);
        let b = flat(10, 10, [110, 90, 100, 255]);
        let r = compare(&a, &b, Tolerance::default(), &[]);
        assert_eq!(r.differing, 0);
        assert!(r.passed);
    }

    #[test]
    fn 違う画素を数えて差分に赤く塗る() {
        let a = flat(10, 10, [0, 0, 0, 255]);
        let mut b = a.clone();
        set(&mut b, 3, 4, [255, 255, 255, 255]);
        let tol = Tolerance {
            channel: 24,
            max_ratio: 0.0,
        };
        let r = compare(&a, &b, tol, &[]);
        assert_eq!(r.differing, 1);
        assert_eq!(r.compared, 100);
        assert!(!r.passed);
        assert_eq!(r.diff.pixel(3, 4), [255, 0, 0, 255]);
        assert_ne!(r.diff.pixel(0, 0), [255, 0, 0, 255]);

        // 1% まで許せば合格
        let r = compare(
            &a,
            &b,
            Tolerance {
                max_ratio: 0.01,
                ..tol
            },
            &[],
        );
        assert!(r.passed);
    }

    #[test]
    fn マスクの中は比べない() {
        let a = flat(10, 10, [0, 0, 0, 255]);
        let mut b = a.clone();
        set(&mut b, 3, 4, [255, 255, 255, 255]);
        let tol = Tolerance {
            channel: 0,
            max_ratio: 0.0,
        };
        let r = compare(&a, &b, tol, &[Rect::new(2.0, 3.0, 2.0, 2.0)]);
        assert_eq!(r.differing, 0);
        assert_eq!(r.compared, 96);
        assert!(r.passed);
    }

    #[test]
    fn 大きさが違えば不合格() {
        let a = flat(10, 10, [0, 0, 0, 255]);
        let b = flat(10, 8, [0, 0, 0, 255]);
        let r = compare(&a, &b, Tolerance::default(), &[]);
        assert!(!r.passed);
        assert_eq!(r.size_mismatch, Some(((10, 10), (10, 8))));
    }
}
