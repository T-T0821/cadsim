//! 安全機器・作業者・AGV をプロジェクトファイル (.cadsim) に書く形 (Bevy に依存しない)
//!
//! ドメインの型 (`SafetyLayout`・`Operator`・`AgvGraph` など) には serde を付けず、ここで写す。
//! 長さは mm、時間は秒。

use serde::{Deserialize, Serialize};

use crate::floor::agv_sim::{AgvGraph, AgvParams, Flow, GraphNode};
use crate::floor::operator_plan::{Operator, OperatorTask};
use crate::safety::device::{DeviceKind, SafetyDevice, SafetyLayout};

/// 床の上の配置一式
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct FloorRecord {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub safety: Vec<SafetyDeviceRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operators: Vec<OperatorRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agv: Option<AgvRecord>,
}

impl FloorRecord {
    pub fn is_empty(&self) -> bool {
        self.safety.is_empty() && self.operators.is_empty() && self.agv.is_none()
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKindRecord {
    LightCurtain,
    AreaSensor,
    Fence,
    EmergencyStop,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SafetyDeviceRecord {
    pub id: u32,
    pub name: String,
    pub kind: DeviceKindRecord,
    pub position: [f64; 3],
    #[serde(default)]
    pub yaw_deg: f64,
    pub size: [f64; 3],
    pub stop_time: f64,
    pub resolution: f64,
    pub required_clearance: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OperatorRecord {
    pub id: u32,
    pub name: String,
    pub start: [f64; 2],
    pub walk_speed: f64,
    #[serde(default)]
    pub tasks: Vec<TaskRecord>,
}

/// 作業者の手順。JSON では `"type"` で見分ける。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum TaskRecord {
    Walk { to: [f64; 2] },
    Pick { label: String, duration: f64 },
    Button { label: String, duration: f64 },
    Wait { duration: f64 },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AgvRecord {
    pub nodes: Vec<AgvNodeRecord>,
    #[serde(default)]
    pub edges: Vec<[u32; 2]>,
    #[serde(default)]
    pub flows: Vec<FlowRecord>,
    pub count: usize,
    pub speed: f64,
    pub load_time: f64,
    pub unload_time: f64,
    pub length: f64,
    pub width: f64,
    pub gap: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AgvNodeRecord {
    pub id: u32,
    pub name: String,
    pub pos: [f64; 2],
    #[serde(default)]
    pub station: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FlowRecord {
    pub from: u32,
    pub to: u32,
    pub interval: f64,
}

/// 開き直したあとの配置
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FloorLayout {
    pub safety: SafetyLayout,
    pub operators: Vec<Operator>,
    pub graph: AgvGraph,
    pub flows: Vec<Flow>,
    pub params: AgvParams,
}

impl FloorRecord {
    /// 配置からファイルの内容を作る
    pub fn capture(
        safety: &SafetyLayout,
        operators: &[Operator],
        graph: &AgvGraph,
        flows: &[Flow],
        params: &AgvParams,
    ) -> Self {
        let agv_used = !graph.nodes.is_empty() || !flows.is_empty();
        Self {
            safety: safety.devices.iter().map(device_record).collect(),
            operators: operators.iter().map(operator_record).collect(),
            agv: agv_used.then(|| AgvRecord {
                nodes: graph
                    .nodes
                    .iter()
                    .map(|n| AgvNodeRecord {
                        id: n.id,
                        name: n.name.clone(),
                        pos: n.pos,
                        station: n.station,
                    })
                    .collect(),
                edges: graph.edges.iter().map(|&(a, b)| [a, b]).collect(),
                flows: flows
                    .iter()
                    .map(|f| FlowRecord {
                        from: f.from,
                        to: f.to,
                        interval: f.interval,
                    })
                    .collect(),
                count: params.count,
                speed: params.speed,
                load_time: params.load_time,
                unload_time: params.unload_time,
                length: params.length,
                width: params.width,
                gap: params.gap,
            }),
        }
    }

    /// ファイルの内容から配置を作り直す。次に振る id は、使われている id の次からにする。
    pub fn restore(&self) -> FloorLayout {
        let devices: Vec<SafetyDevice> = self.safety.iter().map(device_from_record).collect();
        let safety = SafetyLayout {
            next_id: devices.iter().map(|d| d.id + 1).max().unwrap_or(1),
            devices,
        };
        let operators = self.operators.iter().map(operator_from_record).collect();
        let mut layout = FloorLayout {
            safety,
            operators,
            ..Default::default()
        };
        if let Some(agv) = &self.agv {
            layout.graph = AgvGraph {
                nodes: agv
                    .nodes
                    .iter()
                    .map(|n| GraphNode {
                        id: n.id,
                        name: n.name.clone(),
                        pos: n.pos,
                        station: n.station,
                    })
                    .collect(),
                edges: agv.edges.iter().map(|&[a, b]| (a, b)).collect(),
                next_id: agv.nodes.iter().map(|n| n.id + 1).max().unwrap_or(1),
            };
            layout.flows = agv
                .flows
                .iter()
                .map(|f| Flow {
                    from: f.from,
                    to: f.to,
                    interval: f.interval,
                })
                .collect();
            layout.params = AgvParams {
                count: agv.count,
                speed: agv.speed,
                load_time: agv.load_time,
                unload_time: agv.unload_time,
                length: agv.length,
                width: agv.width,
                gap: agv.gap,
            };
        }
        layout
    }
}

fn device_record(d: &SafetyDevice) -> SafetyDeviceRecord {
    SafetyDeviceRecord {
        id: d.id,
        name: d.name.clone(),
        kind: match d.kind {
            DeviceKind::LightCurtain => DeviceKindRecord::LightCurtain,
            DeviceKind::AreaSensor => DeviceKindRecord::AreaSensor,
            DeviceKind::Fence => DeviceKindRecord::Fence,
            DeviceKind::EmergencyStop => DeviceKindRecord::EmergencyStop,
        },
        position: d.position,
        yaw_deg: d.yaw_deg,
        size: d.size,
        stop_time: d.stop_time,
        resolution: d.resolution,
        required_clearance: d.required_clearance,
    }
}

fn device_from_record(r: &SafetyDeviceRecord) -> SafetyDevice {
    let kind = match r.kind {
        DeviceKindRecord::LightCurtain => DeviceKind::LightCurtain,
        DeviceKindRecord::AreaSensor => DeviceKind::AreaSensor,
        DeviceKindRecord::Fence => DeviceKind::Fence,
        DeviceKindRecord::EmergencyStop => DeviceKind::EmergencyStop,
    };
    SafetyDevice {
        id: r.id,
        name: r.name.clone(),
        kind,
        position: r.position,
        yaw_deg: r.yaw_deg,
        size: r.size,
        stop_time: r.stop_time,
        resolution: r.resolution,
        required_clearance: r.required_clearance,
    }
}

fn operator_record(op: &Operator) -> OperatorRecord {
    OperatorRecord {
        id: op.id,
        name: op.name.clone(),
        start: op.start,
        walk_speed: op.walk_speed,
        tasks: op
            .tasks
            .iter()
            .map(|t| match t {
                OperatorTask::Walk { to } => TaskRecord::Walk { to: *to },
                OperatorTask::Pick { label, duration } => TaskRecord::Pick {
                    label: label.clone(),
                    duration: *duration,
                },
                OperatorTask::Button { label, duration } => TaskRecord::Button {
                    label: label.clone(),
                    duration: *duration,
                },
                OperatorTask::Wait { duration } => TaskRecord::Wait {
                    duration: *duration,
                },
            })
            .collect(),
    }
}

fn operator_from_record(r: &OperatorRecord) -> Operator {
    Operator {
        id: r.id,
        name: r.name.clone(),
        start: r.start,
        walk_speed: r.walk_speed,
        tasks: r
            .tasks
            .iter()
            .map(|t| match t {
                TaskRecord::Walk { to } => OperatorTask::Walk { to: *to },
                TaskRecord::Pick { label, duration } => OperatorTask::Pick {
                    label: label.clone(),
                    duration: *duration,
                },
                TaskRecord::Button { label, duration } => OperatorTask::Button {
                    label: label.clone(),
                    duration: *duration,
                },
                TaskRecord::Wait { duration } => OperatorTask::Wait {
                    duration: *duration,
                },
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_round_trips_through_json() {
        let mut safety = SafetyLayout::default();
        safety.add(DeviceKind::LightCurtain, [100.0, 200.0]);
        safety.add(DeviceKind::Fence, [0.0, 0.0]);
        safety.devices[1].yaw_deg = 90.0;
        let mut op = Operator::new(3, [1.0, 2.0]);
        op.tasks = vec![
            OperatorTask::Walk { to: [5.0, 6.0] },
            OperatorTask::Pick {
                label: "棚".into(),
                duration: 2.0,
            },
            OperatorTask::Button {
                label: "起動".into(),
                duration: 1.0,
            },
            OperatorTask::Wait { duration: 0.5 },
        ];
        let mut graph = AgvGraph::default();
        let a = graph.add_node([0.0, 0.0], true);
        let b = graph.add_node([1000.0, 0.0], true);
        graph.toggle_edge(a, b);
        let flows = vec![Flow {
            from: a,
            to: b,
            interval: 20.0,
        }];
        let params = AgvParams {
            count: 3,
            ..Default::default()
        };

        let record = FloorRecord::capture(&safety, &[op.clone()], &graph, &flows, &params);
        let json = serde_json::to_string(&record).unwrap();
        let parsed: FloorRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, record);
        let layout = parsed.restore();
        assert_eq!(layout.safety, safety);
        assert_eq!(layout.operators, vec![op]);
        assert_eq!(layout.graph, graph);
        assert_eq!(layout.flows, flows);
        assert_eq!(layout.params, params);
    }

    #[test]
    fn empty_site_writes_nothing() {
        let record = FloorRecord::capture(
            &SafetyLayout::default(),
            &[],
            &AgvGraph::default(),
            &[],
            &AgvParams::default(),
        );
        assert!(record.is_empty());
    }
}
