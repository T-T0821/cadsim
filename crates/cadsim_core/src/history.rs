//! 元に戻す / やり直しの中身 (ドメイン + ユースケース)
//!
//! 変更は「済ませてから記録する」方式。モデルを書き換えたあとで、その変更を戻す・やり直すための
//! 操作を [`UndoStack`] に積む。モデルの変更は [`ModelChange`] で表し、中身は変更前と変更後の
//! ノードの写し (`Snapshot`)。ドラッグのように毎フレーム値が変わる編集は [`ModelEdits`] が
//! 変更前を覚えておき、区切りで1件にまとめる。
//!
//! 画面の状態や、ほかの文書 (タイミングチャートなど) をまとめて戻す仕組みはアプリ側の `history` にある。

use std::borrow::{Borrow, BorrowMut};

use crate::model::{Model, Node, NodeId, NodeKind, Snapshot};

/// 覚えておく履歴の上限
pub const MAX_HISTORY: usize = 200;

/// 戻すか、やり直すか
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Request {
    Undo,
    Redo,
}

/// 戻す履歴とやり直す履歴。`C` は履歴1件分の操作。
pub struct UndoStack<C> {
    undo: Vec<C>,
    redo: Vec<C>,
}

impl<C> Default for UndoStack<C> {
    fn default() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

impl<C> UndoStack<C> {
    /// 済ませた変更を積む。やり直しの履歴は捨て、上限を超えたら古いものから捨てる。
    pub fn push(&mut self, command: C) {
        self.redo.clear();
        self.undo.push(command);
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// 次に戻す操作
    pub fn next_undo(&self) -> Option<&C> {
        self.undo.last()
    }

    /// 次にやり直す操作
    pub fn next_redo(&self) -> Option<&C> {
        self.redo.last()
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// 戻す (やり直す) 操作を取り出す。実行したら [`UndoStack::finish`] で反対側に積む。
    pub fn take(&mut self, request: Request) -> Option<C> {
        match request {
            Request::Undo => self.undo.pop(),
            Request::Redo => self.redo.pop(),
        }
    }

    /// 実行した操作を反対側 (戻したならやり直す側) に積む
    pub fn finish(&mut self, request: Request, command: C) {
        match request {
            Request::Undo => self.redo.push(command),
            Request::Redo => self.undo.push(command),
        }
    }
}

/// モデルの変更1回分。(変更前, 変更後) の組を並べたもの。
/// 追加は変更前が None、削除は変更後が None。
pub struct ModelChange {
    pub label: String,
    pub changes: Vec<(Option<Snapshot>, Option<Snapshot>)>,
}

impl ModelChange {
    pub fn undo(&self, model: &mut Model) {
        for (before, after) in self.changes.iter().rev() {
            replace(model, after, before);
        }
    }

    pub fn redo(&self, model: &mut Model) {
        for (before, after) in &self.changes {
            replace(model, before, after);
        }
    }

    /// 追加したノードの変更 (箱の追加、STEP の読み込み)
    pub fn added(model: &Model, id: NodeId, label: String) -> Self {
        Self {
            label,
            changes: vec![(None, model.snapshot(id))],
        }
    }

    /// ノードを削除する。無ければ何もせず None。
    pub fn remove(model: &mut Model, id: NodeId) -> Option<Self> {
        let before = model.snapshot(id)?;
        model.remove(id);
        Some(Self {
            label: format!("{} を削除", node_name(&before)),
            changes: vec![(Some(before), None)],
        })
    }

    /// すべて閉じる。空なら何もせず None。
    pub fn clear_all(model: &mut Model) -> Option<Self> {
        let before = model.snapshot_all();
        if before.is_empty() {
            return None;
        }
        model.clear();
        Some(Self {
            label: "すべて閉じる".into(),
            changes: removals(before),
        })
    }

    /// モデルを丸ごと作り直す (サンプル装置の読み込みなど)。
    /// `M` はモデルそのものか、モデルを包んだ型 (アプリ側の Resource)。
    pub fn replace_all<M: BorrowMut<Model>>(
        model: &mut M,
        label: String,
        rebuild: impl FnOnce(&mut M),
    ) -> Self {
        let before = Borrow::<Model>::borrow(&*model).snapshot_all();
        rebuild(model);
        let after = Borrow::<Model>::borrow(&*model).snapshot_all();
        let mut changes = removals(before);
        changes.extend(after.into_iter().map(|a| (None, Some(a))));
        Self { label, changes }
    }
}

/// ドラッグ中の編集。変更前の写しを覚えておき、区切りで1件の [`ModelChange`] にする。
#[derive(Default)]
pub struct ModelEdits {
    pending: Option<Snapshot>,
}

impl ModelEdits {
    /// ノードを書き換える直前に呼ぶ。同じノードへの連続した編集は最初の状態だけを覚える。
    /// 別のノードの編集を覚えていたら、それを1件にして返す。
    pub fn begin(&mut self, model: &Model, id: NodeId) -> Option<ModelChange> {
        if self.pending.as_ref().is_some_and(|p| p.root == id) {
            return None;
        }
        let finished = self.finish(model);
        self.pending = model.snapshot(id);
        finished
    }

    /// 覚えておいた編集を1件にする。値が変わっていなければ None。
    pub fn finish(&mut self, model: &Model) -> Option<ModelChange> {
        let before = self.pending.take()?;
        let after = model.snapshot(before.root);
        if after.as_ref().is_some_and(|a| same_snapshot(a, &before)) {
            return None;
        }
        let name = node_name(&before);
        Some(ModelChange {
            label: format!("{name} を編集"),
            changes: vec![(Some(before), after)],
        })
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn clear(&mut self) {
        self.pending = None;
    }
}

/// `from` の状態を消して `to` の状態にする
fn replace(model: &mut Model, from: &Option<Snapshot>, to: &Option<Snapshot>) {
    if let Some(from) = from {
        model.remove(from.root);
    }
    if let Some(to) = to {
        model.restore(to);
    }
}

/// 最上位のノードをすべて消す変更。戻すときに元の並び順で戻るよう、後ろのノードから消す順に並べる
/// (戻すときは逆順に、前のノードから元の位置へ差し込まれる)。
fn removals(before: Vec<Snapshot>) -> Vec<(Option<Snapshot>, Option<Snapshot>)> {
    before.into_iter().rev().map(|b| (Some(b), None)).collect()
}

fn node_name(snapshot: &Snapshot) -> &str {
    snapshot.nodes.first().map_or("", |(_, n)| n.name.as_str())
}

/// 編集で変わりうるところ (名前・配置・色・物理の設定・箱の寸法) が同じなら true。
/// 表示/非表示は履歴に載せないので比べない。
fn same_snapshot(a: &Snapshot, b: &Snapshot) -> bool {
    let same_node = |x: &Node, y: &Node| {
        x.name == y.name
            && x.placement == y.placement
            && x.color == y.color
            && x.physics == y.physics
            && x.script == y.script
            && x.custom_part == y.custom_part
            && x.children == y.children
            && match (&x.kind, &y.kind) {
                (NodeKind::Box(p), NodeKind::Box(q)) => p == q,
                (NodeKind::Standard(p), NodeKind::Standard(q)) => p == q,
                _ => true,
            }
    };
    a.parent == b.parent
        && a.index == b.index
        && a.nodes.len() == b.nodes.len()
        && a.nodes
            .iter()
            .zip(&b.nodes)
            .all(|((i, x), (j, y))| i == j && same_node(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_drops_redo_on_push_and_keeps_the_limit() {
        let mut stack = UndoStack::default();
        stack.push(1);
        stack.push(2);
        let c = stack.take(Request::Undo).unwrap();
        stack.finish(Request::Undo, c);
        assert_eq!(stack.next_redo(), Some(&2));
        stack.push(3);
        assert_eq!(stack.redo_len(), 0);
        for i in 0..MAX_HISTORY + 10 {
            stack.push(i);
        }
        assert_eq!(stack.undo_len(), MAX_HISTORY);
    }

    #[test]
    fn edits_of_one_node_become_one_change() {
        let mut model = Model::default();
        let id = model.add_box([100.0, 100.0, 100.0], [0.0; 3]);
        let mut edits = ModelEdits::default();
        assert!(edits.begin(&model, id).is_none());
        model.get_mut(id).unwrap().name = "a".into();
        assert!(edits.begin(&model, id).is_none());
        model.get_mut(id).unwrap().name = "b".into();
        let change = edits.finish(&model).unwrap();
        change.undo(&mut model);
        assert_ne!(model.get(id).unwrap().name, "b");
        change.redo(&mut model);
        assert_eq!(model.get(id).unwrap().name, "b");
        // 変わっていなければ積まない
        assert!(edits.begin(&model, id).is_none());
        assert!(edits.finish(&model).is_none());
    }

    #[test]
    fn clear_all_restores_in_order() {
        let mut model = Model::default();
        let a = model.add_box([1.0; 3], [0.0; 3]);
        let b = model.add_box([2.0; 3], [0.0; 3]);
        let change = ModelChange::clear_all(&mut model).unwrap();
        assert!(model.roots.is_empty());
        change.undo(&mut model);
        assert_eq!(model.roots, vec![a, b]);
    }
}
