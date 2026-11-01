//! 紹介動画のカメラワーク (Bevy 非依存)
//!
//! 動画の時刻ごとにカメラの視点 ([`CameraPose`]) を置いた「通り道」([`CameraPath`]) を持ち、
//! その間をなめらかにつないだ視点を返す。動画の書き出しはフレームごとにこの視点へカメラを合わせる。
//!
//! 視点は 3D ビューのカメラと同じ「注視点・向き (ヨー / ピッチ)・距離」で表す。
//! 距離は対数でつなぐので、遠くから寄るときも速さが急に変わらない。
//! ヨーは近い向きから回る (359° → 1° で一周しない)。

pub mod record;

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use glam::{EulerRot, Quat, Vec3};

/// 視点を足すとき、前の視点から何秒あとに置くか
pub const DEFAULT_GAP: f64 = 3.0;

/// カメラの視点。3D ビューの注視点カメラと同じ表し方
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPose {
    pub focus: Vec3,
    /// 鉛直軸まわりの向き (ラジアン)。0 で +Z 側から -Z 方向を見る
    pub yaw: f32,
    /// 見下ろす角度 (ラジアン)。正で上から見下ろす
    pub pitch: f32,
    /// 注視点からカメラまでの距離
    pub distance: f32,
}

impl CameraPose {
    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, -self.pitch, 0.0)
    }

    /// カメラの位置
    pub fn eye(&self) -> Vec3 {
        self.focus + self.rotation() * Vec3::new(0.0, 0.0, self.distance)
    }
}

/// 通り道の上の 1 点: 動画の `time` 秒目にこの視点になる
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraKey {
    /// 動画の頭からの秒数
    pub time: f64,
    pub pose: CameraPose,
}

/// 視点と視点のつなぎ方
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Interpolation {
    /// 曲線でなめらかにつなぎ、最初と最後はゆっくり動き出して止まる
    #[default]
    Smooth,
    /// 一定の速さでまっすぐつなぐ
    Linear,
}

impl Interpolation {
    pub const ALL: [Interpolation; 2] = [Interpolation::Smooth, Interpolation::Linear];

    pub fn label(self) -> &'static str {
        match self {
            Interpolation::Smooth => "なめらか",
            Interpolation::Linear => "一定の速さ",
        }
    }

    /// 保存用の名前
    pub fn key(self) -> &'static str {
        match self {
            Interpolation::Smooth => "smooth",
            Interpolation::Linear => "linear",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|i| i.key() == key)
    }
}

/// カメラの通り道。視点は時刻の順に並べて持つ
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CameraPath {
    keys: Vec<CameraKey>,
    pub interpolation: Interpolation,
}

impl CameraPath {
    /// 視点の並びから作る (時刻の順に並べ直し、負の時刻は 0 にする)
    pub fn from_keys(keys: Vec<CameraKey>, interpolation: Interpolation) -> Self {
        let mut path = Self {
            keys,
            interpolation,
        };
        for key in &mut path.keys {
            key.time = sanitize_time(key.time);
        }
        path.sort();
        path
    }

    pub fn keys(&self) -> &[CameraKey] {
        &self.keys
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// 動かせる (視点が 2 つ以上あり、長さがある) か
    pub fn is_playable(&self) -> bool {
        self.keys.len() >= 2 && self.duration() > 0.0
    }

    /// 最後の視点の時刻 (動画の頭から通り道の終わりまでの秒数)
    pub fn duration(&self) -> f64 {
        self.keys.last().map_or(0.0, |k| k.time)
    }

    /// 最後の視点の `DEFAULT_GAP` 秒あとに視点を足す。足した視点の番号を返す
    pub fn push(&mut self, pose: CameraPose) -> usize {
        let time = match self.keys.last() {
            Some(last) => last.time + DEFAULT_GAP,
            None => 0.0,
        };
        self.keys.push(CameraKey { time, pose });
        self.keys.len() - 1
    }

    /// `index` 番目の視点を今の視点で置き換える (時刻はそのまま)
    pub fn set_pose(&mut self, index: usize, pose: CameraPose) {
        if let Some(key) = self.keys.get_mut(index) {
            key.pose = pose;
        }
    }

    /// `index` 番目の視点の時刻を変える。並べ直したあとの番号を返す
    pub fn set_time(&mut self, index: usize, time: f64) -> usize {
        let Some(key) = self.keys.get_mut(index) else {
            return index;
        };
        key.time = sanitize_time(time);
        let moved = *key;
        self.sort();
        self.keys.iter().position(|k| *k == moved).unwrap_or(index)
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.keys.len() {
            self.keys.remove(index);
        }
    }

    pub fn clear(&mut self) {
        self.keys.clear();
    }

    /// 全体の長さを `seconds` 秒にそろえる (視点の時刻の比はそのまま)
    pub fn scale_to(&mut self, seconds: f64) {
        let duration = self.duration();
        let seconds = sanitize_time(seconds);
        if duration <= 0.0 || seconds <= 0.0 {
            return;
        }
        let factor = seconds / duration;
        for key in &mut self.keys {
            key.time *= factor;
        }
    }

    /// 動画の `time` 秒目の視点。視点がなければ None。
    /// 最初の視点より前は最初の視点、最後の視点よりあとは最後の視点のまま止まる。
    pub fn sample(&self, time: f64) -> Option<CameraPose> {
        let first = self.keys.first()?;
        let last = self.keys.last()?;
        if self.keys.len() == 1 || time <= first.time {
            return Some(first.pose);
        }
        if time >= last.time {
            return Some(last.pose);
        }
        // time を挟む区間 [i, i + 1]
        let i = self.keys.partition_point(|k| k.time <= time) - 1;
        let (a, b) = (&self.keys[i], &self.keys[i + 1]);
        let span = b.time - a.time;
        if span <= 0.0 {
            return Some(b.pose);
        }
        let s = (time - a.time) / span;
        let values = self.unwrapped_values();
        let v = match self.interpolation {
            Interpolation::Linear => lerp(&values[i], &values[i + 1], s),
            Interpolation::Smooth => {
                let (ma, mb) = (self.tangent(&values, i), self.tangent(&values, i + 1));
                hermite(&values[i], &values[i + 1], &ma, &mb, s, span)
            }
        };
        Some(from_values(&v))
    }

    /// 書き出す前に通り道の目安を描く点 (カメラの位置) を `count` 個返す
    pub fn eye_polyline(&self, count: usize) -> Vec<Vec3> {
        if self.keys.is_empty() {
            return Vec::new();
        }
        let count = count.max(2);
        let (start, end) = (self.keys[0].time, self.duration());
        (0..count)
            .filter_map(|i| {
                let t = start + (end - start) * i as f64 / (count - 1) as f64;
                self.sample(t).map(|p| p.eye())
            })
            .collect()
    }

    /// 注視点のまわりをぐるっと一周する通り道 (`seconds` 秒)。
    /// 今の視点から始め、90° ずつ視点を置いて元の向きに戻る。
    pub fn turntable(start: CameraPose, seconds: f64) -> Self {
        let seconds = sanitize_time(seconds).max(1.0);
        let keys = (0..=4)
            .map(|i| CameraKey {
                time: seconds * f64::from(i) / 4.0,
                pose: CameraPose {
                    yaw: start.yaw + FRAC_PI_2 * i as f32,
                    ..start
                },
            })
            .collect();
        Self {
            keys,
            interpolation: Interpolation::Smooth,
        }
    }

    /// 全体を見渡す視点から、`target` に寄って少し回り込む通り道 (`seconds` 秒)
    pub fn fly_in(overview: CameraPose, target: CameraPose, seconds: f64) -> Self {
        let seconds = sanitize_time(seconds).max(1.0);
        let keys = vec![
            CameraKey {
                time: 0.0,
                pose: overview,
            },
            CameraKey {
                time: seconds * 0.6,
                pose: target,
            },
            CameraKey {
                time: seconds,
                pose: CameraPose {
                    yaw: target.yaw + 0.5,
                    ..target
                },
            },
        ];
        Self {
            keys,
            interpolation: Interpolation::Smooth,
        }
    }

    fn sort(&mut self) {
        self.keys.sort_by(|a, b| a.time.total_cmp(&b.time));
    }

    /// 各視点を補間用の数値 (注視点 xyz, ヨー, ピッチ, 距離の対数) にする。
    /// ヨーは前の視点から近い向きへ回るようにつなげる
    fn unwrapped_values(&self) -> Vec<[f64; 6]> {
        let mut out: Vec<[f64; 6]> = Vec::with_capacity(self.keys.len());
        let mut yaw = 0.0f64;
        for (i, key) in self.keys.iter().enumerate() {
            let raw = f64::from(key.pose.yaw);
            yaw = if i == 0 {
                raw
            } else {
                let prev = f64::from(self.keys[i - 1].pose.yaw);
                yaw + wrap_angle(raw - prev)
            };
            let p = &key.pose;
            out.push([
                f64::from(p.focus.x),
                f64::from(p.focus.y),
                f64::from(p.focus.z),
                yaw,
                f64::from(p.pitch),
                f64::from(p.distance.max(1e-6)).ln(),
            ]);
        }
        out
    }

    /// `i` 番目の視点での変化の速さ (1 秒あたり)。
    /// 端は 0 にして、ゆっくり動き出して止まるようにする。
    fn tangent(&self, values: &[[f64; 6]], i: usize) -> [f64; 6] {
        if i == 0 || i + 1 >= values.len() {
            return [0.0; 6];
        }
        let dt = self.keys[i + 1].time - self.keys[i - 1].time;
        if dt <= 0.0 {
            return [0.0; 6];
        }
        let mut m = [0.0; 6];
        for (c, m) in m.iter_mut().enumerate() {
            *m = (values[i + 1][c] - values[i - 1][c]) / dt;
        }
        m
    }
}

fn sanitize_time(time: f64) -> f64 {
    if time.is_finite() { time.max(0.0) } else { 0.0 }
}

/// 角度を (-π, π] に収める
fn wrap_angle(angle: f64) -> f64 {
    let pi = f64::from(PI);
    let tau = f64::from(TAU);
    let a = (angle + pi).rem_euclid(tau) - pi;
    if a <= -pi { a + tau } else { a }
}

fn lerp(a: &[f64; 6], b: &[f64; 6], s: f64) -> [f64; 6] {
    std::array::from_fn(|c| a[c] + (b[c] - a[c]) * s)
}

/// 3 次エルミート補間。`ma` / `mb` は 1 秒あたりの変化、`span` は区間の秒数
fn hermite(
    a: &[f64; 6],
    b: &[f64; 6],
    ma: &[f64; 6],
    mb: &[f64; 6],
    s: f64,
    span: f64,
) -> [f64; 6] {
    let (s2, s3) = (s * s, s * s * s);
    let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
    let h10 = s3 - 2.0 * s2 + s;
    let h01 = -2.0 * s3 + 3.0 * s2;
    let h11 = s3 - s2;
    std::array::from_fn(|c| h00 * a[c] + h10 * span * ma[c] + h01 * b[c] + h11 * span * mb[c])
}

fn from_values(v: &[f64; 6]) -> CameraPose {
    CameraPose {
        focus: Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32),
        yaw: v[3] as f32,
        pitch: (v[4] as f32).clamp(-FRAC_PI_2, FRAC_PI_2),
        distance: v[5].exp() as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(x: f32, yaw: f32, distance: f32) -> CameraPose {
        CameraPose {
            focus: Vec3::new(x, 0.0, 0.0),
            yaw,
            pitch: 0.3,
            distance,
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn empty_path_has_no_pose() {
        let path = CameraPath::default();
        assert!(path.sample(0.0).is_none());
        assert!(!path.is_playable());
        assert_eq!(path.duration(), 0.0);
    }

    #[test]
    fn push_spaces_keys_by_the_default_gap() {
        let mut path = CameraPath::default();
        path.push(pose(0.0, 0.0, 1.0));
        path.push(pose(1.0, 0.0, 1.0));
        path.push(pose(2.0, 0.0, 1.0));
        let times: Vec<f64> = path.keys().iter().map(|k| k.time).collect();
        assert_eq!(times, vec![0.0, DEFAULT_GAP, 2.0 * DEFAULT_GAP]);
        assert!(path.is_playable());
    }

    #[test]
    fn sample_hits_keys_and_holds_outside() {
        let mut path = CameraPath::default();
        path.push(pose(0.0, 0.0, 1.0));
        path.push(pose(3.0, 0.0, 1.0));
        path.push(pose(9.0, 0.0, 1.0));
        for interpolation in Interpolation::ALL {
            path.interpolation = interpolation;
            for key in path.keys() {
                let p = path.sample(key.time).unwrap();
                assert!(close(p.focus.x, key.pose.focus.x), "{interpolation:?}");
            }
            assert!(close(path.sample(-1.0).unwrap().focus.x, 0.0));
            assert!(close(path.sample(100.0).unwrap().focus.x, 9.0));
        }
    }

    #[test]
    fn linear_moves_at_constant_speed() {
        let mut path = CameraPath::from_keys(
            vec![
                CameraKey {
                    time: 0.0,
                    pose: pose(0.0, 0.0, 1.0),
                },
                CameraKey {
                    time: 2.0,
                    pose: pose(4.0, 0.0, 1.0),
                },
            ],
            Interpolation::Linear,
        );
        assert!(close(path.sample(0.5).unwrap().focus.x, 1.0));
        assert!(close(path.sample(1.0).unwrap().focus.x, 2.0));
        // なめらかなつなぎは端でゆっくり動く
        path.interpolation = Interpolation::Smooth;
        assert!(path.sample(0.5).unwrap().focus.x < 1.0);
        assert!(close(path.sample(1.0).unwrap().focus.x, 2.0));
    }

    #[test]
    fn smooth_passes_through_middle_keys_without_stopping() {
        let path = CameraPath::from_keys(
            (0..3)
                .map(|i| CameraKey {
                    time: f64::from(i),
                    pose: pose(i as f32, 0.0, 1.0),
                })
                .collect(),
            Interpolation::Smooth,
        );
        // 真ん中の視点の前後で動き続けている (止まらない)
        let before = path.sample(0.99).unwrap().focus.x;
        let after = path.sample(1.01).unwrap().focus.x;
        assert!(after - before > 0.015);
    }

    #[test]
    fn distance_is_interpolated_in_log_space() {
        let path = CameraPath::from_keys(
            vec![
                CameraKey {
                    time: 0.0,
                    pose: pose(0.0, 0.0, 1.0),
                },
                CameraKey {
                    time: 1.0,
                    pose: pose(0.0, 0.0, 100.0),
                },
            ],
            Interpolation::Linear,
        );
        assert!(close(path.sample(0.5).unwrap().distance, 10.0));
    }

    #[test]
    fn yaw_turns_the_short_way() {
        let path = CameraPath::from_keys(
            vec![
                CameraKey {
                    time: 0.0,
                    pose: pose(0.0, 3.0, 1.0),
                },
                CameraKey {
                    time: 1.0,
                    pose: pose(0.0, -3.0, 1.0),
                },
            ],
            Interpolation::Linear,
        );
        // 3 → -3 は 6 ラジアン戻るのではなく、π をまたいで 0.28 ラジアン進む
        let mid = path.sample(0.5).unwrap().yaw;
        assert!(close(wrap_angle(f64::from(mid)) as f32, PI), "{mid}");
    }

    #[test]
    fn turntable_goes_all_the_way_around() {
        let start = pose(1.0, 0.2, 5.0);
        let path = CameraPath::turntable(start, 8.0);
        assert_eq!(path.len(), 5);
        assert_eq!(path.duration(), 8.0);
        let mid = path.sample(4.0).unwrap();
        assert!(close(mid.yaw, 0.2 + PI));
        let end = path.sample(8.0).unwrap();
        assert!(close(end.yaw, 0.2 + TAU));
        // 注視点と距離は変わらない
        assert!(close(mid.distance, 5.0));
        assert!(close(mid.focus.x, 1.0));
    }

    #[test]
    fn set_time_keeps_keys_sorted() {
        let mut path = CameraPath::default();
        path.push(pose(0.0, 0.0, 1.0));
        path.push(pose(1.0, 0.0, 1.0));
        path.push(pose(2.0, 0.0, 1.0));
        let moved = path.set_time(0, 10.0);
        assert_eq!(moved, 2);
        assert!(close(path.keys()[2].pose.focus.x, 0.0));
        assert_eq!(path.duration(), 10.0);
        assert_eq!(path.set_time(1, -5.0), 0);
        assert_eq!(path.keys()[0].time, 0.0);
    }

    #[test]
    fn scale_to_stretches_times() {
        let mut path = CameraPath::default();
        path.push(pose(0.0, 0.0, 1.0));
        path.push(pose(1.0, 0.0, 1.0));
        path.push(pose(2.0, 0.0, 1.0));
        path.scale_to(12.0);
        let times: Vec<f64> = path.keys().iter().map(|k| k.time).collect();
        assert_eq!(times, vec![0.0, 6.0, 12.0]);
    }

    #[test]
    fn eye_matches_orbit_camera() {
        // ヨー 0・ピッチ 0 なら +Z 側から見る
        let p = CameraPose {
            focus: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            distance: 2.0,
        };
        assert!(p.eye().distance(Vec3::new(0.0, 0.0, 2.0)) < 1e-5);
        // 見下ろすとカメラは上にある
        let above = CameraPose { pitch: 0.5, ..p };
        assert!(above.eye().y > 0.0);
    }

    #[test]
    fn eye_polyline_samples_the_whole_path() {
        let path = CameraPath::turntable(pose(0.0, 0.0, 1.0), 4.0);
        let line = path.eye_polyline(9);
        assert_eq!(line.len(), 9);
        assert!(line[0].distance(*line.last().unwrap()) < 1e-4);
    }

    #[test]
    fn interpolation_keys_round_trip() {
        for i in Interpolation::ALL {
            assert_eq!(Interpolation::from_key(i.key()), Some(i));
        }
        assert_eq!(Interpolation::from_key("bogus"), None);
    }
}
