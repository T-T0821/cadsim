//! 動画書き出しのドメインとユースケース (Bevy 非依存)
//!
//! - どの時刻を何枚撮るか ([`FramePlan`]) を決める
//! - 撮った画像を切り抜いて書き込み先 ([`VideoSink`] ポート) に渡し、進み具合を数える ([`RecordingSession`])
//!
//! 画面のキャプチャ (Bevy) とファイルへのエンコード (アダプター) は外側の層が受け持つ。
//! シミュレーション時刻はフレーム番号だけから決まるので、描画の速さに関係なく毎回同じ動画になる。

pub mod encode;
mod mp4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// H.264 の MP4。PowerPoint や Teams でそのまま再生できる
    Mp4,
    /// アニメーション GIF。256 色だがどこでも表示できる
    Gif,
    /// 1 フレーム 1 枚の PNG をフォルダに並べる
    PngSequence,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Mp4, Format::Gif, Format::PngSequence];

    pub fn label(self) -> &'static str {
        match self {
            Format::Mp4 => "MP4 (H.264)",
            Format::Gif => "GIF",
            Format::PngSequence => "PNG 連番",
        }
    }

    /// 出力ファイルの拡張子。PNG 連番はフォルダなので None
    pub fn extension(self) -> Option<&'static str> {
        match self {
            Format::Mp4 => Some("mp4"),
            Format::Gif => Some("gif"),
            Format::PngSequence => None,
        }
    }

    /// 選べるフレームレート。GIF は 1/100 秒単位でしか待ち時間を持てず、
    /// 速すぎると再生ソフトが遅くするので低めに抑える
    pub fn fps_choices(self) -> &'static [u32] {
        match self {
            Format::Gif => &[10, 15, 20, 25],
            Format::Mp4 | Format::PngSequence => &[24, 30, 60],
        }
    }

    pub fn default_fps(self) -> u32 {
        match self {
            Format::Gif => 15,
            Format::Mp4 | Format::PngSequence => 30,
        }
    }
}

/// RGBA8 の画像 1 枚
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    /// 縦横を 1/`factor` に縮める (factor × factor 画素の平均)
    pub fn downscale(self, factor: u32) -> Frame {
        if factor <= 1 {
            return self;
        }
        let (w, h) = ((self.width / factor).max(1), (self.height / factor).max(1));
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        let n = factor * factor;
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0u32; 4];
                for dy in 0..factor {
                    let row = ((y * factor + dy) * self.width) as usize;
                    for dx in 0..factor {
                        let i = (row + (x * factor + dx) as usize) * 4;
                        for (c, s) in sum.iter_mut().enumerate() {
                            *s += u32::from(self.rgba[i + c]);
                        }
                    }
                }
                rgba.extend(sum.map(|s| ((s + n / 2) / n) as u8));
            }
        }
        Frame {
            width: w,
            height: h,
            rgba,
        }
    }

    /// 右端・下端を切って縦横を偶数にする (H.264 の 4:2:0 は偶数サイズが必要)
    pub fn crop_to_even(self) -> Frame {
        let (w, h) = (self.width & !1, self.height & !1);
        if (w, h) == (self.width, self.height) {
            return self;
        }
        self.crop(0, 0, w, h)
    }

    /// (x, y) から幅 w・高さ h を切り抜く。画像からはみ出す分は詰める
    pub fn crop(self, x: u32, y: u32, w: u32, h: u32) -> Frame {
        let x = x.min(self.width);
        let y = y.min(self.height);
        let w = w.min(self.width - x);
        let h = h.min(self.height - y);
        if (x, y, w, h) == (0, 0, self.width, self.height) {
            return self;
        }
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for row in y..y + h {
            let start = ((row * self.width + x) * 4) as usize;
            rgba.extend_from_slice(&self.rgba[start..start + (w * 4) as usize]);
        }
        Frame {
            width: w,
            height: h,
            rgba,
        }
    }
}

/// どの時刻を何枚撮るか
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FramePlan {
    pub fps: u32,
    speed: f64,
    cycle_time: f64,
    pub frames: u32,
}

impl FramePlan {
    /// `cycle_time` 秒のサイクルを `cycles` 回分、`speed` 倍速・`fps` で撮る
    pub fn new(cycle_time: f64, cycles: u32, fps: u32, speed: f64) -> Self {
        let video_seconds = cycle_time * f64::from(cycles) / speed;
        Self {
            fps,
            speed,
            cycle_time,
            // 最後のフレームはサイクルの終わり (= 次のサイクルの頭) の手前まで。
            // 繰り返し再生したときに同じ絵が 2 枚続かないようにする
            frames: (video_seconds * f64::from(fps) - 1e-6).ceil().max(1.0) as u32,
        }
    }

    /// `index` 枚目のフレームを撮るシミュレーション時刻
    pub fn time(&self, index: u32) -> f64 {
        let t = f64::from(index) * self.speed / f64::from(self.fps);
        if self.cycle_time > 0.0 {
            t % self.cycle_time
        } else {
            0.0
        }
    }

    pub fn video_seconds(&self) -> f64 {
        f64::from(self.frames) / f64::from(self.fps)
    }

    /// 動画を少なくとも `seconds` 秒にする。カメラの通り道の終わりまで撮るのに使う。
    /// 最後のフレームがちょうど `seconds` 秒目になるように 1 枚足す
    pub fn extend_to(mut self, seconds: f64) -> Self {
        if seconds.is_finite() && seconds > 0.0 {
            let needed = (seconds * f64::from(self.fps) - 1e-6).ceil() as u32 + 1;
            self.frames = self.frames.max(needed);
        }
        self
    }

    /// `index` 枚目のフレームの動画の頭からの秒数 (カメラの通り道の時刻)
    pub fn video_time(&self, index: u32) -> f64 {
        f64::from(index) / f64::from(self.fps)
    }
}

/// 画像 1 枚の中の切り抜き範囲 (物理ピクセル)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// 撮ったフレームの書き込み先 (ポート)。実装はアダプター層のエンコーダ
pub trait VideoSink {
    fn push(&mut self, frame: Frame) -> Result<(), String>;
}

/// 1 回の書き出し。外側の層は次のように使う:
/// `current_time` の時刻に再生位置を合わせて描画 → キャプチャした画像を `submit` → 終わるまで繰り返す
pub struct RecordingSession<S> {
    plan: FramePlan,
    crop: Option<CropRect>,
    next: u32,
    sink: S,
}

impl<S: VideoSink> RecordingSession<S> {
    /// `crop` が None ならキャプチャした画像全体を使う
    pub fn new(plan: FramePlan, crop: Option<CropRect>, sink: S) -> Self {
        Self {
            plan,
            crop,
            next: 0,
            sink,
        }
    }

    /// 次に撮るフレームのシミュレーション時刻。撮り終えたら None
    pub fn current_time(&self) -> Option<f64> {
        (self.next < self.plan.frames).then(|| self.plan.time(self.next))
    }

    /// 次に撮るフレームの動画の頭からの秒数。撮り終えたら None
    pub fn current_video_time(&self) -> Option<f64> {
        (self.next < self.plan.frames).then(|| self.plan.video_time(self.next))
    }

    /// `current_time` の時刻で撮った画像を渡す
    pub fn submit(&mut self, frame: Frame) -> Result<(), String> {
        let frame = match self.crop {
            Some(c) => frame.crop(c.x, c.y, c.width, c.height),
            None => frame,
        };
        self.sink.push(frame)?;
        self.next += 1;
        Ok(())
    }

    /// (撮り終えた枚数, 全体の枚数)
    pub fn progress(&self) -> (u32, u32) {
        (self.next, self.plan.frames)
    }

    pub fn into_sink(self) -> S {
        self.sink
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32) -> Frame {
        Frame {
            width,
            height,
            rgba: vec![128; (width * height * 4) as usize],
        }
    }

    /// 受け取ったフレームの大きさを覚えるだけの書き込み先
    #[derive(Default)]
    struct Collect(Vec<(u32, u32)>);

    impl VideoSink for Collect {
        fn push(&mut self, frame: Frame) -> Result<(), String> {
            self.0.push((frame.width, frame.height));
            Ok(())
        }
    }

    #[test]
    fn plan_samples_one_cycle_without_repeating_the_first_frame() {
        let plan = FramePlan::new(4.8, 1, 30, 1.0);
        assert_eq!(plan.frames, 144);
        assert_eq!(plan.time(0), 0.0);
        assert!((plan.time(143) - 143.0 / 30.0).abs() < 1e-9);
        assert!(plan.time(143) < 4.8);
    }

    #[test]
    fn plan_wraps_over_cycles_and_applies_speed() {
        // 2 サイクルを 0.5 倍速: 2 秒のサイクルが 8 秒の動画になる
        let plan = FramePlan::new(2.0, 2, 10, 0.5);
        assert_eq!(plan.frames, 80);
        assert!((plan.time(10) - 0.5).abs() < 1e-9);
        assert!((plan.time(45) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn session_walks_through_the_plan_and_crops() {
        let plan = FramePlan::new(0.3, 1, 10, 1.0);
        let crop = CropRect {
            x: 2,
            y: 1,
            width: 4,
            height: 2,
        };
        let mut session = RecordingSession::new(plan, Some(crop), Collect::default());
        let mut times = Vec::new();
        while let Some(t) = session.current_time() {
            times.push(t);
            session.submit(solid(8, 6)).unwrap();
        }
        assert_eq!(times.len(), 3);
        assert!((times[2] - 0.2).abs() < 1e-9);
        assert_eq!(session.progress(), (3, 3));
        assert_eq!(session.into_sink().0, vec![(4, 2); 3]);
    }

    #[test]
    fn extend_to_covers_the_camera_path() {
        // 動きのないモデルでも、カメラの通り道の長さだけ撮る
        let plan = FramePlan::new(0.0, 1, 30, 1.0).extend_to(2.0);
        assert_eq!(plan.frames, 61);
        assert_eq!(plan.video_time(60), 2.0);
        assert_eq!(plan.time(60), 0.0);
        // チャートのほうが長ければそのまま
        let long = FramePlan::new(10.0, 1, 30, 1.0);
        assert_eq!(long.extend_to(2.0).frames, long.frames);
    }

    #[test]
    fn downscale_averages_blocks() {
        let frame = Frame {
            width: 2,
            height: 2,
            rgba: vec![
                0, 0, 0, 255, 100, 0, 0, 255, 0, 200, 0, 255, 100, 200, 4, 255,
            ],
        };
        let half = frame.downscale(2);
        assert_eq!((half.width, half.height), (1, 1));
        assert_eq!(half.rgba, vec![50, 100, 1, 255]);
    }

    #[test]
    fn crop_to_even_drops_last_row_and_column() {
        let frame = solid(5, 3).crop_to_even();
        assert_eq!((frame.width, frame.height), (4, 2));
        assert_eq!(frame.rgba.len(), 4 * 2 * 4);
    }

    #[test]
    fn crop_takes_the_requested_area() {
        let frame = Frame {
            width: 3,
            height: 2,
            rgba: (0..24).collect(),
        };
        let part = frame.crop(1, 1, 5, 5);
        assert_eq!((part.width, part.height), (2, 1));
        assert_eq!(part.rgba, (16..24).collect::<Vec<u8>>());
    }
}
