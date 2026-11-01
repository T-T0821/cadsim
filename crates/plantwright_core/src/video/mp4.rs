//! H.264 の映像だけを入れる最小限の MP4 ライター
//!
//! エンコーダが出す Annex-B 形式 (スタートコード区切り) の NAL を受け取り、
//! SPS / PPS は `avcC` に、残りは長さ前置き形式のサンプルとして `mdat` に書く。
//! `moov` は最後にまとめて書く (再生ソフトはファイル全体を読めるので問題ない)。

use std::io::{self, Seek, SeekFrom, Write};

const NAL_IDR: u8 = 5;
const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;
/// AUD とフィラーは MP4 には入れない
const NAL_AUD: u8 = 9;
const NAL_FILLER: u8 = 12;

/// `moov` の時間単位 (ミリ秒)
const MOVIE_TIMESCALE: u32 = 1000;

pub struct Mp4Writer<W: Write + Seek> {
    out: W,
    width: u16,
    height: u16,
    /// 映像トラックの時間単位 (1 秒あたりの刻み数)。1 フレーム = 1 刻み
    fps: u32,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    /// `mdat` ヘッダの位置 (サイズを最後に書き戻す)
    mdat_start: u64,
    /// ファイル中の現在位置
    pos: u64,
    samples: Vec<Sample>,
}

struct Sample {
    offset: u64,
    size: u32,
    /// このサンプルの表示時間 (フレーム数)
    duration: u32,
    keyframe: bool,
}

impl<W: Write + Seek> Mp4Writer<W> {
    pub fn new(mut out: W, width: u16, height: u16, fps: u32) -> io::Result<Self> {
        let mut head = Vec::new();
        write_box(&mut head, b"ftyp", |b| {
            b.extend_from_slice(b"isom");
            put_u32(b, 0x200);
            for brand in [b"isom", b"iso2", b"avc1", b"mp41"] {
                b.extend_from_slice(brand);
            }
        });
        let mdat_start = head.len() as u64;
        // 4GB を超えてもよいように 64bit サイズ形式 (size = 1 + largesize) で書く
        put_u32(&mut head, 1);
        head.extend_from_slice(b"mdat");
        put_u64(&mut head, 0);
        out.write_all(&head)?;
        Ok(Self {
            out,
            width,
            height,
            fps,
            sps: None,
            pps: None,
            mdat_start,
            pos: head.len() as u64,
            samples: Vec::new(),
        })
    }

    /// 1 フレーム分の Annex-B ビットストリームを書く。
    /// 中身が空 (エンコーダがフレームを捨てた) ときは、直前のフレームを 1 フレーム長く表示する。
    pub fn write_frame(&mut self, annex_b: &[u8]) -> io::Result<()> {
        let mut data = Vec::with_capacity(annex_b.len() + 16);
        let mut keyframe = false;
        for nal in split_annex_b(annex_b) {
            match nal[0] & 0x1f {
                NAL_SPS => {
                    self.sps.get_or_insert_with(|| nal.to_vec());
                }
                NAL_PPS => {
                    self.pps.get_or_insert_with(|| nal.to_vec());
                }
                NAL_AUD | NAL_FILLER => {}
                kind => {
                    keyframe |= kind == NAL_IDR;
                    put_u32(&mut data, nal.len() as u32);
                    data.extend_from_slice(nal);
                }
            }
        }
        if data.is_empty() {
            if let Some(last) = self.samples.last_mut() {
                last.duration += 1;
            }
            return Ok(());
        }
        self.out.write_all(&data)?;
        self.samples.push(Sample {
            offset: self.pos,
            size: data.len() as u32,
            duration: 1,
            keyframe,
        });
        self.pos += data.len() as u64;
        Ok(())
    }

    /// `mdat` のサイズを確定し、`moov` を書いて出力先を返す。
    pub fn finish(mut self) -> io::Result<W> {
        let (Some(sps), Some(pps)) = (&self.sps, &self.pps) else {
            return Err(io::Error::other("H.264 の SPS / PPS がありません"));
        };
        if sps.len() < 4 {
            return Err(io::Error::other("H.264 の SPS が短すぎます"));
        }
        let moov = self.moov(sps, pps);
        self.out.write_all(&moov)?;
        self.out.seek(SeekFrom::Start(self.mdat_start + 8))?;
        self.out
            .write_all(&(self.pos - self.mdat_start).to_be_bytes())?;
        self.out.seek(SeekFrom::End(0))?;
        self.out.flush()?;
        Ok(self.out)
    }

    fn moov(&self, sps: &[u8], pps: &[u8]) -> Vec<u8> {
        let frames: u64 = self.samples.iter().map(|s| u64::from(s.duration)).sum();
        let media_duration = frames as u32;
        let movie_duration = (frames * u64::from(MOVIE_TIMESCALE) / u64::from(self.fps)) as u32;
        let (w, h) = (self.width, self.height);

        let mut moov = Vec::new();
        write_box(&mut moov, b"moov", |b| {
            write_full_box(b, b"mvhd", 0, 0, |b| {
                put_u32(b, 0); // creation_time
                put_u32(b, 0); // modification_time
                put_u32(b, MOVIE_TIMESCALE);
                put_u32(b, movie_duration);
                put_u32(b, 0x0001_0000); // rate 1.0
                put_u16(b, 0x0100); // volume 1.0
                b.extend_from_slice(&[0; 10]);
                put_matrix(b);
                b.extend_from_slice(&[0; 24]); // pre_defined
                put_u32(b, 2); // next_track_ID
            });
            write_box(b, b"trak", |b| {
                // flags: track_enabled | track_in_movie
                write_full_box(b, b"tkhd", 0, 3, |b| {
                    put_u32(b, 0);
                    put_u32(b, 0);
                    put_u32(b, 1); // track_ID
                    put_u32(b, 0);
                    put_u32(b, movie_duration);
                    b.extend_from_slice(&[0; 8]);
                    put_u16(b, 0); // layer
                    put_u16(b, 0); // alternate_group
                    put_u16(b, 0); // volume (映像なので 0)
                    put_u16(b, 0);
                    put_matrix(b);
                    put_u32(b, u32::from(w) << 16);
                    put_u32(b, u32::from(h) << 16);
                });
                write_box(b, b"mdia", |b| {
                    write_full_box(b, b"mdhd", 0, 0, |b| {
                        put_u32(b, 0);
                        put_u32(b, 0);
                        put_u32(b, self.fps);
                        put_u32(b, media_duration);
                        put_u16(b, 0x55c4); // language "und"
                        put_u16(b, 0);
                    });
                    write_full_box(b, b"hdlr", 0, 0, |b| {
                        put_u32(b, 0);
                        b.extend_from_slice(b"vide");
                        b.extend_from_slice(&[0; 12]);
                        b.extend_from_slice(b"VideoHandler\0");
                    });
                    write_box(b, b"minf", |b| {
                        write_full_box(b, b"vmhd", 0, 1, |b| b.extend_from_slice(&[0; 8]));
                        write_box(b, b"dinf", |b| {
                            write_full_box(b, b"dref", 0, 0, |b| {
                                put_u32(b, 1);
                                // flags 1: データは同じファイル内
                                write_full_box(b, b"url ", 0, 1, |_| {});
                            });
                        });
                        write_box(b, b"stbl", |b| self.stbl(b, sps, pps));
                    });
                });
            });
        });
        moov
    }

    fn stbl(&self, b: &mut Vec<u8>, sps: &[u8], pps: &[u8]) {
        write_full_box(b, b"stsd", 0, 0, |b| {
            put_u32(b, 1);
            write_box(b, b"avc1", |b| {
                b.extend_from_slice(&[0; 6]);
                put_u16(b, 1); // data_reference_index
                b.extend_from_slice(&[0; 16]);
                put_u16(b, self.width);
                put_u16(b, self.height);
                put_u32(b, 0x0048_0000); // 72 dpi
                put_u32(b, 0x0048_0000);
                put_u32(b, 0);
                put_u16(b, 1); // frame_count
                b.extend_from_slice(&[0; 32]); // compressorname
                put_u16(b, 0x0018); // depth
                put_u16(b, 0xffff); // pre_defined = -1
                write_box(b, b"avcC", |b| {
                    b.push(1);
                    b.extend_from_slice(&sps[1..4]); // profile, compatibility, level
                    b.push(0xff); // NAL 長は 4 バイト
                    b.push(0xe1); // SPS 1 個
                    put_u16(b, sps.len() as u16);
                    b.extend_from_slice(sps);
                    b.push(1); // PPS 1 個
                    put_u16(b, pps.len() as u16);
                    b.extend_from_slice(pps);
                });
            });
        });

        // 表示時間: 同じ長さが続くところはまとめる
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for s in &self.samples {
            match runs.last_mut() {
                Some((count, duration)) if *duration == s.duration => *count += 1,
                _ => runs.push((1, s.duration)),
            }
        }
        write_full_box(b, b"stts", 0, 0, |b| {
            put_u32(b, runs.len() as u32);
            for (count, duration) in &runs {
                put_u32(b, *count);
                put_u32(b, *duration);
            }
        });

        let keyframes: Vec<u32> = (1..)
            .zip(&self.samples)
            .filter(|(_, s)| s.keyframe)
            .map(|(n, _)| n)
            .collect();
        write_full_box(b, b"stss", 0, 0, |b| {
            put_u32(b, keyframes.len() as u32);
            for n in &keyframes {
                put_u32(b, *n);
            }
        });

        // 1 チャンク = 1 サンプルにして、各サンプルの位置をそのまま並べる
        write_full_box(b, b"stsc", 0, 0, |b| {
            put_u32(b, 1);
            put_u32(b, 1); // first_chunk
            put_u32(b, 1); // samples_per_chunk
            put_u32(b, 1); // sample_description_index
        });
        write_full_box(b, b"stsz", 0, 0, |b| {
            put_u32(b, 0);
            put_u32(b, self.samples.len() as u32);
            for s in &self.samples {
                put_u32(b, s.size);
            }
        });
        if self.pos <= u64::from(u32::MAX) {
            write_full_box(b, b"stco", 0, 0, |b| {
                put_u32(b, self.samples.len() as u32);
                for s in &self.samples {
                    put_u32(b, s.offset as u32);
                }
            });
        } else {
            write_full_box(b, b"co64", 0, 0, |b| {
                put_u32(b, self.samples.len() as u32);
                for s in &self.samples {
                    put_u64(b, s.offset);
                }
            });
        }
    }
}

/// Annex-B のバイト列をスタートコード (00 00 01 / 00 00 00 01) で区切り、NAL ごとに返す
pub fn split_annex_b(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::with_capacity(starts.len());
    for (k, &(_, body)) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map_or(data.len(), |&(code, _)| code);
        // 4 バイトのスタートコードの先頭の 0 は前の NAL に含めない
        while end > body && data[end - 1] == 0 {
            end -= 1;
        }
        if end > body {
            nals.push(&data[body..end]);
        }
    }
    nals
}

fn write_box(out: &mut Vec<u8>, kind: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let start = out.len();
    put_u32(out, 0);
    out.extend_from_slice(kind);
    body(out);
    let size = (out.len() - start) as u32;
    out[start..start + 4].copy_from_slice(&size.to_be_bytes());
}

fn write_full_box(
    out: &mut Vec<u8>,
    kind: &[u8; 4],
    version: u8,
    flags: u32,
    body: impl FnOnce(&mut Vec<u8>),
) {
    write_box(out, kind, |b| {
        put_u32(b, (u32::from(version) << 24) | (flags & 0x00ff_ffff));
        body(b);
    });
}

/// 単位行列 (回転・拡大なし)
fn put_matrix(b: &mut Vec<u8>) {
    for v in [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        put_u32(b, v);
    }
}

fn put_u16(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_be_bytes());
}

fn put_u32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_be_bytes());
}

fn put_u64(b: &mut Vec<u8>, v: u64) {
    b.extend_from_slice(&v.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// テスト用: ボックスの並び (種類と中身) を読む
    fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
        let mut out = Vec::new();
        let mut i = 0;
        while i + 8 <= data.len() {
            let mut size = u32::from_be_bytes(data[i..i + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = data[i + 4..i + 8].try_into().unwrap();
            let mut header = 8;
            if size == 1 {
                size = u64::from_be_bytes(data[i + 8..i + 16].try_into().unwrap()) as usize;
                header = 16;
            }
            out.push((kind, &data[i + header..i + size]));
            i += size;
        }
        out
    }

    fn find<'a>(data: &'a [u8], path: &[&[u8; 4]]) -> &'a [u8] {
        let mut cur = data;
        for kind in path {
            cur = boxes(cur)
                .into_iter()
                .find(|(k, _)| k == *kind)
                .unwrap_or_else(|| panic!("{} がない", String::from_utf8_lossy(*kind)))
                .1;
        }
        cur
    }

    #[test]
    fn split_handles_3_and_4_byte_start_codes() {
        let data = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4, 5,
        ];
        let nals = split_annex_b(&data);
        assert_eq!(nals, vec![&[0x67, 1, 2][..], &[0x68, 3], &[0x65, 4, 5]]);
    }

    #[test]
    fn writes_samples_and_parameter_sets() {
        let mut w = Mp4Writer::new(Cursor::new(Vec::new()), 64, 48, 30).unwrap();
        // 1 枚目: SPS + PPS + IDR
        w.write_frame(&[
            0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1e, 0xaa, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65,
            0x88, 0x84,
        ])
        .unwrap();
        // 2 枚目: P フレーム、3 枚目: 空 (2 枚目を 2 フレーム表示)
        w.write_frame(&[0, 0, 0, 1, 0x41, 0x9a]).unwrap();
        w.write_frame(&[]).unwrap();
        let data = w.finish().unwrap().into_inner();

        let top: Vec<[u8; 4]> = boxes(&data).iter().map(|(k, _)| *k).collect();
        assert_eq!(top, [*b"ftyp", *b"mdat", *b"moov"]);

        // mdat には長さ前置きの IDR と P だけが入る
        let mdat = find(&data, &[b"mdat"]);
        assert_eq!(mdat, [0, 0, 0, 3, 0x65, 0x88, 0x84, 0, 0, 0, 2, 0x41, 0x9a]);

        let stbl = find(&data, &[b"moov", b"trak", b"mdia", b"minf", b"stbl"]);
        // avcC: profile 0x42 / level 0x1e と SPS・PPS
        let stsd = find(stbl, &[b"stsd"]);
        let avc1 = &boxes(&stsd[8..])[0];
        assert_eq!(&avc1.0, b"avc1");
        let avcc = find(&avc1.1[78..], &[b"avcC"]);
        assert_eq!(&avcc[..6], &[1, 0x42, 0xc0, 0x1e, 0xff, 0xe1]);
        assert_eq!(&avcc[6..8], &[0, 5]);
        assert_eq!(&avcc[8..13], &[0x67, 0x42, 0xc0, 0x1e, 0xaa]);

        // stts: 1 フレームが 1 つ、2 フレームが 1 つ
        assert_eq!(
            find(stbl, &[b"stts"]),
            [
                0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 2
            ]
        );
        // stss: 1 枚目だけがキーフレーム
        assert_eq!(find(stbl, &[b"stss"]), [0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1]);
        // stco: サンプルの位置が mdat の中を指している
        let stco = find(stbl, &[b"stco"]);
        let first = u32::from_be_bytes(stco[8..12].try_into().unwrap()) as usize;
        assert_eq!(&data[first..first + 5], &[0, 0, 0, 3, 0x65]);

        // mdhd: 30fps で 3 フレーム、mvhd: 100ms
        let mdhd = find(&data, &[b"moov", b"trak", b"mdia", b"mdhd"]);
        assert_eq!(&mdhd[12..20], &[0, 0, 0, 30, 0, 0, 0, 3]);
        let mvhd = find(&data, &[b"moov", b"mvhd"]);
        assert_eq!(&mvhd[16..20], &100u32.to_be_bytes());
    }
}
