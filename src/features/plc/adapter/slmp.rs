//! SLMP (MC プロトコル) で三菱電機の PLC とつなぐ
//!
//! シミュレーターがクライアントになり、PLC に周期的に
//! - PLC の出力 (I/O 表の出力の信号を割り当てたデバイス。初期値は M0〜・D0〜) を一括読出しで読む
//! - 設備の状態 (入力の信号。初期値は M100〜・D100〜) を一括書込みで書く
//!
//! 電文と TCP / UDP の通信は `plantwright_core::plc::slmp`、信号とデバイスの対応は `domain::device_map` にある。
//! 通信は別スレッドで行い、画面の更新は止めない。切れたら1秒ごとにつなぎ直す。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use plantwright_core::plc::melsec::{Device, DeviceRef};
use plantwright_core::plc::slmp::{ConnectOptions, Encoding, FrameKind, SlmpClient, Transport};

use crate::features::plc::domain::device_map::DeviceMap;
use crate::features::plc::domain::io::{IoTable, IoValues};
use crate::features::plc::usecase::{LinkStatus, PlcLink};

const TIMEOUT: Duration = Duration::from_secs(1);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlmpSettings {
    pub host: String,
    pub port: u16,
    pub transport: Transport,
    pub frame: FrameKind,
    pub encoding: Encoding,
    /// 通信の周期 (ミリ秒)
    pub poll_ms: u64,
    /// I/O 表の信号を置くデバイス
    pub devices: DeviceMap,
}

impl Default for SlmpSettings {
    fn default() -> Self {
        Self {
            host: "192.168.3.39".into(),
            port: 5000,
            transport: Transport::Tcp,
            frame: FrameKind::E3,
            encoding: Encoding::Binary,
            poll_ms: 20,
            devices: DeviceMap::default(),
        }
    }
}

/// 1回の通信でやること (どのデバイスを読み、何を書くか)
#[derive(Clone, Debug, Default, PartialEq)]
struct Request {
    read_bits: Vec<DeviceRef>,
    read_words: Vec<DeviceRef>,
    write_bits: Vec<(DeviceRef, bool)>,
    write_words: Vec<(DeviceRef, u16)>,
}

/// 読んだ結果
#[derive(Clone, Debug, Default)]
struct Readback {
    bits: HashMap<DeviceRef, bool>,
    words: HashMap<DeviceRef, u16>,
}

/// 画面側と通信スレッドで共有するもの
#[derive(Default)]
struct Shared {
    request: Request,
    readback: Readback,
    status: LinkStatus,
}

/// SLMP クライアント。作ると通信スレッドが動き出し、捨てると止まる。
pub struct SlmpLink {
    devices: DeviceMap,
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SlmpLink {
    pub fn start(settings: SlmpSettings) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let devices = settings.devices;
        let thread = {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            thread::spawn(move || communicate(&settings, &shared, &stop))
        };
        Self {
            devices,
            shared,
            stop,
            thread: Some(thread),
        }
    }

    fn shared(&self) -> MutexGuard<'_, Shared> {
        // 通信スレッドが panic しても画面側は動き続けたいので、毒された Mutex も使う
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for SlmpLink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl PlcLink for SlmpLink {
    fn exchange(&mut self, table: &IoTable, io: &mut IoValues) {
        let located = self.devices.locate_all(table);
        let mut request = Request::default();
        for (p, device) in table.points.iter().zip(&located) {
            // デバイスに置けない信号 (画面に警告を出す) と、種類の合わないデバイスは飛ばす
            let Some(device) = *device else { continue };
            if device.device.is_bit() == p.point.is_word() {
                continue;
            }
            match (p.point.is_output(), p.point.is_word()) {
                (true, false) => request.read_bits.push(device),
                (true, true) => request.read_words.push(device),
                (false, false) => request.write_bits.push((device, io.bit(p.point))),
                (false, true) => request
                    .write_words
                    .push((device, p.encode_word(io.word(p.point)))),
            }
        }

        let mut shared = self.shared();
        shared.request = request;
        // まだ一度も読めていない出力は OFF / 0 のままにする
        for (p, device) in table.points.iter().zip(located) {
            let Some(device) = device.filter(|_| p.point.is_output()) else {
                continue;
            };
            if p.point.is_word() {
                if let Some(&raw) = shared.readback.words.get(&device) {
                    io.set_word(p.point, p.decode_word(raw));
                }
            } else if let Some(&on) = shared.readback.bits.get(&device) {
                io.set_bit(p.point, on);
            }
        }
    }

    fn status(&self) -> LinkStatus {
        self.shared().status.clone()
    }
}

/// 通信スレッドの本体。stop が立つまで、つなぐ → 周期的に読み書き → 切れたらつなぎ直す。
fn communicate(settings: &SlmpSettings, shared: &Mutex<Shared>, stop: &AtomicBool) {
    let lock = || shared.lock().unwrap_or_else(|e| e.into_inner());
    let set_status = |status| lock().status = status;
    let poll = Duration::from_millis(settings.poll_ms.max(1));
    let options = ConnectOptions {
        host: settings.host.clone(),
        port: settings.port,
        transport: settings.transport,
        frame: settings.frame,
        encoding: settings.encoding,
        timeout: TIMEOUT,
        ..Default::default()
    };

    while !stop.load(Ordering::Relaxed) {
        set_status(LinkStatus::Connecting);
        let mut client = match SlmpClient::connect(&options) {
            Ok(client) => client,
            Err(e) => {
                set_status(LinkStatus::Error(format!("接続できません: {e}")));
                sleep_unless_stopped(RETRY_INTERVAL, stop);
                continue;
            }
        };
        while !stop.load(Ordering::Relaxed) {
            let started = Instant::now();
            let request = lock().request.clone();
            match transfer(&mut client, &request) {
                Ok(readback) => {
                    let mut s = lock();
                    s.readback = readback;
                    s.status = LinkStatus::Connected {
                        cycle_ms: started.elapsed().as_secs_f64() * 1000.0,
                    };
                }
                Err(e) => {
                    set_status(LinkStatus::Error(e.to_string()));
                    sleep_unless_stopped(RETRY_INTERVAL, stop);
                    break;
                }
            }
            thread::sleep(poll.saturating_sub(started.elapsed()));
        }
    }
    set_status(LinkStatus::Disconnected);
}

/// 1周期分の読み書き。同じデバイスの連続した番号はまとめて1回の要求にする。
fn transfer(
    client: &mut SlmpClient,
    request: &Request,
) -> Result<Readback, plantwright_core::plc::slmp::SlmpError> {
    let mut readback = Readback::default();
    for (device, start, count) in runs(&request.read_bits) {
        let values = client.read_bits(device, start, count)?;
        let refs = (start..).map(|n| DeviceRef::new(device, n));
        readback.bits.extend(refs.zip(values));
    }
    for (device, start, count) in runs(&request.read_words) {
        let values = client.read_words(device, start, count)?;
        let refs = (start..).map(|n| DeviceRef::new(device, n));
        readback.words.extend(refs.zip(values));
    }
    for (device, start, values) in consecutive_values(&request.write_bits) {
        client.write_bits(device, start, &values)?;
    }
    for (device, start, values) in consecutive_values(&request.write_words) {
        client.write_words(device, start, &values)?;
    }
    Ok(readback)
}

fn sort_key(r: &DeviceRef) -> (&'static str, u32) {
    (r.device.name(), r.number)
}

/// デバイスの並びを、同じデバイスで番号が連続した区間 (デバイス, 先頭, 点数) にまとめる
fn runs(refs: &[DeviceRef]) -> Vec<(Device, u32, usize)> {
    let mut sorted = refs.to_vec();
    sorted.sort_unstable_by_key(sort_key);
    sorted.dedup();
    let mut runs: Vec<(Device, u32, usize)> = Vec::new();
    for r in sorted {
        match runs.last_mut() {
            Some((device, start, count))
                if *device == r.device && *start as u64 + *count as u64 == r.number as u64 =>
            {
                *count += 1
            }
            _ => runs.push((r.device, r.number, 1)),
        }
    }
    runs
}

/// (デバイス, 値) の並びを、連続した区間 (デバイス, 先頭, 値の列) にまとめる
fn consecutive_values<T: Copy>(values: &[(DeviceRef, T)]) -> Vec<(Device, u32, Vec<T>)> {
    let lookup: HashMap<DeviceRef, T> = values.iter().copied().collect();
    let refs: Vec<DeviceRef> = values.iter().map(|(r, _)| *r).collect();
    runs(&refs)
        .into_iter()
        .map(|(device, start, count)| {
            let chunk = (start..start + count as u32)
                .map(|n| lookup[&DeviceRef::new(device, n)])
                .collect();
            (device, start, chunk)
        })
        .collect()
}

fn sleep_unless_stopped(duration: Duration, stop: &AtomicBool) {
    let until = Instant::now() + duration;
    while Instant::now() < until && !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::plc::domain::device_map::DeviceArea;
    use crate::features::plc::domain::io::{AddressPlan, Point};
    use plantwright_core::motion::chart::ActuatorKind;
    use plantwright_core::motion::chart::TimingChart;
    use plantwright_core::plc::slmp::mock::MockPlc;

    #[test]
    fn devices_are_grouped_into_runs() {
        let m = |n| DeviceRef::new(Device::M, n);
        let y = |n| DeviceRef::new(Device::Y, n);
        assert_eq!(
            runs(&[m(101), y(0), m(100), m(103), y(1), m(100)]),
            vec![(Device::M, 100, 2), (Device::M, 103, 1), (Device::Y, 0, 2)]
        );
        assert_eq!(
            consecutive_values(&[(m(5), true), (m(6), false), (y(9), true)]),
            vec![
                (Device::M, 5, vec![true, false]),
                (Device::Y, 9, vec![true])
            ]
        );
    }

    /// テスト用の PLC (SLMP サーバー) 相手に、出力が届き、入力が書かれるまで通信する
    fn exchange_with_mock(transport: Transport, frame: FrameKind, encoding: Encoding) {
        let mut chart = TimingChart::default();
        let cylinder = chart.add_actuator(ActuatorKind::Cylinder);
        let axis = chart.add_actuator(ActuatorKind::LinearAxis);
        let plan = AddressPlan::default();
        let table = IoTable::from_chart(&chart, &plan);
        // 出力は Y20〜 と D0〜、入力は X10〜 と D100〜 に置く
        let devices = DeviceMap {
            output_bits: DeviceArea {
                device: Device::Y,
                head: 0x20,
            },
            input_bits: DeviceArea {
                device: Device::X,
                head: 0x10,
            },
            ..Default::default()
        };

        let plc = MockPlc::start(transport, frame, encoding).unwrap();
        // PLC が「シリンダ出」と「軸を 125.0 へ」を出している (1 = 0.1)
        plc.memory().set_bit(Device::Y, 0x20, true);
        plc.memory().set_word(Device::D, 0, 1250);

        let mut link = SlmpLink::start(SlmpSettings {
            host: "127.0.0.1".into(),
            port: plc.port(),
            transport,
            frame,
            encoding,
            poll_ms: 5,
            devices,
        });
        let mut io = IoValues::default();
        io.set_bit(Point::CylinderRetracted(cylinder), true);
        io.set_word(Point::AxisPosition(axis), -50.0);

        let what = format!("{transport:?} {frame:?} {encoding:?}");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !io.bit(Point::CylinderExtend(cylinder)) || plc.memory().word(Device::D, 100) == 0 {
            assert!(
                Instant::now() < deadline,
                "{what} 状態: {:?}",
                link.status()
            );
            link.exchange(&table, &mut io);
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(io.word(Point::AxisTarget(axis)), 125.0, "{what}");
        // 入力ビット: X10 = 出端, X11 = 戻端, X12 = 位置決め完了
        let memory = plc.memory();
        let bits: Vec<bool> = (0x10..0x13).map(|n| memory.bit(Device::X, n)).collect();
        assert_eq!(bits, [false, true, false], "{what}");
        assert_eq!(memory.word(Device::D, 100), (-500i16) as u16, "{what}");
        drop(memory);
        assert!(
            matches!(link.status(), LinkStatus::Connected { .. }),
            "{what}"
        );
    }

    #[test]
    fn exchanges_io_with_an_slmp_server_over_tcp() {
        exchange_with_mock(Transport::Tcp, FrameKind::E3, Encoding::Binary);
        exchange_with_mock(Transport::Tcp, FrameKind::E4, Encoding::Ascii);
    }

    #[test]
    fn exchanges_io_with_an_slmp_server_over_udp() {
        exchange_with_mock(Transport::Udp, FrameKind::E3, Encoding::Ascii);
        exchange_with_mock(Transport::Udp, FrameKind::E4, Encoding::Binary);
    }

    #[test]
    fn unreachable_plc_reports_an_error() {
        // 待ち受けていないポート
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let link = SlmpLink::start(SlmpSettings {
            host: "127.0.0.1".into(),
            port,
            ..Default::default()
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(link.status(), LinkStatus::Error(_)) {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
    }
}
