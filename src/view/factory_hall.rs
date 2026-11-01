//! 3D ビューの背景の工場の建屋 (UI 層): コンクリートの床・通路の線・壁・柱・高窓と、メートルのグリッド
//!
//! 形は `plantwright_core::factory_hall` が決め、ここではメッシュにして置くだけにする。
//! 形が変わったとき (広さの設定・モデルの外接箱が 5 m の区切りをまたいだとき) だけ作り直す。
//! 建屋は mm で作り、モデルの単位に合わせて親の Transform で縮める。

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    light::NotShadowCaster,
    math::{DVec2, DVec3},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use plantwright_core::factory_hall::{
    self, COLUMN_GUARD_HEIGHT, COLUMN_SIZE, CONCRETE_TILE, HallLayout, MarkingKind, PanelKind,
    Side, Sides, WALL_HEIGHT,
};
use plantwright_core::library::catalog::units_per_mm;
use plantwright_core::math::Aabb;

use plantwright_core::model::Model;

/// 建屋のメッシュの目印。ライブラリの部品を落とす先の判定などから外す
#[derive(Component)]
pub struct HallMesh;

/// 壁の側に属する部品。カメラがその壁の外にいるときは隠す
#[derive(Component)]
pub struct HallPiece(Sides);

/// 建屋の根のエンティティ
#[derive(Component)]
pub struct HallRoot;

/// いま置いている建屋の形 (mm) と、モデルの単位への縮尺
#[derive(Resource, Default)]
pub struct Hall {
    pub layout: Option<HallLayout>,
    /// モデルの単位 / mm
    pub scale: f64,
}

impl Hall {
    /// 床の点 (mm) → 3D の点 (モデルの単位)
    pub fn world(&self, p: DVec2, lift: f64) -> Option<Vec3> {
        let layout = self.layout.as_ref()?;
        Some((layout.to_world(p, lift) * self.scale).as_vec3())
    }
}

#[derive(Resource)]
pub struct HallMaterials {
    floor: Handle<StandardMaterial>,
    walkway: Handle<StandardMaterial>,
    aisle_line: Handle<StandardMaterial>,
    dado: Handle<StandardMaterial>,
    upper: Handle<StandardMaterial>,
    window: Handle<StandardMaterial>,
    column: Handle<StandardMaterial>,
    column_guard: Handle<StandardMaterial>,
}

pub fn create_materials(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let floor_texture = images.add(concrete_image());
    let lit = |color: Color, roughness: f32| StandardMaterial {
        base_color: color,
        perceptual_roughness: roughness,
        reflectance: 0.2,
        ..default()
    };
    // 壁は光の当たり方で明るさがばらつかないよう、決めた色のまま描く (背景として落ち着かせる)
    let flat = |color: Color| StandardMaterial {
        base_color: color,
        unlit: true,
        ..default()
    };
    commands.insert_resource(HallMaterials {
        floor: materials.add(StandardMaterial {
            base_color_texture: Some(floor_texture),
            // 明るい灰色の部品が床に埋もれないよう、模様を少し暗くして使う
            ..lit(Color::srgb(0.74, 0.74, 0.75), 0.92)
        }),
        walkway: materials.add(lit(Color::srgb(0.22, 0.48, 0.32), 0.85)),
        aisle_line: materials.add(lit(Color::srgb(0.92, 0.74, 0.12), 0.7)),
        dado: materials.add(flat(Color::srgb(0.30, 0.38, 0.38))),
        upper: materials.add(flat(Color::srgb(0.42, 0.45, 0.49))),
        window: materials.add(flat(Color::srgb(0.66, 0.76, 0.86))),
        column: materials.add(lit(Color::srgb(0.58, 0.60, 0.63), 0.8)),
        column_guard: materials.add(lit(Color::srgb(0.95, 0.75, 0.1), 0.7)),
    });
}

/// 設定とモデルの外接箱から建屋の形を決め、変わっていれば置き直す
#[allow(clippy::too_many_arguments)]
pub fn sync_hall(
    mut commands: Commands,
    settings: Res<super::viewport::ViewSettings>,
    cache: Res<super::viewport::SceneCache>,
    model: Res<Model>,
    materials: Res<HallMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut hall: ResMut<Hall>,
    roots: Query<Entity, With<HallRoot>>,
) {
    let scale = units_per_mm(&model.unit);
    let wanted = settings.hall.show.then(|| {
        let scene_mm = cache.scene_bounds.map(|b| Aabb {
            min: b.min / scale,
            max: b.max / scale,
        });
        factory_hall::layout(scene_mm, &settings.hall)
    });
    if wanted == hall.layout && scale == hall.scale {
        return;
    }
    for root in &roots {
        commands.entity(root).despawn();
    }
    hall.scale = scale;
    hall.layout = wanted;
    let Some(layout) = &hall.layout else {
        return;
    };
    spawn_hall(&mut commands, layout, scale, &materials, &mut meshes);
}

fn spawn_hall(
    commands: &mut Commands,
    layout: &HallLayout,
    scale: f64,
    materials: &HallMaterials,
    meshes: &mut Assets<Mesh>,
) {
    let mut children: Vec<(Mesh, Handle<StandardMaterial>, Transform, Option<Sides>)> = Vec::new();

    // 床 (グリッドの線と重ならないよう少し下げる)。模様は世界の座標に貼るので、広さが変わっても動かない
    let f = layout.floor;
    let mut floor = QuadBatch::default();
    floor.flat(f.min, f.max, layout.floor_y - 1.0, true);
    children.push((
        floor.into_mesh(),
        materials.floor.clone(),
        Transform::IDENTITY,
        None,
    ));

    // 床の線
    for (kind, lift, material) in [
        (MarkingKind::Walkway, -0.6, &materials.walkway),
        (MarkingKind::AisleLine, -0.4, &materials.aisle_line),
    ] {
        let mut batch = QuadBatch::default();
        for m in layout.markings.iter().filter(|m| m.kind == kind) {
            batch.flat(m.rect.min, m.rect.max, layout.floor_y + lift, false);
        }
        if !batch.is_empty() {
            children.push((
                batch.into_mesh(),
                material.clone(),
                Transform::IDENTITY,
                None,
            ));
        }
    }

    // 壁の板。壁ごと・種類ごとに1つのメッシュにまとめる (壁ごとに隠すため)
    for side in Side::ALL {
        for (kind, material, inset) in [
            (PanelKind::Dado, &materials.dado, 0.0),
            (PanelKind::Upper, &materials.upper, 0.0),
            // 窓は壁の少し手前に貼る
            (PanelKind::Window, &materials.window, 15.0),
        ] {
            let mut batch = QuadBatch::default();
            for p in layout
                .panels
                .iter()
                .filter(|p| p.side == side && p.kind == kind)
            {
                let offset = side.inward() * inset;
                batch.wall(
                    p.from + offset,
                    p.to + offset,
                    layout.floor_y + p.bottom,
                    layout.floor_y + p.top,
                    side.inward(),
                );
            }
            if !batch.is_empty() {
                children.push((
                    batch.into_mesh(),
                    material.clone(),
                    Transform::IDENTITY,
                    Some(Sides::of(&[side])),
                ));
            }
        }
    }

    // 柱 (根元に黄色の帯)
    let shaft = meshes.add(Cuboid::new(
        COLUMN_SIZE as f32,
        (WALL_HEIGHT - COLUMN_GUARD_HEIGHT) as f32,
        COLUMN_SIZE as f32,
    ));
    let guard = meshes.add(Cuboid::new(
        COLUMN_SIZE as f32 + 20.0,
        COLUMN_GUARD_HEIGHT as f32,
        COLUMN_SIZE as f32 + 20.0,
    ));

    commands
        .spawn((
            HallRoot,
            Transform::from_scale(Vec3::splat(scale as f32)),
            Visibility::default(),
        ))
        .with_children(|root| {
            for (mesh, material, transform, sides) in children {
                // 床・床の線・壁は影を落とさない (屋根がないので、壁の影が床の端を暗くするだけになる)。
                // 影を受けるのはそのまま
                let mut e = root.spawn((
                    HallMesh,
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(material),
                    transform,
                    NotShadowCaster,
                ));
                if let Some(sides) = sides {
                    e.insert(HallPiece(sides));
                }
            }
            for c in &layout.columns {
                let base = layout.to_world(c.center, 0.0);
                let at =
                    |y: f64| Transform::from_translation(DVec3::new(base.x, y, base.z).as_vec3());
                root.spawn((
                    HallMesh,
                    HallPiece(c.sides),
                    Mesh3d(shaft.clone()),
                    MeshMaterial3d(materials.column.clone()),
                    at(base.y + (COLUMN_GUARD_HEIGHT + WALL_HEIGHT) * 0.5),
                ));
                root.spawn((
                    HallMesh,
                    HallPiece(c.sides),
                    Mesh3d(guard.clone()),
                    MeshMaterial3d(materials.column_guard.clone()),
                    at(base.y + COLUMN_GUARD_HEIGHT * 0.5),
                ));
            }
        });
}

/// カメラが建屋の外にいる側の壁と柱を隠し、外から覗いても中の装置が見えるようにする
pub fn hide_near_walls(
    hall: Res<Hall>,
    camera: Single<&GlobalTransform, With<Camera3d>>,
    mut pieces: Query<(&HallPiece, &mut Visibility)>,
) {
    let Some(layout) = &hall.layout else {
        return;
    };
    let eye = camera.translation().as_dvec3() / hall.scale;
    let hidden = factory_hall::hidden_sides(layout.floor, eye);
    for (piece, mut visibility) in &mut pieces {
        let want = if piece.0.any_in(hidden) {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        visibility.set_if_neq(want);
    }
}

/// メートルのグリッドを床に描く (建屋を出していないときは何もしない)。描いたら true
pub fn draw_hall_grid(gizmos: &mut Gizmos, hall: &Hall) -> bool {
    let Some(layout) = &hall.layout else {
        return false;
    };
    let minor = Color::srgba(0.12, 0.13, 0.15, 0.28);
    let major = Color::srgba(0.08, 0.09, 0.1, 0.6);
    let g = layout.grid;
    let (xs, zs) = g.lines();
    let at = |x: f64, z: f64| hall.world(DVec2::new(x, z), 0.0).expect("建屋がある");
    for (x, is_major) in xs {
        gizmos.line(
            at(x, g.rect.min.y),
            at(x, g.rect.max.y),
            if is_major { major } else { minor },
        );
    }
    for (z, is_major) in zs {
        gizmos.line(
            at(g.rect.min.x, z),
            at(g.rect.max.x, z),
            if is_major { major } else { minor },
        );
    }
    true
}

/// コンクリートの模様。縮小 (ミップマップ) も入れて、遠くでちらつかないようにする
fn concrete_image() -> Image {
    let levels = factory_hall::concrete_texture(512);
    let size = 512;
    let mut image = Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        levels[0].clone(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.mip_level_count = levels.len() as u32;
    image.data = Some(levels.concat());
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 16,
        ..default()
    });
    image
}

/// 四角形をまとめて1つのメッシュにする
#[derive(Default)]
struct QuadBatch {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl QuadBatch {
    fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// 上向きの水平な四角形 (床の座標の min〜max、高さ y)。`world_uv` なら模様を世界の座標に合わせて貼る
    fn flat(&mut self, min: DVec2, max: DVec2, y: f64, world_uv: bool) {
        let corners = [
            DVec2::new(min.x, min.y),
            DVec2::new(max.x, min.y),
            DVec2::new(max.x, max.y),
            DVec2::new(min.x, max.y),
        ];
        let uv = |p: DVec2| {
            if world_uv {
                (p / CONCRETE_TILE).as_vec2().to_array()
            } else {
                [0.0, 0.0]
            }
        };
        self.push(
            corners.map(|p| DVec3::new(p.x, y, p.y)),
            DVec3::Y,
            corners.map(uv),
        );
    }

    /// 縦の壁の板。`inward` (床の座標) の側から見える
    fn wall(&mut self, from: DVec2, to: DVec2, bottom: f64, top: f64, inward: DVec2) {
        let corners = [
            DVec3::new(from.x, bottom, from.y),
            DVec3::new(to.x, bottom, to.y),
            DVec3::new(to.x, top, to.y),
            DVec3::new(from.x, top, from.y),
        ];
        self.push(
            corners,
            DVec3::new(inward.x, 0.0, inward.y),
            [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
        );
    }

    /// 周りに並んだ4つの角。表 (反時計回りに見える側) が `normal` を向くよう三角形の向きを決める
    fn push(&mut self, corners: [DVec3; 4], normal: DVec3, uvs: [[f32; 2]; 4]) {
        let base = self.positions.len() as u32;
        let facing = (corners[1] - corners[0])
            .cross(corners[2] - corners[0])
            .dot(normal);
        let order: [u32; 6] = if facing >= 0.0 {
            [0, 1, 2, 0, 2, 3]
        } else {
            [0, 2, 1, 0, 3, 2]
        };
        self.positions
            .extend(corners.map(|c| c.as_vec3().to_array()));
        self.normals.extend([normal.as_vec3().to_array(); 4]);
        self.uvs.extend(uvs);
        self.indices.extend(order.map(|i| base + i));
    }

    fn into_mesh(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
        .with_inserted_indices(Indices::U32(self.indices))
    }
}
