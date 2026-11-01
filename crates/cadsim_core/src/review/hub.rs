//! ホストの配り役
//!
//! つないできた人を名簿に入れ、届いたメッセージを送り主以外の全員に配る。
//! ホスト自身も番号 0 の参加者として同じ規則で扱う。通信 (TCP) からは切り離してあり、
//! 「誰から何が届いたか」を渡すと「誰に何を送るか」を返すだけにしている。

use std::collections::{BTreeMap, BTreeSet};

use super::{
    HOST_ID, Message, PROTOCOL_VERSION, PeerId, PeerInfo, ViewPose, clean_name, peer_color,
};

/// 配り役が返す仕事
#[derive(Clone, Debug, PartialEq)]
pub enum Outgoing {
    /// この人にこのメッセージを送る
    Send(PeerId, Message),
    /// この人との接続を切る
    Disconnect(PeerId),
}

/// 後から入った人に、すでにいる人の今の様子を伝えるために覚えておくもの
#[derive(Clone, Debug, Default)]
struct Latest {
    view: Option<ViewPose>,
    select: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Hub {
    passphrase: String,
    project: String,
    /// 名簿 (ホストを含む)
    peers: BTreeMap<PeerId, PeerInfo>,
    latest: BTreeMap<PeerId, Latest>,
    /// つないだが、まだ名乗っていない接続
    pending: BTreeSet<PeerId>,
    next_id: PeerId,
}

impl Hub {
    /// ホストの名前・合言葉 (空なら誰でも入れる)・開いているプロジェクト名で始める
    pub fn new(host_name: &str, passphrase: &str, project: &str) -> Self {
        let host = PeerInfo {
            id: HOST_ID,
            name: clean_name(host_name),
            color: peer_color(HOST_ID),
        };
        Self {
            passphrase: passphrase.to_string(),
            project: project.to_string(),
            peers: BTreeMap::from([(HOST_ID, host)]),
            latest: BTreeMap::new(),
            pending: BTreeSet::new(),
            next_id: HOST_ID + 1,
        }
    }

    /// ホスト自身に送る最初のメッセージ (自分の番号と名簿)
    pub fn host_welcome(&self) -> Message {
        Message::Welcome {
            you: HOST_ID,
            project: self.project.clone(),
            peers: self.peers.values().cloned().collect(),
        }
    }

    /// 新しい接続に番号を振る。名乗る (`Hello`) までは名簿に入れない
    pub fn connect(&mut self) -> PeerId {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id);
        id
    }

    /// 名簿にいる人の数 (ホストを含む)
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// `from` からメッセージが届いた
    pub fn receive(&mut self, from: PeerId, message: Message) -> Vec<Outgoing> {
        if self.pending.contains(&from) {
            return self.greet(from, message);
        }
        if !self.peers.contains_key(&from) {
            return Vec::new();
        }
        let Some(message) = message.with_sender(from) else {
            // 名乗りなどは参加した後には意味がないので捨てる
            return Vec::new();
        };
        match &message {
            Message::View { pose, .. } => self.latest.entry(from).or_default().view = Some(*pose),
            Message::Select { part, .. } => {
                self.latest.entry(from).or_default().select = part.clone()
            }
            Message::Chat { text, .. } if text.is_empty() => return Vec::new(),
            _ => {}
        }
        self.broadcast(from, message)
    }

    /// `id` との接続が切れた
    pub fn disconnect(&mut self, id: PeerId) -> Vec<Outgoing> {
        self.pending.remove(&id);
        self.latest.remove(&id);
        if id == HOST_ID || self.peers.remove(&id).is_none() {
            return Vec::new();
        }
        self.broadcast(id, Message::Left { id })
    }

    fn greet(&mut self, from: PeerId, message: Message) -> Vec<Outgoing> {
        let Message::Hello {
            version,
            name,
            passphrase,
            ..
        } = message
        else {
            // 名乗る前に別のメッセージを送ってくる相手は plantwright ではない
            self.pending.remove(&from);
            return vec![Outgoing::Disconnect(from)];
        };
        self.pending.remove(&from);
        let reason = if version != PROTOCOL_VERSION {
            Some(format!(
                "plantwright の版が合いません (相手 {version}・ホスト {PROTOCOL_VERSION})。同じ版にそろえてください"
            ))
        } else if passphrase != self.passphrase {
            Some("合言葉が違います".to_string())
        } else {
            None
        };
        if let Some(reason) = reason {
            return vec![
                Outgoing::Send(from, Message::Rejected { reason }),
                Outgoing::Disconnect(from),
            ];
        }
        let info = PeerInfo {
            id: from,
            name: clean_name(&name),
            color: peer_color(from),
        };
        let others: Vec<PeerInfo> = self.peers.values().cloned().collect();
        self.peers.insert(from, info.clone());
        let mut out = vec![Outgoing::Send(
            from,
            Message::Welcome {
                you: from,
                project: self.project.clone(),
                peers: self.peers.values().cloned().collect(),
            },
        )];
        // すでにいる人の今の視点と選んだ部品を伝える
        for peer in &others {
            if let Some(latest) = self.latest.get(&peer.id) {
                if let Some(pose) = latest.view {
                    out.push(Outgoing::Send(from, Message::View { id: peer.id, pose }));
                }
                if latest.select.is_some() {
                    out.push(Outgoing::Send(
                        from,
                        Message::Select {
                            id: peer.id,
                            part: latest.select.clone(),
                        },
                    ));
                }
            }
        }
        out.extend(self.broadcast(from, Message::Joined(info)));
        out
    }

    /// `from` 以外の名簿の全員に送る
    fn broadcast(&self, from: PeerId, message: Message) -> Vec<Outgoing> {
        self.peers
            .keys()
            .filter(|&&id| id != from)
            .map(|&id| Outgoing::Send(id, message.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(name: &str, passphrase: &str) -> Message {
        Message::Hello {
            version: PROTOCOL_VERSION,
            name: name.into(),
            passphrase: passphrase.into(),
            project: "line1".into(),
        }
    }

    fn pose(x: f32) -> ViewPose {
        ViewPose {
            eye: [x, 1.0, 5.0],
            focus: [0.0; 3],
            orthographic: false,
        }
    }

    fn sent_to(out: &[Outgoing], id: PeerId) -> Vec<Message> {
        out.iter()
            .filter_map(|o| match o {
                Outgoing::Send(to, m) if *to == id => Some(m.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn joining_peer_gets_welcome_and_others_hear_about_it() {
        let mut hub = Hub::new("ホスト", "", "line1");
        let a = hub.connect();
        let out = hub.receive(a, hello("佐藤", ""));
        let to_a = sent_to(&out, a);
        let Message::Welcome { you, peers, .. } = &to_a[0] else {
            panic!("{to_a:?}")
        };
        assert_eq!(*you, a);
        assert_eq!(peers.len(), 2);
        assert_eq!(
            sent_to(&out, HOST_ID),
            vec![Message::Joined(PeerInfo {
                id: a,
                name: "佐藤".into(),
                color: peer_color(a)
            })]
        );
        assert_eq!(hub.peer_count(), 2);
    }

    #[test]
    fn wrong_passphrase_or_version_is_rejected() {
        let mut hub = Hub::new("ホスト", "secret", "line1");
        let a = hub.connect();
        let out = hub.receive(a, hello("佐藤", "nope"));
        assert!(
            matches!(&out[0], Outgoing::Send(id, Message::Rejected { reason }) if *id == a && reason.contains("合言葉"))
        );
        assert_eq!(out[1], Outgoing::Disconnect(a));
        assert_eq!(hub.peer_count(), 1);

        let b = hub.connect();
        let out = hub.receive(
            b,
            Message::Hello {
                version: PROTOCOL_VERSION + 1,
                name: "x".into(),
                passphrase: "secret".into(),
                project: String::new(),
            },
        );
        assert!(
            matches!(&out[0], Outgoing::Send(_, Message::Rejected { reason }) if reason.contains("版"))
        );
    }

    #[test]
    fn talking_before_hello_disconnects() {
        let mut hub = Hub::new("ホスト", "", "");
        let a = hub.connect();
        let out = hub.receive(
            a,
            Message::Ping {
                id: 0,
                point: [0.0; 3],
            },
        );
        assert_eq!(out, vec![Outgoing::Disconnect(a)]);
    }

    #[test]
    fn messages_are_relayed_to_everyone_else_with_sender_id() {
        let mut hub = Hub::new("ホスト", "", "");
        let a = hub.connect();
        hub.receive(a, hello("A", ""));
        let b = hub.connect();
        hub.receive(b, hello("B", ""));
        // A が B になりすまそうとしても、id は A に直される
        let out = hub.receive(
            a,
            Message::Ping {
                id: b,
                point: [1.0, 2.0, 3.0],
            },
        );
        let expected = Message::Ping {
            id: a,
            point: [1.0, 2.0, 3.0],
        };
        assert_eq!(sent_to(&out, HOST_ID), vec![expected.clone()]);
        assert_eq!(sent_to(&out, b), vec![expected]);
        assert!(sent_to(&out, a).is_empty());
        // ホスト自身のメッセージも同じように配る
        let out = hub.receive(
            HOST_ID,
            Message::Chat {
                id: 0,
                text: "見て".into(),
            },
        );
        assert_eq!(out.len(), 2);
        // 空のチャットは配らない
        assert!(
            hub.receive(
                a,
                Message::Chat {
                    id: a,
                    text: "  ".into()
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn late_joiner_receives_current_views_and_selection() {
        let mut hub = Hub::new("ホスト", "", "");
        hub.receive(
            HOST_ID,
            Message::View {
                id: 0,
                pose: pose(1.0),
            },
        );
        hub.receive(
            HOST_ID,
            Message::Select {
                id: 0,
                part: Some("組立 / 部品".into()),
            },
        );
        let a = hub.connect();
        let out = hub.receive(a, hello("A", ""));
        let to_a = sent_to(&out, a);
        assert!(to_a.contains(&Message::View {
            id: 0,
            pose: pose(1.0)
        }));
        assert!(to_a.contains(&Message::Select {
            id: 0,
            part: Some("組立 / 部品".into())
        }));
    }

    #[test]
    fn leaving_is_announced_once() {
        let mut hub = Hub::new("ホスト", "", "");
        let a = hub.connect();
        hub.receive(a, hello("A", ""));
        let out = hub.disconnect(a);
        assert_eq!(sent_to(&out, HOST_ID), vec![Message::Left { id: a }]);
        assert!(hub.disconnect(a).is_empty());
        // 名乗る前に切れた接続は誰にも知らせない
        let b = hub.connect();
        assert!(hub.disconnect(b).is_empty());
        // 抜けた人からのメッセージは配らない
        assert!(
            hub.receive(
                a,
                Message::Ping {
                    id: a,
                    point: [0.0; 3]
                }
            )
            .is_empty()
        );
    }
}
