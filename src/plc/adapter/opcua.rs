//! OPC UA で外部の PLC とつなぐ
//!
//! シミュレーターが OPC UA クライアントになり、I/O 表で「OPC UA ノード」を決めた信号だけをやりとりする。
//! - PLC の出力 (PLC → 設備) のノードは購読 (サブスクリプション) し、値が変わったら受け取る
//! - 設備の状態 (設備 → PLC) は、変わった値だけノードに書き込む
//!
//! 書き込むときはノードの今の値の型 (Boolean / Int16 / Float など) に合わせる。
//! 整数のノードには I/O 表のスケールを掛けた値、実数のノードにはそのままの値を使う。
//! 通信は別スレッドで行い、画面の更新は止めない。切れたら1秒ごとにつなぎ直す。
//! 暗号化なし (SecurityPolicy None)・匿名ログインでつなぐ。

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use opcua::client::{ClientBuilder, DataChangeCallback, IdentityToken, Session};
use opcua::types::{
    AttributeId, DataValue, EndpointDescription, MessageSecurityMode, MonitoredItemCreateRequest,
    NodeId, NumericRange, ReadValueId, StatusCode, TimestampsToReturn, UserTokenPolicy, Variant,
    WriteValue,
};

use crate::plc::domain::io::{IoTable, IoValues};
use crate::plc::usecase::{LinkStatus, PlcLink};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpcUaSettings {
    /// "opc.tcp://192.168.0.10:4840" のようなエンドポイント URL
    pub endpoint: String,
    /// 購読の周期と、書き込みを確かめる周期 (ミリ秒)
    pub interval_ms: u64,
}

impl Default for OpcUaSettings {
    fn default() -> Self {
        Self {
            endpoint: "opc.tcp://192.168.0.10:4840".into(),
            interval_ms: 50,
        }
    }
}

/// 1つの信号の値 (I/O 表の種類に合わせた形)
#[derive(Clone, Copy, Debug, PartialEq)]
enum Signal {
    Bit(bool),
    Word { value: f64, scale: f64 },
}

/// 画面側と通信スレッドで共有するもの
#[derive(Default)]
struct Shared {
    /// 購読するノード (PLC の出力)
    watch: Vec<String>,
    /// 書き込みたい値 (設備の状態)。ノード → 値
    writes: HashMap<String, Signal>,
    /// 購読で受け取った最新の値。ノード → 値
    received: HashMap<String, Variant>,
    status: LinkStatus,
}

/// OPC UA クライアント。作ると通信スレッドが動き出し、捨てると止まる。
pub struct OpcUaClient {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl OpcUaClient {
    pub fn start(settings: OpcUaSettings) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(communicate(settings, shared, stop)),
                    Err(e) => lock(&shared).status = LinkStatus::Error(e.to_string()),
                }
            })
        };
        Self {
            shared,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for OpcUaClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl PlcLink for OpcUaClient {
    fn exchange(&mut self, table: &IoTable, io: &mut IoValues) {
        let mut shared = lock(&self.shared);
        let mut watch = Vec::new();
        for p in table.points.iter().filter(|p| !p.node.is_empty()) {
            if p.point.is_output() {
                watch.push(p.node.clone());
                if let Some(signal) = shared
                    .received
                    .get(&p.node)
                    .and_then(|v| to_signal(v, p.point.is_word(), p.scale))
                {
                    match signal {
                        Signal::Bit(on) => io.set_bit(p.point, on),
                        Signal::Word { value, .. } => io.set_word(p.point, value),
                    }
                }
            } else {
                let signal = if p.point.is_word() {
                    Signal::Word {
                        value: io.word(p.point),
                        scale: p.scale,
                    }
                } else {
                    Signal::Bit(io.bit(p.point))
                };
                shared.writes.insert(p.node.clone(), signal);
            }
        }
        shared.watch = watch;
    }

    fn status(&self) -> LinkStatus {
        lock(&self.shared).status.clone()
    }
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    // 通信スレッドが panic しても画面側は動き続けたいので、毒された Mutex も使う
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

/// 通信スレッドの本体。stop が立つまで、つなぐ → 購読と書き込みを続ける → 切れたらつなぎ直す。
async fn communicate(settings: OpcUaSettings, shared: Arc<Mutex<Shared>>, stop: Arc<AtomicBool>) {
    let set_status = |status| lock(&shared).status = status;
    while !stop.load(Ordering::Relaxed) {
        set_status(LinkStatus::Connecting);
        let (session, handle) = match connect(&settings).await {
            Ok(connection) => connection,
            Err(e) => {
                set_status(LinkStatus::Error(format!("接続できません: {e}")));
                tokio::time::sleep(RETRY_INTERVAL).await;
                continue;
            }
        };
        if let Err(e) = run_session(&settings, &session, &shared, &stop, &handle).await {
            set_status(LinkStatus::Error(e));
            tokio::time::sleep(RETRY_INTERVAL).await;
        }
        let _ = session.disconnect().await;
        handle.abort();
    }
    set_status(LinkStatus::Disconnected);
}

/// つながったセッションと、その裏で動く通信ループ
type Connection = (Arc<Session>, tokio::task::JoinHandle<StatusCode>);

async fn connect(settings: &OpcUaSettings) -> Result<Connection, String> {
    let mut client = ClientBuilder::new()
        .application_name("cadsim")
        .application_uri("urn:cadsim")
        .create_sample_keypair(false)
        .trust_server_certs(true)
        // 証明書の置き場所。暗号化なしでも作られるので、作業フォルダを汚さないよう一時フォルダにする
        .pki_dir(std::env::temp_dir().join("cadsim-opcua-pki"))
        .session_retry_limit(0)
        .client()
        .map_err(|e| e.join(", "))?;
    let endpoint: EndpointDescription = (
        settings.endpoint.as_str(),
        "None",
        MessageSecurityMode::None,
        UserTokenPolicy::anonymous(),
    )
        .into();
    let connecting = client.connect_to_matching_endpoint(endpoint, IdentityToken::Anonymous);
    let (session, event_loop) = tokio::time::timeout(CONNECT_TIMEOUT, connecting)
        .await
        .map_err(|_| "応答がありません".to_string())?
        .map_err(|e| e.to_string())?;
    Ok((session, event_loop.spawn()))
}

/// つながっている間、購読するノードの変化に合わせて購読を作り直し、変わった入力を書き込む
async fn run_session(
    settings: &OpcUaSettings,
    session: &Arc<Session>,
    shared: &Arc<Mutex<Shared>>,
    stop: &AtomicBool,
    event_loop: &tokio::task::JoinHandle<StatusCode>,
) -> Result<(), String> {
    let interval = Duration::from_millis(settings.interval_ms.max(10));
    if !tokio::time::timeout(CONNECT_TIMEOUT, session.wait_for_connection())
        .await
        .unwrap_or(false)
    {
        return Err("セッションを開けません".into());
    }

    let mut subscribed: Vec<String> = Vec::new();
    let mut subscription_id = None;
    // ノードの型 (書き込むときに合わせる) と、最後に書いた値
    let mut node_types: HashMap<String, Variant> = HashMap::new();
    let mut written: HashMap<String, Variant> = HashMap::new();

    while !stop.load(Ordering::Relaxed) {
        if event_loop.is_finished() {
            return Err("接続が切れました".into());
        }
        let started = tokio::time::Instant::now();
        let (watch, writes) = {
            let s = lock(shared);
            (s.watch.clone(), s.writes.clone())
        };

        if watch != subscribed {
            if let Some(id) = subscription_id.take() {
                let _ = session.delete_subscription(id).await;
            }
            if !watch.is_empty() {
                subscription_id = Some(subscribe(session, shared, &watch, interval).await?);
            }
            subscribed = watch;
        }

        // 型がまだ分からないノードを読む
        let unknown: Vec<&String> = writes
            .keys()
            .filter(|n| !node_types.contains_key(*n))
            .collect();
        if !unknown.is_empty() {
            let ids = unknown
                .iter()
                .map(|n| parse_node(n).map(ReadValueId::from))
                .collect::<Result<Vec<_>, _>>()?;
            let values = session
                .read(&ids, TimestampsToReturn::Neither, 0.0)
                .await
                .map_err(|e| format!("読み出しエラー: {e}"))?;
            for (node, value) in unknown.into_iter().zip(values) {
                node_types.insert(node.clone(), value.value.unwrap_or_default());
            }
        }

        let mut to_write = Vec::new();
        for (node, signal) in &writes {
            let Some(variant) = node_types.get(node).and_then(|t| to_variant(*signal, t)) else {
                continue;
            };
            if written.get(node) != Some(&variant) {
                to_write.push(WriteValue {
                    node_id: parse_node(node)?,
                    attribute_id: AttributeId::Value as u32,
                    index_range: NumericRange::None,
                    value: DataValue::value_only(variant.clone()),
                });
                written.insert(node.clone(), variant);
            }
        }
        if !to_write.is_empty() {
            let results = session
                .write(&to_write)
                .await
                .map_err(|e| format!("書き込みエラー: {e}"))?;
            if let Some((w, status)) = to_write.iter().zip(&results).find(|(_, s)| !s.is_good()) {
                written.remove(&w.node_id.to_string());
                lock(shared).status =
                    LinkStatus::Error(format!("{} に書き込めません: {status}", w.node_id));
                tokio::time::sleep(interval).await;
                continue;
            }
        }

        lock(shared).status = LinkStatus::Connected {
            cycle_ms: started.elapsed().as_secs_f64() * 1000.0,
        };
        tokio::time::sleep(interval.saturating_sub(started.elapsed())).await;
    }
    Ok(())
}

/// PLC の出力のノードを購読し、変わった値を `shared.received` に入れる
async fn subscribe(
    session: &Arc<Session>,
    shared: &Arc<Mutex<Shared>>,
    nodes: &[String],
    interval: Duration,
) -> Result<u32, String> {
    let received = Arc::clone(shared);
    let callback = DataChangeCallback::new(move |value, item| {
        if let Some(v) = value.value {
            let node = item.item_to_monitor().node_id.to_string();
            lock(&received).received.insert(node, v);
        }
    });
    let id = session
        .create_subscription(interval, 30, 10, 0, 0, true, callback)
        .await
        .map_err(|e| format!("購読を作れません: {e}"))?;
    let items = nodes
        .iter()
        .map(|n| parse_node(n).map(MonitoredItemCreateRequest::from))
        .collect::<Result<Vec<_>, _>>()?;
    session
        .create_monitored_items(id, TimestampsToReturn::Neither, items)
        .await
        .map_err(|e| format!("購読するノードを登録できません: {e}"))?;
    Ok(id)
}

fn parse_node(node: &str) -> Result<NodeId, String> {
    NodeId::from_str(node).map_err(|_| format!("ノード ID「{node}」を読めません"))
}

/// 受け取った値を信号の値にする。整数はスケールを掛け、実数はそのまま使う。
fn to_signal(variant: &Variant, is_word: bool, scale: f64) -> Option<Signal> {
    let number = |v: f64, integer: bool| {
        Some(if is_word {
            Signal::Word {
                value: if integer { v * scale } else { v },
                scale,
            }
        } else {
            Signal::Bit(v != 0.0)
        })
    };
    match *variant {
        Variant::Boolean(b) if is_word => number(if b { 1.0 } else { 0.0 }, true),
        Variant::Boolean(b) => Some(Signal::Bit(b)),
        Variant::SByte(v) => number(v as f64, true),
        Variant::Byte(v) => number(v as f64, true),
        Variant::Int16(v) => number(v as f64, true),
        Variant::UInt16(v) => number(v as f64, true),
        Variant::Int32(v) => number(v as f64, true),
        Variant::UInt32(v) => number(v as f64, true),
        Variant::Int64(v) => number(v as f64, true),
        Variant::UInt64(v) => number(v as f64, true),
        Variant::Float(v) => number(v as f64, false),
        Variant::Double(v) => number(v, false),
        _ => None,
    }
}

/// 信号の値を、ノードの今の値 (`like`) と同じ型にする。対応しない型なら None。
fn to_variant(signal: Signal, like: &Variant) -> Option<Variant> {
    let (raw, real) = match signal {
        Signal::Bit(on) => (if on { 1.0 } else { 0.0 }, if on { 1.0 } else { 0.0 }),
        Signal::Word { value, scale } => ((value / scale).round(), value),
    };
    // 整数の型の範囲に丸める
    macro_rules! int {
        ($t:ty, $variant:ident) => {
            Some(Variant::$variant(
                raw.clamp(<$t>::MIN as f64, <$t>::MAX as f64) as $t,
            ))
        };
    }
    match like {
        Variant::Boolean(_) => Some(Variant::Boolean(raw != 0.0)),
        Variant::SByte(_) => int!(i8, SByte),
        Variant::Byte(_) => int!(u8, Byte),
        Variant::Int16(_) => int!(i16, Int16),
        Variant::UInt16(_) => int!(u16, UInt16),
        Variant::Int32(_) => int!(i32, Int32),
        Variant::UInt32(_) => int!(u32, UInt32),
        Variant::Int64(_) => int!(i64, Int64),
        Variant::UInt64(_) => int!(u64, UInt64),
        Variant::Float(_) => Some(Variant::Float(real as f32)),
        Variant::Double(_) => Some(Variant::Double(real)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_nodes_use_the_scale_and_real_nodes_do_not() {
        let word = |value| Signal::Word { value, scale: 0.1 };
        assert_eq!(
            to_signal(&Variant::Int16(1250), true, 0.1),
            Some(word(125.0))
        );
        assert_eq!(
            to_signal(&Variant::Double(12.5), true, 0.1),
            Some(word(12.5))
        );
        assert_eq!(
            to_variant(word(125.0), &Variant::Int32(0)),
            Some(Variant::Int32(1250))
        );
        assert_eq!(
            to_variant(word(12.5), &Variant::Float(0.0)),
            Some(Variant::Float(12.5))
        );
    }

    #[test]
    fn bits_follow_the_node_type() {
        assert_eq!(
            to_signal(&Variant::Boolean(true), false, 1.0),
            Some(Signal::Bit(true))
        );
        assert_eq!(
            to_signal(&Variant::UInt16(0), false, 1.0),
            Some(Signal::Bit(false))
        );
        assert_eq!(
            to_variant(Signal::Bit(true), &Variant::Boolean(false)),
            Some(Variant::Boolean(true))
        );
        assert_eq!(
            to_variant(Signal::Bit(true), &Variant::Int16(0)),
            Some(Variant::Int16(1))
        );
    }

    #[test]
    fn out_of_range_values_are_clamped_and_strings_are_skipped() {
        let big = Signal::Word {
            value: 1e9,
            scale: 1.0,
        };
        assert_eq!(
            to_variant(big, &Variant::Int16(0)),
            Some(Variant::Int16(i16::MAX))
        );
        assert_eq!(to_variant(big, &Variant::from("text")), None);
        assert_eq!(to_signal(&Variant::from("text"), true, 1.0), None);
    }

    #[test]
    fn node_ids_are_parsed() {
        assert!(parse_node("ns=2;s=Line1.Lifter.Out").is_ok());
        assert!(parse_node("ns=3;i=1001").is_ok());
        assert!(parse_node("not a node").is_err());
    }

    #[test]
    fn unreachable_server_reports_an_error() {
        // 使われていないポートにつなぎ、エラーになって画面側が止まらないことを確かめる
        let client = OpcUaClient::start(OpcUaSettings {
            endpoint: "opc.tcp://127.0.0.1:1/".into(),
            interval_ms: 20,
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !matches!(client.status(), LinkStatus::Error(_)) {
            assert!(
                std::time::Instant::now() < deadline,
                "{:?}",
                client.status()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// 本物の OPC UA サーバーとやりとりする (手で実行する)。
    /// サーバーに ns=2 の Cyl.Out (Boolean)、Cyl.Ext (Boolean)、Axis.Target (Int16 = 1250)、
    /// Axis.Pos (Float) を用意し、`CADSIM_OPCUA_ENDPOINT=opc.tcp://… cargo test -- --ignored opcua` で動かす。
    #[test]
    #[ignore]
    fn exchanges_io_with_a_real_server() {
        use crate::motion::{ActuatorKind, TimingChart};
        use crate::plc::domain::io::{AddressPlan, Point, PointSettings};

        let endpoint = std::env::var("CADSIM_OPCUA_ENDPOINT").expect("CADSIM_OPCUA_ENDPOINT");
        let mut chart = TimingChart::default();
        let cyl = chart.add_actuator(ActuatorKind::Cylinder);
        let axis = chart.add_actuator(ActuatorKind::LinearAxis);
        let mut plan = AddressPlan::default();
        for (point, node) in [
            (Point::CylinderExtend(cyl), "ns=2;s=Cyl.Out"),
            (Point::CylinderExtended(cyl), "ns=2;s=Cyl.Ext"),
            (Point::AxisTarget(axis), "ns=2;s=Axis.Target"),
            (Point::AxisPosition(axis), "ns=2;s=Axis.Pos"),
        ] {
            let settings = PointSettings {
                node: node.into(),
                ..Default::default()
            };
            plan.custom.insert(point, settings);
        }
        let table = IoTable::from_chart(&chart, &plan);

        let mut client = OpcUaClient::start(OpcUaSettings {
            endpoint,
            interval_ms: 20,
        });
        let mut io = IoValues::default();
        io.set_bit(Point::CylinderExtended(cyl), true);
        io.set_word(Point::AxisPosition(axis), 42.5);
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !(io.bit(Point::CylinderExtend(cyl)) && io.word(Point::AxisTarget(axis)) == 125.0) {
            assert!(
                std::time::Instant::now() < deadline,
                "{:?}",
                client.status()
            );
            client.exchange(&table, &mut io);
            thread::sleep(Duration::from_millis(20));
        }
        // 書き込みが終わるまで少し回す (サーバー側のログで Cyl.Ext=True, Axis.Pos=42.5 を確かめる)
        for _ in 0..50 {
            client.exchange(&table, &mut io);
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            matches!(client.status(), LinkStatus::Connected { .. }),
            "{:?}",
            client.status()
        );
    }
}
