//! お祝いの紙吹雪の軌道

/// 紙吹雪の1片。位置は表示領域の幅・高さを 1 とした割合。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConfettiPiece {
    pub x0: f32,
    pub vx: f32,
    pub vy: f32,
    pub spin: f32,
    pub color: usize,
}

/// 紙吹雪の重力 (表示領域の高さ / 秒²)
const CONFETTI_GRAVITY: f32 = 1.4;

impl ConfettiPiece {
    /// 打ち上げてから `t` 秒後の位置 (x, y) と回転角
    pub fn at(&self, t: f32) -> (f32, f32, f32) {
        let x = self.x0 + self.vx * t;
        let y = self.vy * t + 0.5 * CONFETTI_GRAVITY * t * t;
        (x, y, self.spin * t)
    }
}

/// 上端から降らせる紙吹雪を `count` 片作る。同じ `seed` なら同じ並び。
pub fn confetti(seed: u64, count: usize, colors: usize) -> Vec<ConfettiPiece> {
    let mut state = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut next = || {
        // xorshift64*
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        (state.wrapping_mul(0x2545F4914F6CDD1D) >> 40) as f32 / (1u64 << 24) as f32
    };
    (0..count)
        .map(|_| ConfettiPiece {
            x0: next(),
            vx: (next() - 0.5) * 0.3,
            vy: -0.2 - next() * 0.4,
            spin: (next() - 0.5) * 12.0,
            color: (next() * colors.max(1) as f32) as usize % colors.max(1),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confetti_falls_and_is_deterministic() {
        let a = confetti(7, 40, 5);
        assert_eq!(a, confetti(7, 40, 5));
        assert_eq!(a.len(), 40);
        for piece in &a {
            assert!((0.0..1.0).contains(&piece.x0));
            assert!(piece.color < 5);
            // 最初は上に飛び、やがて下に落ちる
            assert!(piece.at(0.05).1 < 0.0);
            assert!(piece.at(3.0).1 > 1.0);
        }
    }
}
