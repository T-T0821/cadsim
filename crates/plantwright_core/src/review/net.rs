//! TCP での送受信
//!
//! ホストは待ち受けのスレッドと配り役のスレッドを持ち、つないできた人ごとに読み取りのスレッドを立てる。
//! 配り役 (`crate::review::Hub`) は配り役のスレッドの中だけで動かすので、ロックはいらない。
//! ホスト自身の画面も番号 0 の参加者として、チャンネル越しに同じ配り役とやり取りする。
//! 参加する側は、接続・書き込みのスレッドと読み取りのスレッドを持つ。
//!
//! 画面 からは、ホストでも参加でも同じ `ReviewLink` の `send` と `poll` だけを使う。

use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::review::{HOST_ID, Hub, Message, Outgoing, PeerId};

use super::wire::{decode, encode};

/// 1 行の最大の長さ (バイト)。これより長い行を送ってくる相手とは切る
pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// つなぐのを待つ時間
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 書き込みが詰まったときに相手をあきらめるまでの時間
const WRITE_TIMEOUT: Duration = Duration::from_secs(3);
/// スレッドが止めるよう言われていないかを見る間隔
const POLL: Duration = Duration::from_millis(50);

/// 画面に届くできごと
#[derive(Clone, Debug, PartialEq)]
pub enum LinkEvent {
    Message(Message),
    /// つながらなかった・切れた。理由を添える
    Closed(String),
}

enum HubEvent {
    Connected(TcpStream),
    Incoming(PeerId, Message),
    Closed(PeerId),
    Local(Message),
    Stop,
}

enum Sink {
    Host(Sender<HubEvent>),
    Client(Sender<Message>),
}

/// ホストとしての待ち受け、または参加した接続
pub struct ReviewLink {
    sink: Sink,
    events: Mutex<Receiver<LinkEvent>>,
    stop: Arc<AtomicBool>,
    /// 参加した側のソケット (閉じるときに読み取りのスレッドを起こすため)
    socket: Arc<Mutex<Option<TcpStream>>>,
    /// ホストなら待ち受けているポート
    port: Option<u16>,
}

impl ReviewLink {
    /// ホストになって `port` で待ち受ける (0 なら空いているポート)。
    /// 最初に自分あての `Welcome` が届く。
    pub fn host(port: u16, hub: Hub) -> io::Result<Self> {
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (hub_tx, hub_rx) = channel();
        let (event_tx, event_rx) = channel();
        let _ = event_tx.send(LinkEvent::Message(hub.host_welcome()));

        {
            let stop = stop.clone();
            let hub_tx = hub_tx.clone();
            thread::Builder::new()
                .name("review-accept".into())
                .spawn(move || accept_loop(listener, hub_tx, stop))?;
        }
        {
            let stop = stop.clone();
            let hub_tx = hub_tx.clone();
            thread::Builder::new()
                .name("review-hub".into())
                .spawn(move || hub_loop(hub, hub_rx, hub_tx, event_tx, stop))?;
        }
        Ok(Self {
            sink: Sink::Host(hub_tx),
            events: Mutex::new(event_rx),
            stop,
            socket: Arc::new(Mutex::new(None)),
            port: Some(port),
        })
    }

    /// `address` ("192.168.1.10:47800" など) のホストへ参加する。
    /// つなぐのは別スレッドなので、すぐ戻る。つながらなければ `Closed` が届く。
    pub fn join(address: &str, hello: Message) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (out_tx, out_rx) = channel();
        let (event_tx, event_rx) = channel();
        let socket = Arc::new(Mutex::new(None));
        {
            let address = address.to_string();
            let stop = stop.clone();
            let socket = socket.clone();
            let thread_tx = event_tx.clone();
            let spawned = thread::Builder::new()
                .name("review-client".into())
                .spawn(move || client_loop(&address, hello, out_rx, thread_tx, socket, stop));
            if let Err(e) = spawned {
                let _ = event_tx.send(LinkEvent::Closed(format!("スレッドを作れません: {e}")));
            }
        }
        Self {
            sink: Sink::Client(out_tx),
            events: Mutex::new(event_rx),
            stop,
            socket,
            port: None,
        }
    }

    /// ホストとして待ち受けているポート
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    pub fn is_host(&self) -> bool {
        matches!(self.sink, Sink::Host(_))
    }

    /// 送る。ホストなら配り役を通してほかの全員に届く
    pub fn send(&self, message: Message) {
        match &self.sink {
            Sink::Host(tx) => {
                let _ = tx.send(HubEvent::Local(message));
            }
            Sink::Client(tx) => {
                let _ = tx.send(message);
            }
        }
    }

    /// 届いたできごとをすべて取り出す
    pub fn poll(&self) -> Vec<LinkEvent> {
        match self.events.lock() {
            Ok(rx) => rx.try_iter().collect(),
            Err(_) => vec![LinkEvent::Closed("通信の内部でエラーが起きました".into())],
        }
    }
}

impl Drop for ReviewLink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Sink::Host(tx) = &self.sink {
            let _ = tx.send(HubEvent::Stop);
        }
        if let Ok(mut socket) = self.socket.lock()
            && let Some(socket) = socket.take()
        {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}

fn accept_loop(listener: TcpListener, hub_tx: Sender<HubEvent>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                if hub_tx.send(HubEvent::Connected(stream)).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(_) => thread::sleep(POLL),
        }
    }
}

fn hub_loop(
    mut hub: Hub,
    hub_rx: Receiver<HubEvent>,
    hub_tx: Sender<HubEvent>,
    event_tx: Sender<LinkEvent>,
    stop: Arc<AtomicBool>,
) {
    let mut writers: HashMap<PeerId, TcpStream> = HashMap::new();
    loop {
        let event = match hub_rx.recv_timeout(POLL * 4) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) if !stop.load(Ordering::SeqCst) => continue,
            Err(_) => break,
        };
        let out = match event {
            HubEvent::Connected(stream) => {
                let _ = accept(&mut hub, stream, &hub_tx, &mut writers);
                Vec::new()
            }
            HubEvent::Incoming(id, message) => hub.receive(id, message),
            HubEvent::Closed(id) => {
                if let Some(w) = writers.remove(&id) {
                    let _ = w.shutdown(Shutdown::Both);
                }
                hub.disconnect(id)
            }
            HubEvent::Local(message) => hub.receive(HOST_ID, message),
            HubEvent::Stop => break,
        };
        let mut queue: VecDeque<Outgoing> = out.into();
        while let Some(job) = queue.pop_front() {
            match job {
                Outgoing::Send(HOST_ID, message) => {
                    let _ = event_tx.send(LinkEvent::Message(message));
                }
                Outgoing::Send(id, message) => {
                    let Some(writer) = writers.get_mut(&id) else {
                        continue;
                    };
                    if writer.write_all(encode(&message).as_bytes()).is_err() {
                        if let Some(w) = writers.remove(&id) {
                            let _ = w.shutdown(Shutdown::Both);
                        }
                        queue.extend(hub.disconnect(id));
                    }
                }
                Outgoing::Disconnect(id) => {
                    if let Some(mut w) = writers.remove(&id) {
                        let _ = w.flush();
                        let _ = w.shutdown(Shutdown::Both);
                    }
                    queue.extend(hub.disconnect(id));
                }
            }
        }
    }
    for (_, writer) in writers {
        let _ = writer.shutdown(Shutdown::Both);
    }
}

/// つないできた人に番号を振り、読み取りのスレッドを立てる
fn accept(
    hub: &mut Hub,
    stream: TcpStream,
    hub_tx: &Sender<HubEvent>,
    writers: &mut HashMap<PeerId, TcpStream>,
) -> Option<PeerId> {
    stream.set_nonblocking(false).ok()?;
    let _ = stream.set_nodelay(true);
    stream.set_write_timeout(Some(WRITE_TIMEOUT)).ok()?;
    let reader = stream.try_clone().ok()?;
    let id = hub.connect();
    let tx = hub_tx.clone();
    let spawned = thread::Builder::new()
        .name(format!("review-peer-{id}"))
        .spawn(move || {
            read_loop(reader, |message| {
                tx.send(HubEvent::Incoming(id, message)).is_ok()
            });
            let _ = tx.send(HubEvent::Closed(id));
        });
    if spawned.is_err() {
        hub.disconnect(id);
        return None;
    }
    writers.insert(id, stream);
    Some(id)
}

fn client_loop(
    address: &str,
    hello: Message,
    out_rx: Receiver<Message>,
    event_tx: Sender<LinkEvent>,
    socket: Arc<Mutex<Option<TcpStream>>>,
    stop: Arc<AtomicBool>,
) {
    let mut stream = match connect(address) {
        Ok(stream) => stream,
        Err(e) => {
            let _ = event_tx.send(LinkEvent::Closed(format!(
                "{address} につながりませんでした: {e}"
            )));
            return;
        }
    };
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let Ok(reader) = stream.try_clone() else {
        let _ = event_tx.send(LinkEvent::Closed("接続を準備できませんでした".into()));
        return;
    };
    if let Ok(mut slot) = socket.lock() {
        if stop.load(Ordering::SeqCst) {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        *slot = stream.try_clone().ok();
    }
    {
        let reader_tx = event_tx.clone();
        let stop = stop.clone();
        let spawned = thread::Builder::new()
            .name("review-client-read".into())
            .spawn(move || {
                let reason = read_loop(reader, |message| {
                    reader_tx.send(LinkEvent::Message(message)).is_ok()
                });
                if !stop.load(Ordering::SeqCst) {
                    let _ = reader_tx.send(LinkEvent::Closed(reason));
                }
            });
        if spawned.is_err() {
            let _ = event_tx.send(LinkEvent::Closed("スレッドを作れません".into()));
            return;
        }
    }
    let mut next = Some(hello);
    loop {
        if let Some(message) = next.take()
            && let Err(e) = stream.write_all(encode(&message).as_bytes())
        {
            if !stop.load(Ordering::SeqCst) {
                let _ = event_tx.send(LinkEvent::Closed(format!("送れませんでした: {e}")));
            }
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        match out_rx.recv_timeout(POLL * 4) {
            Ok(message) => next = Some(message),
            Err(RecvTimeoutError::Timeout) if !stop.load(Ordering::SeqCst) => {}
            Err(_) => {
                let _ = stream.shutdown(Shutdown::Both);
                return;
            }
        }
    }
}

fn connect(address: &str) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "アドレスが見つかりません");
    for addr in address.to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// 1 行ずつ読んでメッセージにし、`deliver` に渡す。切れたら理由を返す。
/// `deliver` が false を返したらやめる。読めない行は飛ばす。
fn read_loop(stream: TcpStream, mut deliver: impl FnMut(Message) -> bool) -> String {
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut reader)
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
        {
            Ok(0) => return "相手が接続を閉じました".into(),
            Ok(_) if line.last() != Some(&b'\n') => {
                return if line.len() > MAX_LINE_BYTES {
                    "長すぎるデータが届いたので切りました".into()
                } else {
                    "相手が接続を閉じました".into()
                };
            }
            Ok(_) => {
                let Ok(text) = std::str::from_utf8(&line) else {
                    continue;
                };
                if let Some(message) = decode(text)
                    && !deliver(message)
                {
                    return "閉じました".into();
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return format!("通信が切れました: {e}"),
        }
    }
}

/// 打ち込まれたホストの場所を「名前かアドレス:ポート」にする。ポートがなければ `default_port` を付ける
pub fn parse_address(input: &str, default_port: u16) -> String {
    let input = input.trim();
    if input.parse::<SocketAddr>().is_ok() {
        return input.to_string();
    }
    let bare = input.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return SocketAddr::new(ip, default_port).to_string();
    }
    if let Some((host, port)) = input.rsplit_once(':')
        && !host.is_empty()
        && port.parse::<u16>().is_ok()
    {
        return input.to_string();
    }
    format!("{input}:{default_port}")
}

/// このパソコンの LAN のアドレス (参加する人に伝える)。分からなければ空
pub fn local_addresses() -> Vec<IpAddr> {
    // UDP は connect してもパケットを送らない。外へ出るときに使うアドレスを OS に選ばせるだけ
    let probe = || -> io::Result<IpAddr> {
        let socket = UdpSocket::bind(("0.0.0.0", 0))?;
        socket.connect(("192.0.2.1", 9))?;
        Ok(socket.local_addr()?.ip())
    };
    probe()
        .ok()
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::{PROTOCOL_VERSION, ViewPose};
    use std::time::Instant;

    fn hello(name: &str, passphrase: &str) -> Message {
        Message::Hello {
            version: PROTOCOL_VERSION,
            name: name.into(),
            passphrase: passphrase.into(),
            project: "line1".into(),
        }
    }

    /// 条件に合うできごとが届くまで待つ (最大 5 秒)
    fn wait_for(link: &ReviewLink, mut pred: impl FnMut(&LinkEvent) -> bool) -> LinkEvent {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            for event in link.poll() {
                if pred(&event) {
                    return event;
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("待っているできごとが届きませんでした");
    }

    #[test]
    fn addresses_get_default_port() {
        assert_eq!(parse_address("192.168.1.5", 47800), "192.168.1.5:47800");
        assert_eq!(
            parse_address(" 192.168.1.5:9000 ", 47800),
            "192.168.1.5:9000"
        );
        assert_eq!(parse_address("::1", 47800), "[::1]:47800");
        assert_eq!(parse_address("[::1]:9000", 47800), "[::1]:9000");
        assert_eq!(parse_address("factory-pc", 47800), "factory-pc:47800");
        assert_eq!(parse_address("factory-pc:1234", 47800), "factory-pc:1234");
    }

    #[test]
    fn host_and_two_guests_see_each_other() {
        let host = ReviewLink::host(0, Hub::new("ホスト", "abc", "line1")).unwrap();
        assert!(host.is_host());
        let port = host.port().unwrap();
        let address = format!("127.0.0.1:{port}");
        wait_for(&host, |e| {
            matches!(e, LinkEvent::Message(Message::Welcome { you: 0, .. }))
        });

        let a = ReviewLink::join(&address, hello("A", "abc"));
        let a_id = match wait_for(&a, |e| {
            matches!(e, LinkEvent::Message(Message::Welcome { .. }))
        }) {
            LinkEvent::Message(Message::Welcome { you, peers, .. }) => {
                assert_eq!(peers.len(), 2);
                you
            }
            _ => unreachable!(),
        };
        wait_for(
            &host,
            |e| matches!(e, LinkEvent::Message(Message::Joined(p)) if p.name == "A"),
        );

        let b = ReviewLink::join(&address, hello("B", "abc"));
        wait_for(&b, |e| {
            matches!(e, LinkEvent::Message(Message::Welcome { .. }))
        });
        wait_for(
            &a,
            |e| matches!(e, LinkEvent::Message(Message::Joined(p)) if p.name == "B"),
        );

        // A の視点がホストと B に届く
        let pose = ViewPose {
            eye: [1.0, 2.0, 3.0],
            focus: [0.0; 3],
            orthographic: false,
        };
        a.send(Message::View { id: 0, pose });
        let expected = LinkEvent::Message(Message::View { id: a_id, pose });
        wait_for(&host, |e| *e == expected);
        wait_for(&b, |e| *e == expected);

        // ホストのチャットが A と B に届く
        host.send(Message::Chat {
            id: 0,
            text: "ここを見て".into(),
        });
        let chat = LinkEvent::Message(Message::Chat {
            id: 0,
            text: "ここを見て".into(),
        });
        wait_for(&a, |e| *e == chat);
        wait_for(&b, |e| *e == chat);

        // A が抜けると、ホストと B に知らせが届く
        drop(a);
        let left = LinkEvent::Message(Message::Left { id: a_id });
        wait_for(&host, |e| *e == left);
        wait_for(&b, |e| *e == left);

        // ホストが終わると B の接続が切れる
        drop(host);
        wait_for(&b, |e| matches!(e, LinkEvent::Closed(_)));
    }

    #[test]
    fn wrong_passphrase_is_rejected_then_closed() {
        let host = ReviewLink::host(0, Hub::new("ホスト", "abc", "")).unwrap();
        let address = format!("127.0.0.1:{}", host.port().unwrap());
        let guest = ReviewLink::join(&address, hello("A", "xyz"));
        // 断りの理由が届いてから切れる (同じ poll で両方届くこともある)
        let mut seen = Vec::new();
        wait_for(&guest, |e| {
            seen.push(e.clone());
            matches!(e, LinkEvent::Closed(_))
        });
        assert!(
            matches!(&seen[0], LinkEvent::Message(Message::Rejected { reason }) if reason.contains("合言葉")),
            "{seen:?}"
        );
    }

    #[test]
    fn unreachable_host_reports_closed() {
        // 待ち受けを閉じた直後のポートにつなぐ
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let guest = ReviewLink::join(&format!("127.0.0.1:{port}"), hello("A", ""));
        wait_for(&guest, |e| matches!(e, LinkEvent::Closed(_)));
    }

    #[test]
    fn overlong_lines_disconnect_the_sender() {
        let host = ReviewLink::host(0, Hub::new("ホスト", "", "")).unwrap();
        let mut raw = TcpStream::connect(("127.0.0.1", host.port().unwrap())).unwrap();
        raw.write_all(encode(&hello("A", "")).as_bytes()).unwrap();
        wait_for(&host, |e| {
            matches!(e, LinkEvent::Message(Message::Joined(_)))
        });
        let junk = vec![b'x'; MAX_LINE_BYTES + 10];
        let _ = raw.write_all(&junk);
        wait_for(&host, |e| {
            matches!(e, LinkEvent::Message(Message::Left { .. }))
        });
    }
}
