//! Modbus TCP で実機の PLC とつなぐ
//!
//! シミュレーターがクライアント (マスター) になり、PLC (サーバー / スレーブ) に周期的に
//! - PLC の出力 (コイル・保持レジスタ) を読む
//! - 設備の状態 (端のスイッチ・現在位置) をコイル・保持レジスタに書く
//!
//! 通信は別スレッドで行い、画面の更新は止めない。切れたら1秒ごとにつなぎ直す。

use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tokio_modbus::Slave;
use tokio_modbus::client::sync::{self, Reader, Writer};

use crate::plc::domain::io::{IoTable, IoValues};
use crate::plc::usecase::{LinkStatus, PlcLink};

/// 1回の要求で読み書きする最大の点数 (Modbus の上限 123〜2000 点より控えめにする)
const MAX_PER_REQUEST: usize = 120;
const TIMEOUT: Duration = Duration::from_secs(1);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModbusSettings {
    pub host: String,
    pub port: u16,
    /// ユニット ID (スレーブ番号)
    pub unit_id: u8,
    /// 通信の周期 (ミリ秒)
    pub poll_ms: u64,
}

impl Default for ModbusSettings {
    fn default() -> Self {
        Self {
            host: "192.168.3.39".into(),
            port: 502,
            unit_id: 1,
            poll_ms: 20,
        }
    }
}

/// 1回の通信でやること (どの番号を読み、何を書くか)
#[derive(Clone, Debug, Default, PartialEq)]
struct Request {
    read_coils: Vec<u16>,
    read_registers: Vec<u16>,
    write_coils: Vec<(u16, bool)>,
    write_registers: Vec<(u16, u16)>,
}

/// 読んだ結果
#[derive(Clone, Debug, Default)]
struct Readback {
    coils: HashMap<u16, bool>,
    registers: HashMap<u16, u16>,
}

/// 画面側と通信スレッドで共有するもの
#[derive(Default)]
struct Shared {
    request: Request,
    readback: Readback,
    status: LinkStatus,
}

/// Modbus TCP クライアント。作ると通信スレッドが動き出し、捨てると止まる。
pub struct ModbusClient {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ModbusClient {
    pub fn start(settings: ModbusSettings) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            thread::spawn(move || communicate(&settings, &shared, &stop))
        };
        Self {
            shared,
            stop,
            thread: Some(thread),
        }
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        // 通信スレッドが panic しても画面側は動き続けたいので、毒された Mutex も使う
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for ModbusClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl PlcLink for ModbusClient {
    fn exchange(&mut self, table: &IoTable, io: &mut IoValues) {
        let mut request = Request::default();
        for p in &table.points {
            match (p.point.is_output(), p.point.is_word()) {
                (true, false) => request.read_coils.push(p.address),
                (true, true) => request.read_registers.push(p.address),
                (false, false) => request.write_coils.push((p.address, io.bit(p.point))),
                (false, true) => request
                    .write_registers
                    .push((p.address, p.encode_word(io.word(p.point)))),
            }
        }

        let mut shared = self.shared();
        shared.request = request;
        // まだ一度も読めていない出力は OFF / 0 のままにする
        for p in table.points.iter().filter(|p| p.point.is_output()) {
            if p.point.is_word() {
                if let Some(&raw) = shared.readback.registers.get(&p.address) {
                    io.set_word(p.point, p.decode_word(raw));
                }
            } else if let Some(&on) = shared.readback.coils.get(&p.address) {
                io.set_bit(p.point, on);
            }
        }
    }

    fn status(&self) -> LinkStatus {
        self.shared().status.clone()
    }
}

/// 通信スレッドの本体。stop が立つまで、つなぐ → 周期的に読み書き → 切れたらつなぎ直す。
fn communicate(settings: &ModbusSettings, shared: &Mutex<Shared>, stop: &AtomicBool) {
    let set_status = |status| shared.lock().unwrap_or_else(|e| e.into_inner()).status = status;
    let poll = Duration::from_millis(settings.poll_ms.max(1));

    while !stop.load(Ordering::Relaxed) {
        set_status(LinkStatus::Connecting);
        let mut ctx = match connect(settings) {
            Ok(ctx) => ctx,
            Err(e) => {
                set_status(LinkStatus::Error(format!("接続できません: {e}")));
                sleep_unless_stopped(RETRY_INTERVAL, stop);
                continue;
            }
        };
        while !stop.load(Ordering::Relaxed) {
            let started = Instant::now();
            let request = shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .request
                .clone();
            match transfer(&mut ctx, &request) {
                Ok(readback) => {
                    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                    s.readback = readback;
                    s.status = LinkStatus::Connected {
                        cycle_ms: started.elapsed().as_secs_f64() * 1000.0,
                    };
                }
                Err(e) => {
                    set_status(LinkStatus::Error(e));
                    sleep_unless_stopped(RETRY_INTERVAL, stop);
                    break;
                }
            }
            thread::sleep(poll.saturating_sub(started.elapsed()));
        }
    }
    set_status(LinkStatus::Disconnected);
}

fn connect(settings: &ModbusSettings) -> Result<sync::Context, String> {
    let address: SocketAddr = (settings.host.as_str(), settings.port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("アドレスが見つかりません")?;
    sync::tcp::connect_slave_with_timeout(address, Slave(settings.unit_id), Some(TIMEOUT))
        .map_err(|e| e.to_string())
}

/// 1周期分の読み書き。連続した番号はまとめて1回の要求にする。
fn transfer(ctx: &mut sync::Context, request: &Request) -> Result<Readback, String> {
    let mut readback = Readback::default();
    for (start, count) in runs(&request.read_coils) {
        let values = flatten(ctx.read_coils(start, count))?;
        readback.coils.extend((start..).zip(values));
    }
    for (start, count) in runs(&request.read_registers) {
        let values = flatten(ctx.read_holding_registers(start, count))?;
        readback.registers.extend((start..).zip(values));
    }
    for chunk in consecutive_values(&request.write_coils) {
        flatten(ctx.write_multiple_coils(chunk.0, &chunk.1))?;
    }
    for chunk in consecutive_values(&request.write_registers) {
        flatten(ctx.write_multiple_registers(chunk.0, &chunk.1))?;
    }
    Ok(readback)
}

/// tokio-modbus の「通信エラー」と「PLC からの例外応答」をまとめて文字列のエラーにする
fn flatten<T>(result: tokio_modbus::Result<T>) -> Result<T, String> {
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(exception)) => Err(format!("PLC が要求を拒否しました: {exception}")),
        Err(e) => Err(format!("通信エラー: {e}")),
    }
}

/// 番号の並びを、連続した区間 (先頭, 個数) にまとめる
fn runs(addresses: &[u16]) -> Vec<(u16, u16)> {
    let mut sorted = addresses.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut runs: Vec<(u16, u16)> = Vec::new();
    for address in sorted {
        match runs.last_mut() {
            Some((start, count))
                if *start as u32 + *count as u32 == address as u32
                    && (*count as usize) < MAX_PER_REQUEST =>
            {
                *count += 1
            }
            _ => runs.push((address, 1)),
        }
    }
    runs
}

/// (番号, 値) の並びを、番号が連続した区間 (先頭, 値の列) にまとめる
fn consecutive_values<T: Copy>(values: &[(u16, T)]) -> Vec<(u16, Vec<T>)> {
    let lookup: HashMap<u16, T> = values.iter().copied().collect();
    let addresses: Vec<u16> = values.iter().map(|(a, _)| *a).collect();
    runs(&addresses)
        .into_iter()
        .map(|(start, count)| {
            let chunk = (start..start + count).map(|a| lookup[&a]).collect();
            (start, chunk)
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

    #[test]
    fn contiguous_addresses_are_grouped() {
        assert_eq!(runs(&[3, 1, 2, 7, 8, 100]), vec![(1, 3), (7, 2), (100, 1)]);
        assert_eq!(
            consecutive_values(&[(5, true), (6, false), (9, true)]),
            vec![(5, vec![true, false]), (9, vec![true])]
        );
    }

    #[test]
    fn long_runs_are_split() {
        let addresses: Vec<u16> = (0..250).collect();
        assert_eq!(runs(&addresses), vec![(0, 120), (120, 120), (240, 10)]);
    }
}

/// 本物の Modbus TCP サーバー (PLC の代わり) を立てて、読み書きが届くことを確かめる
#[cfg(test)]
mod server_tests {
    use std::future::{self, Ready};
    use std::sync::{Arc, Mutex};

    use tokio::net::TcpListener;
    use tokio_modbus::server::Service;
    use tokio_modbus::server::tcp::{Server, accept_tcp_connection};
    use tokio_modbus::{ExceptionCode, Request, Response};

    use super::*;
    use crate::motion::{ActuatorKind, TimingChart};
    use crate::plc::domain::io::{AddressPlan, Point};

    /// コイルと保持レジスタを 200 点ずつ持つだけの PLC
    #[derive(Clone, Default)]
    struct FakePlc {
        coils: Arc<Mutex<Vec<bool>>>,
        registers: Arc<Mutex<Vec<u16>>>,
    }

    impl Service for FakePlc {
        type Request = Request<'static>;
        type Response = Response;
        type Exception = ExceptionCode;
        type Future = Ready<Result<Response, ExceptionCode>>;

        fn call(&self, req: Request<'static>) -> Self::Future {
            let mut coils = self.coils.lock().unwrap();
            let mut registers = self.registers.lock().unwrap();
            let range = |start: u16, count: usize| start as usize..start as usize + count;
            let response = match req {
                Request::ReadCoils(a, n) => {
                    Response::ReadCoils(coils[range(a, n as usize)].to_vec())
                }
                Request::ReadHoldingRegisters(a, n) => {
                    Response::ReadHoldingRegisters(registers[range(a, n as usize)].to_vec())
                }
                Request::WriteMultipleCoils(a, values) => {
                    coils[range(a, values.len())].copy_from_slice(&values);
                    Response::WriteMultipleCoils(a, values.len() as u16)
                }
                Request::WriteMultipleRegisters(a, values) => {
                    registers[range(a, values.len())].copy_from_slice(&values);
                    Response::WriteMultipleRegisters(a, values.len() as u16)
                }
                _ => return future::ready(Err(ExceptionCode::IllegalFunction)),
            };
            future::ready(Ok(response))
        }
    }

    /// 別スレッドでサーバーを動かし、待ち受けているポートを返す
    fn start_server(plc: FakePlc) -> u16 {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            runtime.block_on(async move {
                let new_service = |_| Ok(Some(plc.clone()));
                let on_connected =
                    |stream, addr| async move { accept_tcp_connection(stream, addr, new_service) };
                let _ = Server::new(listener).serve(&on_connected, |_| {}).await;
            });
        });
        port
    }

    #[test]
    fn exchanges_io_with_a_modbus_server() {
        let plc = FakePlc {
            coils: Arc::new(Mutex::new(vec![false; 200])),
            registers: Arc::new(Mutex::new(vec![0; 200])),
        };
        let port = start_server(plc.clone());

        let mut chart = TimingChart::default();
        let cylinder = chart.add_actuator(ActuatorKind::Cylinder);
        let axis = chart.add_actuator(ActuatorKind::LinearAxis);
        let table = IoTable::from_chart(&chart, &AddressPlan::default());

        // PLC が「シリンダ出」と「軸を 125.0 へ」を出している (1 = 0.1)
        plc.coils.lock().unwrap()[0] = true;
        plc.registers.lock().unwrap()[0] = 1250;

        let mut client = ModbusClient::start(ModbusSettings {
            host: "127.0.0.1".into(),
            port,
            unit_id: 1,
            poll_ms: 5,
        });
        let mut io = IoValues::default();
        io.set_bit(Point::CylinderRetracted(cylinder), true);
        io.set_word(Point::AxisPosition(axis), -50.0);

        let deadline = Instant::now() + Duration::from_secs(5);
        while !io.bit(Point::CylinderExtend(cylinder)) || plc.registers.lock().unwrap()[100] == 0 {
            assert!(Instant::now() < deadline, "状態: {:?}", client.status());
            client.exchange(&table, &mut io);
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(io.word(Point::AxisTarget(axis)), 125.0);
        // 入力ビット: 100 = 出端, 101 = 戻端, 102 = 位置決め完了
        assert_eq!(plc.coils.lock().unwrap()[100..103], [false, true, false]);
        assert_eq!(plc.registers.lock().unwrap()[100], (-500i16) as u16);
        assert!(matches!(client.status(), LinkStatus::Connected { .. }));
    }
}
