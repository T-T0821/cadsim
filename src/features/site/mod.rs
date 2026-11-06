//! 現場のデータ: 床に敷く平面図 (DXF / PDF / 画像) と縮尺合わせ、計測した点群 (PLY / LAS / LAZ / E57)
//!
//! | ファイル | 役割 |
//! | --- | --- |
//! | `plantwright_core::site::underlay` | 下敷きの配置と縮尺合わせの計算 |
//! | `plantwright_core::site::pointcloud` | 点群の間引き・LOD・設備との干渉と距離 |
//! | `plantwright_core::site::model` | プロジェクトに属する現場データの入れ物 |
//! | `cloud_view.rs` | 点群の 3D 表示 (LOD) と、干渉チェックの実行・表示 |
//! | `view.rs` | 下敷きの 3D 表示 |
//! | `panel.rs` | 「平面図」パネル |
//!
//! ファイルの読み込みは `formats`、保存は project.rs が受け持つ。
//! ここは Bevy のシステムで「入力を集める → ドメインを呼ぶ → 表示に写す」だけをする。

mod cloud_view;
mod panel;
mod view;

use plantwright_core::site::{model, pointcloud, underlay};

use std::path::PathBuf;
use std::sync::{
    Mutex,
    mpsc::{Receiver, Sender, channel},
};

use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::EguiPrimaryContextPass;

use crate::document::history::{TrackDocExt, TrackedDoc};
use crate::document::import::ImportJobs;
use crate::viewport::camera::{CameraRequest, ViewportRect, alt_pressed, pointer_in_3d_view};
use model::Site;
use plantwright_core::model::Model;
use plantwright_core::model::file_name;
use plantwright_core::site::plan_file::read_plan_file;
use plantwright_core::site::pointcloud_read::{default_settings, is_cloud_file, read_cloud};
use pointcloud::{CloudSettings, PlacedCloud, PointCloud};
use underlay::{PlanContent, Underlay, unit_in_mm};

pub struct SitePlugin;

impl Plugin for SitePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SiteJobs>()
            .track_doc::<SiteDoc>()
            .init_resource::<PlanTool>()
            .init_resource::<cloud_view::CloudDisplay>()
            .init_resource::<cloud_view::CloudCheck>()
            .add_systems(
                Update,
                (
                    start_loading,
                    receive_loaded,
                    pick_plan_point
                        .run_if(pointer_in_3d_view)
                        .run_if(plan_tool_on),
                    view::sync_underlays,
                    view::draw_plan_tool,
                    cloud_view::sync_clouds,
                    cloud_view::run_cloud_check,
                    cloud_view::draw_cloud_hits,
                )
                    .chain(),
            )
            .add_systems(EguiPrimaryContextPass, panel::site_panel);
    }
}

/// 平面図の下敷きと点群の追加・削除・配置の変更を元に戻す履歴に載せる。
/// 図面と点の中身は `Arc` で共有しているので、写しても重くない。
struct SiteDoc;

impl TrackedDoc for SiteDoc {
    type Value = Site;
    const LABEL: &'static str = "平面図・点群を編集";

    fn read(world: &World) -> Site {
        world.resource::<Model>().site.clone()
    }

    fn write(world: &mut World, mut value: Site) {
        let mut model = world.resource_mut::<Model>();
        // 表示を作り直してもらうため、番号は今より進める
        value.revision = model.site.revision + 1;
        model.site = value;
    }

    fn same(a: &Site, b: &Site) -> bool {
        a.same_layout(b)
    }
}

/// 別スレッドで読んだもの
enum SiteLoaded {
    Plan(PlanContent),
    Cloud(PointCloud, CloudSettings),
}

/// 読み込みの結果 (ファイル名, 読み込み元, 中身 or エラー文)
type SiteResult = (String, PathBuf, Result<SiteLoaded, String>);

#[derive(Resource)]
struct SiteJobs {
    sender: Sender<SiteResult>,
    receiver: Mutex<Receiver<SiteResult>>,
}

impl Default for SiteJobs {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver: Mutex::new(receiver),
        }
    }
}

/// 「開く」で選ばれた平面図・点群を別スレッドで読む
fn start_loading(mut jobs: ResMut<ImportJobs>, site_jobs: Res<SiteJobs>, model: Res<Model>) {
    let model_mm = unit_in_mm(&model.unit);
    for path in std::mem::take(&mut jobs.site_files) {
        let name = file_name(&path);
        jobs.loading.push(name.clone());
        let sender = site_jobs.sender.clone();
        let mut settings = default_settings(&path, model_mm);
        // PLY は単位を持たないので、OBJ / STL と同じ単位の設定で読む
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("ply"))
        {
            settings.unit_mm = jobs.units.meters_per_unit * 1000.0;
            settings.z_up = jobs.units.z_up;
        }
        std::thread::spawn(move || {
            let result = if is_cloud_file(&path) {
                read_cloud(&path, &settings, model_mm).map(|c| SiteLoaded::Cloud(c, settings))
            } else {
                read_plan_file(&path).map(SiteLoaded::Plan)
            };
            let _ = sender.send((name, path, result));
        });
    }
}

fn receive_loaded(
    mut jobs: ResMut<ImportJobs>,
    site_jobs: Res<SiteJobs>,
    mut model: ResMut<Model>,
    mut dock: ResMut<crate::ui::dock::Dock>,
    mut camera: MessageWriter<CameraRequest>,
) {
    let finished: Vec<SiteResult> = match site_jobs.receiver.lock() {
        Ok(receiver) => receiver.try_iter().collect(),
        Err(_) => return,
    };
    for (name, path, result) in finished {
        jobs.loading.retain(|n| *n != name);
        let message = match result {
            Ok(SiteLoaded::Plan(content)) => add_plan(&mut model, name, path, content),
            Ok(SiteLoaded::Cloud(cloud, settings)) => {
                let kept = cloud.points.len();
                let seen = cloud.source_points;
                let spacing = cloud.voxel;
                model
                    .site
                    .add_cloud(PlacedCloud::new(name.clone(), path, cloud, settings));
                format!(
                    "{name} を点群として読み込みました ({} 点 → {} 点に間引き、間隔 {} {})",
                    format_count(seen),
                    format_count(kept as u64),
                    plantwright_core::drawing::format_length(spacing),
                    model.unit
                )
            }
            Err(err) => {
                error!("{name}: {err}");
                jobs.messages.push(format!("{name}: {err}"));
                continue;
            }
        };
        dock.set_open(plantwright_core::workspace::Tool::Site, true);
        camera.write(CameraRequest::FitAll);
        jobs.messages.push(message);
    }
}

/// 点の数を「1234万」のように短く書く
pub fn format_count(n: u64) -> String {
    if n >= 100_000_000 {
        format!("{:.1}億", n as f64 / 1e8)
    } else if n >= 10_000 {
        format!("{}万", n / 10_000)
    } else {
        n.to_string()
    }
}

fn add_plan(model: &mut Model, name: String, path: PathBuf, content: PlanContent) -> String {
    let known_scale = content.unit_mm.is_some();
    let lines = content.segment_count();
    let model_mm = unit_in_mm(&model.unit);
    let mut underlay = Underlay::new(name.clone(), path, content, model_mm);
    // 単位の分からない画像は、幅が 10 m になる大きさで敷いておく
    if !known_scale
        && let Some((min, max)) = underlay.content.bounds()
        && max.x > min.x
    {
        underlay.scale = 10_000.0 / model_mm / (max.x - min.x);
    }
    // 図面の左下をワールドの原点に合わせる
    if let Some((min, _)) = underlay.content.bounds() {
        underlay.place(min, DVec3::ZERO);
    }
    model.site.add_underlay(underlay);
    let mut message = format!("{name} を平面図として敷きました");
    if lines > 0 {
        message += &format!(" (線 {lines} 本)");
    }
    if !known_scale {
        message += "。「平面図・点群」パネルの「縮尺合わせ」で実寸に合わせてください";
    }
    message
}

/// 「平面図」パネルの状態と、縮尺合わせの道具
#[derive(Resource)]
pub struct PlanTool {
    /// 縮尺合わせ中の下敷きの番号
    pub calibrating: Option<usize>,
    /// クリックした点 (ワールド)。2点そろったら距離を入れてもらう
    pub points: Vec<DVec3>,
    /// カーソルの下の床の点 (線の端点に吸い付いた後)
    pub hover: Option<DVec3>,
    /// 2点間の実際の距離 (モデルの単位)
    pub distance: f64,
    /// 1点目をワールドの原点に移す
    pub move_to_origin: bool,
    /// 1点目 → 2点目の向きを X 軸に合わせる
    pub align_x: bool,
}

impl Default for PlanTool {
    fn default() -> Self {
        Self {
            calibrating: None,
            points: Vec::new(),
            hover: None,
            distance: 1000.0,
            move_to_origin: false,
            align_x: false,
        }
    }
}

impl PlanTool {
    pub fn start(&mut self, index: usize) {
        self.calibrating = Some(index);
        self.points.clear();
        self.hover = None;
    }

    pub fn stop(&mut self) {
        self.calibrating = None;
        self.points.clear();
        self.hover = None;
    }

    /// 縮尺合わせを下敷きに当てはめる
    pub fn apply(&mut self, underlay: &mut Underlay) -> bool {
        let [a, b] = match self.points.as_slice() {
            [a, b] => [*a, *b],
            _ => return false,
        };
        if !underlay.calibrate(a, b, self.distance) {
            return false;
        }
        // 縮尺を変えたので2点目の位置も変わる。図面の座標で持っておく
        let plan_b = underlay.to_plan(b);
        let b = underlay.to_world(plan_b);
        if self.align_x {
            underlay.align_to_x(a, b);
        }
        if self.move_to_origin {
            let plan_a = underlay.to_plan(a);
            underlay.place(plan_a, DVec3::ZERO);
        }
        self.stop();
        true
    }
}

/// 縮尺合わせ中か (3D ビューのクリックを部品の選択に使わない)
pub fn plan_tool_on(tool: Res<PlanTool>) -> bool {
    tool.calibrating.is_some()
}

pub fn plan_tool_off(tool: Res<PlanTool>) -> bool {
    tool.calibrating.is_none()
}

/// カーソルの下の床 (下敷きの高さ) の点を求め、クリックで縮尺合わせの点にする
fn pick_plan_point(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    viewport: Res<ViewportRect>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    model: Res<Model>,
    mut tool: ResMut<PlanTool>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        if tool.points.pop().is_none() {
            tool.stop();
        }
        return;
    }
    let Some(underlay) = tool.calibrating.and_then(|i| model.site.underlays.get(i)) else {
        tool.stop();
        return;
    };
    let Some(cursor) = window.cursor_position().filter(|&c| viewport.contains(c)) else {
        tool.hover = None;
        return;
    };
    let (camera, camera_transform) = *camera;
    let Ok(ray) = camera.viewport_to_world(camera_transform, cursor) else {
        return;
    };
    let origin = ray.origin.as_dvec3();
    let dir = ray.direction.as_dvec3();
    if dir.y.abs() < 1e-9 {
        tool.hover = None;
        return;
    }
    let t = (underlay.elevation - origin.y) / dir.y;
    if t <= 0.0 {
        tool.hover = None;
        return;
    }
    let floor = origin + dir * t;
    // 吸い付く距離は、カーソルから 12 px ぶんの床の長さ
    let reach = camera
        .viewport_to_world(camera_transform, cursor + Vec2::new(12.0, 0.0))
        .ok()
        .map(|r| {
            let o = r.origin.as_dvec3();
            let d = r.direction.as_dvec3();
            let t = (underlay.elevation - o.y) / d.y;
            (o + d * t).distance(floor)
        })
        .unwrap_or(0.0);
    let point = underlay.nearest_vertex(floor, reach).unwrap_or(floor);
    tool.hover = Some(point);
    if buttons.just_pressed(MouseButton::Left) && !alt_pressed(&keys) && tool.points.len() < 2 {
        tool.points.push(point);
    }
}

impl PlanTool {
    /// ビューの下に出す案内 (縮尺合わせ中だけ)
    pub fn hint(&self) -> Option<String> {
        self.calibrating?;
        Some(
            match self.points.len() {
                0 => {
                    "平面図の上で、実際の長さが分かる線の1点目をクリックしてください (Esc でやめる)"
                }
                1 => "2点目をクリックしてください (Esc で1点目を取り消し)",
                _ => "2点の実際の距離をパネルに入れると縮尺が決まります",
            }
            .to_owned(),
        )
    }
}
