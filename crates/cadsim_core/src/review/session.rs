//! 自分の画面から見たレビューの様子
//!
//! 届いたメッセージから、ほかの参加者の視点・指している点・選んだ部品・チャットを組み立てる。
//! 視点を合わせる・配置を写すといった、画面の側でしてほしいことは `Effect` で返す。

use std::collections::BTreeMap;

use super::{Message, PeerId, PeerInfo, ViewPose};

/// 「ここを見て」の印を出しておく秒数
pub const PING_SECONDS: f64 = 4.0;

/// チャットを覚えておく件数
pub const CHAT_HISTORY: usize = 200;

/// ほかの参加者の今の様子
#[derive(Clone, Debug, PartialEq)]
pub struct RemotePeer {
    pub info: PeerInfo,
    pub view: Option<ViewPose>,
    /// マウスで指している点。3D ビューの外にいるときは None
    pub pointer: Option<[f32; 3]>,
    /// 選んだ部品のツリーの道順
    pub selected: Option<String>,
    /// 最後のチャットとその時刻 (アバターの横に吹き出しを出す)
    pub last_chat: Option<(String, f64)>,
}

/// 「ここを見て」の印
#[derive(Clone, Debug, PartialEq)]
pub struct PingMark {
    pub from: PeerId,
    pub point: [f32; 3],
    /// 置いた時刻 (秒)
    pub at: f64,
}

impl PingMark {
    /// 置いてからの割合 (0 で置いたばかり、1 で消える)
    pub fn age(&self, now: f64) -> f32 {
        ((now - self.at) / PING_SECONDS).clamp(0.0, 1.0) as f32
    }
}

/// チャットの 1 行
#[derive(Clone, Debug, PartialEq)]
pub struct ChatLine {
    /// 書いた人。お知らせ (入った・抜けた) なら None
    pub from: Option<PeerId>,
    pub name: String,
    pub text: String,
}

/// 画面の側でしてほしいこと
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// 自分の視点をこの視点へ動かす (呼ばれた・追いかけている)
    MoveCamera(ViewPose),
    /// 部品の配置を書き換える
    ApplyPlacement { part: String, matrix: [f64; 16] },
}

#[derive(Clone, Debug, Default)]
pub struct ReviewSession {
    /// 自分の番号。ホストから迎えられるまでは None
    pub me: Option<PeerId>,
    /// ホストが開いているプロジェクト名
    pub host_project: Option<String>,
    /// ホストに断られた理由
    pub rejected: Option<String>,
    /// 視点を追いかけている人
    pub following: Option<PeerId>,
    peers: BTreeMap<PeerId, RemotePeer>,
    pings: Vec<PingMark>,
    chat: Vec<ChatLine>,
}

impl ReviewSession {
    /// 迎えられて参加中か
    pub fn joined(&self) -> bool {
        self.me.is_some()
    }

    /// ほかの参加者 (番号順)
    pub fn peers(&self) -> impl Iterator<Item = &RemotePeer> {
        self.peers.values()
    }

    pub fn peer(&self, id: PeerId) -> Option<&RemotePeer> {
        self.peers.get(&id)
    }

    /// 出ている「ここを見て」の印
    pub fn pings(&self) -> &[PingMark] {
        &self.pings
    }

    pub fn chat(&self) -> &[ChatLine] {
        &self.chat
    }

    /// 自分が書いたチャットを履歴に足す (ホストは自分に送り返さないので)
    pub fn add_own_chat(&mut self, name: &str, text: &str) {
        let me = self.me;
        self.push_chat(me, name.to_string(), text.to_string());
    }

    /// 自分が置いた印を足す
    pub fn add_own_ping(&mut self, point: [f32; 3], now: f64) {
        if let Some(me) = self.me {
            self.pings.push(PingMark {
                from: me,
                point,
                at: now,
            });
        }
    }

    /// 古い印を消す
    pub fn prune(&mut self, now: f64) {
        self.pings.retain(|p| now - p.at < PING_SECONDS);
    }

    /// 切れたので、相手の様子を消す (チャットの履歴と断られた理由は残す)
    pub fn reset(&mut self) {
        self.me = None;
        self.host_project = None;
        self.following = None;
        self.peers.clear();
        self.pings.clear();
    }

    /// 届いたメッセージを反映する
    pub fn apply(&mut self, message: Message, now: f64) -> Option<Effect> {
        match message {
            Message::Welcome {
                you,
                project,
                peers,
            } => {
                self.me = Some(you);
                self.host_project = Some(project);
                self.rejected = None;
                self.peers = peers
                    .into_iter()
                    .filter(|p| p.id != you)
                    .map(|info| (info.id, RemotePeer::new(info)))
                    .collect();
            }
            Message::Rejected { reason } => self.rejected = Some(reason),
            Message::Joined(info) => {
                if Some(info.id) != self.me {
                    self.notice(format!("{} さんが入りました", info.name));
                    self.peers.insert(info.id, RemotePeer::new(info));
                }
            }
            Message::Left { id } => {
                if let Some(peer) = self.peers.remove(&id) {
                    self.notice(format!("{} さんが抜けました", peer.info.name));
                }
                if self.following == Some(id) {
                    self.following = None;
                }
                self.pings.retain(|p| p.from != id);
            }
            Message::View { id, pose } => {
                let peer = self.peers.get_mut(&id)?;
                peer.view = Some(pose);
                if self.following == Some(id) {
                    return Some(Effect::MoveCamera(pose));
                }
            }
            Message::Pointer { id, point } => self.peers.get_mut(&id)?.pointer = point,
            Message::Ping { id, point } => {
                if self.peers.contains_key(&id) {
                    self.pings.push(PingMark {
                        from: id,
                        point,
                        at: now,
                    });
                }
            }
            Message::Select { id, part } => self.peers.get_mut(&id)?.selected = part,
            Message::Chat { id, text } => {
                let peer = self.peers.get_mut(&id)?;
                peer.last_chat = Some((text.clone(), now));
                let name = peer.info.name.clone();
                self.push_chat(Some(id), name, text);
            }
            Message::Summon { id, pose } => {
                let name = self.peers.get(&id)?.info.name.clone();
                self.following = None;
                self.notice(format!("{name} さんの視点に合わせました"));
                return Some(Effect::MoveCamera(pose));
            }
            Message::Placement { id, part, matrix } => {
                self.peers.get(&id)?;
                return Some(Effect::ApplyPlacement { part, matrix });
            }
            Message::Hello { .. } => {}
        }
        None
    }

    fn notice(&mut self, text: String) {
        self.push_chat(None, String::new(), text);
    }

    fn push_chat(&mut self, from: Option<PeerId>, name: String, text: String) {
        self.chat.push(ChatLine { from, name, text });
        if self.chat.len() > CHAT_HISTORY {
            let extra = self.chat.len() - CHAT_HISTORY;
            self.chat.drain(..extra);
        }
    }
}

impl RemotePeer {
    fn new(info: PeerInfo) -> Self {
        Self {
            info,
            view: None,
            pointer: None,
            selected: None,
            last_chat: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::peer_color;
    use super::*;

    fn info(id: PeerId, name: &str) -> PeerInfo {
        PeerInfo {
            id,
            name: name.into(),
            color: peer_color(id),
        }
    }

    fn pose() -> ViewPose {
        ViewPose {
            eye: [0.0, 1.0, 5.0],
            focus: [0.0; 3],
            orthographic: false,
        }
    }

    fn joined() -> ReviewSession {
        let mut s = ReviewSession::default();
        s.apply(
            Message::Welcome {
                you: 2,
                project: "line1".into(),
                peers: vec![info(0, "ホスト"), info(2, "自分")],
            },
            0.0,
        );
        s
    }

    #[test]
    fn welcome_lists_others_but_not_me() {
        let s = joined();
        assert_eq!(s.me, Some(2));
        assert_eq!(s.host_project.as_deref(), Some("line1"));
        let names: Vec<_> = s.peers().map(|p| p.info.name.as_str()).collect();
        assert_eq!(names, ["ホスト"]);
    }

    #[test]
    fn joins_and_leaves_are_noted_in_chat() {
        let mut s = joined();
        s.apply(Message::Joined(info(3, "鈴木")), 1.0);
        assert!(s.peer(3).is_some());
        s.following = Some(3);
        s.apply(Message::Left { id: 3 }, 2.0);
        assert!(s.peer(3).is_none());
        assert_eq!(s.following, None);
        let texts: Vec<_> = s.chat().iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["鈴木 さんが入りました", "鈴木 さんが抜けました"]);
    }

    #[test]
    fn following_moves_my_camera_only_for_the_followed_peer() {
        let mut s = joined();
        s.apply(Message::Joined(info(3, "鈴木")), 0.0);
        assert_eq!(
            s.apply(
                Message::View {
                    id: 3,
                    pose: pose()
                },
                0.0
            ),
            None
        );
        s.following = Some(0);
        assert_eq!(
            s.apply(
                Message::View {
                    id: 3,
                    pose: pose()
                },
                0.0
            ),
            None
        );
        assert_eq!(
            s.apply(
                Message::View {
                    id: 0,
                    pose: pose()
                },
                0.0
            ),
            Some(Effect::MoveCamera(pose()))
        );
        assert_eq!(s.peer(0).unwrap().view, Some(pose()));
    }

    #[test]
    fn summon_moves_camera_and_stops_following() {
        let mut s = joined();
        s.following = Some(0);
        assert_eq!(
            s.apply(
                Message::Summon {
                    id: 0,
                    pose: pose()
                },
                0.0
            ),
            Some(Effect::MoveCamera(pose()))
        );
        assert_eq!(s.following, None);
    }

    #[test]
    fn pings_expire() {
        let mut s = joined();
        s.apply(
            Message::Ping {
                id: 0,
                point: [1.0; 3],
            },
            10.0,
        );
        // 知らない人からの印は無視する
        s.apply(
            Message::Ping {
                id: 9,
                point: [1.0; 3],
            },
            10.0,
        );
        assert_eq!(s.pings().len(), 1);
        assert!((s.pings()[0].age(12.0) - 0.5).abs() < 1e-6);
        s.prune(10.0 + PING_SECONDS + 0.1);
        assert!(s.pings().is_empty());
    }

    #[test]
    fn pointer_selection_chat_and_placement() {
        let mut s = joined();
        s.apply(
            Message::Pointer {
                id: 0,
                point: Some([1.0, 0.0, 0.0]),
            },
            0.0,
        );
        s.apply(
            Message::Select {
                id: 0,
                part: Some("A / B".into()),
            },
            0.0,
        );
        s.apply(
            Message::Chat {
                id: 0,
                text: "ここ狭い".into(),
            },
            3.0,
        );
        let host = s.peer(0).unwrap();
        assert_eq!(host.pointer, Some([1.0, 0.0, 0.0]));
        assert_eq!(host.selected.as_deref(), Some("A / B"));
        assert_eq!(host.last_chat, Some(("ここ狭い".into(), 3.0)));
        assert_eq!(s.chat().last().unwrap().name, "ホスト");
        let matrix = [1.0; 16];
        assert_eq!(
            s.apply(
                Message::Placement {
                    id: 0,
                    part: "A / B".into(),
                    matrix
                },
                0.0
            ),
            Some(Effect::ApplyPlacement {
                part: "A / B".into(),
                matrix
            })
        );
    }

    #[test]
    fn reset_forgets_peers_but_keeps_chat() {
        let mut s = joined();
        s.add_own_chat("自分", "よろしく");
        s.reset();
        assert!(!s.joined());
        assert_eq!(s.peers().count(), 0);
        assert_eq!(s.chat().len(), 1);
    }

    #[test]
    fn chat_history_is_bounded() {
        let mut s = joined();
        for i in 0..CHAT_HISTORY + 10 {
            s.add_own_chat("自分", &i.to_string());
        }
        assert_eq!(s.chat().len(), CHAT_HISTORY);
        assert_eq!(s.chat()[0].text, "10");
    }
}
