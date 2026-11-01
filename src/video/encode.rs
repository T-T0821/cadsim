//! キャプチャした画像を動画ファイルに書くアダプター (Bevy 非依存)
//!
//! 画面のキャプチャはメインスレッドで、エンコードは別スレッドで行う。
//! どの形式も外部ツール (ffmpeg など) は使わない。

use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::{self, JoinHandle};

use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, RateControlMode, UsageType,
    VuiConfig,
};
use openh264::formats::{RgbaSliceU8, YUVBuffer};

use super::mp4::Mp4Writer;
use cadsim_core::video::{Format, Frame, VideoSink};

/// 書き出しの設定
#[derive(Clone, Debug)]
pub struct EncodeSettings {
    pub format: Format,
    pub fps: u32,
    /// 1 なら等倍、2 なら縦横 1/2
    pub downscale: u32,
    pub path: PathBuf,
}

/// エンコード用のスレッド。フレームを `push` で渡し、最後に `finish` で完了を待つ
pub struct EncoderThread {
    sender: Option<SyncSender<Frame>>,
    handle: JoinHandle<io::Result<PathBuf>>,
}

impl EncoderThread {
    pub fn spawn(settings: EncodeSettings) -> Self {
        // キャプチャが先に進みすぎてメモリを使いすぎないよう、待ち行列は短くする
        let (sender, receiver) = sync_channel(4);
        let handle = thread::spawn(move || encode_all(&settings, receiver));
        Self {
            sender: Some(sender),
            handle,
        }
    }

    /// もうフレームを送らないことを伝える。この後 `is_finished` が true になったら `join` する
    pub fn close(&mut self) {
        self.sender = None;
    }

    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    pub fn join(self) -> io::Result<PathBuf> {
        drop(self.sender);
        self.handle
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("エンコード中にエラーが起きました")))
    }
}

impl VideoSink for EncoderThread {
    /// エンコード側が失敗して止まっていたら Err (原因は `join` で受け取る)
    fn push(&mut self, frame: Frame) -> Result<(), String> {
        let sent = self
            .sender
            .as_ref()
            .is_some_and(|sender| sender.send(frame).is_ok());
        if sent {
            Ok(())
        } else {
            Err("エンコードが止まりました".into())
        }
    }
}

fn encode_all(settings: &EncodeSettings, frames: Receiver<Frame>) -> io::Result<PathBuf> {
    let mut sink: Box<dyn FileSink> = match settings.format {
        Format::Mp4 => Box::new(Mp4Sink::new(&settings.path, settings.fps)),
        Format::Gif => Box::new(GifSink::new(&settings.path, settings.fps)),
        Format::PngSequence => Box::new(PngSink::new(&settings.path)?),
    };
    let mut count = 0;
    for frame in frames {
        sink.push(frame.downscale(settings.downscale))?;
        count += 1;
    }
    if count == 0 {
        return Err(io::Error::other("フレームがありません"));
    }
    sink.finish()?;
    Ok(settings.path.clone())
}

/// 形式ごとの書き込み先 (エンコード用スレッドの中で使う)
trait FileSink {
    fn push(&mut self, frame: Frame) -> io::Result<()>;
    fn finish(self: Box<Self>) -> io::Result<()>;
}

// ---------------------------------------------------------------------------
// MP4 (OpenH264 + 自前の MP4 ライター)
// ---------------------------------------------------------------------------

struct Mp4Sink {
    path: PathBuf,
    fps: u32,
    /// 1 枚目のフレームでサイズが決まってから作る
    state: Option<Mp4State>,
}

struct Mp4State {
    encoder: Encoder,
    writer: Mp4Writer<BufWriter<File>>,
    size: (u32, u32),
}

impl Mp4Sink {
    fn new(path: &Path, fps: u32) -> Self {
        Self {
            path: path.to_owned(),
            fps,
            state: None,
        }
    }
}

/// 解像度とフレームレートに見合うビットレート (CAD 画面は平坦なので控えめでも十分きれい)
fn bitrate_for(width: u32, height: u32, fps: u32) -> u32 {
    let bps = f64::from(width) * f64::from(height) * f64::from(fps) * 0.12;
    bps.clamp(1_000_000.0, 40_000_000.0) as u32
}

impl FileSink for Mp4Sink {
    fn push(&mut self, frame: Frame) -> io::Result<()> {
        let frame = frame.crop_to_even();
        if frame.width < 16 || frame.height < 16 {
            return Err(io::Error::other("画面が小さすぎます"));
        }
        if self.state.is_none() {
            let config = EncoderConfig::new()
                .usage_type(UsageType::ScreenContentRealTime)
                .rate_control_mode(RateControlMode::Bitrate)
                .bitrate(BitRate::from_bps(bitrate_for(
                    frame.width,
                    frame.height,
                    self.fps,
                )))
                .max_frame_rate(FrameRate::from_hz(self.fps as f32))
                // 2 秒ごとにキーフレームを入れて、シークしやすくする
                .intra_frame_period(IntraFramePeriod::from_num_frames(self.fps * 2))
                .skip_frames(false)
                .adaptive_quantization(false)
                .background_detection(false)
                .vui(VuiConfig::bt601());
            let encoder = Encoder::with_api_config(OpenH264API::from_source(), config)
                .map_err(io::Error::other)?;
            let file = BufWriter::new(File::create(&self.path)?);
            let writer = Mp4Writer::new(file, frame.width as u16, frame.height as u16, self.fps)?;
            self.state = Some(Mp4State {
                encoder,
                writer,
                size: (frame.width, frame.height),
            });
        }
        let Some(Mp4State {
            encoder,
            writer,
            size,
        }) = &mut self.state
        else {
            unreachable!()
        };
        if *size != (frame.width, frame.height) {
            return Err(io::Error::other(
                "書き出し中にウィンドウの大きさが変わりました",
            ));
        }
        let dims = (frame.width as usize, frame.height as usize);
        let yuv = YUVBuffer::from_rgba8_source(RgbaSliceU8::new(&frame.rgba, dims));
        let bitstream = encoder.encode(&yuv).map_err(io::Error::other)?;
        writer.write_frame(&bitstream.to_vec())
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        if let Some(state) = self.state {
            state.writer.finish()?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// GIF
// ---------------------------------------------------------------------------

struct GifSink {
    path: PathBuf,
    fps: u32,
    index: u32,
    encoder: Option<gif::Encoder<BufWriter<File>>>,
}

impl GifSink {
    fn new(path: &Path, fps: u32) -> Self {
        Self {
            path: path.to_owned(),
            fps,
            index: 0,
            encoder: None,
        }
    }
}

/// `index` 枚目のフレームの表示時間 (1/100 秒)。
/// 端数を次のフレームに回して、全体の長さが実際の時間とずれないようにする
pub fn gif_delay(index: u32, fps: u32) -> u16 {
    let at = |i: u32| (u64::from(i) * 100 + u64::from(fps) / 2) / u64::from(fps);
    (at(index + 1) - at(index)) as u16
}

impl FileSink for GifSink {
    fn push(&mut self, frame: Frame) -> io::Result<()> {
        let (w, h) = (
            u16::try_from(frame.width).map_err(io::Error::other)?,
            u16::try_from(frame.height).map_err(io::Error::other)?,
        );
        if self.encoder.is_none() {
            let file = BufWriter::new(File::create(&self.path)?);
            let mut encoder = gif::Encoder::new(file, w, h, &[]).map_err(io::Error::other)?;
            encoder
                .set_repeat(gif::Repeat::Infinite)
                .map_err(io::Error::other)?;
            self.encoder = Some(encoder);
        }
        let mut rgba = frame.rgba;
        // speed は 1 (高画質・遅い) 〜 30。10 が画質と速さのバランスがよい
        let mut gif_frame = gif::Frame::from_rgba_speed(w, h, &mut rgba, 10);
        gif_frame.delay = gif_delay(self.index, self.fps);
        self.index += 1;
        let Some(encoder) = &mut self.encoder else {
            unreachable!()
        };
        encoder.write_frame(&gif_frame).map_err(io::Error::other)
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        if let Some(encoder) = self.encoder {
            let mut file = encoder.into_inner().map_err(io::Error::other)?;
            io::Write::flush(&mut file)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PNG 連番
// ---------------------------------------------------------------------------

struct PngSink {
    dir: PathBuf,
    index: u32,
}

impl PngSink {
    fn new(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        Ok(Self {
            dir: dir.to_owned(),
            index: 0,
        })
    }
}

impl FileSink for PngSink {
    fn push(&mut self, frame: Frame) -> io::Result<()> {
        let path = self.dir.join(format!("frame_{:05}.png", self.index));
        self.index += 1;
        image::save_buffer(
            path,
            &frame.rgba,
            frame.width,
            frame.height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(io::Error::other)
    }

    fn finish(self: Box<Self>) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(width: u32, height: u32, shift: u32) -> Frame {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let on = ((x + shift) / 8 + y / 8).is_multiple_of(2);
                let v = if on { 220 } else { 40 };
                rgba.extend([v, v / 2, 255 - v, 255]);
            }
        }
        Frame {
            width,
            height,
            rgba,
        }
    }

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("plantwright-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn encode(format: Format, path: &Path, frames: usize) -> PathBuf {
        let mut enc = EncoderThread::spawn(EncodeSettings {
            format,
            fps: format.default_fps(),
            downscale: 1,
            path: path.to_owned(),
        });
        for i in 0..frames {
            enc.push(checker(66, 50, i as u32 * 2)).unwrap();
        }
        enc.join().unwrap()
    }

    #[test]
    fn gif_delays_add_up_to_real_time() {
        for fps in Format::Gif.fps_choices() {
            let total: u32 = (0..*fps).map(|i| u32::from(gif_delay(i, *fps))).sum();
            assert_eq!(total, 100, "{fps} fps");
        }
    }

    #[test]
    fn encodes_mp4() {
        let path = encode(Format::Mp4, &temp_path("test.mp4"), 12);
        let data = fs::read(&path).unwrap();
        assert_eq!(&data[4..8], b"ftyp");
        // moov の中に 66x50 → 66x50 (偶数) の avc1 がある
        let avc1 = data.windows(4).position(|w| w == b"avc1").unwrap();
        assert!(data[avc1 + 4..].windows(4).any(|w| w == b"avcC"));
        let stsz = data.windows(4).rposition(|w| w == b"stsz").unwrap();
        let count = u32::from_be_bytes(data[stsz + 12..stsz + 16].try_into().unwrap());
        assert_eq!(count, 12);
    }

    #[test]
    fn encodes_gif() {
        let path = encode(Format::Gif, &temp_path("test.gif"), 5);
        let decoder = gif::DecodeOptions::new()
            .read_info(File::open(&path).unwrap())
            .unwrap();
        assert_eq!((decoder.width(), decoder.height()), (66, 50));
        let mut decoder = decoder;
        let mut frames = 0;
        while decoder.read_next_frame().unwrap().is_some() {
            frames += 1;
        }
        assert_eq!(frames, 5);
    }

    #[test]
    fn encodes_png_sequence() {
        let dir = encode(Format::PngSequence, &temp_path("png_seq"), 3);
        for i in 0..3 {
            assert!(dir.join(format!("frame_{i:05}.png")).is_file());
        }
    }
}
