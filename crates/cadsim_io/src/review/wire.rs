//! メッセージを 1 行の JSON にする
//!
//! `{"t":"view","id":1,"pose":{"eye":[…],"focus":[…],"ortho":false}}` のように、
//! 種類を `t` に書いた JSON を改行で区切って送る。

use cadsim_core::review::{Message, PeerInfo, ViewPose};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct WirePeer {
    id: u32,
    name: String,
    color: [u8; 3],
}

#[derive(Serialize, Deserialize, Clone, Copy)]
struct WirePose {
    eye: [f32; 3],
    focus: [f32; 3],
    #[serde(default)]
    ortho: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Wire {
    Hello {
        version: u32,
        name: String,
        #[serde(default)]
        passphrase: String,
        #[serde(default)]
        project: String,
    },
    Welcome {
        you: u32,
        #[serde(default)]
        project: String,
        peers: Vec<WirePeer>,
    },
    Rejected {
        reason: String,
    },
    Joined {
        peer: WirePeer,
    },
    Left {
        id: u32,
    },
    View {
        id: u32,
        pose: WirePose,
    },
    Pointer {
        id: u32,
        point: Option<[f32; 3]>,
    },
    Ping {
        id: u32,
        point: [f32; 3],
    },
    Select {
        id: u32,
        part: Option<String>,
    },
    Chat {
        id: u32,
        text: String,
    },
    Summon {
        id: u32,
        pose: WirePose,
    },
    Placement {
        id: u32,
        part: String,
        matrix: Vec<f64>,
    },
}

impl From<&PeerInfo> for WirePeer {
    fn from(p: &PeerInfo) -> Self {
        Self {
            id: p.id,
            name: p.name.clone(),
            color: p.color,
        }
    }
}

impl From<WirePeer> for PeerInfo {
    fn from(p: WirePeer) -> Self {
        Self {
            id: p.id,
            name: p.name,
            color: p.color,
        }
    }
}

impl From<ViewPose> for WirePose {
    fn from(p: ViewPose) -> Self {
        Self {
            eye: p.eye,
            focus: p.focus,
            ortho: p.orthographic,
        }
    }
}

impl From<WirePose> for ViewPose {
    fn from(p: WirePose) -> Self {
        Self {
            eye: p.eye,
            focus: p.focus,
            orthographic: p.ortho,
        }
    }
}

fn to_wire(message: &Message) -> Wire {
    match message.clone() {
        Message::Hello {
            version,
            name,
            passphrase,
            project,
        } => Wire::Hello {
            version,
            name,
            passphrase,
            project,
        },
        Message::Welcome {
            you,
            project,
            peers,
        } => Wire::Welcome {
            you,
            project,
            peers: peers.iter().map(WirePeer::from).collect(),
        },
        Message::Rejected { reason } => Wire::Rejected { reason },
        Message::Joined(peer) => Wire::Joined {
            peer: WirePeer::from(&peer),
        },
        Message::Left { id } => Wire::Left { id },
        Message::View { id, pose } => Wire::View {
            id,
            pose: pose.into(),
        },
        Message::Pointer { id, point } => Wire::Pointer { id, point },
        Message::Ping { id, point } => Wire::Ping { id, point },
        Message::Select { id, part } => Wire::Select { id, part },
        Message::Chat { id, text } => Wire::Chat { id, text },
        Message::Summon { id, pose } => Wire::Summon {
            id,
            pose: pose.into(),
        },
        Message::Placement { id, part, matrix } => Wire::Placement {
            id,
            part,
            matrix: matrix.to_vec(),
        },
    }
}

fn from_wire(wire: Wire) -> Option<Message> {
    Some(match wire {
        Wire::Hello {
            version,
            name,
            passphrase,
            project,
        } => Message::Hello {
            version,
            name,
            passphrase,
            project,
        },
        Wire::Welcome {
            you,
            project,
            peers,
        } => Message::Welcome {
            you,
            project,
            peers: peers.into_iter().map(PeerInfo::from).collect(),
        },
        Wire::Rejected { reason } => Message::Rejected { reason },
        Wire::Joined { peer } => Message::Joined(peer.into()),
        Wire::Left { id } => Message::Left { id },
        Wire::View { id, pose } => Message::View {
            id,
            pose: pose.into(),
        },
        Wire::Pointer { id, point } => Message::Pointer { id, point },
        Wire::Ping { id, point } => Message::Ping { id, point },
        Wire::Select { id, part } => Message::Select { id, part },
        Wire::Chat { id, text } => Message::Chat { id, text },
        Wire::Summon { id, pose } => Message::Summon {
            id,
            pose: pose.into(),
        },
        Wire::Placement { id, part, matrix } => Message::Placement {
            id,
            part,
            matrix: matrix.try_into().ok()?,
        },
    })
}

/// メッセージを 1 行の JSON (末尾に改行) にする
pub fn encode(message: &Message) -> String {
    let mut line = serde_json::to_string(&to_wire(message)).expect("JSON にできない値はない");
    line.push('\n');
    line
}

/// 1 行の JSON をメッセージにする。読めない行・知らない種類は None
pub fn decode(line: &str) -> Option<Message> {
    let wire: Wire = serde_json::from_str(line.trim()).ok()?;
    let message = from_wire(wire)?;
    // NaN や無限大の座標は表示を壊すので受け取らない
    finite(&message).then_some(message)
}

fn finite(message: &Message) -> bool {
    let all = |v: &[f32]| v.iter().all(|x| x.is_finite());
    match message {
        Message::View { pose, .. } | Message::Summon { pose, .. } => {
            all(&pose.eye) && all(&pose.focus)
        }
        Message::Pointer { point: Some(p), .. } | Message::Ping { point: p, .. } => all(p),
        Message::Placement { matrix, .. } => matrix.iter().all(|x| x.is_finite()),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadsim_core::review::{PROTOCOL_VERSION, peer_color};

    fn pose() -> ViewPose {
        ViewPose {
            eye: [1.0, 2.0, 3.0],
            focus: [0.0, 0.5, 0.0],
            orthographic: true,
        }
    }

    #[test]
    fn every_message_round_trips() {
        let peer = PeerInfo {
            id: 1,
            name: "佐藤".into(),
            color: peer_color(1),
        };
        let messages = [
            Message::Hello {
                version: PROTOCOL_VERSION,
                name: "佐藤".into(),
                passphrase: "abc".into(),
                project: "line1".into(),
            },
            Message::Welcome {
                you: 1,
                project: "line1".into(),
                peers: vec![peer.clone()],
            },
            Message::Rejected {
                reason: "合言葉".into(),
            },
            Message::Joined(peer),
            Message::Left { id: 1 },
            Message::View {
                id: 1,
                pose: pose(),
            },
            Message::Pointer { id: 1, point: None },
            Message::Pointer {
                id: 1,
                point: Some([1.0, 2.0, 3.0]),
            },
            Message::Ping {
                id: 1,
                point: [0.0, 1.0, 0.0],
            },
            Message::Select {
                id: 1,
                part: Some("組立 / 部品".into()),
            },
            Message::Chat {
                id: 1,
                text: "改行\nも\"引用\"も".into(),
            },
            Message::Summon {
                id: 1,
                pose: pose(),
            },
            Message::Placement {
                id: 1,
                part: "A".into(),
                matrix: std::array::from_fn(|i| i as f64),
            },
        ];
        for message in messages {
            let line = encode(&message);
            assert!(line.ends_with('\n'));
            assert_eq!(line.matches('\n').count(), 1, "1 行に収まる: {line}");
            assert_eq!(decode(&line), Some(message));
        }
    }

    #[test]
    fn garbage_and_non_finite_values_are_dropped() {
        assert_eq!(decode("hello"), None);
        assert_eq!(decode(r#"{"t":"unknown"}"#), None);
        assert_eq!(
            decode(r#"{"t":"placement","id":1,"part":"A","matrix":[1,2]}"#),
            None
        );
        // JSON には NaN が書けないので、巨大な数で無限大を作る
        assert_eq!(decode(r#"{"t":"ping","id":1,"point":[1e999,0,0]}"#), None);
    }
}
