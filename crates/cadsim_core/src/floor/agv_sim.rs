//! AGV (無人搬送車) の走行経路と配車のシミュレーション
//!
//! - 走行経路は床の上のグラフ (点と、点を結ぶ直線の通路)。通路は両方向に走れる (2車線とみなす)。
//! - 搬送の要求 (「A から B へ」を一定の間隔で) が出ると、空いている AGV のうち
//!   積み込み場所までの道のりがいちばん短いものに割り当てる (先に出た要求から順に)。
//! - 3本以上の通路が集まる点は交差点。交差点には同時に 1台しか入れない (排他)。
//!   交差点の手前で予約し、通り抜けたら解放する。短い通路でつながった交差点はまとめて予約し、
//!   向かい合った AGV がお互いの交差点を持ったまま止まる (デッドロック) のを防ぐ。
//! - 同じ通路を同じ向きに走る AGV は車間を空けて続く。
//! - 台数を変えて同じ条件で走らせ、搬送数・リードタイム・稼働率を比べられる。
//!
//! Bevy に依存しない純粋なデータと計算だけを置く。長さは mm、時間は秒。

use std::collections::{BinaryHeap, HashMap, VecDeque};

use crate::line::AgvEvent;
use glam::DVec2;

/// 経路の点
#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
    pub id: u32,
    pub name: String,
    /// 床の位置 (x, z)
    pub pos: [f64; 2],
    /// 積み降ろしの場所 (ステーション) か
    pub station: bool,
}

/// 走行経路のグラフ
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgvGraph {
    pub nodes: Vec<GraphNode>,
    /// 通路 (点の id の組、向きなし)
    pub edges: Vec<(u32, u32)>,
    pub next_id: u32,
}

impl AgvGraph {
    pub fn add_node(&mut self, pos: [f64; 2], station: bool) -> u32 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        let name = if station {
            format!("ST{id}")
        } else {
            format!("P{id}")
        };
        self.nodes.push(GraphNode {
            id,
            name,
            pos,
            station,
        });
        id
    }

    pub fn node(&self, id: u32) -> Option<&GraphNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: u32) -> Option<&mut GraphNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn pos(&self, id: u32) -> DVec2 {
        self.node(id)
            .map_or(DVec2::ZERO, |n| DVec2::from_array(n.pos))
    }

    pub fn remove_node(&mut self, id: u32) {
        self.nodes.retain(|n| n.id != id);
        self.edges.retain(|&(a, b)| a != id && b != id);
    }

    pub fn has_edge(&self, a: u32, b: u32) -> bool {
        self.edges
            .iter()
            .any(|&(x, y)| (x, y) == (a, b) || (x, y) == (b, a))
    }

    /// 通路があれば消し、なければ足す
    pub fn toggle_edge(&mut self, a: u32, b: u32) {
        if a == b {
            return;
        }
        if self.has_edge(a, b) {
            self.edges
                .retain(|&(x, y)| (x, y) != (a, b) && (x, y) != (b, a));
        } else {
            self.edges.push((a, b));
        }
    }

    pub fn neighbors(&self, id: u32) -> impl Iterator<Item = u32> + '_ {
        self.edges.iter().filter_map(move |&(a, b)| {
            if a == id {
                Some(b)
            } else if b == id {
                Some(a)
            } else {
                None
            }
        })
    }

    /// 3本以上の通路が集まる点 (交差点)
    pub fn is_intersection(&self, id: u32) -> bool {
        self.neighbors(id).count() >= 3
    }

    pub fn edge_length(&self, a: u32, b: u32) -> f64 {
        self.pos(a).distance(self.pos(b))
    }

    pub fn stations(&self) -> Vec<u32> {
        self.nodes
            .iter()
            .filter(|n| n.station)
            .map(|n| n.id)
            .collect()
    }

    /// いちばん短い道のり (通る点の並びと長さ)
    pub fn shortest_path(&self, from: u32, to: u32) -> Option<(Vec<u32>, f64)> {
        self.node(from)?;
        self.node(to)?;
        let mut dist: HashMap<u32, f64> = HashMap::from([(from, 0.0)]);
        let mut prev: HashMap<u32, u32> = HashMap::new();
        let mut heap = BinaryHeap::from([Visit(0.0, from)]);
        while let Some(Visit(d, id)) = heap.pop() {
            if id == to {
                break;
            }
            if d > dist.get(&id).copied().unwrap_or(f64::INFINITY) {
                continue;
            }
            for next in self.neighbors(id) {
                let nd = d + self.edge_length(id, next);
                if nd < dist.get(&next).copied().unwrap_or(f64::INFINITY) {
                    dist.insert(next, nd);
                    prev.insert(next, id);
                    heap.push(Visit(nd, next));
                }
            }
        }
        let total = *dist.get(&to)?;
        let mut path = vec![to];
        let mut at = to;
        while at != from {
            at = prev[&at];
            path.push(at);
        }
        path.reverse();
        Some((path, total))
    }
}

/// ダイクストラ法の候補 (距離の短い順に取り出す)
#[derive(PartialEq)]
struct Visit(f64, u32);

impl Eq for Visit {}

impl PartialOrd for Visit {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Visit {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.0.total_cmp(&self.0).then(other.1.cmp(&self.1))
    }
}

/// 搬送の要求の出方: `from` から `to` へ、`interval` 秒ごとに 1件
#[derive(Clone, Debug, PartialEq)]
pub struct Flow {
    pub from: u32,
    pub to: u32,
    pub interval: f64,
}

/// AGV の性能と台数
#[derive(Clone, Debug, PartialEq)]
pub struct AgvParams {
    pub count: usize,
    /// 走行速度 (mm/s)
    pub speed: f64,
    pub load_time: f64,
    pub unload_time: f64,
    /// 車体の長さ・幅 (mm)
    pub length: f64,
    pub width: f64,
    /// 前の車・交差点との間に空ける距離 (mm)
    pub gap: f64,
}

impl Default for AgvParams {
    fn default() -> Self {
        Self {
            count: 2,
            speed: 1000.0,
            load_time: 10.0,
            unload_time: 10.0,
            length: 1200.0,
            width: 800.0,
            gap: 500.0,
        }
    }
}

impl AgvParams {
    /// 交差点の中心からどれだけ手前で止まって予約するか / 通り過ぎたら解放するか
    fn zone_margin(&self) -> f64 {
        self.length / 2.0 + self.gap
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    /// 決まった間隔の要求なら、その要求の番号
    pub flow: Option<usize>,
    /// ライン連携で頼まれた搬送なら、その荷札 ([`AgvSim::request`])
    pub tag: Option<u64>,
    pub from: u32,
    pub to: u32,
    /// 要求が出た時刻
    pub created: f64,
    /// AGV に割り当てた時刻
    pub assigned: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    ToPickup,
    Loading,
    ToDrop,
    Unloading,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Idle => "待機",
            Phase::ToPickup => "積込へ",
            Phase::Loading => "積込中",
            Phase::ToDrop => "搬送中",
            Phase::Unloading => "荷降ろし中",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vehicle {
    pub id: usize,
    pub phase: Phase,
    /// これから通る点。`route[0]` はいま出発した (または止まっている) 点。
    pub route: Vec<u32>,
    /// `route[0]` から `route[1]` へ進んだ距離
    pub s: f64,
    pub job: Option<Job>,
    timer: f64,
    /// 予約している交差点
    held: Vec<u32>,
    /// 通り過ぎた交差点と、その後に進んだ距離 (解放の判定に使う)
    passed: Vec<(u32, f64)>,
    /// 空車でないときの時間
    pub busy_time: f64,
    /// 交差点や前の車を待って止まっていた時間
    pub blocked_time: f64,
    pub blocked: bool,
    pub distance: f64,
}

impl Vehicle {
    pub fn at_node(&self) -> u32 {
        self.route[0]
    }

    /// 床の位置と向き
    pub fn pose(&self, graph: &AgvGraph) -> (DVec2, DVec2) {
        let a = graph.pos(self.route[0]);
        match self.route.get(1) {
            Some(&next) => {
                let b = graph.pos(next);
                let dir = (b - a).normalize_or(DVec2::X);
                (a + dir * self.s, dir)
            }
            None => (a, DVec2::X),
        }
    }
}

/// 集計
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgvStats {
    pub delivered: usize,
    pub lead_time_sum: f64,
    pub wait_time_sum: f64,
}

/// 実行中のシミュレーション
#[derive(Clone, Debug)]
pub struct AgvSim {
    pub time: f64,
    pub vehicles: Vec<Vehicle>,
    pub pending: VecDeque<Job>,
    pub stats: AgvStats,
    /// ライン連携で頼まれた搬送の出来事 ([`AgvSim::take_events`])
    events: Vec<AgvEvent>,
    /// 交差点 → 予約している AGV
    reservations: HashMap<u32, usize>,
    next_spawn: Vec<f64>,
    graph: AgvGraph,
    flows: Vec<Flow>,
    params: AgvParams,
}

/// 1回の計算で進める最大の時間 (秒)。大きく飛ばすと車間や交差点の判定が粗くなる。
const MAX_STEP: f64 = 0.05;

impl AgvSim {
    /// AGV をステーション (なければすべての点) に順に置いて始める
    pub fn new(graph: &AgvGraph, flows: &[Flow], params: &AgvParams) -> Self {
        let mut homes = graph.stations();
        if homes.is_empty() {
            homes = graph.nodes.iter().map(|n| n.id).collect();
        }
        let mut sim = Self {
            time: 0.0,
            vehicles: Vec::new(),
            pending: VecDeque::new(),
            stats: AgvStats::default(),
            events: Vec::new(),
            reservations: HashMap::new(),
            next_spawn: vec![0.0; flows.len()],
            graph: graph.clone(),
            flows: flows.to_vec(),
            params: params.clone(),
        };
        if homes.is_empty() {
            return sim;
        }
        for id in 0..params.count {
            let home = homes[id % homes.len()];
            let mut held = Vec::new();
            if graph.is_intersection(home) && !sim.reservations.contains_key(&home) {
                sim.reservations.insert(home, id);
                held.push(home);
            }
            sim.vehicles.push(Vehicle {
                id,
                phase: Phase::Idle,
                route: vec![home],
                s: 0.0,
                job: None,
                timer: 0.0,
                held,
                passed: Vec::new(),
                busy_time: 0.0,
                blocked_time: 0.0,
                blocked: false,
                distance: 0.0,
            });
        }
        sim
    }

    pub fn graph(&self) -> &AgvGraph {
        &self.graph
    }

    pub fn params(&self) -> &AgvParams {
        &self.params
    }

    /// 交差点を予約している AGV
    pub fn holder(&self, node: u32) -> Option<usize> {
        self.reservations.get(&node).copied()
    }

    /// ライン連携: `from` で積んで `to` で降ろす搬送を1件頼む。
    /// 積み込み・荷降ろしが終わると `tag` 付きの出来事 ([`AgvSim::take_events`]) を出す。
    pub fn request(&mut self, from: u32, to: u32, tag: u64) {
        self.pending.push_back(Job {
            flow: None,
            tag: Some(tag),
            from,
            to,
            created: self.time,
            assigned: None,
        });
    }

    /// ライン連携で頼まれた搬送の、前回からの出来事を取り出す
    pub fn take_events(&mut self) -> Vec<AgvEvent> {
        std::mem::take(&mut self.events)
    }

    /// 荷を積んで走っている (積み込みが終わってから荷降ろしが終わるまで)
    pub fn is_loaded(v: &Vehicle) -> bool {
        matches!(v.phase, Phase::ToDrop | Phase::Unloading)
    }

    /// `dt` 秒進める。`stopped` が true の間 (安全停止中) は AGV を動かさない。
    pub fn advance(&mut self, dt: f64, stopped: bool) {
        let mut left = dt.max(0.0);
        while left > 1e-12 {
            let step = left.min(MAX_STEP);
            self.step(step, stopped);
            left -= step;
        }
    }

    fn step(&mut self, dt: f64, stopped: bool) {
        self.time += dt;
        self.spawn_jobs();
        self.dispatch();
        for index in 0..self.vehicles.len() {
            if self.vehicles[index].phase != Phase::Idle {
                self.vehicles[index].busy_time += dt;
            }
            if stopped {
                self.vehicles[index].blocked = false;
                continue;
            }
            self.update_vehicle(index, dt);
        }
    }

    fn spawn_jobs(&mut self) {
        for (index, flow) in self.flows.iter().enumerate() {
            if flow.interval <= 0.0 {
                continue;
            }
            while self.next_spawn[index] <= self.time {
                self.pending.push_back(Job {
                    flow: Some(index),
                    tag: None,
                    from: flow.from,
                    to: flow.to,
                    created: self.next_spawn[index],
                    assigned: None,
                });
                self.next_spawn[index] += flow.interval;
            }
        }
    }

    /// 古い要求から順に、積み込み場所に近い空車へ割り当てる
    fn dispatch(&mut self) {
        let mut remaining = VecDeque::new();
        while let Some(mut job) = self.pending.pop_front() {
            let best = self
                .vehicles
                .iter()
                .filter(|v| v.phase == Phase::Idle)
                .filter_map(|v| {
                    let (path, length) = self.graph.shortest_path(v.at_node(), job.from)?;
                    Some((v.id, path, length))
                })
                .min_by(|a, b| a.2.total_cmp(&b.2));
            match best {
                Some((id, path, _)) => {
                    job.assigned = Some(self.time);
                    let v = &mut self.vehicles[id];
                    v.route = path;
                    v.s = 0.0;
                    v.phase = Phase::ToPickup;
                    v.job = Some(job);
                }
                None => remaining.push_back(job),
            }
        }
        self.pending = remaining;
    }

    fn update_vehicle(&mut self, index: usize, dt: f64) {
        let phase = self.vehicles[index].phase;
        match phase {
            Phase::Idle => {}
            Phase::Loading | Phase::Unloading => {
                let v = &mut self.vehicles[index];
                v.timer -= dt;
                if v.timer <= 0.0 {
                    self.finish_station_work(index);
                }
            }
            Phase::ToPickup | Phase::ToDrop => {
                if self.vehicles[index].route.len() < 2 {
                    self.arrive(index);
                } else {
                    self.drive(index, dt);
                }
            }
        }
    }

    fn arrive(&mut self, index: usize) {
        let load = self.params.load_time;
        let unload = self.params.unload_time;
        let v = &mut self.vehicles[index];
        v.s = 0.0;
        match v.phase {
            Phase::ToPickup => {
                v.phase = Phase::Loading;
                v.timer = load;
            }
            Phase::ToDrop => {
                v.phase = Phase::Unloading;
                v.timer = unload;
            }
            _ => {}
        }
    }

    fn finish_station_work(&mut self, index: usize) {
        let time = self.time;
        let phase = self.vehicles[index].phase;
        match phase {
            Phase::Loading => {
                let Some(job) = self.vehicles[index].job.clone() else {
                    self.vehicles[index].phase = Phase::Idle;
                    return;
                };
                let at = self.vehicles[index].at_node();
                let v = &mut self.vehicles[index];
                match self.graph.shortest_path(at, job.to) {
                    Some((path, _)) => {
                        v.route = path;
                        v.phase = Phase::ToDrop;
                        if let Some(tag) = job.tag {
                            self.events.push(AgvEvent::PickedUp { tag });
                        }
                    }
                    // 降ろし先に行けない要求は捨てる
                    None => {
                        v.job = None;
                        v.phase = Phase::Idle;
                        if let Some(tag) = job.tag {
                            self.events.push(AgvEvent::Abandoned { tag });
                        }
                    }
                }
            }
            Phase::Unloading => {
                let v = &mut self.vehicles[index];
                if let Some(job) = v.job.take() {
                    if let Some(tag) = job.tag {
                        self.events.push(AgvEvent::Delivered { tag });
                    }
                    self.stats.delivered += 1;
                    self.stats.lead_time_sum += time - job.created;
                    self.stats.wait_time_sum += job.assigned.unwrap_or(time) - job.created;
                }
                v.phase = Phase::Idle;
            }
            _ => {}
        }
    }

    /// 通路を進める。交差点の予約と車間を守る。
    fn drive(&mut self, index: usize, dt: f64) {
        let margin = self.params.zone_margin();
        let (from, to) = {
            let v = &self.vehicles[index];
            (v.route[0], v.route[1])
        };
        let length = self.graph.edge_length(from, to);
        let s = self.vehicles[index].s;
        let mut limit = s + self.params.speed * dt;

        // 交差点の手前で予約する。取れなければ手前で止まる。
        if self.graph.is_intersection(to) && !self.vehicles[index].held.contains(&to) {
            let stop_at = (length - margin).max(0.0);
            if limit >= stop_at {
                let chain = self.reservation_chain(index);
                if self.try_reserve(index, &chain) {
                    // 予約できた
                } else {
                    limit = limit.min(stop_at.max(s));
                }
            }
        }

        // 同じ通路を同じ向きに走る前の車との車間。
        // ステーションには複数台が並べる (待機列) とみなし、止まっている車は数えない。
        let headway = self.params.length + self.params.gap;
        let next_after = self.vehicles[index].route.get(2).copied();
        for other in &self.vehicles {
            if other.id == index || other.route.len() < 2 {
                continue;
            }
            let ahead = if other.route[0] == from && other.route[1] == to {
                (other.s > s).then_some(other.s)
            } else if other.route[0] == to && Some(other.route[1]) == next_after {
                // 次の通路に出たばかりの車
                Some(length + other.s)
            } else {
                None
            };
            if let Some(position) = ahead {
                limit = limit.min((position - headway).max(s));
            }
        }

        let moved = (limit - s).max(0.0);
        let v = &mut self.vehicles[index];
        v.blocked = moved < self.params.speed * dt * 0.5;
        if v.blocked {
            v.blocked_time += dt;
        }
        v.distance += moved;
        v.s = limit;
        for passed in &mut v.passed {
            passed.1 += moved;
        }
        if v.s >= length - 1e-9 {
            let carry = (v.s - length).max(0.0);
            v.route.remove(0);
            v.s = if v.route.len() >= 2 { carry } else { 0.0 };
            let node = v.route[0];
            if v.held.contains(&node) {
                v.passed.push((node, carry));
            }
        }
        self.release_passed(index);
        if self.vehicles[index].route.len() < 2 {
            self.arrive(index);
        }
    }

    /// これから入る交差点と、短い通路で続く交差点 (まとめて予約する)
    fn reservation_chain(&self, index: usize) -> Vec<u32> {
        let v = &self.vehicles[index];
        let reach = 2.0 * self.params.zone_margin();
        let mut chain = vec![v.route[1]];
        for k in 2..v.route.len() {
            let (a, b) = (v.route[k - 1], v.route[k]);
            if self.graph.is_intersection(b) && self.graph.edge_length(a, b) < reach {
                chain.push(b);
            } else {
                break;
            }
        }
        chain
    }

    fn try_reserve(&mut self, index: usize, chain: &[u32]) -> bool {
        let free = chain.iter().all(|n| {
            self.reservations
                .get(n)
                .is_none_or(|&holder| holder == index)
        });
        if !free {
            return false;
        }
        for &n in chain {
            self.reservations.insert(n, index);
            if !self.vehicles[index].held.contains(&n) {
                self.vehicles[index].held.push(n);
            }
        }
        true
    }

    /// 通り過ぎて車体が出きった交差点を解放する
    fn release_passed(&mut self, index: usize) {
        let margin = self.params.zone_margin();
        let v = &mut self.vehicles[index];
        let mut released = Vec::new();
        v.passed.retain(|&(node, since)| {
            // まだその点に止まっている (経路の終点) なら持ったまま
            let leaving = since >= margin;
            if leaving {
                released.push(node);
            }
            !leaving
        });
        v.held.retain(|n| !released.contains(n));
        for node in released {
            if self.reservations.get(&node) == Some(&index) {
                self.reservations.remove(&node);
            }
        }
    }

    /// 1時間あたりの搬送数
    pub fn throughput_per_hour(&self) -> f64 {
        if self.time <= 0.0 {
            0.0
        } else {
            self.stats.delivered as f64 / self.time * 3600.0
        }
    }

    pub fn average_lead_time(&self) -> f64 {
        if self.stats.delivered == 0 {
            0.0
        } else {
            self.stats.lead_time_sum / self.stats.delivered as f64
        }
    }

    pub fn utilization(&self) -> f64 {
        if self.time <= 0.0 || self.vehicles.is_empty() {
            return 0.0;
        }
        let busy: f64 = self.vehicles.iter().map(|v| v.busy_time).sum();
        busy / (self.time * self.vehicles.len() as f64) * 100.0
    }
}

/// 台数ごとの比較結果
#[derive(Clone, Debug, PartialEq)]
pub struct FleetResult {
    pub count: usize,
    pub delivered: usize,
    pub throughput_per_hour: f64,
    pub average_lead_time: f64,
    pub utilization: f64,
    /// 交差点や前の車を待った時間の割合 (%)
    pub blocked_ratio: f64,
    /// 終了時に残っていた要求の数 (台数が足りないと増える)
    pub backlog: usize,
}

/// 台数を変えて `duration` 秒ずつ走らせて比べる
pub fn compare_fleets(
    graph: &AgvGraph,
    flows: &[Flow],
    params: &AgvParams,
    counts: impl IntoIterator<Item = usize>,
    duration: f64,
) -> Vec<FleetResult> {
    counts
        .into_iter()
        .map(|count| {
            let params = AgvParams {
                count,
                ..params.clone()
            };
            let mut sim = AgvSim::new(graph, flows, &params);
            sim.advance(duration, false);
            let blocked: f64 = sim.vehicles.iter().map(|v| v.blocked_time).sum();
            let busy = sim.time * count.max(1) as f64;
            FleetResult {
                count,
                delivered: sim.stats.delivered,
                throughput_per_hour: sim.throughput_per_hour(),
                average_lead_time: sim.average_lead_time(),
                utilization: sim.utilization(),
                blocked_ratio: if busy > 0.0 {
                    blocked / busy * 100.0
                } else {
                    0.0
                },
                backlog: sim.pending.len()
                    + sim.vehicles.iter().filter(|v| v.job.is_some()).count(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 十字路: 中央の交差点 C から東西南北に 5 m の通路、端はステーション
    fn cross() -> (AgvGraph, [u32; 5]) {
        let mut g = AgvGraph::default();
        let c = g.add_node([0.0, 0.0], false);
        let e = g.add_node([5000.0, 0.0], true);
        let w = g.add_node([-5000.0, 0.0], true);
        let n = g.add_node([0.0, -5000.0], true);
        let s = g.add_node([0.0, 5000.0], true);
        for x in [e, w, n, s] {
            g.toggle_edge(c, x);
        }
        (g, [c, e, w, n, s])
    }

    #[test]
    fn requested_job_reports_pickup_and_delivery_with_its_tag() {
        let (g, [_, e, w, _, _]) = cross();
        let params = AgvParams {
            count: 1,
            ..AgvParams::default()
        };
        let mut sim = AgvSim::new(&g, &[], &params);
        sim.request(e, w, 7);
        let mut events = Vec::new();
        for _ in 0..600 {
            sim.advance(0.5, false);
            events.extend(sim.take_events());
        }
        assert_eq!(
            events,
            vec![
                AgvEvent::PickedUp { tag: 7 },
                AgvEvent::Delivered { tag: 7 }
            ]
        );
        assert_eq!(sim.stats.delivered, 1);
    }

    #[test]
    fn shortest_path_goes_through_center() {
        let (g, [c, e, w, ..]) = cross();
        let (path, length) = g.shortest_path(w, e).unwrap();
        assert_eq!(path, vec![w, c, e]);
        assert!((length - 10000.0).abs() < 1e-9);
        assert!(g.is_intersection(c));
        assert!(!g.is_intersection(e));
    }

    #[test]
    fn single_agv_delivers_in_expected_time() {
        let (g, [_, e, w, ..]) = cross();
        let params = AgvParams {
            count: 1,
            load_time: 5.0,
            unload_time: 5.0,
            ..Default::default()
        };
        // AGV は最初のステーション E にいる。W → E を 1件だけ (間隔を長くする)
        let flows = [Flow {
            from: w,
            to: e,
            interval: 1000.0,
        }];
        let mut sim = AgvSim::new(&g, &flows, &params);
        assert_eq!(sim.vehicles[0].at_node(), e);
        // E → W 10 s、積込 5 s、W → E 10 s、荷降ろし 5 s = 30 s
        sim.advance(29.0, false);
        assert_eq!(sim.stats.delivered, 0);
        sim.advance(1.5, false);
        assert_eq!(sim.stats.delivered, 1);
        // 0.05 秒刻みで進めるので、状態の切り替わりごとに少し遅れる
        assert!(
            (sim.average_lead_time() - 30.0).abs() < 0.2,
            "{}",
            sim.average_lead_time()
        );
    }

    #[test]
    fn only_one_agv_inside_intersection() {
        let (g, [c, e, w, n, s]) = cross();
        let params = AgvParams {
            count: 4,
            load_time: 1.0,
            unload_time: 1.0,
            ..Default::default()
        };
        let flows = [
            Flow {
                from: e,
                to: w,
                interval: 3.0,
            },
            Flow {
                from: n,
                to: s,
                interval: 3.0,
            },
            Flow {
                from: w,
                to: e,
                interval: 3.0,
            },
            Flow {
                from: s,
                to: n,
                interval: 3.0,
            },
        ];
        let mut sim = AgvSim::new(&g, &flows, &params);
        let zone = params.zone_margin() - 1.0;
        for _ in 0..4000 {
            sim.advance(0.05, false);
            let inside: Vec<usize> = sim
                .vehicles
                .iter()
                .filter(|v| v.pose(&g).0.distance(g.pos(c)) < zone)
                .map(|v| v.id)
                .collect();
            assert!(inside.len() <= 1, "t={} inside={inside:?}", sim.time);
            for id in inside {
                assert_eq!(sim.holder(c), Some(id));
            }
        }
        assert!(sim.stats.delivered > 10);
    }

    #[test]
    fn safety_stop_freezes_vehicles() {
        let (g, [_, e, w, ..]) = cross();
        let flows = [Flow {
            from: w,
            to: e,
            interval: 1000.0,
        }];
        let mut sim = AgvSim::new(&g, &flows, &AgvParams::default());
        sim.advance(2.0, false);
        let before = sim.vehicles[0].pose(&g).0;
        sim.advance(5.0, true);
        assert_eq!(sim.vehicles[0].pose(&g).0, before);
    }

    #[test]
    fn more_vehicles_raise_throughput_until_demand_is_met() {
        let (g, [_, e, w, n, s]) = cross();
        let flows = [
            Flow {
                from: e,
                to: w,
                interval: 8.0,
            },
            Flow {
                from: n,
                to: s,
                interval: 8.0,
            },
        ];
        let results = compare_fleets(&g, &flows, &AgvParams::default(), [1, 2, 4], 1800.0);
        assert!(results[1].throughput_per_hour > results[0].throughput_per_hour);
        assert!(results[2].throughput_per_hour >= results[1].throughput_per_hour);
        assert!(results[0].backlog > results[2].backlog);
    }
}
