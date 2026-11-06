//! 部品の「役割」: プロパティの「この部品を…にする」ボタンと、3D で向き・長さを決める道具
//!
//! 役割の種類・既定値・ドラッグの計算は `plantwright_core::part_role`。
//! ここはプロパティのボタン (`panel`)、マウスとキーの読み方 (`systems`)、3D の矢印・点・半透明の
//! 動いた先と、足りない設定の「!」 (`view`) だけを持つ。
//!
//! 道具の間は `PointerLock` を持ち、クリックでの選択・ギズモ・右クリックメニューを止める。案内はビューの下の
//! 共通の案内バー (`ui/view_hints.rs`) に出る。
//!
//! 確定 (Enter) するまではタイミングチャートとワーク搬送の設定に書かないので、Esc で何も残らない。
//! 確定した変更は、チャートとワーク搬送の設定の履歴 (TrackedDoc) に載るので Ctrl+Z で戻る。

pub mod panel;
pub mod systems;
pub mod view;

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;
use plantwright_core::part_role::RoleTool;

use crate::ui::font::install_japanese_font;
use crate::viewport::camera::keyboard_free;
use crate::viewport::gizmo::transform_gizmo_system;

pub struct PartRolePlugin;

impl Plugin for PartRolePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoleTool>()
            .init_gizmo_group::<view::RoleGizmos>()
            .add_systems(Startup, view::configure_gizmos)
            .add_systems(
                Update,
                (
                    systems::role_keys.run_if(keyboard_free),
                    systems::role_input,
                    systems::hold_pointer,
                    view::draw_preview,
                )
                    .chain(),
            )
            .add_systems(
                EguiPrimaryContextPass,
                (view::draw_labels, view::draw_issue_marks)
                    .after(install_japanese_font)
                    .after(transform_gizmo_system),
            );
    }
}
