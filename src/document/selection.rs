//! いま選んでいるもの (画面の状態)。設計データそのものは `plantwright_core::model::Model`

use bevy::prelude::*;

use plantwright_core::model::NodeId;

/// いま選んでいるもの
#[derive(Resource, Default)]
pub struct Selection {
    /// 選択中のノード (部品でもアセンブリでもよい)
    pub node: Option<NodeId>,
    /// ビューで面をクリックしたときの面番号
    pub face: Option<usize>,
    /// ツリーの行にカーソルが乗っているノード (ビューで薄く強調する)
    pub hovered: Option<NodeId>,
    /// ビューで選んだとき、ツリーをそこまで開いてスクロールする
    pub reveal_in_tree: bool,
}

impl Selection {
    pub fn select(&mut self, node: NodeId) {
        self.node = Some(node);
        self.face = None;
    }

    pub fn clear(&mut self) {
        self.node = None;
        self.face = None;
    }
}
