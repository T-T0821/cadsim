//! 複数人で同じ工場をレビューする (ドメイン)
//!
//! 離れた人どうしが同じプロジェクトを開き、互いの視点 (カメラ) と指している場所を見ながら話す。
//! 1人が「ホスト」になって待ち受け、ほかの人がそこへ「参加」する。ホストはメッセージを
//! ほかの全員へ配る (`hub`)。各自の画面は届いたメッセージから相手の様子を組み立てる (`session`)。
//!
//! ここには通信の中身 (`Message`) と、その受け渡しの規則だけを置く。
//! TCP での送受信と JSON の形は `cadsim_io::review`、3D の表示と画面は UI 層 (`src/review/`)。

mod hub;
mod placement;
mod session;
mod throttle;
mod view;

pub use hub::{Hub, Outgoing};
pub use placement::PlacementTracker;
pub use session::{ChatLine, Effect, PingMark, RemotePeer, ReviewSession};
pub use throttle::SendThrottle;
pub use view::{ViewPose, orbit_angles};

/// 参加者の番号。ホストは 0
pub type PeerId = u32;

/// ホストの番号
pub const HOST_ID: PeerId = 0;

/// 通信の取り決めの版。合わない相手とはつながない
pub const PROTOCOL_VERSION: u32 = 1;

/// 待ち受けの既定のポート番号
pub const DEFAULT_PORT: u16 = 47800;

/// 名前の最大の長さ (文字数)
pub const MAX_NAME_CHARS: usize = 32;

/// チャット 1 件の最大の長さ (文字数)
pub const MAX_CHAT_CHARS: usize = 500;

/// 参加者の名前と色
#[derive(Clone, Debug, PartialEq)]
pub struct PeerInfo {
    pub id: PeerId,
    pub name: String,
    /// アバター・指さし・選んだ部品の枠に使う色 (sRGB)
    pub color: [u8; 3],
}

/// 参加者の色。番号の順に配る (見分けやすい 8 色をくり返す)
pub fn peer_color(id: PeerId) -> [u8; 3] {
    const COLORS: [[u8; 3]; 8] = [
        [255, 170, 40],  // 橙 (ホスト)
        [60, 180, 255],  // 水色
        [120, 220, 90],  // 黄緑
        [240, 90, 200],  // 桃
        [255, 230, 70],  // 黄
        [150, 120, 255], // 紫
        [70, 230, 200],  // 青緑
        [255, 110, 100], // 赤
    ];
    COLORS[id as usize % COLORS.len()]
}

/// 参加者どうしでやり取りするメッセージ
///
/// `id` を持つものは「誰の」様子か。ホストは届いたメッセージの `id` を送り主の番号で
/// 書き直してから配るので、ほかの人になりすますことはできない。
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// 参加する人 → ホスト: 名前と合言葉
    Hello {
        version: u32,
        name: String,
        passphrase: String,
        /// 開いているプロジェクトの名前 (違うものを開いていたら知らせるため)
        project: String,
    },
    /// ホスト → 参加した人: あなたの番号と、いまいる人
    Welcome {
        you: PeerId,
        project: String,
        peers: Vec<PeerInfo>,
    },
    /// ホスト → 参加しようとした人: 断った理由
    Rejected { reason: String },
    /// 誰かが入った
    Joined(PeerInfo),
    /// 誰かが抜けた
    Left { id: PeerId },
    /// 視点が動いた
    View { id: PeerId, pose: ViewPose },
    /// マウスで指している 3D の点。3D ビューの外なら None
    Pointer { id: PeerId, point: Option<[f32; 3]> },
    /// 「ここを見て」の印を置いた
    Ping { id: PeerId, point: [f32; 3] },
    /// 選んだ部品 (ツリーの道順 "組立 / 部品" で表す)。何も選んでいなければ None
    Select { id: PeerId, part: Option<String> },
    /// チャット
    Chat { id: PeerId, text: String },
    /// 全員の視点をこの視点に合わせる
    Summon { id: PeerId, pose: ViewPose },
    /// 部品の配置を動かした (親から見た配置の 4×4 行列, 列優先)
    Placement {
        id: PeerId,
        part: String,
        matrix: [f64; 16],
    },
}

impl Message {
    /// 送り主の番号を持つメッセージなら、それを `id` に書き直したもの
    fn with_sender(self, sender: PeerId) -> Option<Message> {
        Some(match self {
            Message::View { pose, .. } => Message::View { id: sender, pose },
            Message::Pointer { point, .. } => Message::Pointer { id: sender, point },
            Message::Ping { point, .. } => Message::Ping { id: sender, point },
            Message::Select { part, .. } => Message::Select { id: sender, part },
            Message::Chat { text, .. } => Message::Chat {
                id: sender,
                text: truncate_chars(text.trim(), MAX_CHAT_CHARS),
            },
            Message::Summon { pose, .. } => Message::Summon { id: sender, pose },
            Message::Placement { part, matrix, .. } => Message::Placement {
                id: sender,
                part,
                matrix,
            },
            Message::Hello { .. }
            | Message::Welcome { .. }
            | Message::Rejected { .. }
            | Message::Joined(_)
            | Message::Left { .. } => return None,
        })
    }
}

/// 文字数で切り詰める
pub fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 名前を整える (前後の空白を取り、長すぎれば切り、空なら「ゲスト」)
pub fn clean_name(name: &str) -> String {
    let name = truncate_chars(name.trim(), MAX_NAME_CHARS);
    if name.is_empty() {
        "ゲスト".to_string()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_never_empty() {
        assert_eq!(clean_name("  山田  "), "山田");
        assert_eq!(clean_name("   "), "ゲスト");
        assert_eq!(
            clean_name(&"あ".repeat(100)).chars().count(),
            MAX_NAME_CHARS
        );
    }

    #[test]
    fn neighbouring_peers_get_different_colors() {
        for id in 0..8 {
            assert_ne!(peer_color(id), peer_color(id + 1));
        }
    }

    #[test]
    fn sender_id_is_overwritten() {
        let msg = Message::Chat {
            id: 99,
            text: "  こんにちは ".into(),
        };
        assert_eq!(
            msg.with_sender(3),
            Some(Message::Chat {
                id: 3,
                text: "こんにちは".into()
            })
        );
        let hello = Message::Rejected { reason: "x".into() };
        assert_eq!(hello.with_sender(3), None);
    }
}
