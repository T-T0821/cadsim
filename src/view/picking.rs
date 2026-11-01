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

use crate::annotation::annotate_tool_off;
use crate::model::Selection;
use crate::simulation::measure::not_measuring;
use crate::view::camera::{alt_pressed, pointer_in_3d_view};
use crate::view::gizmo::gizmo_not_focused;
use crate::view::viewport::{PartHit, PartPicker};
use plantwright_core::model::Model;

pub struct PartPickingPlugin;

impl Plugin for PartPickingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
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
                .run_if(crate::motion::teach_tool_off)
                .run_if(crate::site::plan_tool_off),
        );
    }
}

/// picking の backend: 3D カメラから飛ばしたレイごとに、最初に当たった部品を知らせる
fn part_picking_backend(
    ray_map: Res<RayMap>,
    cameras: Query<&Camera, With<Camera3d>>,
    mut picker: PartPicker,
    mut output: MessageWriter<PointerHits>,
) {
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
fn pick_part(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut presses: MessageReader<PointerPress>,
    model: Res<Model>,
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
    match hit {
        Some(hit) => {
            // 標準部品は本体ではなく部品全体を選ぶ (面の強調もしない)
            let target = model.pick_target(hit.node);
            selection.node = Some(target);
            selection.face = (target == hit.node).then_some(hit.face);
            selection.reveal_in_tree = true;
        }
        None => selection.clear(),
    }
}
