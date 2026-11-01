//! 複数人で同じ工場をレビューする (UI 層)
//!
//! 1人がホストになって待ち受け、ほかの人が参加する。互いの視点 (アバター)・マウスで指している点・
//! 「ここを見て」の印・選んだ部品・チャットを見せ合う。配置の変更を共有することもできる。
//!
//! - 受け渡しの規則と相手の様子の組み立ては `cadsim_core::review`
//! - TCP と JSON は `cadsim_io::review`
//! - ここは「入力を集めて送る → 届いたものを Resource とカメラ・モデルに写す」だけ。
//!   3D の表示は `view.rs`、パネルは `panel.rs`。

mod panel;
mod view;

use std::collections::HashMap;

use bevy::{input::mouse::AccumulatedMouseScroll, prelude::*, window::PrimaryWindow};
use bevy_egui::EguiPrimaryContextPass;
use std::f32::consts::FRAC_PI_2;

use cadsim_core::camera_path::CameraPose;
use cadsim_core::model::NodeId;
use cadsim_core::review::{
    self as core_review, DEFAULT_PORT, Effect, Hub, Message, PROTOCOL_VERSION, PlacementTracker,
    ReviewSession, SendThrottle, ViewPose,
};
use cadsim_io::review::{LinkEvent, ReviewLink, local_addresses, parse_address};

use crate::model::{Model, Selection};
use crate::view::camera::{OrbitCamera, ViewportRect, alt_pressed};
use crate::view::viewport::PartPicker;

pub struct ReviewPlugin;

impl Plugin for ReviewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Review>()
            .add_systems(
                Update,
                (
                    receive,
                    (share_view, share_pointer, share_selection, share_placements),
                    view::draw,
                )
                    .chain(),
            )
            .add_systems(EguiPrimaryContextPass, (panel::review_panel, view::labels));
    }
}

/// パネルに打ち込む設定
#[derive(Clone, Debug)]
pub struct ReviewForm {
    pub name: String,
    pub passphrase: String,
    /// ホストの待ち受けのポート
    pub port: String,
    /// 参加するホストのアドレス
    pub address: String,
    /// 配置の変更も送る・受け取る
    pub share_placement: bool,
}

impl Default for ReviewForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            passphrase: String::new(),
            port: DEFAULT_PORT.to_string(),
            address: String::new(),
            share_placement: false,
        }
    }
}

/// 複数人レビューの状態 (UI 層の包み)
#[derive(Resource, Default)]
pub struct Review {
    pub session: ReviewSession,
    link: Option<ReviewLink>,
    pub form: ReviewForm,
    /// 最後に切れた・つながらなかった理由
    pub status: Option<String>,
    /// チャットの入力欄
    pub chat_input: String,
    /// ホストのとき、参加する人に伝えるアドレス
    pub host_addresses: Vec<String>,
    /// 自分のマウスが指している 3D の点 (「ここを見て」の印を置く場所)
    pointer: Option<[f32; 3]>,
    view_throttle: SendThrottle<ViewPose>,
    pointer_throttle: SendThrottle<Option<[f32; 3]>>,
    /// 最後に送った選択 (None はまだ送っていない)
    sent_selection: Option<Option<String>>,
    placement: PlacementTracker,
    labels: PartLabels,
    /// 配置を最後に調べた時刻
    placement_checked_at: f64,
    /// まだ調べていない配置の変更がある
    placement_dirty: bool,
}

/// 部品のツリーの道順 ("組立 / 部品") と番号の対応。部品を番号ではなく道順で受け渡すために使う
#[derive(Default)]
struct PartLabels {
    by_id: Vec<(NodeId, String)>,
    by_label: HashMap<String, Option<NodeId>>,
    /// 作ったときのノード数と時刻
    built_for: Option<(usize, f64)>,
}

impl PartLabels {
    /// ノードの数が変わったか、モデルが変わってから少したったら作り直す
    fn refresh(&mut self, model: &Model, model_changed: bool, now: f64) {
        let count = model.all_nodes().len();
        let stale = match self.built_for {
            None => true,
            Some((built_count, at)) => built_count != count || (model_changed && now - at > 0.5),
        };
        if !stale {
            return;
        }
        self.by_id = model
            .all_nodes()
            .into_iter()
            .map(|id| (id, model.path_label(id)))
            .collect();
        self.by_label.clear();
        for (id, label) in &self.by_id {
            self.by_label
                .entry(label.clone())
                // 同じ道順が 2 つ以上あれば、どれか分からないので None
                .and_modify(|v| *v = None)
                .or_insert(Some(*id));
        }
        self.built_for = Some((count, now));
    }

    fn find(&self, label: &str) -> Option<NodeId> {
        self.by_label.get(label).copied().flatten()
    }
}

impl Review {
    pub fn connected(&self) -> bool {
        self.link.is_some()
    }

    pub fn is_host(&self) -> bool {
        self.link.as_ref().is_some_and(ReviewLink::is_host)
    }

    /// ホストとして待ち受けているポート
    pub fn host_port(&self) -> Option<u16> {
        self.link.as_ref().and_then(ReviewLink::port)
    }

    fn my_name(&self) -> String {
        core_review::clean_name(&self.form.name)
    }

    /// ホストになる
    pub fn start_host(&mut self, project: &str) {
        let Ok(port) = self.form.port.trim().parse::<u16>() else {
            self.status = Some("ポート番号は 1〜65535 の数で書いてください".into());
            return;
        };
        let hub = Hub::new(&self.my_name(), &self.form.passphrase, project);
        match ReviewLink::host(port, hub) {
            Ok(link) => {
                let port = link.port().unwrap_or(port);
                self.host_addresses = local_addresses()
                    .into_iter()
                    .map(|ip| std::net::SocketAddr::new(ip, port).to_string())
                    .collect();
                self.begin(link);
            }
            Err(e) => {
                self.status = Some(format!("ポート {port} で待ち受けられません: {e}"));
            }
        }
    }

    /// ホストへ参加する
    pub fn join(&mut self, project: &str) {
        if self.form.address.trim().is_empty() {
            self.status = Some("ホストのアドレスを書いてください".into());
            return;
        }
        let address = parse_address(&self.form.address, DEFAULT_PORT);
        let hello = Message::Hello {
            version: PROTOCOL_VERSION,
            name: self.my_name(),
            passphrase: self.form.passphrase.clone(),
            project: project.to_string(),
        };
        self.begin(ReviewLink::join(&address, hello));
    }

    /// 抜ける (ホストなら全員の接続が切れる)
    pub fn leave(&mut self) {
        self.link = None;
        self.session.reset();
        self.status = None;
    }

    fn begin(&mut self, link: ReviewLink) {
        self.link = Some(link);
        self.session.reset();
        self.session.rejected = None;
        self.status = None;
        self.view_throttle.reset();
        self.pointer_throttle.reset();
        self.sent_selection = None;
        self.placement.reset();
        self.placement_dirty = true;
    }

    fn send(&self, message: Message) {
        if self.session.joined()
            && let Some(link) = &self.link
        {
            link.send(message);
        }
    }

    /// チャットを送る
    pub fn send_chat(&mut self) {
        let text = core_review::truncate_chars(self.chat_input.trim(), core_review::MAX_CHAT_CHARS);
        if text.is_empty() || !self.session.joined() {
            return;
        }
        self.send(Message::Chat {
            id: 0,
            text: text.clone(),
        });
        let name = self.my_name();
        self.session.add_own_chat(&name, &text);
        self.chat_input.clear();
    }

    /// 全員の視点を自分の視点に合わせる
    pub fn summon(&self, orbit: &OrbitCamera) {
        self.send(Message::Summon {
            id: 0,
            pose: pose_of(orbit),
        });
    }

    /// 「配置の変更も共有する」を切り替えたら、今の配置から数え直す
    pub fn placement_sharing_changed(&mut self) {
        self.placement.reset();
        self.placement_dirty = true;
    }
}

/// 自分の視点
fn pose_of(orbit: &OrbitCamera) -> ViewPose {
    ViewPose {
        eye: orbit.eye().into(),
        focus: orbit.focus.into(),
        orthographic: orbit.orthographic,
    }
}

/// 届いたメッセージを反映し、視点を合わせる・配置を写す
fn receive(
    time: Res<Time>,
    mut review: ResMut<Review>,
    mut orbit: ResMut<OrbitCamera>,
    mut model: ResMut<Model>,
) {
    let now = time.elapsed_secs_f64();
    let review = review.bypass_change_detection();
    review.session.prune(now);
    let Some(link) = &review.link else {
        return;
    };
    let mut closed = None;
    let mut effects = Vec::new();
    for event in link.poll() {
        match event {
            LinkEvent::Message(message) => {
                if let Some(effect) = review.session.apply(message, now) {
                    effects.push(effect);
                }
            }
            LinkEvent::Closed(reason) => closed = Some(reason),
        }
    }
    for effect in effects {
        match effect {
            Effect::MoveCamera(pose) => {
                let (yaw, pitch, distance) = core_review::orbit_angles(&pose);
                orbit.orthographic = pose.orthographic;
                orbit.fly_to(CameraPose {
                    focus: pose.focus(),
                    yaw,
                    pitch: pitch.clamp(-FRAC_PI_2, FRAC_PI_2),
                    distance: distance.clamp(1e-3, 1e6),
                });
            }
            Effect::ApplyPlacement { part, matrix } => {
                if !review.form.share_placement {
                    continue;
                }
                review.labels.refresh(&model, model.is_changed(), now);
                let Some(id) = review.labels.find(&part) else {
                    continue;
                };
                if let Some(node) = model.get_mut(id) {
                    node.placement = bevy::math::DMat4::from_cols_array(&matrix);
                    review.placement.note(&part, matrix);
                }
            }
        }
    }
    if let Some(reason) = closed {
        review.status = Some(review.session.rejected.clone().unwrap_or(reason));
        review.link = None;
        review.session.reset();
    }
}

/// 自分の視点を送る。追いかけ中に自分でカメラを動かしたら追うのをやめる
#[allow(clippy::too_many_arguments)]
fn share_view(
    time: Res<Time>,
    mut review: ResMut<Review>,
    orbit: Res<OrbitCamera>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    scroll: Res<AccumulatedMouseScroll>,
    viewport: Res<ViewportRect>,
) {
    if !review.session.joined() {
        return;
    }
    let review = review.bypass_change_detection();
    if review.session.following.is_some() && !viewport.pointer_on_ui {
        let grabbed = buttons.any_just_pressed([MouseButton::Right, MouseButton::Middle])
            || (alt_pressed(&keys) && buttons.just_pressed(MouseButton::Left))
            || scroll.delta.y != 0.0;
        if grabbed {
            review.session.following = None;
        }
    }
    let pose = pose_of(&orbit);
    if review
        .view_throttle
        .offer(&pose, time.elapsed_secs_f64(), |a, b| a.moved_from(b))
    {
        review.send(Message::View { id: 0, pose });
    }
}

/// マウスで指している点を送る。P キーで「ここを見て」の印を置く
#[allow(clippy::too_many_arguments)]
fn share_pointer(
    time: Res<Time>,
    mut review: ResMut<Review>,
    window: Single<&Window, With<PrimaryWindow>>,
    viewport: Res<ViewportRect>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    picker: PartPicker,
    keys: Res<ButtonInput<KeyCode>>,
    recorder: Res<crate::view::recording::Recorder>,
) {
    if !review.session.joined() {
        return;
    }
    let review = review.bypass_change_detection();
    let now = time.elapsed_secs_f64();
    let point = window
        .cursor_position()
        .filter(|&c| viewport.contains(c) && !viewport.pointer_on_ui)
        .and_then(|cursor| {
            let (camera, transform) = *camera;
            camera.viewport_to_world(transform, cursor).ok()
        })
        .and_then(|ray| {
            picker.cast(ray).map(|hit| hit.point).or_else(|| {
                // 部品に当たらなければ床 (y = 0) を指す
                let t = -ray.origin.y / ray.direction.y;
                (ray.direction.y.abs() > 1e-6 && t > 0.0).then(|| ray.origin + *ray.direction * t)
            })
        })
        .map(<[f32; 3]>::from);
    review.pointer = point;
    if review
        .pointer_throttle
        .offer(&point, now, |a, b| pointer_moved(*a, *b))
    {
        review.send(Message::Pointer { id: 0, point });
    }
    let typing = viewport.typing_in_ui || recorder.is_capturing();
    let modifier = keys.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]);
    if !typing
        && !modifier
        && keys.just_pressed(KeyCode::KeyP)
        && let Some(point) = point
    {
        review.send(Message::Ping { id: 0, point });
        review.session.add_own_ping(point, now);
    }
}

fn pointer_moved(a: Option<[f32; 3]>, b: Option<[f32; 3]>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Vec3::from(a).distance(Vec3::from(b)) > 1e-4,
        (None, None) => false,
        _ => true,
    }
}

/// 選んだ部品を送る
fn share_selection(
    time: Res<Time>,
    mut review: ResMut<Review>,
    selection: Res<Selection>,
    model: Res<Model>,
) {
    if !review.session.joined() {
        return;
    }
    let review = review.bypass_change_detection();
    review
        .labels
        .refresh(&model, model.is_changed(), time.elapsed_secs_f64());
    let part = selection.node.map(|id| model.path_label(id));
    if review.sent_selection.as_ref() != Some(&part) {
        review.sent_selection = Some(part.clone());
        review.send(Message::Select { id: 0, part });
    }
}

/// 「配置の変更も共有する」がオンなら、動かした部品の配置を送る
fn share_placements(time: Res<Time>, mut review: ResMut<Review>, model: Res<Model>) {
    if !review.session.joined() || !review.form.share_placement {
        return;
    }
    let now = time.elapsed_secs_f64();
    let review = review.bypass_change_detection();
    review.placement_dirty |= model.is_changed();
    // 動かしている間は 1 秒に 20 回まで
    if !review.placement_dirty || now - review.placement_checked_at < 0.05 {
        return;
    }
    review.placement_checked_at = now;
    review.placement_dirty = false;
    review.labels.refresh(&model, model.is_changed(), now);
    let current = review.labels.by_id.iter().filter_map(|(id, label)| {
        model
            .get(*id)
            .map(|node| (label.clone(), node.placement.to_cols_array()))
    });
    let changes = review.placement.changes(current.collect::<Vec<_>>());
    for (part, matrix) in changes {
        review.send(Message::Placement {
            id: 0,
            part,
            matrix,
        });
    }
}
