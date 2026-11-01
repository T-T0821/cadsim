//! SLMP (MC プロトコル) の電文の組み立てと読み取り
//!
//! 3E / 4E フレーム、バイナリ / ASCII コードに対応する。コマンドは一括読出し (0401) と一括書込み (1401) の
//! ビット単位・ワード単位 (Q/L シリーズ互換のサブコマンド 0000 / 0001。iQ-R も受け付ける)。
//! 要求と応答の両方を組み立て・読み取りできるので、テスト用の PLC (`super::mock`) も同じコードを使う。
//!
//! 電文の形 (3E・バイナリ、D100 から 3 ワード読む例):
//!
//! ```text
//! 50 00 | 00 | FF | FF 03 | 00 | 0C 00 | 10 00 | 01 04 | 00 00 | 64 00 00 | A8 | 03 00
//! サブ    ネット PC   要求先I/O 局番 データ長 監視タイマ コマンド サブコマンド 先頭番号    コード 点数
//! ```
//!
//! ASCII では同じ並びを 16 進数の文字で書く (数値は上位から)。4E はサブヘッダの後ろにシリアル番号と 0000 が入る。

use std::fmt;

use crate::plc::melsec::Device;

/// フレームの種類
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FrameKind {
    /// 3E フレーム (QnA 互換)
    #[default]
    E3,
    /// 4E フレーム (シリアル番号付き。要求と応答を番号で突き合わせられる)
    E4,
}

/// 電文のコード
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Encoding {
    #[default]
    Binary,
    Ascii,
}

/// 要求の宛先 (ネットワーク番号・PC 番号・要求先ユニット I/O 番号・要求先局番)。
/// 初期値は「自局 (直接つないだ CPU)」。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Route {
    pub network: u8,
    pub pc: u8,
    pub module_io: u16,
    pub station: u8,
}

impl Default for Route {
    fn default() -> Self {
        Self {
            network: 0x00,
            pc: 0xFF,
            module_io: 0x03FF,
            station: 0x00,
        }
    }
}

/// コマンド
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Command {
    /// ビット単位の一括読出し
    ReadBits {
        device: Device,
        start: u32,
        count: u16,
    },
    /// ワード単位の一括読出し
    ReadWords {
        device: Device,
        start: u32,
        count: u16,
    },
    /// ビット単位の一括書込み
    WriteBits {
        device: Device,
        start: u32,
        values: Vec<bool>,
    },
    /// ワード単位の一括書込み
    WriteWords {
        device: Device,
        start: u32,
        values: Vec<u16>,
    },
}

impl Command {
    fn code(&self) -> (u16, u16) {
        match self {
            Command::ReadBits { .. } => (0x0401, 0x0001),
            Command::ReadWords { .. } => (0x0401, 0x0000),
            Command::WriteBits { .. } => (0x1401, 0x0001),
            Command::WriteWords { .. } => (0x1401, 0x0000),
        }
    }
}

/// 応答の中身
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Reply {
    Bits(Vec<bool>),
    Words(Vec<u16>),
    /// 書込みの正常応答 (データなし)
    Written,
}

/// 1つの電文 (要求または応答) の、コマンドの手前までの共通部分
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Header {
    pub frame: FrameKind,
    pub encoding: Encoding,
    /// 4E のシリアル番号 (3E では 0)
    pub serial: u16,
    pub route: Route,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FrameError {
    /// PLC が異常の終了コードを返した
    EndCode(u16),
    /// 電文の形がおかしい
    Malformed(String),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::EndCode(code) => {
                write!(f, "PLC が異常を返しました (終了コード {code:04X}H")?;
                match code {
                    0xC059 => write!(f, ": コマンドを受け付けません)"),
                    0xC056 => write!(f, ": デバイスの範囲外です)"),
                    0xC051..=0xC054 => write!(f, ": 点数が多すぎます)"),
                    0xC05C => write!(f, ": 要求の内容が誤っています)"),
                    0xC061 => write!(f, ": データ長が合いません)"),
                    _ => write!(f, ")"),
                }
            }
            FrameError::Malformed(why) => write!(f, "電文を読めません: {why}"),
        }
    }
}

impl std::error::Error for FrameError {}

fn malformed(why: impl Into<String>) -> FrameError {
    FrameError::Malformed(why.into())
}

/// SLMP のデバイスコード (バイナリ 1 バイト、ASCII 2 文字)
pub fn device_code(device: Device) -> (u8, &'static str) {
    match device {
        Device::X => (0x9C, "X*"),
        Device::Y => (0x9D, "Y*"),
        Device::M => (0x90, "M*"),
        Device::L => (0x92, "L*"),
        Device::B => (0xA0, "B*"),
        Device::D => (0xA8, "D*"),
        Device::W => (0xB4, "W*"),
        Device::R => (0xAF, "R*"),
    }
}

fn device_from_binary(code: u8) -> Option<Device> {
    Device::ALL.into_iter().find(|&d| device_code(d).0 == code)
}

fn device_from_ascii(code: &str) -> Option<Device> {
    Device::ALL.into_iter().find(|&d| device_code(d).1 == code)
}

const REQUEST_SUBHEADER: [u16; 2] = [0x5000, 0x5400];
const RESPONSE_SUBHEADER: [u16; 2] = [0xD000, 0xD400];

fn subheader(frame: FrameKind, response: bool) -> u16 {
    let table = if response {
        RESPONSE_SUBHEADER
    } else {
        REQUEST_SUBHEADER
    };
    match frame {
        FrameKind::E3 => table[0],
        FrameKind::E4 => table[1],
    }
}

/// 電文の先頭から「データ長」の欄までの長さ (バイト。ASCII では文字数)。要求・応答とも同じ。
/// TCP ではまずこれだけ読み、[`data_length`] で残りの長さを知る。
pub fn header_len(frame: FrameKind, encoding: Encoding) -> usize {
    let binary = match frame {
        FrameKind::E3 => 9,
        FrameKind::E4 => 13,
    };
    match encoding {
        Encoding::Binary => binary,
        Encoding::Ascii => binary * 2,
    }
}

/// [`header_len`] だけ読んだ先頭から、残りの長さ (データ長の欄の値) を読む
pub fn data_length(head: &[u8], encoding: Encoding) -> Result<usize, FrameError> {
    let mut r = Reader::new(encoding, head);
    let len_at = head.len() - field_width(encoding, 2);
    r.pos = len_at;
    Ok(r.u16()? as usize)
}

fn field_width(encoding: Encoding, bytes: usize) -> usize {
    match encoding {
        Encoding::Binary => bytes,
        Encoding::Ascii => bytes * 2,
    }
}

/// 電文を書く (バイナリはリトルエンディアン、ASCII は上位からの 16 進数)
struct Writer {
    encoding: Encoding,
    buf: Vec<u8>,
}

impl Writer {
    fn new(encoding: Encoding) -> Self {
        Self {
            encoding,
            buf: Vec::new(),
        }
    }

    fn u8(&mut self, v: u8) {
        match self.encoding {
            Encoding::Binary => self.buf.push(v),
            Encoding::Ascii => self.buf.extend(format!("{v:02X}").bytes()),
        }
    }

    fn u16(&mut self, v: u16) {
        match self.encoding {
            Encoding::Binary => self.buf.extend(v.to_le_bytes()),
            Encoding::Ascii => self.buf.extend(format!("{v:04X}").bytes()),
        }
    }

    /// サブヘッダは バイナリでも上位から書く (50 00)
    fn subheader(&mut self, v: u16) {
        match self.encoding {
            Encoding::Binary => self.buf.extend(v.to_be_bytes()),
            Encoding::Ascii => self.buf.extend(format!("{v:04X}").bytes()),
        }
    }

    fn device(&mut self, device: Device, number: u32) {
        let (binary, ascii) = device_code(device);
        match self.encoding {
            Encoding::Binary => {
                self.buf.extend(&number.to_le_bytes()[..3]);
                self.buf.push(binary);
            }
            Encoding::Ascii => {
                self.buf.extend(ascii.bytes());
                let number = if device.is_hex() {
                    format!("{number:06X}")
                } else {
                    format!("{number:06}")
                };
                self.buf.extend(number.bytes());
            }
        }
    }

    fn bits(&mut self, values: &[bool]) {
        match self.encoding {
            // 1 点 4 ビット、先の点が上位
            Encoding::Binary => {
                for pair in values.chunks(2) {
                    let high = if pair[0] { 0x10 } else { 0 };
                    let low = if pair.get(1) == Some(&true) { 0x01 } else { 0 };
                    self.buf.push(high | low);
                }
            }
            Encoding::Ascii => self
                .buf
                .extend(values.iter().map(|&on| if on { b'1' } else { b'0' })),
        }
    }

    fn words(&mut self, values: &[u16]) {
        for &v in values {
            self.u16(v);
        }
    }
}

/// 電文を読む
struct Reader<'a> {
    encoding: Encoding,
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(encoding: Encoding, data: &'a [u8]) -> Self {
        Self {
            encoding,
            data,
            pos: 0,
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], FrameError> {
        let end = self.pos + n;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| malformed("電文が途中で切れています"))?;
        self.pos = end;
        Ok(slice)
    }

    fn hex(&mut self, chars: usize) -> Result<u32, FrameError> {
        let text = std::str::from_utf8(self.take(chars)?).map_err(|_| malformed("文字が不正"))?;
        u32::from_str_radix(text, 16)
            .map_err(|_| malformed(format!("16 進数ではありません: {text}")))
    }

    fn u8(&mut self) -> Result<u8, FrameError> {
        match self.encoding {
            Encoding::Binary => Ok(self.take(1)?[0]),
            Encoding::Ascii => Ok(self.hex(2)? as u8),
        }
    }

    fn u16(&mut self) -> Result<u16, FrameError> {
        match self.encoding {
            Encoding::Binary => {
                let b = self.take(2)?;
                Ok(u16::from_le_bytes([b[0], b[1]]))
            }
            Encoding::Ascii => Ok(self.hex(4)? as u16),
        }
    }

    fn subheader(&mut self) -> Result<u16, FrameError> {
        match self.encoding {
            Encoding::Binary => {
                let b = self.take(2)?;
                Ok(u16::from_be_bytes([b[0], b[1]]))
            }
            Encoding::Ascii => Ok(self.hex(4)? as u16),
        }
    }

    fn device(&mut self) -> Result<(Device, u32), FrameError> {
        match self.encoding {
            Encoding::Binary => {
                let b = self.take(4)?;
                let number = u32::from_le_bytes([b[0], b[1], b[2], 0]);
                let device = device_from_binary(b[3])
                    .ok_or_else(|| malformed(format!("知らないデバイスコード {:02X}", b[3])))?;
                Ok((device, number))
            }
            Encoding::Ascii => {
                let code =
                    std::str::from_utf8(self.take(2)?).map_err(|_| malformed("文字が不正"))?;
                let device = device_from_ascii(code)
                    .ok_or_else(|| malformed(format!("知らないデバイスコード {code}")))?;
                let digits =
                    std::str::from_utf8(self.take(6)?).map_err(|_| malformed("文字が不正"))?;
                let radix = if device.is_hex() { 16 } else { 10 };
                let number = u32::from_str_radix(digits, radix)
                    .map_err(|_| malformed(format!("デバイス番号を読めません: {digits}")))?;
                Ok((device, number))
            }
        }
    }

    fn bits(&mut self, count: usize) -> Result<Vec<bool>, FrameError> {
        match self.encoding {
            Encoding::Binary => {
                let bytes = self.take(count.div_ceil(2))?;
                Ok((0..count)
                    .map(|i| {
                        let b = bytes[i / 2];
                        if i % 2 == 0 {
                            b & 0x10 != 0
                        } else {
                            b & 0x01 != 0
                        }
                    })
                    .collect())
            }
            Encoding::Ascii => self
                .take(count)?
                .iter()
                .map(|c| match c {
                    b'0' => Ok(false),
                    b'1' => Ok(true),
                    _ => Err(malformed("ビットの値が 0 / 1 ではありません")),
                })
                .collect(),
        }
    }

    fn words(&mut self, count: usize) -> Result<Vec<u16>, FrameError> {
        (0..count).map(|_| self.u16()).collect()
    }

    fn rest(&self) -> usize {
        self.data.len() - self.pos
    }
}

/// 共通部分とデータ長の欄を書き、`body` (監視タイマ / 終了コード以降) を続ける
fn assemble(header: &Header, response: bool, body: Writer) -> Vec<u8> {
    let mut w = Writer::new(header.encoding);
    w.subheader(subheader(header.frame, response));
    if header.frame == FrameKind::E4 {
        w.u16(header.serial);
        w.u16(0);
    }
    w.u8(header.route.network);
    w.u8(header.route.pc);
    w.u16(header.route.module_io);
    w.u8(header.route.station);
    w.u16(body.buf.len() as u16);
    w.buf.extend(body.buf);
    w.buf
}

/// 共通部分を読み、データ長の欄のあとの位置にした Reader を返す
fn read_header<'a>(
    frame: FrameKind,
    encoding: Encoding,
    response: bool,
    data: &'a [u8],
) -> Result<(Header, Reader<'a>), FrameError> {
    let mut r = Reader::new(encoding, data);
    let sub = r.subheader()?;
    if sub != subheader(frame, response) {
        return Err(malformed(format!("サブヘッダが違います ({sub:04X})")));
    }
    let serial = if frame == FrameKind::E4 {
        let serial = r.u16()?;
        r.u16()?;
        serial
    } else {
        0
    };
    let route = Route {
        network: r.u8()?,
        pc: r.u8()?,
        module_io: r.u16()?,
        station: r.u8()?,
    };
    let len = r.u16()? as usize;
    if r.rest() != len {
        return Err(malformed(format!(
            "データ長 {len} と実際の長さ {} が違います",
            r.rest()
        )));
    }
    let header = Header {
        frame,
        encoding,
        serial,
        route,
    };
    Ok((header, r))
}

/// 要求の電文を作る。`timer` は監視タイマ (250 ms 単位、0 なら無限に待つ)。
pub fn encode_request(header: &Header, timer: u16, command: &Command) -> Vec<u8> {
    let mut body = Writer::new(header.encoding);
    body.u16(timer);
    let (cmd, sub) = command.code();
    body.u16(cmd);
    body.u16(sub);
    match command {
        Command::ReadBits {
            device,
            start,
            count,
        }
        | Command::ReadWords {
            device,
            start,
            count,
        } => {
            body.device(*device, *start);
            body.u16(*count);
        }
        Command::WriteBits {
            device,
            start,
            values,
        } => {
            body.device(*device, *start);
            body.u16(values.len() as u16);
            body.bits(values);
        }
        Command::WriteWords {
            device,
            start,
            values,
        } => {
            body.device(*device, *start);
            body.u16(values.len() as u16);
            body.words(values);
        }
    }
    assemble(header, false, body)
}

/// 応答の電文を読む。`command` は送った要求 (読んだ点数を知るため)。
pub fn decode_response(
    frame: FrameKind,
    encoding: Encoding,
    command: &Command,
    data: &[u8],
) -> Result<(Header, Reply), FrameError> {
    let (header, mut r) = read_header(frame, encoding, true, data)?;
    let end_code = r.u16()?;
    if end_code != 0 {
        return Err(FrameError::EndCode(end_code));
    }
    let reply = match command {
        Command::ReadBits { count, .. } => Reply::Bits(r.bits(*count as usize)?),
        Command::ReadWords { count, .. } => Reply::Words(r.words(*count as usize)?),
        Command::WriteBits { .. } | Command::WriteWords { .. } => Reply::Written,
    };
    Ok((header, reply))
}

/// 要求の電文を読む (テスト用の PLC が使う)
pub fn decode_request(
    frame: FrameKind,
    encoding: Encoding,
    data: &[u8],
) -> Result<(Header, Command), FrameError> {
    let (header, mut r) = read_header(frame, encoding, false, data)?;
    let _timer = r.u16()?;
    let cmd = r.u16()?;
    let sub = r.u16()?;
    let (device, start) = r.device()?;
    let count = r.u16()?;
    let command = match (cmd, sub) {
        (0x0401, 0x0001) => Command::ReadBits {
            device,
            start,
            count,
        },
        (0x0401, 0x0000) => Command::ReadWords {
            device,
            start,
            count,
        },
        (0x1401, 0x0001) => Command::WriteBits {
            device,
            start,
            values: r.bits(count as usize)?,
        },
        (0x1401, 0x0000) => Command::WriteWords {
            device,
            start,
            values: r.words(count as usize)?,
        },
        _ => return Err(FrameError::EndCode(0xC059)),
    };
    Ok((header, command))
}

/// 応答の電文を作る (テスト用の PLC が使う)。`Err` の終了コードなら異常応答にする。
pub fn encode_response(header: &Header, reply: Result<&Reply, u16>) -> Vec<u8> {
    let mut body = Writer::new(header.encoding);
    match reply {
        Ok(reply) => {
            body.u16(0);
            match reply {
                Reply::Bits(values) => body.bits(values),
                Reply::Words(values) => body.words(values),
                Reply::Written => {}
            }
        }
        Err(code) => {
            body.u16(code);
            // 異常応答の後ろには、応答した局の情報とコマンドが付く (読む側は使わない)
            body.u8(header.route.network);
            body.u8(header.route.pc);
            body.u16(header.route.module_io);
            body.u8(header.route.station);
            body.u16(0);
            body.u16(0);
        }
    }
    assemble(header, true, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(frame: FrameKind, encoding: Encoding) -> Header {
        Header {
            frame,
            encoding,
            serial: 0x1234,
            route: Route::default(),
        }
    }

    fn hex(bytes: &str) -> Vec<u8> {
        bytes
            .split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    #[test]
    fn binary_3e_read_words_matches_the_manual() {
        // 三菱のマニュアルの例: D100 から 3 ワード読む
        let request = encode_request(
            &header(FrameKind::E3, Encoding::Binary),
            0x0010,
            &Command::ReadWords {
                device: Device::D,
                start: 100,
                count: 3,
            },
        );
        assert_eq!(
            request,
            hex("50 00 00 FF FF 03 00 0C 00 10 00 01 04 00 00 64 00 00 A8 03 00")
        );
    }

    #[test]
    fn ascii_3e_read_bits_matches_the_manual() {
        // M100 から 8 点をビット単位で読む
        let request = encode_request(
            &header(FrameKind::E3, Encoding::Ascii),
            0x0010,
            &Command::ReadBits {
                device: Device::M,
                start: 100,
                count: 8,
            },
        );
        assert_eq!(
            String::from_utf8(request).unwrap(),
            "500000FF03FF000018001004010001M*0001000008"
        );
    }

    #[test]
    fn ascii_hex_devices_use_hex_numbers() {
        let request = encode_request(
            &header(FrameKind::E3, Encoding::Ascii),
            0x0010,
            &Command::ReadBits {
                device: Device::X,
                start: 0x1F,
                count: 1,
            },
        );
        assert!(
            String::from_utf8(request)
                .unwrap()
                .ends_with("X*00001F0001")
        );
    }

    #[test]
    fn binary_bits_are_packed_two_per_byte() {
        let request = encode_request(
            &header(FrameKind::E3, Encoding::Binary),
            0x0010,
            &Command::WriteBits {
                device: Device::M,
                start: 100,
                values: vec![true, false, false, true, true],
            },
        );
        assert!(request.ends_with(&hex("64 00 00 90 05 00 10 01 10")));
    }

    #[test]
    fn binary_4e_carries_the_serial_number() {
        let request = encode_request(
            &header(FrameKind::E4, Encoding::Binary),
            0x0010,
            &Command::ReadWords {
                device: Device::D,
                start: 0,
                count: 1,
            },
        );
        assert_eq!(&request[..6], &hex("54 00 34 12 00 00")[..]);
        assert_eq!(
            request.len(),
            header_len(FrameKind::E4, Encoding::Binary) + 12
        );
    }

    #[test]
    fn responses_are_read_back() {
        // D100〜D102 = 1, 0x1234, 0xFFFF (3E バイナリの応答)
        let read = Command::ReadWords {
            device: Device::D,
            start: 100,
            count: 3,
        };
        let data = hex("D0 00 00 FF FF 03 00 08 00 00 00 01 00 34 12 FF FF");
        let (_, reply) = decode_response(FrameKind::E3, Encoding::Binary, &read, &data).unwrap();
        assert_eq!(reply, Reply::Words(vec![1, 0x1234, 0xFFFF]));

        // 異常応答
        let data = hex("D0 00 00 FF FF 03 00 0B 00 56 C0 00 FF FF 03 00 01 04 00 00");
        assert_eq!(
            decode_response(FrameKind::E3, Encoding::Binary, &read, &data),
            Err(FrameError::EndCode(0xC056))
        );
    }

    #[test]
    fn every_frame_round_trips() {
        let commands = [
            Command::ReadBits {
                device: Device::X,
                start: 0x10,
                count: 5,
            },
            Command::ReadWords {
                device: Device::W,
                start: 0x100,
                count: 2,
            },
            Command::WriteBits {
                device: Device::Y,
                start: 0x20,
                values: vec![true, false, true],
            },
            Command::WriteWords {
                device: Device::D,
                start: 200,
                values: vec![0, 0x7FFF, 0x8000],
            },
        ];
        let replies = [
            Reply::Bits(vec![true, true, false, false, true]),
            Reply::Words(vec![0xABCD, 1]),
            Reply::Written,
            Reply::Written,
        ];
        for frame in [FrameKind::E3, FrameKind::E4] {
            for encoding in [Encoding::Binary, Encoding::Ascii] {
                // 3E にはシリアル番号がない
                let h = Header {
                    serial: if frame == FrameKind::E4 { 0x1234 } else { 0 },
                    ..header(frame, encoding)
                };
                for (command, reply) in commands.iter().zip(&replies) {
                    let request = encode_request(&h, 4, command);
                    let head = &request[..header_len(frame, encoding)];
                    assert_eq!(
                        data_length(head, encoding).unwrap(),
                        request.len() - head.len()
                    );
                    let (got_header, got) = decode_request(frame, encoding, &request).unwrap();
                    assert_eq!((&got_header, &got), (&h, command));

                    let response = encode_response(&h, Ok(reply));
                    let (got_header, got) =
                        decode_response(frame, encoding, command, &response).unwrap();
                    assert_eq!((got_header.serial, &got), (h.serial, reply));

                    let error = encode_response(&h, Err(0xC059));
                    assert_eq!(
                        decode_response(frame, encoding, command, &error),
                        Err(FrameError::EndCode(0xC059))
                    );
                }
            }
        }
    }

    #[test]
    fn broken_frames_are_errors() {
        let read = Command::ReadWords {
            device: Device::D,
            start: 0,
            count: 1,
        };
        // 途中で切れている
        let data = hex("D0 00 00 FF FF 03 00 04 00 00 00");
        assert!(decode_response(FrameKind::E3, Encoding::Binary, &read, &data).is_err());
        // 要求のサブヘッダ
        let data = hex("50 00 00 FF FF 03 00 04 00 00 00 01 00");
        assert!(decode_response(FrameKind::E3, Encoding::Binary, &read, &data).is_err());
    }
}
