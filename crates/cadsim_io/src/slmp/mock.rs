//! テスト用の SLMP サーバー (三菱 PLC のふり)
//!
//! 実機の PLC がなくても通信を確かめられるよう、デバイスメモリを持って一括読出し・一括書込みに答える。
//! TCP / UDP、3E / 4E、バイナリ / ASCII のどれでも待ち受けられる。捨てると止まる。

use std::collections::HashMap;
use std::io::{ErrorKind, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cadsim_core::melsec::{Device, DeviceRef};

use super::client::{SlmpError, Transport, read_frame};
use super::frame::{self, Command, Encoding, FrameError, FrameKind, Header, Reply};

/// PLC のデバイスメモリ。書かれていない点は OFF / 0。
#[derive(Clone, Debug, Default)]
pub struct DeviceMemory {
    bits: HashMap<DeviceRef, bool>,
    words: HashMap<DeviceRef, u16>,
}

impl DeviceMemory {
    pub fn bit(&self, device: Device, number: u32) -> bool {
        self.bits
            .get(&DeviceRef::new(device, number))
            .copied()
            .unwrap_or(false)
    }

    pub fn set_bit(&mut self, device: Device, number: u32, on: bool) {
        self.bits.insert(DeviceRef::new(device, number), on);
    }

    pub fn word(&self, device: Device, number: u32) -> u16 {
        self.words
            .get(&DeviceRef::new(device, number))
            .copied()
            .unwrap_or(0)
    }

    pub fn set_word(&mut self, device: Device, number: u32, value: u16) {
        self.words.insert(DeviceRef::new(device, number), value);
    }

    /// 要求を実行して応答を返す。デバイスの種類が合わなければ終了コード C05C。
    pub fn apply(&mut self, command: &Command) -> Result<Reply, u16> {
        const WRONG_DEVICE: u16 = 0xC05C;
        match command {
            Command::ReadBits {
                device,
                start,
                count,
            } if device.is_bit() => Ok(Reply::Bits(
                (0..*count as u32)
                    .map(|i| self.bit(*device, start + i))
                    .collect(),
            )),
            Command::ReadWords {
                device,
                start,
                count,
            } if !device.is_bit() => Ok(Reply::Words(
                (0..*count as u32)
                    .map(|i| self.word(*device, start + i))
                    .collect(),
            )),
            Command::WriteBits {
                device,
                start,
                values,
            } if device.is_bit() => {
                for (n, &on) in (*start..).zip(values) {
                    self.set_bit(*device, n, on);
                }
                Ok(Reply::Written)
            }
            Command::WriteWords {
                device,
                start,
                values,
            } if !device.is_bit() => {
                for (n, &v) in (*start..).zip(values) {
                    self.set_word(*device, n, v);
                }
                Ok(Reply::Written)
            }
            _ => Err(WRONG_DEVICE),
        }
    }
}

struct State {
    memory: Mutex<DeviceMemory>,
    requests: AtomicUsize,
    stop: AtomicBool,
}

/// 待ち受け中のテスト用 PLC
pub struct MockPlc {
    address: SocketAddr,
    state: Arc<State>,
    threads: Vec<JoinHandle<()>>,
}

impl MockPlc {
    /// 127.0.0.1 の空いているポートで待ち受ける
    pub fn start(
        transport: Transport,
        frame: FrameKind,
        encoding: Encoding,
    ) -> std::io::Result<Self> {
        let state = Arc::new(State {
            memory: Mutex::new(DeviceMemory::default()),
            requests: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
        });
        let (address, thread) = match transport {
            Transport::Tcp => {
                let listener = TcpListener::bind("127.0.0.1:0")?;
                listener.set_nonblocking(true)?;
                let address = listener.local_addr()?;
                let state = Arc::clone(&state);
                let thread = thread::spawn(move || serve_tcp(listener, frame, encoding, &state));
                (address, thread)
            }
            Transport::Udp => {
                let socket = UdpSocket::bind("127.0.0.1:0")?;
                socket.set_read_timeout(Some(Duration::from_millis(20)))?;
                let address = socket.local_addr()?;
                let state = Arc::clone(&state);
                let thread = thread::spawn(move || serve_udp(socket, frame, encoding, &state));
                (address, thread)
            }
        };
        Ok(Self {
            address,
            state,
            threads: vec![thread],
        })
    }

    pub fn port(&self) -> u16 {
        self.address.port()
    }

    /// デバイスメモリを見る・書き換える
    pub fn memory(&self) -> MutexGuard<'_, DeviceMemory> {
        self.state.memory.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// これまでに答えた要求の数
    pub fn requests(&self) -> usize {
        self.state.requests.load(Ordering::Relaxed)
    }
}

impl Drop for MockPlc {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// 要求の電文1つに答える
fn respond(request: &[u8], frame: FrameKind, encoding: Encoding, state: &State) -> Vec<u8> {
    state.requests.fetch_add(1, Ordering::Relaxed);
    match frame::decode_request(frame, encoding, request) {
        Ok((header, command)) => {
            let reply = state
                .memory
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .apply(&command);
            frame::encode_response(&header, reply.as_ref().map_err(|&code| code))
        }
        Err(e) => {
            let code = match e {
                FrameError::EndCode(code) => code,
                FrameError::Malformed(_) => 0xC061,
            };
            // 読めなかった要求には、宛先を初期値にした異常応答を返す
            let header = Header {
                frame,
                encoding,
                ..Default::default()
            };
            frame::encode_response(&header, Err(code))
        }
    }
}

fn serve_tcp(listener: TcpListener, frame: FrameKind, encoding: Encoding, state: &Arc<State>) {
    let mut connections = Vec::new();
    while !state.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let state = Arc::clone(state);
                connections.push(thread::spawn(move || {
                    serve_connection(stream, frame, encoding, &state)
                }));
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => break,
        }
    }
    for c in connections {
        let _ = c.join();
    }
}

fn serve_connection(mut stream: TcpStream, frame: FrameKind, encoding: Encoding, state: &State) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(20)));
    while !state.stop.load(Ordering::Relaxed) {
        match read_frame(&mut stream, frame, encoding) {
            Ok(request) => {
                let response = respond(&request, frame, encoding, state);
                if stream.write_all(&response).is_err() {
                    return;
                }
            }
            Err(SlmpError::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                // 時間切れは待ち続ける (電文の途中で切れることはテストでは起きない)
            }
            Err(_) => return,
        }
    }
}

fn serve_udp(socket: UdpSocket, frame: FrameKind, encoding: Encoding, state: &State) {
    let mut buf = vec![0u8; 8192];
    while !state.stop.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                let response = respond(&buf[..n], frame, encoding, state);
                let _ = socket.send_to(&response, from);
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::client::{ConnectOptions, SlmpClient};
    use super::*;

    fn connect(
        plc: &MockPlc,
        transport: Transport,
        frame: FrameKind,
        encoding: Encoding,
    ) -> SlmpClient {
        SlmpClient::connect(&ConnectOptions {
            host: "127.0.0.1".into(),
            port: plc.port(),
            transport,
            frame,
            encoding,
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn client_reads_and_writes_over_every_combination() {
        for transport in [Transport::Tcp, Transport::Udp] {
            for frame in [FrameKind::E3, FrameKind::E4] {
                for encoding in [Encoding::Binary, Encoding::Ascii] {
                    let plc = MockPlc::start(transport, frame, encoding).unwrap();
                    plc.memory().set_bit(Device::Y, 0x1F, true);
                    plc.memory().set_word(Device::D, 100, 1234);

                    let mut client = connect(&plc, transport, frame, encoding);
                    let what = format!("{transport:?} {frame:?} {encoding:?}");
                    assert_eq!(
                        client.read_bits(Device::Y, 0x1E, 3).unwrap(),
                        vec![false, true, false],
                        "{what}"
                    );
                    assert_eq!(
                        client.read_words(Device::D, 99, 2).unwrap(),
                        vec![0, 1234],
                        "{what}"
                    );
                    client
                        .write_bits(Device::M, 100, &[true, false, true])
                        .unwrap();
                    client.write_words(Device::W, 0x10, &[0xFFFF, 7]).unwrap();
                    let memory = plc.memory();
                    assert!(
                        memory.bit(Device::M, 100) && memory.bit(Device::M, 102),
                        "{what}"
                    );
                    assert_eq!(memory.word(Device::W, 0x11), 7, "{what}");
                }
            }
        }
    }

    #[test]
    fn many_points_are_sent_in_several_requests() {
        let plc = MockPlc::start(Transport::Tcp, FrameKind::E3, Encoding::Binary).unwrap();
        let mut client = connect(&plc, Transport::Tcp, FrameKind::E3, Encoding::Binary);
        let values: Vec<u16> = (0..1000).collect();
        client.write_words(Device::D, 0, &values).unwrap();
        assert_eq!(client.read_words(Device::D, 0, 1000).unwrap(), values);
        assert_eq!(plc.requests(), 6);
    }

    #[test]
    fn plc_errors_are_reported() {
        let plc = MockPlc::start(Transport::Tcp, FrameKind::E3, Encoding::Ascii).unwrap();
        let mut client = connect(&plc, Transport::Tcp, FrameKind::E3, Encoding::Ascii);
        // ビットデバイスをワードとして読もうとする
        let error = client.read_words(Device::M, 0, 1).unwrap_err();
        assert!(matches!(
            error,
            SlmpError::Frame(FrameError::EndCode(0xC05C))
        ));
        assert!(error.to_string().contains("C05C"));
        // 続けて正しい要求は通る
        assert_eq!(client.read_bits(Device::M, 0, 1).unwrap(), vec![false]);
    }
}
