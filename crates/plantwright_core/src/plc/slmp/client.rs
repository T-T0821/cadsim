//! SLMP クライアント: TCP または UDP で PLC に要求を送り、応答を待つ (同期)
//!
//! 1回の要求の点数には上限があるので、多い点数は分けて送る。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use crate::plc::melsec::Device;

use super::frame::{
    self, Command, Encoding, FrameError, FrameKind, Header, Reply, Route, data_length, header_len,
};

/// 1回の要求で読み書きする最大の点数。Q シリーズの一括読出しの上限 (ワード 960 点など) より控えめにする。
pub const MAX_POINTS: usize = 480;

/// 通信路
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Transport {
    #[default]
    Tcp,
    Udp,
}

/// つなぎ方
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectOptions {
    pub host: String,
    pub port: u16,
    pub transport: Transport,
    pub frame: FrameKind,
    pub encoding: Encoding,
    pub route: Route,
    /// 応答を待つ時間
    pub timeout: Duration,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            host: "192.168.3.39".into(),
            port: 5000,
            transport: Transport::Tcp,
            frame: FrameKind::E3,
            encoding: Encoding::Binary,
            route: Route::default(),
            timeout: Duration::from_secs(1),
        }
    }
}

#[derive(Debug)]
pub enum SlmpError {
    Io(std::io::Error),
    Frame(FrameError),
}

impl std::fmt::Display for SlmpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SlmpError::Io(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                write!(f, "応答がありません (時間切れ)")
            }
            SlmpError::Io(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                write!(f, "応答がありません (時間切れ)")
            }
            SlmpError::Io(e) => write!(f, "通信エラー: {e}"),
            SlmpError::Frame(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SlmpError {}

impl From<std::io::Error> for SlmpError {
    fn from(e: std::io::Error) -> Self {
        SlmpError::Io(e)
    }
}

impl From<FrameError> for SlmpError {
    fn from(e: FrameError) -> Self {
        SlmpError::Frame(e)
    }
}

enum Socket {
    Tcp(TcpStream),
    Udp(UdpSocket),
}

/// PLC とのつながり1本
pub struct SlmpClient {
    socket: Socket,
    frame: FrameKind,
    encoding: Encoding,
    route: Route,
    timeout: Duration,
    /// 4E のシリアル番号 (要求ごとに 1 ずつ増やす)
    serial: u16,
}

impl SlmpClient {
    pub fn connect(options: &ConnectOptions) -> Result<Self, SlmpError> {
        let address: SocketAddr = (options.host.as_str(), options.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "アドレスが見つかりません")
            })?;
        let socket = match options.transport {
            Transport::Tcp => {
                let stream = TcpStream::connect_timeout(&address, options.timeout)?;
                stream.set_read_timeout(Some(options.timeout))?;
                stream.set_write_timeout(Some(options.timeout))?;
                stream.set_nodelay(true)?;
                Socket::Tcp(stream)
            }
            Transport::Udp => {
                let local: SocketAddr = if address.is_ipv4() {
                    "0.0.0.0:0".parse().unwrap()
                } else {
                    "[::]:0".parse().unwrap()
                };
                let socket = UdpSocket::bind(local)?;
                socket.connect(address)?;
                socket.set_read_timeout(Some(options.timeout))?;
                Socket::Udp(socket)
            }
        };
        Ok(Self {
            socket,
            frame: options.frame,
            encoding: options.encoding,
            route: options.route,
            timeout: options.timeout,
            serial: 0,
        })
    }

    /// 要求を1つ送り、応答を返す
    pub fn execute(&mut self, command: &Command) -> Result<Reply, SlmpError> {
        self.serial = self.serial.wrapping_add(1);
        let header = Header {
            frame: self.frame,
            encoding: self.encoding,
            serial: self.serial,
            route: self.route,
        };
        // 監視タイマは 250 ms 単位。待つ時間に合わせる (最低 1)
        let timer = (self.timeout.as_millis() / 250).clamp(1, u16::MAX as u128) as u16;
        let request = frame::encode_request(&header, timer, command);
        let deadline = Instant::now() + self.timeout;
        match &mut self.socket {
            Socket::Tcp(stream) => stream.write_all(&request)?,
            Socket::Udp(socket) => {
                socket.send(&request)?;
            }
        }
        loop {
            let response = match &mut self.socket {
                Socket::Tcp(stream) => read_frame(stream, self.frame, self.encoding)?,
                // UDP は 1 データグラムが 1 電文
                Socket::Udp(socket) => {
                    let mut buf = vec![0u8; 8192];
                    let n = socket.recv(&mut buf)?;
                    buf.truncate(n);
                    buf
                }
            };
            let (got, reply) =
                frame::decode_response(self.frame, self.encoding, command, &response)?;
            // 4E では番号の違う応答 (前の要求の遅れた応答) を捨てる
            if self.frame == FrameKind::E3 || got.serial == self.serial {
                return Ok(reply);
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into());
            }
        }
    }

    /// ビットデバイスを `count` 点読む
    pub fn read_bits(
        &mut self,
        device: Device,
        start: u32,
        count: usize,
    ) -> Result<Vec<bool>, SlmpError> {
        let mut values = Vec::with_capacity(count);
        for (offset, n) in chunks(count) {
            let command = Command::ReadBits {
                device,
                start: start + offset as u32,
                count: n as u16,
            };
            match self.execute(&command)? {
                Reply::Bits(v) => values.extend(v),
                _ => return Err(FrameError::Malformed("応答の種類が違います".into()).into()),
            }
        }
        Ok(values)
    }

    /// ワードデバイスを `count` 点読む
    pub fn read_words(
        &mut self,
        device: Device,
        start: u32,
        count: usize,
    ) -> Result<Vec<u16>, SlmpError> {
        let mut values = Vec::with_capacity(count);
        for (offset, n) in chunks(count) {
            let command = Command::ReadWords {
                device,
                start: start + offset as u32,
                count: n as u16,
            };
            match self.execute(&command)? {
                Reply::Words(v) => values.extend(v),
                _ => return Err(FrameError::Malformed("応答の種類が違います".into()).into()),
            }
        }
        Ok(values)
    }

    pub fn write_bits(
        &mut self,
        device: Device,
        start: u32,
        values: &[bool],
    ) -> Result<(), SlmpError> {
        for (offset, n) in chunks(values.len()) {
            self.execute(&Command::WriteBits {
                device,
                start: start + offset as u32,
                values: values[offset..offset + n].to_vec(),
            })?;
        }
        Ok(())
    }

    pub fn write_words(
        &mut self,
        device: Device,
        start: u32,
        values: &[u16],
    ) -> Result<(), SlmpError> {
        for (offset, n) in chunks(values.len()) {
            self.execute(&Command::WriteWords {
                device,
                start: start + offset as u32,
                values: values[offset..offset + n].to_vec(),
            })?;
        }
        Ok(())
    }
}

/// ストリームから電文を1つ読む (要求でも応答でも)。先頭を読んでデータ長を知り、残りを読む。
pub fn read_frame(
    stream: &mut impl Read,
    frame: FrameKind,
    encoding: Encoding,
) -> Result<Vec<u8>, SlmpError> {
    let mut data = vec![0u8; header_len(frame, encoding)];
    stream.read_exact(&mut data)?;
    let rest = data_length(&data, encoding)?;
    let head = data.len();
    data.resize(head + rest, 0);
    stream.read_exact(&mut data[head..])?;
    Ok(data)
}

/// `count` 点を (先頭からのずれ, 点数) に分ける
fn chunks(count: usize) -> impl Iterator<Item = (usize, usize)> {
    (0..count)
        .step_by(MAX_POINTS)
        .map(move |offset| (offset, MAX_POINTS.min(count - offset)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_requests_are_split() {
        assert_eq!(chunks(0).count(), 0);
        assert_eq!(
            chunks(1000).collect::<Vec<_>>(),
            vec![(0, 480), (480, 480), (960, 40)]
        );
    }
}
