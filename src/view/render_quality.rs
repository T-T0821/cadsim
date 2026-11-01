//! 3D ビューの描画の質 (UI 層): 影・環境光・SSAO・接地の影・ブルーム・アンチエイリアス
//!
//! どの段階で何を使うか、影や陰影をどの範囲に落とすかは `cadsim_core::render_quality` が決め、
//! ここではそれを 3D カメラと主光源のコンポーネントに写すだけにする。
//! 選んだ段階は `cadsim_io::render_settings` で設定フォルダの `render.json` に保存する。

use std::path::PathBuf;

use bevy::anti_alias::smaa::Smaa;
use bevy::camera::Hdr;
use bevy::core_pipeline::prepass::{DepthPrepass, NormalPrepass};
use bevy::light::{
    CascadeShadowConfigBuilder, DirectionalLightShadowMap, EnvironmentMapLight, GlobalAmbientLight,
    ShadowFilteringMethod,
};
use bevy::pbr::{
    ContactShadows, ScreenSpaceAmbientOcclusion, ScreenSpaceAmbientOcclusionQualityLevel,
};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy_egui::egui;

use cadsim_core::library::catalog::units_per_mm;
use cadsim_core::render_quality::{
    Antialias, AoLevel, OcclusionScale, RenderProfile, RenderQuality, ShadowRange, occlusion_scale,
    shadow_range, units_per_meter,
};
use cadsim_io::render_settings;

use super::camera::{CameraSystems, OrbitCamera};
use super::viewport::SceneCache;
use crate::model::Model;

/// 影を落とす主光源 (太陽の代わりの平行光)
#[derive(Component)]
pub struct KeyLight;

/// 環境光を使わないときの全体の明るさ (Bevy の標準と同じ)
const PLAIN_AMBIENT: f32 = 80.0;
/// 環境光を使うときの全体の明るさ。周りの色は環境光が受け持つので弱くする
const ENVIRONMENT_AMBIENT: f32 = 15.0;
/// 環境光の強さ (cd/m²)
const ENVIRONMENT_INTENSITY: f32 = 240.0;

pub struct RenderQualityPlugin;

impl Plugin for RenderQualityPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(RenderQualityUi::load())
            .init_resource::<FittedRanges>()
            .add_systems(Startup, create_environment)
            .add_systems(
                Update,
                (apply_quality, fit_ranges, save_quality)
                    .chain()
                    .after(CameraSystems),
            );
    }
}

/// 選んでいる画質と、その保存先
#[derive(Resource)]
pub struct RenderQualityUi {
    pub quality: RenderQuality,
    path: Option<PathBuf>,
    saved: RenderQuality,
    /// カメラに写した段階 (まだなら None)
    applied: Option<RenderQuality>,
}

impl RenderQualityUi {
    fn load() -> Self {
        let path = render_settings::default_path();
        let quality = path
            .as_deref()
            .map(render_settings::load)
            .unwrap_or_default();
        Self {
            quality,
            path,
            saved: quality,
            applied: None,
        }
    }
}

/// 環境光に使う、天井・壁・床の色の立方体マップ
#[derive(Resource)]
struct Environment(EnvironmentMapLight);

fn create_environment(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    // 上は高窓と天井の明るい白、横は建屋の壁の灰色、下はコンクリートの床の色
    let light = EnvironmentMapLight::hemispherical_gradient(
        &mut images,
        Color::srgb(0.92, 0.95, 1.0),
        Color::srgb(0.55, 0.57, 0.6),
        Color::srgb(0.36, 0.35, 0.33),
    )
    .with_intensity(ENVIRONMENT_INTENSITY);
    commands.insert_resource(Environment(light));
}

/// 段階が変わったら、カメラと主光源の機能を付け外しする
#[allow(clippy::too_many_arguments)]
fn apply_quality(
    mut commands: Commands,
    mut ui: ResMut<RenderQualityUi>,
    environment: Res<Environment>,
    mut shadow_map: ResMut<DirectionalLightShadowMap>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut fitted: ResMut<FittedRanges>,
    camera: Single<Entity, With<Camera3d>>,
    mut key_lights: Query<&mut DirectionalLight, With<KeyLight>>,
) {
    if ui.applied == Some(ui.quality) {
        return;
    }
    ui.applied = Some(ui.quality);
    let profile = ui.quality.profile();
    let mut camera = commands.entity(*camera);
    set_camera_features(&mut camera, &profile, &environment.0);

    if let Some(shadows) = profile.shadows {
        let size = shadows.map_size as usize;
        if shadow_map.size != size {
            shadow_map.size = size;
        }
    }
    ambient.brightness = if profile.environment_light {
        ENVIRONMENT_AMBIENT
    } else {
        PLAIN_AMBIENT
    };
    for mut light in &mut key_lights {
        light.shadow_maps_enabled = profile.shadows.is_some();
        light.contact_shadows_enabled = profile.contact_shadows;
    }
    // 影と陰影の範囲は、新しい段数などで作り直す
    *fitted = FittedRanges::default();
}

fn set_camera_features(
    camera: &mut EntityCommands,
    profile: &RenderProfile,
    environment: &EnvironmentMapLight,
) {
    match profile.antialias {
        Antialias::Msaa4 => {
            camera.insert(Msaa::Sample4).remove::<Smaa>();
        }
        Antialias::Smaa => {
            camera.insert((Msaa::Off, Smaa::default()));
        }
    }
    if profile.hdr() {
        camera.insert(Hdr);
    } else {
        camera.remove::<Hdr>();
    }
    if profile.bloom {
        camera.insert(Bloom {
            // 光るエフェクトと強い照り返しだけが少しにじむくらいに抑える
            intensity: 0.08,
            ..Bloom::NATURAL
        });
    } else {
        camera.remove::<Bloom>();
    }
    if profile.environment_light {
        camera.insert(environment.clone());
    } else {
        camera.remove::<EnvironmentMapLight>();
    }
    camera.insert(match profile.shadows {
        Some(s) if s.soft => ShadowFilteringMethod::Gaussian,
        _ => ShadowFilteringMethod::Hardware2x2,
    });
    match profile.ambient_occlusion {
        Some(level) => {
            camera.insert(ScreenSpaceAmbientOcclusion {
                quality_level: match level {
                    AoLevel::Low => ScreenSpaceAmbientOcclusionQualityLevel::Low,
                    AoLevel::Medium => ScreenSpaceAmbientOcclusionQualityLevel::Medium,
                    AoLevel::High => ScreenSpaceAmbientOcclusionQualityLevel::High,
                },
                ..default()
            });
        }
        None => {
            camera.remove::<(ScreenSpaceAmbientOcclusion, NormalPrepass)>();
        }
    }
    if profile.contact_shadows {
        camera.insert(ContactShadows::default());
    } else {
        camera.remove::<ContactShadows>();
    }
    // SSAO と接地の影が要る深さの前描きは、どちらも使わないなら外して軽くする
    if profile.ambient_occlusion.is_none() && !profile.contact_shadows {
        camera.remove::<DepthPrepass>();
    }
}

/// いまの影と陰影の範囲 (少し変わっただけでは作り直さない)
#[derive(Resource, Default)]
struct FittedRanges {
    shadow: Option<ShadowRange>,
    occlusion: Option<OcclusionScale>,
}

/// カメラの距離とシーンの大きさに合わせて、影の範囲と SSAO・接地の影の長さを決める
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn fit_ranges(
    mut commands: Commands,
    ui: Res<RenderQualityUi>,
    orbit: Res<OrbitCamera>,
    cache: Res<SceneCache>,
    model: Res<Model>,
    mut fitted: ResMut<FittedRanges>,
    mut key_lights: Query<(Entity, &mut DirectionalLight), With<KeyLight>>,
    mut camera: Single<
        (
            Option<&mut ScreenSpaceAmbientOcclusion>,
            Option<&mut ContactShadows>,
        ),
        With<Camera3d>,
    >,
) {
    let profile = ui.quality.profile();
    let meter = units_per_meter(units_per_mm(&model.unit));
    let radius = cache
        .scene_bounds
        .map_or(0.0, |b| (b.max - b.min).length() as f32 / 2.0);

    if let Some(shadows) = profile.shadows {
        let range = shadow_range(orbit.distance, radius, meter);
        if fitted.shadow.is_none_or(|old| range.differs_from(&old)) {
            fitted.shadow = Some(range);
            let config = CascadeShadowConfigBuilder {
                num_cascades: shadows.cascades as usize,
                minimum_distance: range.minimum,
                maximum_distance: range.maximum,
                first_cascade_far_bound: range.first_cascade_far,
                overlap_proportion: 0.2,
            }
            .build();
            for (entity, mut light) in &mut key_lights {
                light.shadow_depth_bias = range.depth_bias;
                commands.entity(entity).insert(config.clone());
            }
        }
    }

    let scale = occlusion_scale(orbit.distance, meter);
    if fitted.occlusion.is_none_or(|old| scale.differs_from(&old)) {
        let (ssao, contact) = &mut *camera;
        // 付け外しの命令がまだ届いていなければ、次のフレームでやり直す
        let expected = profile.ambient_occlusion.is_some() == ssao.is_some()
            && profile.contact_shadows == contact.is_some();
        if expected {
            fitted.occlusion = Some(scale);
        }
        if let Some(ssao) = ssao {
            ssao.radius = scale.ao_radius;
            ssao.constant_object_thickness = scale.ao_thickness;
        }
        if let Some(contact) = contact {
            contact.length = scale.contact_length;
            contact.thickness = scale.contact_thickness;
        }
    }
}

fn save_quality(mut ui: ResMut<RenderQualityUi>) {
    if ui.quality == ui.saved {
        return;
    }
    ui.saved = ui.quality;
    if let Some(path) = &ui.path
        && let Err(err) = render_settings::save(path, ui.quality)
    {
        warn!(
            "画質の設定を保存できませんでした ({}): {err}",
            path.display()
        );
    }
}

/// 「表示」メニューの画質の切り替え
pub fn show_menu_items(ui: &mut egui::Ui, render: &mut RenderQualityUi) {
    ui.menu_button(format!("画質: {}", render.quality.label()), |ui| {
        for quality in RenderQuality::ALL {
            if ui
                .radio(render.quality == quality, quality.label())
                .on_hover_text(quality.description())
                .clicked()
            {
                render.quality = quality;
                ui.close();
            }
        }
    })
    .response
    .on_hover_text("影や陰影の付け方を選ぶ。動きが重いときは「軽い」にする");
}
