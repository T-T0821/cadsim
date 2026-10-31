//! 元に戻す (Ctrl+Z) / やり直し (Ctrl+Y, Ctrl+Shift+Z)
//!
//! 変更は「済ませてから記録する」方式。モデルを書き換えたあとで、
//! その変更を戻す・やり直すための `Command` を `History` に積む。
//!
//! モデルの変更 (箱の追加、STEP の読み込み、削除、寸法・配置・名前・色の編集) は
//! すべて `ModelChange` で表す。中身は変更前と変更後のノードの写し (`Snapshot`)。
//! タイミングチャートなど別のデータを履歴に載せるときは、`Command` を実装した型を作って
//! `History::push` に渡せばよい。
//!
//! ギズモやパネルのドラッグは毎フレーム値が変わるので、`begin_edit` で変更前を覚えておき、
//! マウスを離した時点で `finish_gesture_when_idle` が1件の履歴にまとめる。
//! 表示/非表示の切り替えは見た目の操作なので履歴に載せない。
//!
//! タイミングチャート・工程モデル・床の配置・平面図のように、画面のあちこちで編集される文書は
//! [`TrackedDoc`] を実装して `app.track_doc::<…>()` で登録する。編集する場所ごとに履歴を積む代わりに、
//! マウスを離して入力欄からも抜けたときに前の状態と比べ、変わっていれば1件の履歴にする。

use std::marker::PhantomData;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use cadsim_core::usecase::edit_tracker::EditTracker;

use crate::gizmo::transform_gizmo_system;
use crate::model::{Model, Node, NodeId, NodeKind, Selection, Snapshot};

/// 覚えておく履歴の上限
const MAX_HISTORY: usize = 200;

pub struct HistoryPlugin;

impl Plugin for HistoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<History>()
            .add_systems(
                Update,
                (record_tracked_docs, apply_history_requests).chain(),
            )
            .add_systems(
                EguiPrimaryContextPass,
                finish_gesture_when_idle.after(transform_gizmo_system),
            );
    }
}

/// 履歴1件分の操作。`World` を受け取るので、どのリソースの変更でも表せる。
pub trait Command: Send + Sync + 'static {
    /// メニューに表示する説明 (例: "箱 1 を編集")
    fn label(&self) -> String;
    fn undo(&self, world: &mut World);
    fn redo(&self, world: &mut World);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Request {
    Undo,
    Redo,
}

#[derive(Resource, Default)]
pub struct History {
    undo_stack: Vec<Box<dyn Command>>,
    redo_stack: Vec<Box<dyn Command>>,
    /// ドラッグ中の編集 (編集前の写し)
    pending: Option<Snapshot>,
    /// メニューやショートカットから頼まれた操作。`apply_history_requests` が処理する。
    pub request: Option<Request>,
    /// 見張っている文書 (タイミングチャートなど)
    trackers: Vec<Box<dyn AnyTracker>>,
    /// マウスを離していて、入力欄にもフォーカスがない (編集の区切り)
    idle: bool,
}

impl History {
    /// 済ませた変更を履歴に積む。やり直しの履歴は捨てる。
    pub fn push(&mut self, command: impl Command) {
        self.push_boxed(Box::new(command));
    }

    fn push_boxed(&mut self, command: Box<dyn Command>) {
        self.redo_stack.clear();
        self.undo_stack.push(command);
        if self.undo_stack.len() > MAX_HISTORY {
            self.undo_stack.remove(0);
        }
    }

    /// 履歴を空にする (新規作成・開くとき)。見張っている文書は、次に見た状態から数え直す。
    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.pending = None;
        self.request = None;
        for tracker in &mut self.trackers {
            tracker.reset();
        }
    }

    /// 見張っている文書の変更を履歴に積む
    fn record_tracked(&mut self, world: &World) {
        let changes: Vec<_> = self
            .trackers
            .iter_mut()
            .filter_map(|t| t.poll(world))
            .collect();
        for change in changes {
            self.push_boxed(change);
        }
    }

    pub fn undo_label(&self) -> Option<String> {
        self.undo_stack.last().map(|c| c.label())
    }

    pub fn redo_label(&self) -> Option<String> {
        self.redo_stack.last().map(|c| c.label())
    }

    /// ノードを書き換える直前に呼ぶ。同じノードへの連続した編集は最初の状態だけを覚える。
    pub fn begin_edit(&mut self, model: &Model, id: NodeId) {
        if self.pending.as_ref().is_some_and(|p| p.root == id) {
            return;
        }
        self.finish_edit(model);
        self.pending = model.snapshot(id);
    }

    /// 覚えておいた編集を1件の履歴にする。値が変わっていなければ何も積まない。
    pub fn finish_edit(&mut self, model: &Model) {
        let Some(before) = self.pending.take() else {
            return;
        };
        let after = model.snapshot(before.root);
        if after.as_ref().is_some_and(|a| same_snapshot(a, &before)) {
            return;
        }
        let name = node_name(&before);
        self.push(ModelChange {
            label: format!("{name} を編集"),
            changes: vec![(Some(before), after)],
        });
    }

    /// 追加したノードを履歴に積む (箱の追加、STEP の読み込み)
    pub fn record_added(&mut self, model: &Model, id: NodeId, label: String) {
        self.finish_edit(model);
        self.push(ModelChange {
            label,
            changes: vec![(None, model.snapshot(id))],
        });
    }

    /// ノードを削除して履歴に積む
    pub fn remove(&mut self, model: &mut Model, id: NodeId) {
        self.finish_edit(model);
        let Some(before) = model.snapshot(id) else {
            return;
        };
        model.remove(id);
        self.push(ModelChange {
            label: format!("{} を削除", node_name(&before)),
            changes: vec![(Some(before), None)],
        });
    }

    /// すべて閉じて履歴に積む
    pub fn clear_all(&mut self, model: &mut Model) {
        self.finish_edit(model);
        let before = model.snapshot_all();
        if before.is_empty() {
            return;
        }
        model.clear();
        self.push(ModelChange {
            label: "すべて閉じる".into(),
            changes: removals(before),
        });
    }

    /// モデルを丸ごと作り直して履歴に積む (サンプル装置の読み込みなど)
    pub fn replace_all(
        &mut self,
        model: &mut Model,
        label: String,
        rebuild: impl FnOnce(&mut Model),
    ) {
        self.finish_edit(model);
        let before = model.snapshot_all();
        rebuild(model);
        let after = model.snapshot_all();
        let mut changes = removals(before);
        changes.extend(after.into_iter().map(|a| (None, Some(a))));
        self.push(ModelChange { label, changes });
    }
}

/// 最上位のノードをすべて消す変更。戻すときに元の並び順で戻るよう、後ろのノードから消す順に並べる
/// (戻すときは逆順に、前のノードから元の位置へ差し込まれる)。
fn removals(before: Vec<Snapshot>) -> Vec<(Option<Snapshot>, Option<Snapshot>)> {
    before.into_iter().rev().map(|b| (Some(b), None)).collect()
}

impl History {}

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
            && x.children == y.children
            && match (&x.kind, &y.kind) {
                (NodeKind::Box(p), NodeKind::Box(q)) => p == q,
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

/// モデルの変更1回分。(変更前, 変更後) の組を並べたもの。
/// 追加は変更前が None、削除は変更後が None。
pub struct ModelChange {
    pub label: String,
    pub changes: Vec<(Option<Snapshot>, Option<Snapshot>)>,
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

impl Command for ModelChange {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn undo(&self, world: &mut World) {
        let mut model = world.resource_mut::<Model>();
        for (before, after) in self.changes.iter().rev() {
            replace(&mut model, after, before);
        }
    }

    fn redo(&self, world: &mut World) {
        let mut model = world.resource_mut::<Model>();
        for (before, after) in &self.changes {
            replace(&mut model, before, after);
        }
    }
}

/// 1つ戻す / やり直す。戻せるものがなければ false。
pub fn run(world: &mut World, request: Request) -> bool {
    let done = world.resource_scope(|world, mut history: Mut<History>| {
        history.finish_edit(world.resource::<Model>());
        history.record_tracked(world);
        let history = &mut *history;
        let (from, to) = match request {
            Request::Undo => (&mut history.undo_stack, &mut history.redo_stack),
            Request::Redo => (&mut history.redo_stack, &mut history.undo_stack),
        };
        let Some(command) = from.pop() else {
            return false;
        };
        match request {
            Request::Undo => command.undo(world),
            Request::Redo => command.redo(world),
        }
        to.push(command);
        // 戻した状態を「変更なし」として覚え直す (戻したこと自体を履歴に積まないように)
        for tracker in &mut history.trackers {
            tracker.sync(world);
        }
        true
    });
    // 消えたノードを選んだままにしない
    world.resource_scope(|world, mut selection: Mut<Selection>| {
        let model = world.resource::<Model>();
        if selection.node.is_some_and(|id| model.get(id).is_none()) {
            selection.clear();
        }
        if selection.hovered.is_some_and(|id| model.get(id).is_none()) {
            selection.hovered = None;
        }
    });
    done
}

/// メニューやショートカット (shortcuts.rs) から頼まれた操作を処理する
fn apply_history_requests(world: &mut World) {
    if let Some(request) = world.resource_mut::<History>().request.take() {
        run(world, request);
    }
}

/// マウスを離し、入力欄からもフォーカスが外れたら、ドラッグ中の編集を1件にまとめる
fn finish_gesture_when_idle(
    mut contexts: EguiContexts,
    buttons: Res<ButtonInput<MouseButton>>,
    model: Res<Model>,
    mut history: ResMut<History>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let idle = !buttons.pressed(MouseButton::Left)
        && !ctx.egui_is_using_pointer()
        && ctx.memory(|m| m.focused().is_none());
    if history.idle != idle {
        history.idle = idle;
    }
    if idle && history.pending.is_some() {
        history.finish_edit(&model);
    }
    Ok(())
}

/// 編集の区切りで、見張っている文書の変更を履歴に積む
fn record_tracked_docs(world: &mut World) {
    world.resource_scope(|world, mut history: Mut<History>| {
        if history.idle && !history.trackers.is_empty() {
            history.bypass_change_detection().record_tracked(world);
        }
    });
}

// ---------------------------------------------------------------------------
// 見張る文書
// ---------------------------------------------------------------------------

/// 画面のあちこちで編集される文書 (タイミングチャート・工程モデルなど)。
/// `app.track_doc::<D>()` で登録すると、変更が自動で元に戻す履歴に載る。
pub trait TrackedDoc: Send + Sync + 'static {
    /// 比べる・戻すための写し。毎フレーム作るので軽く写せる形にする (大きな中身は `Arc` で共有する)
    type Value: Clone + Send + Sync + 'static;
    /// 履歴の説明 (例: "タイミングチャートを編集")
    const LABEL: &'static str;
    fn read(world: &World) -> Self::Value;
    fn write(world: &mut World, value: Self::Value);
    fn same(a: &Self::Value, b: &Self::Value) -> bool;
}

pub trait TrackDocExt {
    fn track_doc<D: TrackedDoc>(&mut self) -> &mut Self;
}

impl TrackDocExt for App {
    fn track_doc<D: TrackedDoc>(&mut self) -> &mut Self {
        self.init_resource::<History>();
        self.world_mut()
            .resource_mut::<History>()
            .trackers
            .push(Box::new(Tracker::<D> {
                inner: EditTracker::default(),
                doc: PhantomData,
            }));
        self
    }
}

trait AnyTracker: Send + Sync {
    fn poll(&mut self, world: &World) -> Option<Box<dyn Command>>;
    fn sync(&mut self, world: &World);
    fn reset(&mut self);
}

struct Tracker<D: TrackedDoc> {
    inner: EditTracker<D::Value>,
    doc: PhantomData<fn() -> D>,
}

impl<D: TrackedDoc> AnyTracker for Tracker<D> {
    fn poll(&mut self, world: &World) -> Option<Box<dyn Command>> {
        let (before, after) = self.inner.observe(D::read(world), D::same)?;
        Some(Box::new(DocChange::<D> { before, after }))
    }

    fn sync(&mut self, world: &World) {
        self.inner.sync(D::read(world));
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

/// 見張っている文書の変更1回分
struct DocChange<D: TrackedDoc> {
    before: D::Value,
    after: D::Value,
}

impl<D: TrackedDoc> Command for DocChange<D> {
    fn label(&self) -> String {
        D::LABEL.into()
    }

    fn undo(&self, world: &mut World) {
        D::write(world, self.before.clone());
    }

    fn redo(&self, world: &mut World) {
        D::write(world, self.after.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::StepSource;
    use crate::step_import::import_step;

    fn world() -> World {
        let mut world = World::new();
        world.insert_resource(Model::default());
        world.insert_resource(Selection::default());
        world.insert_resource(History::default());
        world
    }

    /// History と Model を同時に借りて f を呼ぶ
    fn edit<T>(world: &mut World, f: impl FnOnce(&mut Model, &mut History) -> T) -> T {
        world.resource_scope(|world, mut history: Mut<History>| {
            f(&mut world.resource_mut::<Model>(), &mut history)
        })
    }

    fn add_box(world: &mut World, size: f64) -> NodeId {
        edit(world, |m, h| {
            let id = m.add_box([size; 3], [0.0; 3]);
            h.record_added(m, id, "追加".into());
            id
        })
    }

    fn box_x(world: &World, id: NodeId) -> Option<f64> {
        match &world.resource::<Model>().get(id)?.kind {
            NodeKind::Box(part) => Some(part.position[0]),
            _ => None,
        }
    }

    fn set_box_x(model: &mut Model, id: NodeId, x: f64) {
        if let Some(NodeKind::Box(part)) = model.get_mut(id).map(|n| &mut n.kind) {
            part.position[0] = x;
        }
    }

    #[test]
    fn undo_and_redo_add() {
        let mut world = world();
        let id = add_box(&mut world, 1.0);
        assert!(run(&mut world, Request::Undo));
        assert!(world.resource::<Model>().is_empty());
        assert!(run(&mut world, Request::Redo));
        assert_eq!(box_x(&world, id), Some(0.0));
        assert!(!run(&mut world, Request::Redo));
    }

    #[test]
    fn undo_delete_restores_order_and_redo_clears_selection() {
        let mut world = world();
        let a = add_box(&mut world, 1.0);
        let b = add_box(&mut world, 2.0);
        edit(&mut world, |m, h| h.remove(m, a));
        assert_eq!(world.resource::<Model>().roots, vec![b]);
        assert!(run(&mut world, Request::Undo));
        assert_eq!(world.resource::<Model>().roots, vec![a, b]);

        world.resource_mut::<Selection>().select(a);
        assert!(run(&mut world, Request::Redo));
        assert_eq!(world.resource::<Selection>().node, None);
    }

    #[test]
    fn drag_becomes_one_step() {
        let mut world = world();
        let id = add_box(&mut world, 1.0);
        edit(&mut world, |m, h| {
            for x in 1..=10 {
                h.begin_edit(m, id);
                set_box_x(m, id, x as f64);
            }
            h.finish_edit(m);
        });
        assert_eq!(world.resource::<History>().undo_stack.len(), 2);
        assert!(run(&mut world, Request::Undo));
        assert_eq!(box_x(&world, id), Some(0.0));
        assert!(run(&mut world, Request::Redo));
        assert_eq!(box_x(&world, id), Some(10.0));
    }

    #[test]
    fn edit_without_change_is_not_recorded() {
        let mut world = world();
        let id = add_box(&mut world, 1.0);
        edit(&mut world, |m, h| {
            h.begin_edit(m, id);
            h.finish_edit(m);
        });
        assert_eq!(world.resource::<History>().undo_stack.len(), 1);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut world = world();
        let id = add_box(&mut world, 1.0);
        add_box(&mut world, 2.0);
        run(&mut world, Request::Undo);
        edit(&mut world, |m, h| {
            h.begin_edit(m, id);
            set_box_x(m, id, 5.0);
            h.finish_edit(m);
        });
        assert!(!run(&mut world, Request::Redo));
    }

    #[test]
    fn physics_only_edit_is_recorded() {
        let mut world = world();
        let id = add_box(&mut world, 1.0);
        edit(&mut world, |m, h| {
            h.begin_edit(m, id);
            m.get_mut(id).unwrap().physics.mode = crate::physics::BodyMode::Dynamic;
            h.finish_edit(m);
        });
        assert_eq!(world.resource::<History>().undo_stack.len(), 2);
        assert!(run(&mut world, Request::Undo));
        let mode = world.resource::<Model>().get(id).unwrap().physics.mode;
        assert_eq!(mode, crate::physics::BodyMode::Mechanism);
    }

    #[test]
    fn replace_all_keeps_order_on_undo() {
        let mut world = world();
        let a = add_box(&mut world, 1.0);
        let b = add_box(&mut world, 2.0);
        let c = add_box(&mut world, 3.0);
        let new = edit(&mut world, |m, h| {
            let mut new = 0;
            h.replace_all(m, "サンプル装置".into(), |m| {
                m.clear();
                new = m.add_box([4.0; 3], [0.0; 3]);
            });
            new
        });
        assert_eq!(world.resource::<Model>().roots, vec![new]);
        assert!(run(&mut world, Request::Undo));
        assert_eq!(world.resource::<Model>().roots, vec![a, b, c]);
        assert!(run(&mut world, Request::Redo));
        assert_eq!(world.resource::<Model>().roots, vec![new]);
        // すべて閉じるも元の並びで戻る
        assert!(run(&mut world, Request::Undo));
        edit(&mut world, |m, h| h.clear_all(m));
        assert!(run(&mut world, Request::Undo));
        assert_eq!(world.resource::<Model>().roots, vec![a, b, c]);
    }

    /// 見張る文書の例: 数値1つのリソース
    #[derive(Resource, Default)]
    struct Counter(i32);

    impl TrackedDoc for Counter {
        type Value = i32;
        const LABEL: &'static str = "数を編集";
        fn read(world: &World) -> i32 {
            world.resource::<Counter>().0
        }
        fn write(world: &mut World, value: i32) {
            world.resource_mut::<Counter>().0 = value;
        }
        fn same(a: &i32, b: &i32) -> bool {
            a == b
        }
    }

    fn tracked_app() -> App {
        let mut app = App::new();
        app.init_resource::<Model>()
            .init_resource::<Selection>()
            .init_resource::<Counter>()
            .track_doc::<Counter>()
            .add_systems(
                Update,
                (record_tracked_docs, apply_history_requests).chain(),
            );
        app.world_mut().resource_mut::<History>().idle = true;
        app.update();
        app
    }

    fn counter(app: &App) -> i32 {
        app.world().resource::<Counter>().0
    }

    fn set_counter(app: &mut App, value: i32) {
        app.world_mut().resource_mut::<Counter>().0 = value;
    }

    fn request(app: &mut App, request: Request) {
        app.world_mut().resource_mut::<History>().request = Some(request);
        app.update();
    }

    #[test]
    fn tracked_doc_edits_become_history_steps() {
        let mut app = tracked_app();
        set_counter(&mut app, 1);
        app.update();
        // ドラッグ中 (idle でない) の変更はまとめて1件
        app.world_mut().resource_mut::<History>().idle = false;
        for v in 2..=5 {
            set_counter(&mut app, v);
            app.update();
        }
        app.world_mut().resource_mut::<History>().idle = true;
        app.update();
        let history = app.world().resource::<History>();
        assert_eq!(history.undo_stack.len(), 2);
        assert_eq!(history.undo_label().as_deref(), Some("数を編集"));

        request(&mut app, Request::Undo);
        assert_eq!(counter(&app), 1);
        // 戻したこと自体は履歴に積まない
        app.update();
        assert_eq!(app.world().resource::<History>().redo_stack.len(), 1);
        request(&mut app, Request::Undo);
        assert_eq!(counter(&app), 0);
        request(&mut app, Request::Redo);
        request(&mut app, Request::Redo);
        assert_eq!(counter(&app), 5);
    }

    #[test]
    fn undo_flushes_unfinished_tracked_edit_and_clear_restarts() {
        let mut app = tracked_app();
        app.world_mut().resource_mut::<History>().idle = false;
        set_counter(&mut app, 7);
        app.update();
        // 入力欄にフォーカスがあるまま Ctrl+Z: 打ち込んだ値を1件にしてから戻す
        request(&mut app, Request::Undo);
        assert_eq!(counter(&app), 0);

        // 開き直したあとは、その状態から数える
        app.world_mut().resource_mut::<History>().idle = true;
        set_counter(&mut app, 3);
        app.world_mut().resource_mut::<History>().clear();
        app.update();
        assert!(app.world().resource::<History>().undo_label().is_none());
        set_counter(&mut app, 4);
        app.update();
        request(&mut app, Request::Undo);
        assert_eq!(counter(&app), 3);
    }

    #[test]
    fn undo_delete_of_sub_assembly_and_clear_all() {
        let file = import_step(
            include_str!("../assets/sample_assembly.step"),
            "sample_assembly.step",
        )
        .unwrap();
        let mut world = world();
        let root = edit(&mut world, |m, h| {
            let root = m.add_imported(file.root, file.unit, StepSource::Sample);
            h.record_added(m, root, "読み込み".into());
            root
        });
        let parts = world.resource::<Model>().measure(root).part_count;
        let child = world.resource::<Model>().get(root).unwrap().children[1];

        edit(&mut world, |m, h| h.remove(m, child));
        assert!(world.resource::<Model>().measure(root).part_count < parts);
        assert!(run(&mut world, Request::Undo));
        let model = world.resource::<Model>();
        assert_eq!(model.measure(root).part_count, parts);
        assert_eq!(model.get(root).unwrap().children[1], child);

        edit(&mut world, |m, h| h.clear_all(m));
        assert!(world.resource::<Model>().is_empty());
        assert!(run(&mut world, Request::Undo));
        assert_eq!(world.resource::<Model>().measure(root).part_count, parts);
    }
}
