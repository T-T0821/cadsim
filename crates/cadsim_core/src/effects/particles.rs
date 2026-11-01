//! 煙と火花の粒
//!
//! 粒は出す場所 ([`Emitter`]) ごとに 1 秒あたりの数だけ生まれ、寿命が来たら消える。
//! 乱数は種の決まった xorshift なので、同じ時間の進め方なら毎回同じ動きになる (画面の自動確認で写真がぶれない)。
//! 粒の総数は [`EffectSettings::max_particles`] で抑える。

use std::collections::BTreeMap;

use glam::DVec3;

use super::{EffectSettings, Emitter, EmitterKey};

/// 重力 (mm/s²)
const GRAVITY: f64 = 9810.0;
/// 1 回に進める時間の上限 (秒)。止まっていたあとで一度に大量の粒を出さないように
const MAX_STEP: f64 = 0.1;
/// 煙の寿命 (秒)
const SMOKE_LIFE: f64 = 2.6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticleKind {
    Spark,
    Smoke,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub kind: ParticleKind,
    pub pos: DVec3,
    pub vel: DVec3,
    /// 生まれてからの時間 (秒)
    pub age: f64,
    /// 寿命 (秒)
    pub life: f64,
    /// 煙: 生まれたときの半径 (mm)。火花: 尾の長さの倍率
    pub size: f64,
}

impl Particle {
    /// 寿命のうち過ぎた割合 (0〜1)
    pub fn progress(&self) -> f64 {
        (self.age / self.life).clamp(0.0, 1.0)
    }

    /// 消えかけの度合い。1 で生まれたて、0 で消える
    pub fn fade(&self) -> f64 {
        1.0 - self.progress()
    }

    /// 煙の今の半径。上がるにつれて広がる
    pub fn smoke_radius(&self) -> f64 {
        self.size * (1.0 + 2.5 * self.progress())
    }

    /// 火花の尾の端 (速さに比例して伸びる)
    pub fn spark_tail(&self) -> DVec3 {
        self.pos - self.vel * (0.02 * self.size)
    }
}

/// 粒の集まり
#[derive(Clone, Debug)]
pub struct ParticleField {
    particles: Vec<Particle>,
    /// 出す場所ごとの、まだ出していない粒の端数 (煙, 火花)
    debt: BTreeMap<EmitterKey, (f64, f64)>,
    rng: u64,
}

impl Default for ParticleField {
    fn default() -> Self {
        Self {
            particles: Vec::new(),
            debt: BTreeMap::new(),
            rng: 0x2545_F491_4F6C_DD1D,
        }
    }
}

impl ParticleField {
    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    pub fn len(&self) -> usize {
        self.particles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }

    pub fn clear(&mut self) {
        self.particles.clear();
        self.debt.clear();
    }

    /// `dt` 秒進める: 今ある粒を動かして寿命の来たものを消し、出す場所から新しい粒を出す
    pub fn step(&mut self, emitters: &[Emitter], settings: &EffectSettings, dt: f64) {
        if !settings.enabled {
            self.clear();
            return;
        }
        let dt = dt.clamp(0.0, MAX_STEP);
        if dt <= 0.0 {
            return;
        }
        for p in &mut self.particles {
            p.age += dt;
            match p.kind {
                ParticleKind::Spark => {
                    p.vel.y -= GRAVITY * dt;
                    p.pos += p.vel * dt;
                }
                ParticleKind::Smoke => {
                    // 空気抵抗で横の動きが弱まり、上へはゆっくり昇り続ける
                    let drag = (-1.5 * dt).exp();
                    p.vel.x *= drag;
                    p.vel.z *= drag;
                    p.pos += p.vel * dt;
                }
            }
        }
        self.particles
            .retain(|p| p.age < p.life && !(p.kind == ParticleKind::Spark && p.pos.y < -50.0));

        // なくなった場所の端数は捨てる
        self.debt
            .retain(|key, _| emitters.iter().any(|e| e.key == *key));
        for e in emitters {
            let look = e.cue.look(settings);
            let debt = self.debt.entry(e.key).or_default();
            debt.0 += look.smoke_per_sec * dt;
            debt.1 += look.sparks_per_sec * dt;
            let smoke = debt.0.floor();
            let sparks = debt.1.floor();
            debt.0 -= smoke;
            debt.1 -= sparks;
            for _ in 0..smoke as usize {
                if self.particles.len() >= settings.max_particles {
                    break;
                }
                let p = self.smoke(e);
                self.particles.push(p);
            }
            for _ in 0..sparks as usize {
                if self.particles.len() >= settings.max_particles {
                    break;
                }
                let p = self.spark(e);
                self.particles.push(p);
            }
        }
    }

    fn smoke(&mut self, e: &Emitter) -> Particle {
        let s = e.scale();
        let spread = e.radius * 0.3;
        let offset = DVec3::new(self.signed() * spread, 0.0, self.signed() * spread);
        Particle {
            kind: ParticleKind::Smoke,
            pos: e.top + offset,
            vel: DVec3::new(
                self.signed() * 120.0 * s,
                (300.0 + 150.0 * self.unit()) * s,
                self.signed() * 120.0 * s,
            ),
            age: 0.0,
            life: SMOKE_LIFE * (0.8 + 0.4 * self.unit()),
            size: (60.0 + 40.0 * self.unit()) * s,
        }
    }

    fn spark(&mut self, e: &Emitter) -> Particle {
        let s = e.scale().sqrt();
        // 上向きの半球に散らす
        let yaw = self.unit() * std::f64::consts::TAU;
        let up = 0.2 + 0.8 * self.unit();
        let side = (1.0 - up * up).sqrt();
        let dir = DVec3::new(side * yaw.cos(), up, side * yaw.sin());
        let speed = (1200.0 + 1800.0 * self.unit()) * s;
        Particle {
            kind: ParticleKind::Spark,
            pos: e.top,
            vel: dir * speed,
            age: 0.0,
            life: 0.25 + 0.4 * self.unit(),
            size: 0.6 + 0.8 * self.unit(),
        }
    }

    /// 0〜1 の乱数
    fn unit(&mut self) -> f64 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// -1〜1 の乱数
    fn signed(&mut self) -> f64 {
        self.unit() * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::Cue;

    fn emitter(key: u32, cue: Cue) -> Emitter {
        Emitter::at(
            EmitterKey::Element(key),
            cue,
            DVec3::new(0.0, 1000.0, 0.0),
            400.0,
        )
    }

    fn run(field: &mut ParticleField, emitters: &[Emitter], settings: &EffectSettings, secs: f64) {
        for _ in 0..(secs * 60.0) as usize {
            field.step(emitters, settings, 1.0 / 60.0);
        }
    }

    #[test]
    fn welding_throws_sparks_that_fall() {
        let mut field = ParticleField::default();
        let settings = EffectSettings::default();
        let weld = [emitter(1, Cue::Welding)];
        field.step(&weld, &settings, 0.05);
        assert!(!field.is_empty());
        assert!(
            field
                .particles()
                .iter()
                .all(|p| p.kind == ParticleKind::Spark && p.vel.y > 0.0)
        );
        // 出すのをやめると、火花はすぐ消える
        run(&mut field, &[], &settings, 1.0);
        assert!(field.is_empty());
    }

    #[test]
    fn fault_smoke_rises_and_grows() {
        let mut field = ParticleField::default();
        let settings = EffectSettings::default();
        let fault = [emitter(1, Cue::Fault)];
        run(&mut field, &fault, &settings, 1.0);
        let smoke: Vec<_> = field
            .particles()
            .iter()
            .filter(|p| p.kind == ParticleKind::Smoke)
            .collect();
        // 1 秒に 9 粒ほど
        assert!((7..=10).contains(&smoke.len()), "{}", smoke.len());
        let oldest = smoke.iter().max_by(|a, b| a.age.total_cmp(&b.age)).unwrap();
        assert!(oldest.pos.y > 1000.0 + 150.0);
        assert!(oldest.smoke_radius() > oldest.size);
        // 光るだけのきっかけは粒を出さない
        let mut quiet = ParticleField::default();
        run(&mut quiet, &[emitter(2, Cue::Jam)], &settings, 1.0);
        assert!(quiet.is_empty());
    }

    #[test]
    fn particle_count_is_capped() {
        let mut field = ParticleField::default();
        let settings = EffectSettings {
            max_particles: 50,
            ..EffectSettings::default()
        };
        let many: Vec<_> = (0..10).map(|i| emitter(i, Cue::Welding)).collect();
        run(&mut field, &many, &settings, 0.5);
        assert!(field.len() <= 50);
        assert!(field.len() >= 40);
    }

    #[test]
    fn turning_off_clears_everything() {
        let mut field = ParticleField::default();
        let on = EffectSettings::default();
        run(&mut field, &[emitter(1, Cue::Welding)], &on, 0.2);
        assert!(!field.is_empty());
        let off = EffectSettings {
            enabled: false,
            ..on
        };
        field.step(&[emitter(1, Cue::Welding)], &off, 1.0 / 60.0);
        assert!(field.is_empty());
        // 火花だけ切ると、溶接は粒を出さない
        let no_sparks = EffectSettings {
            sparks: false,
            ..on
        };
        run(&mut field, &[emitter(1, Cue::Welding)], &no_sparks, 0.5);
        assert!(field.is_empty());
    }

    #[test]
    fn same_steps_give_same_particles() {
        let settings = EffectSettings::default();
        let emitters = [emitter(1, Cue::Welding), emitter(2, Cue::Fault)];
        let mut a = ParticleField::default();
        let mut b = ParticleField::default();
        run(&mut a, &emitters, &settings, 0.7);
        run(&mut b, &emitters, &settings, 0.7);
        assert_eq!(a.particles(), b.particles());
    }

    #[test]
    fn long_pause_does_not_burst() {
        let mut field = ParticleField::default();
        let settings = EffectSettings::default();
        field.step(&[emitter(1, Cue::Welding)], &settings, 5.0);
        // 0.1 秒分 (140 粒/秒 × 0.1) だけ
        assert!(field.len() <= 14, "{}", field.len());
    }
}
