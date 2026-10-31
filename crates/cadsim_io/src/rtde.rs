//! Universal Robots の RTDE (Real-Time Data Exchange) で、実機の関節角度を受け取る
//!
//! ロボットのコントローラ (ポート 30004) に TCP でつなぎ、次の順にやり取りする。
//!
//! 1. プロトコルのバージョン 2 を要求する (`V`)
//! 2. 受け取る値 `actual_q` (6 関節の角度、ラジアン) と送る周期を登録する (`O`)
//! 3. 送信を始めてもらう (`S`)
//! 4. あとは決めた周期で届くデータ (`U`) から角度を取り出す
//!
//! パケットは「長さ (u16, ヘッダー込み) + 種類 (u8) + 中身」で、数値はすべてビッグエンディアン。
//! 通信は別スレッドで行い、切れたら 1 秒ごとにつなぎ直す。

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// RTDE の標準のポート
pub const DEFAULT_PORT: u16 = 30004;
const PROTOCOL_VERSION: u16 = 2;
const TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
/// 受け取る値の名前
const OUTPUT: &str = "actual_q";

/// パケットの種類
pub mod kind {
    pub const REQUEST_PROTOCOL_VERSION: u8 = b'V';
    pub const TEXT_MESSAGE: u8 = b'M';
    pub const DATA_PACKAGE: u8 = b'U';
    pub const SETUP_OUTPUTS: u8 = b'O';
    pub const START: u8 = b'S';
    pub const PAUSE: u8 = b'P';
}

/// 届いたパケット 1 つ
#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    pub kind: u8,
    pub payload: Vec<u8>,
}

/// パケットを送る形 (長さ + 種類 + 中身) にする
pub fn encode(kind: u8, payload: &[u8]) -> Vec<u8> {
    let len = (payload.len() + 3) as u16;
    let mut out = Vec::with_capacity(len as usize);
    out.extend_from_slice(&len.to_be_bytes());
    out.push(kind);
    out.extend_from_slice(payload);
    out
}

/// プロトコルのバージョンの要求
pub fn request_protocol_version() -> Vec<u8> {
    encode(
        kind::REQUEST_PROTOCOL_VERSION,
        &PROTOCOL_VERSION.to_be_bytes(),
    )
}

/// 受け取る値の登録 (周期 `frequency` Hz、値の名前はカンマ区切り)
pub fn setup_outputs(frequency: f64, names: &[&str]) -> Vec<u8> {
    let mut payload = frequency.to_be_bytes().to_vec();
    payload.extend_from_slice(names.join(",").as_bytes());
    encode(kind::SETUP_OUTPUTS, &payload)
}

/// 受け取る値の登録への返事: 番号 (レシピ ID) と、値ごとの型の名前
#[derive(Clone, Debug, PartialEq)]
pub struct OutputRecipe {
    pub id: u8,
    pub types: Vec<String>,
}

pub fn parse_setup_outputs(payload: &[u8]) -> Option<OutputRecipe> {
    let (&id, rest) = payload.split_first()?;
    let types = String::from_utf8_lossy(rest)
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    Some(OutputRecipe { id, types })
}

/// 受け取ったデータから 6 関節の角度 (ラジアン) を取り出す。レシピの番号が違えば None。
pub fn parse_joint_angles(payload: &[u8], recipe: u8) -> Option<[f64; 6]> {
    let (&id, rest) = payload.split_first()?;
    if id != recipe || rest.len() < 48 {
        return None;
    }
    let mut q = [0.0; 6];
    for (i, value) in q.iter_mut().enumerate() {
        let bytes: [u8; 8] = rest[i * 8..i * 8 + 8].try_into().ok()?;
        *value = f64::from_be_bytes(bytes);
    }
    Some(q)
}

/// 受け取ったバイト列をためて、パケットの区切りで切り出す
#[derive(Default)]
pub struct PacketReader {
    buffer: Vec<u8>,
}

impl PacketReader {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// そろったパケットを 1 つ取り出す。長さが壊れていたら Err。
    pub fn next_packet(&mut self) -> Result<Option<Packet>, String> {
        if self.buffer.len() < 3 {
            return Ok(None);
        }
        let len = u16::from_be_bytes([self.buffer[0], self.buffer[1]]) as usize;
        if len < 3 {
            return Err(format!("パケットの長さが不正です ({len})"));
        }
        if self.buffer.len() < len {
            return Ok(None);
        }
        let packet = Packet {
            kind: self.buffer[2],
            payload: self.buffer[3..len].to_vec(),
        };
        self.buffer.drain(..len);
        Ok(Some(packet))
    }
}

/// つなぐ先
#[derive(Clone, Debug, PartialEq)]
pub struct RtdeSettings {
    pub host: String,
    pub port: u16,
    /// 角度を送ってもらう周期 (Hz)。e-Series は 500 まで、CB シリーズは 125 まで。
    pub frequency: f64,
}

impl Default for RtdeSettings {
    fn default() -> Self {
        Self {
            host: "192.168.0.10".into(),
            port: DEFAULT_PORT,
            frequency: 125.0,
        }
    }
}

/// 通信の状態
#[derive(Clone, Debug, Default, PartialEq)]
pub enum RtdeStatus {
    #[default]
    Connecting,
    Receiving,
    /// つながらない・切れた (理由)。少し待ってつなぎ直す。
    Failed(String),
}

/// 受け取った角度
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointSample {
    /// 6 関節の角度 (ラジアン。根元から順)
    pub q: [f64; 6],
    /// 何個目のデータか (新しいデータが届いたかを見分ける)
    pub sequence: u64,
    pub received: Instant,
}

#[derive(Default)]
struct Shared {
    status: RtdeStatus,
    latest: Option<JointSample>,
}

/// RTDE のクライアント。作ると通信スレッドが動き出し、捨てると止まる。
pub struct RtdeClient {
    settings: RtdeSettings,
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl RtdeClient {
    pub fn start(settings: RtdeSettings) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (settings, shared, stop) = (settings.clone(), shared.clone(), stop.clone());
            thread::spawn(move || run(&settings, &shared, &stop))
        };
        Self {
            settings,
            shared,
            stop,
            thread: Some(thread),
        }
    }

    pub fn settings(&self) -> &RtdeSettings {
        &self.settings
    }

    pub fn status(&self) -> RtdeStatus {
        self.shared
            .lock()
            .map(|s| s.status.clone())
            .unwrap_or_default()
    }

    /// いちばん新しい角度
    pub fn latest(&self) -> Option<JointSample> {
        self.shared.lock().ok().and_then(|s| s.latest)
    }
}

impl Drop for RtdeClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn set_status(shared: &Mutex<Shared>, status: RtdeStatus) {
    if let Ok(mut s) = shared.lock() {
        s.status = status;
    }
}

fn run(settings: &RtdeSettings, shared: &Mutex<Shared>, stop: &AtomicBool) {
    let mut sequence = 0;
    while !stop.load(Ordering::Relaxed) {
        set_status(shared, RtdeStatus::Connecting);
        let result = session(settings, shared, stop, &mut sequence);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let message = match result {
            Ok(()) => "接続が切れました".to_string(),
            Err(e) => e,
        };
        set_status(shared, RtdeStatus::Failed(message));
        // 止める指示にすぐ応えられるよう、細かく区切って待つ
        let until = Instant::now() + RETRY_INTERVAL;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

/// つないでから切れるまで
fn session(
    settings: &RtdeSettings,
    shared: &Mutex<Shared>,
    stop: &AtomicBool,
    sequence: &mut u64,
) -> Result<(), String> {
    let address = (settings.host.as_str(), settings.port)
        .to_socket_addrs()
        .map_err(|e| format!("アドレスが不正です: {e}"))?
        .next()
        .ok_or("アドレスが見つかりません")?;
    let mut stream = TcpStream::connect_timeout(&address, TIMEOUT)
        .map_err(|e| format!("接続できません: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| e.to_string())?;
    let _ = stream.set_nodelay(true);
    let mut reader = PacketReader::default();
    let send = |stream: &mut TcpStream, bytes: Vec<u8>| {
        stream
            .write_all(&bytes)
            .map_err(|e| format!("送れません: {e}"))
    };

    send(&mut stream, request_protocol_version())?;
    let reply = wait_for(
        &mut stream,
        &mut reader,
        kind::REQUEST_PROTOCOL_VERSION,
        stop,
    )?;
    if reply.payload.first() != Some(&1) {
        return Err("RTDE のバージョン 2 に対応していません".into());
    }

    send(&mut stream, setup_outputs(settings.frequency, &[OUTPUT]))?;
    let reply = wait_for(&mut stream, &mut reader, kind::SETUP_OUTPUTS, stop)?;
    let recipe = parse_setup_outputs(&reply.payload).ok_or("登録の返事が不正です")?;
    if recipe.id == 0 || recipe.types.first().map(String::as_str) != Some("VECTOR6D") {
        return Err(format!(
            "{OUTPUT} を受け取れません ({})",
            recipe.types.join(",")
        ));
    }

    send(&mut stream, encode(kind::START, &[]))?;
    let reply = wait_for(&mut stream, &mut reader, kind::START, stop)?;
    if reply.payload.first() != Some(&1) {
        return Err("送信を始められません".into());
    }
    set_status(shared, RtdeStatus::Receiving);

    let mut last = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = send(&mut stream, encode(kind::PAUSE, &[]));
            return Ok(());
        }
        while let Some(packet) = reader.next_packet()? {
            if packet.kind != kind::DATA_PACKAGE {
                continue;
            }
            if let Some(q) = parse_joint_angles(&packet.payload, recipe.id) {
                *sequence += 1;
                last = Instant::now();
                if let Ok(mut s) = shared.lock() {
                    s.latest = Some(JointSample {
                        q,
                        sequence: *sequence,
                        received: last,
                    });
                }
            }
        }
        if last.elapsed() > TIMEOUT {
            return Err("データが届きません".into());
        }
        read_some(&mut stream, &mut reader)?;
    }
}

/// 受け取れた分を `reader` にためる。何も届かなければそのまま戻る。接続が閉じたら Err。
fn read_some(stream: &mut TcpStream, reader: &mut PacketReader) -> Result<(), String> {
    let mut buffer = [0u8; 4096];
    match stream.read(&mut buffer) {
        Ok(0) => Err("接続が閉じられました".into()),
        Ok(n) => {
            reader.push(&buffer[..n]);
            Ok(())
        }
        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(()),
        Err(e) => Err(format!("受け取れません: {e}")),
    }
}

/// 種類 `want` のパケットが届くまで待つ (途中のメッセージなどは読み飛ばす)
fn wait_for(
    stream: &mut TcpStream,
    reader: &mut PacketReader,
    want: u8,
    stop: &AtomicBool,
) -> Result<Packet, String> {
    let until = Instant::now() + TIMEOUT;
    loop {
        while let Some(packet) = reader.next_packet()? {
            if packet.kind == want {
                return Ok(packet);
            }
        }
        if Instant::now() > until || stop.load(Ordering::Relaxed) {
            return Err("ロボットから返事がありません".into());
        }
        read_some(stream, reader)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn packets_are_split_even_when_they_arrive_in_pieces() {
        let mut bytes = encode(kind::START, &[1]);
        bytes.extend(encode(kind::TEXT_MESSAGE, b"hello"));
        let mut reader = PacketReader::default();
        reader.push(&bytes[..2]);
        assert_eq!(reader.next_packet(), Ok(None));
        reader.push(&bytes[2..6]);
        assert_eq!(
            reader.next_packet(),
            Ok(Some(Packet {
                kind: kind::START,
                payload: vec![1]
            }))
        );
        assert_eq!(reader.next_packet(), Ok(None));
        reader.push(&bytes[6..]);
        let message = reader.next_packet().unwrap().unwrap();
        assert_eq!(message.payload, b"hello");
    }

    #[test]
    fn setup_outputs_packet_layout() {
        let bytes = setup_outputs(125.0, &["actual_q"]);
        assert_eq!(
            u16::from_be_bytes([bytes[0], bytes[1]]) as usize,
            bytes.len()
        );
        assert_eq!(bytes[2], b'O');
        assert_eq!(f64::from_be_bytes(bytes[3..11].try_into().unwrap()), 125.0);
        assert_eq!(&bytes[11..], b"actual_q");
        assert_eq!(request_protocol_version(), vec![0, 5, b'V', 0, 2]);
    }

    fn data_package(recipe: u8, q: [f64; 6]) -> Vec<u8> {
        let mut payload = vec![recipe];
        for v in q {
            payload.extend_from_slice(&v.to_be_bytes());
        }
        encode(kind::DATA_PACKAGE, &payload)
    }

    #[test]
    fn joint_angles_need_the_registered_recipe() {
        let q = [0.1, -1.2, 1.5, 0.0, 1.57, -3.0];
        let packet = data_package(3, q);
        assert_eq!(parse_joint_angles(&packet[3..], 3), Some(q));
        assert_eq!(parse_joint_angles(&packet[3..], 1), None);
        assert_eq!(parse_joint_angles(&packet[3..20], 3), None);
    }

    /// 手元に立てた偽のコントローラとやり取りして、角度を受け取れること
    #[test]
    fn client_receives_angles_from_a_controller() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let q = [0.0, -1.5, 1.4, -1.3, -1.2, 0.25];
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = PacketReader::default();
            let mut next = |stream: &mut TcpStream| loop {
                if let Some(p) = reader.next_packet().unwrap() {
                    return p;
                }
                let mut buf = [0u8; 256];
                let n = stream.read(&mut buf).unwrap();
                reader.push(&buf[..n]);
            };
            let version = next(&mut stream);
            assert_eq!(version.payload, vec![0, 2]);
            // 返事の前に関係ないメッセージを混ぜる
            stream
                .write_all(&encode(kind::TEXT_MESSAGE, b"info"))
                .unwrap();
            stream
                .write_all(&encode(kind::REQUEST_PROTOCOL_VERSION, &[1]))
                .unwrap();
            let setup = next(&mut stream);
            assert_eq!(&setup.payload[8..], b"actual_q");
            let mut reply = vec![1];
            reply.extend_from_slice(b"VECTOR6D");
            stream
                .write_all(&encode(kind::SETUP_OUTPUTS, &reply))
                .unwrap();
            assert_eq!(next(&mut stream).kind, kind::START);
            stream.write_all(&encode(kind::START, &[1])).unwrap();
            for _ in 0..50 {
                if stream.write_all(&data_package(1, q)).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        });

        let client = RtdeClient::start(RtdeSettings {
            host: "127.0.0.1".into(),
            port,
            frequency: 125.0,
        });
        let until = Instant::now() + Duration::from_secs(5);
        let sample = loop {
            if let Some(sample) = client.latest() {
                break sample;
            }
            assert!(
                Instant::now() < until,
                "角度が届かない: {:?}",
                client.status()
            );
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(sample.q, q);
        assert_eq!(client.status(), RtdeStatus::Receiving);
        drop(client);
        server.join().unwrap();
    }
}
