//! 3D ビューの部品の当たり判定を Bevy の picking (`bevy_picking`) につなぐ
//!
//! - 当たりの計算は今までどおり `PartPicker` (部品の外接箱の BVH → 部品内の三角形の BVH) で行い、
//!   その結果を picking の backend として `PointerHits` に出す。Bevy の標準のメッシュの当たり判定
//!   (`MeshPickingPlugin`) は使わない。部品を1つずつ調べるので数千部品では重く、画面上で小さい部品の
//!   粗いメッシュ (LOD) に当たってしまうため。
//! - ホバー・押した・ドラッグなどのイベントや、egui の画面の下を指しているときに止めるのは Bevy に任せる
//!   (egui は bevy_egui が自分の backend で上に重ねる)。
//! - 当たった部品と面は `HitData` の `extra` に `PartHit` として載せる。
//!   クリックでの選択は `PointerPress`、計測・寸法・教示点・レビューの指さしは `PartUnderPointer`
//!   (Bevy の `HoverMap`) から読む。
//! - ダブルクリックでその部品を画面いっぱいに、右クリック (動かさずに離す) で部品のメニューを開く。
//!   判定は `plantwright_core::pointer` (`ClickWatch`)、メニューを描くのは `ui/context_menu.rs`。

use bevy::{
    ecs::system::SystemParam,
    picking::{
        PickingSystems,
        backend::{HitData, PointerHits, ray::RayMap},
        events::PointerPress,
        hover::HoverMap,
        pointer::{PointerButton, PointerId},
    },
    prelude::*,
};

use crate::document::selection::Selection;
use crate::features::annotation::annotate_tool_off;
use crate::features::modeling::hole_tool_off;
use crate::features::simulation::measure::not_measuring;
use crate::viewport::camera::{CameraRequest, ViewportRect, alt_pressed, pointer_in_3d_view};
use crate::viewport::gizmo::gizmo_not_focused;
use crate::viewport::parts::{PartHit, PartPicker};
use bevy::window::PrimaryWindow;
use plantwright_core::model::Model;
use plantwright_core::pointer::{ClickWatch, PartMenu, PointerLock, ViewOverlay};

pub struct PartPickingPlugin;

impl Plugin for PartPickingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClickWatch>()
            .init_resource::<ViewOverlay>()
            .init_resource::<PointerLock>()
            .add_systems(
                PreUpdate,
                part_picking_backend.in_set(PickingSystems::Backend),
            )
            .add_systems(
                Update,
                pick_part
                    .run_if(pointer_in_3d_view)
                    .run_if(gizmo_not_focused)
                    .run_if(not_measuring)
                    .run_if(annotate_tool_off)
                    .run_if(hole_tool_off)
                    .run_if(pointer_free)
                    .run_if(crate::features::motion::teach_tool_off)
                    .run_if(crate::features::site::plan_tool_off),
            )
            .add_systems(Update, context_click);
    }
}

/// picking の backend: 3D カメラから飛ばしたレイごとに、最初に当たった部品を知らせる
fn part_picking_backend(
    ray_map: Res<RayMap>,
    cameras: Query<&Camera, With<Camera3d>>,
    mut picker: PartPicker,
    mut output: MessageWriter<PointerHits>,
) {
    // レイが無いフレームでも部品の変更を取り込む (Changed の差分を見逃さないため)
    picker.refresh();
    for (&ray_id, &ray) in ray_map.iter() {
        let Ok(camera) = cameras.get(ray_id.camera) else {
            continue;
        };
        let Some(hit) = picker.cast(ray) else {
            continue;
        };
        let depth = (hit.point - ray.origin).dot(*ray.direction);
        let data = HitData::new_with_extra(ray_id.camera, depth, Some(hit.point), None, hit);
        output.write(PointerHits::new(
            ray_id.pointer,
            vec![(hit.entity, data)],
            camera.order as f32,
        ));
    }
}

/// マウスのカーソルの下にある部品 (Bevy の picking のホバー)。
/// egui の画面の下や 3D ビューの外では None。
#[derive(SystemParam)]
pub struct PartUnderPointer<'w> {
    hover: Res<'w, HoverMap>,
}

impl PartUnderPointer<'_> {
    pub fn hit(&self) -> Option<PartHit> {
        self.hover
            .get(&PointerId::Mouse)?
            .values()
            .find_map(|data| data.extra_as::<PartHit>().copied())
    }
}

/// 3D ビューで左ボタンを押したら、押した部品と面を選ぶ (何もないところなら選択を外す)
#[allow(clippy::too_many_arguments)]
fn pick_part(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut presses: MessageReader<PointerPress>,
    model: Res<Model>,
    time: Res<Time>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut clicks: ResMut<ClickWatch>,
    mut camera: MessageWriter<CameraRequest>,
    mut selection: ResMut<Selection>,
) {
    // 選択を止めている間 (ギズモの操作中など) に溜まったものは捨てる。
    // Alt + 左ドラッグはオービットなので選択しない
    if !buttons.just_pressed(MouseButton::Left) || alt_pressed(&keys) {
        presses.clear();
        return;
    }
    let hit = presses
        .read()
        .filter(|press| {
            press.pointer.id == PointerId::Mouse && press.button == PointerButton::Primary
        })
        .last()
        .and_then(|press| press.hit.extra_as::<PartHit>().copied());
    let double = window
        .cursor_position()
        .is_some_and(|p| clicks.left_pressed(p.into(), time.elapsed_secs_f64()));
    match hit {
        Some(hit) => {
            // 標準部品は本体ではなく部品全体を選ぶ (面の強調もしない)
            let target = model.pick_target(hit.node);
            selection.node = Some(target);
            selection.face = (target == hit.node).then_some(hit.face);
            selection.reveal_in_tree = true;
            // ダブルクリックでその部品を画面いっぱいに
            if double {
                camera.write(CameraRequest::FitSelection);
            }
        }
        None => selection.clear(),
    }
}

/// 3D でクリックを使う道具がないとき (`PointerLock` を参照)
pub fn pointer_free(lock: Res<PointerLock>) -> bool {
    lock.is_free()
}

/// 右ボタンを押して動かさずに離したら、その場所にメニューを開く (動かしたら見回しなので開かない)。
/// 部品の上なら部品のメニュー (その部品を選ぶ)、何もないところならビューのメニュー。
#[allow(clippy::too_many_arguments)]
fn context_click(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    viewport: Res<ViewportRect>,
    under_pointer: PartUnderPointer,
    model: Res<Model>,
    lock: Res<PointerLock>,
    mut clicks: ResMut<ClickWatch>,
    mut overlay: ResMut<ViewOverlay>,
    mut selection: ResMut<Selection>,
) {
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let at: [f32; 2] = cursor.into();
    if buttons.just_pressed(MouseButton::Right) {
        // メニューの上やパネルの上で押したとき、道具がクリックを使っているときは、
        // メニューを閉じるだけ (Alt + 右はズーム)
        if !viewport.pointer_on_ui
            && !alt_pressed(&keys)
            && overlay.menu.is_none()
            && lock.is_free()
        {
            clicks.right_pressed(at);
        } else {
            overlay.menu = None;
            clicks.right_released(at);
        }
    }
    if buttons.just_released(MouseButton::Right)
        && let Some(at) = clicks.right_released(at)
    {
        let node = under_pointer.hit().map(|hit| model.pick_target(hit.node));
        if let Some(node) = node {
            selection.select(node);
            selection.reveal_in_tree = true;
        }
        overlay.menu = Some(PartMenu { node, at });
    }
}
