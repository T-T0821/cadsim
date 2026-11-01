//! 3D ビューの描画の質 (影・環境光・SSAO・ブルーム・アンチエイリアス) の段階と、
//! シーンの大きさに合わせた影や陰影の範囲の決め方
//!
//! Bevy に依存しない。どの段階で何を使うかと、数値の決め方だけをここに置き、
//! アプリ側 (`src/view/render_quality.rs`) がそれを Bevy のカメラと光に写す。
//!
//! 長さはすべて「モデルの単位」(mm のモデルなら mm) で受け取り、返す。

/// 画質の段階。遅い PC では軽いものを選ぶ
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub enum RenderQuality {
    /// 影も後処理も使わない。以前の見た目に近く、いちばん軽い
    Low,
    /// 影と環境光を使う。内蔵 GPU のノート PC でも動く重さ
    Standard,
    /// 影をなめらかにし、隅の陰影 (SSAO)・光のにじみ (ブルーム) を足す
    #[default]
    High,
}

impl RenderQuality {
    pub const ALL: [RenderQuality; 3] = [Self::Low, Self::Standard, Self::High];

    /// 画面に出す名前
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "軽い",
            Self::Standard => "標準",
            Self::High => "きれい",
        }
    }

    /// メニューで名前の横に出す説明
    pub fn description(self) -> &'static str {
        match self {
            Self::Low => "影や後処理を使わず、いちばん軽く描く。動きがカクつく PC 向け",
            Self::Standard => "部品の影と、周りから回り込む光 (環境光) を付ける",
            Self::High => {
                "影をなめらかにし、部品の隅や接地面の陰影と光のにじみを足して、製品らしく見せる"
            }
        }
    }

    /// 設定ファイルに書く名前
    pub fn key(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Standard => "standard",
            Self::High => "high",
        }
    }

    /// 設定ファイルの名前から。知らない名前なら None
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.key() == key)
    }

    /// この段階で使う機能
    pub fn profile(self) -> RenderProfile {
        match self {
            Self::Low => RenderProfile {
                shadows: None,
                environment_light: false,
                ambient_occlusion: None,
                bloom: false,
                antialias: Antialias::Msaa4,
            },
            Self::Standard => RenderProfile {
                shadows: Some(ShadowProfile {
                    map_size: 2048,
                    cascades: 2,
                    soft: false,
                }),
                environment_light: true,
                ambient_occlusion: None,
                bloom: false,
                antialias: Antialias::Msaa4,
            },
            Self::High => RenderProfile {
                shadows: Some(ShadowProfile {
                    map_size: 4096,
                    cascades: 3,
                    soft: true,
                }),
                environment_light: true,
                ambient_occlusion: Some(AoLevel::Medium),
                bloom: true,
                // SSAO は MSAA と一緒に使えないので、後処理のアンチエイリアス (SMAA) にする
                antialias: Antialias::Smaa,
            },
        }
    }
}

/// ある画質の段階で使う機能の組み合わせ
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RenderProfile {
    /// 主光源 (太陽の代わりの平行光) の影。None なら影を落とさない
    pub shadows: Option<ShadowProfile>,
    /// 空と床の色から回り込む光 (環境光) と、それを映す反射
    pub environment_light: bool,
    /// 部品の隅や重なりを暗くする陰影 (SSAO)
    pub ambient_occlusion: Option<AoLevel>,
    /// 明るい所 (光るエフェクト・強い照り返し) の光のにじみ
    pub bloom: bool,
    pub antialias: Antialias,
}

impl RenderProfile {
    /// 明るさを HDR で計算する必要があるか (ブルームに要る)
    pub fn hdr(&self) -> bool {
        self.bloom
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ShadowProfile {
    /// 影の地図の 1 辺の画素数 (2 のべき)
    pub map_size: u32,
    /// カメラからの距離で分ける段の数。多いほど近くの影が細かい
    pub cascades: u32,
    /// 影の縁をぼかしてなめらかにする (重い)。false なら 2x2 の軽いぼかし
    pub soft: bool,
}

/// SSAO の細かさ
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AoLevel {
    Low,
    Medium,
    High,
}

/// 縁のギザギザを消す方法
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Antialias {
    /// 4 倍のマルチサンプル
    Msaa4,
    /// 描いた後の画像から縁を探してなめらかにする (SSAO と両立する)
    Smaa,
}

/// 主光源の影を落とす範囲 (カメラからの距離、モデルの単位)
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ShadowRange {
    /// これより近い所には影を落とさない
    pub minimum: f32,
    /// 1 段目の終わり。ここまでがいちばん細かい
    pub first_cascade_far: f32,
    /// これより遠い所には影を落とさない
    pub maximum: f32,
    /// 自分に影が落ちて縞になるのを防ぐずらし (モデルの単位)
    pub depth_bias: f32,
}

impl ShadowRange {
    /// 前の範囲から大きく変わったか。少しの違いで影を作り直さないために使う
    pub fn differs_from(&self, other: &ShadowRange) -> bool {
        let rel = |a: f32, b: f32| (a - b).abs() > 0.1 * a.abs().max(b.abs()).max(f32::EPSILON);
        rel(self.minimum, other.minimum)
            || rel(self.first_cascade_far, other.first_cascade_far)
            || rel(self.maximum, other.maximum)
            || rel(self.depth_bias, other.depth_bias)
    }
}

/// 1 m がモデルの単位でいくつか。`units_per_mm` (mm なら 1、m なら 0.001) から
pub fn units_per_meter(units_per_mm: f64) -> f32 {
    (units_per_mm * 1000.0) as f32
}

/// 影を落とす範囲を、カメラから注視点までの距離と、見えている物 (モデルと建屋) の
/// 半径から決める。
///
/// 注視点のまわりのモデル全体が影の範囲に入り、近くに寄ったときは細かい影になるようにする。
pub fn shadow_range(camera_distance: f32, scene_radius: f32, units_per_meter: f32) -> ShadowRange {
    let meter = units_per_meter.max(f32::EPSILON);
    let distance = camera_distance.max(0.01 * meter);
    let radius = scene_radius.max(0.1 * meter);
    // 注視点の向こう側にあるモデルの端まで。ただし遠くの建屋の奥 (100 m 超) までは追わない
    let maximum = (distance + radius * 1.5).min(distance + 100.0 * meter);
    let minimum = (distance * 0.02).min(maximum * 0.01);
    let first_cascade_far = (distance * 0.75)
        .clamp(minimum * 2.0, maximum * 0.5)
        .max(minimum * 2.0);
    // Bevy の標準 (m の世界で 0.02) を、影の範囲の広さに合わせて大きくする。
    // 範囲が広いほど影の地図の 1 画素が大きく、縞が出やすい
    let depth_bias = 0.02 * meter * (maximum / (20.0 * meter)).clamp(0.25, 4.0);
    ShadowRange {
        minimum,
        first_cascade_far,
        maximum,
        depth_bias,
    }
}

/// SSAO の届く長さ (モデルの単位)
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct OcclusionScale {
    /// SSAO が周りを調べる半径
    pub ao_radius: f32,
    /// SSAO が見込む物の厚み
    pub ao_thickness: f32,
}

/// SSAO の半径を、カメラの距離に合わせて決める。
///
/// 画面の中で同じくらいの太さの陰影になるよう距離に比例させ、
/// 実寸で 1 cm〜50 cm の間に収める (小さな部品でも、工場全体でも不自然にならないように)
pub fn occlusion_scale(camera_distance: f32, units_per_meter: f32) -> OcclusionScale {
    let meter = units_per_meter.max(f32::EPSILON);
    let ao_radius = (camera_distance * 0.03).clamp(0.01 * meter, 0.5 * meter);
    OcclusionScale {
        ao_radius,
        ao_thickness: ao_radius * 0.35,
    }
}

impl OcclusionScale {
    /// 前の値から大きく変わったか (10% を超えたら)
    pub fn differs_from(&self, other: &OcclusionScale) -> bool {
        let rel = |a: f32, b: f32| (a - b).abs() > 0.1 * a.abs().max(b.abs()).max(f32::EPSILON);
        rel(self.ao_radius, other.ao_radius)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_and_unknown_is_none() {
        for q in RenderQuality::ALL {
            assert_eq!(RenderQuality::from_key(q.key()), Some(q));
            assert!(!q.label().is_empty());
            assert!(!q.description().is_empty());
        }
        assert_eq!(RenderQuality::from_key("ultra"), None);
    }

    #[test]
    fn low_uses_nothing_heavy() {
        let p = RenderQuality::Low.profile();
        assert_eq!(p.shadows, None);
        assert_eq!(p.ambient_occlusion, None);
        assert!(!p.bloom && !p.environment_light && !p.hdr());
        assert_eq!(p.antialias, Antialias::Msaa4);
    }

    #[test]
    fn higher_levels_add_features() {
        let s = RenderQuality::Standard.profile();
        let h = RenderQuality::High.profile();
        assert!(s.shadows.is_some() && s.environment_light);
        let (ss, hs) = (s.shadows.unwrap(), h.shadows.unwrap());
        assert!(hs.map_size >= ss.map_size && hs.cascades >= ss.cascades);
        assert!(h.ambient_occlusion.is_some() && h.bloom && h.hdr());
    }

    #[test]
    fn ssao_never_combined_with_msaa() {
        // Bevy の SSAO は MSAA を切らないと動かない
        for q in RenderQuality::ALL {
            let p = q.profile();
            if p.ambient_occlusion.is_some() {
                assert_ne!(p.antialias, Antialias::Msaa4, "{q:?}");
            }
        }
    }

    #[test]
    fn shadow_map_sizes_are_powers_of_two() {
        for q in RenderQuality::ALL {
            if let Some(s) = q.profile().shadows {
                assert!(s.map_size.is_power_of_two() && s.cascades >= 1);
            }
        }
    }

    #[test]
    fn shadow_range_covers_scene_behind_focus() {
        // mm のモデル: 2 m 先を見ていて、半径 1 m のモデル
        let r = shadow_range(2000.0, 1000.0, 1000.0);
        assert!(r.maximum >= 2000.0 + 1000.0);
        assert!(r.minimum < r.first_cascade_far && r.first_cascade_far < r.maximum);
        assert!(r.minimum > 0.0);
    }

    #[test]
    fn shadow_range_is_valid_for_tiny_and_huge_values() {
        for (distance, radius, meter) in [
            (0.0, 0.0, 1000.0),
            (1e-6, 1e-6, 1.0),
            (5.0, 3.0, 1.0),
            (1e7, 1e7, 1000.0),
            (300.0, 50.0, 39.37),
        ] {
            let r = shadow_range(distance, radius, meter);
            assert!(r.minimum >= 0.0, "{r:?}");
            assert!(r.minimum < r.first_cascade_far, "{r:?}");
            assert!(r.first_cascade_far < r.maximum, "{r:?}");
            assert!(r.depth_bias > 0.0 && r.depth_bias.is_finite(), "{r:?}");
        }
    }

    #[test]
    fn shadow_range_scales_with_units() {
        // 同じ実寸のシーンを mm と m で表したら、範囲も 1000 倍違う
        let mm = shadow_range(5000.0, 3000.0, 1000.0);
        let m = shadow_range(5.0, 3.0, 1.0);
        assert!((mm.maximum / m.maximum - 1000.0).abs() < 1.0);
        assert!((mm.depth_bias / m.depth_bias - 1000.0).abs() < 1.0);
    }

    #[test]
    fn far_hall_is_not_chased_forever() {
        // 半径 1 km の敷地を 10 m から見ても、影は 100 m 先までに抑えて細かさを保つ
        let r = shadow_range(10.0, 1000.0, 1.0);
        assert!(r.maximum <= 110.0 + 1e-3);
    }

    #[test]
    fn small_change_does_not_count_as_different() {
        let a = shadow_range(2000.0, 1000.0, 1000.0);
        assert!(!a.differs_from(&shadow_range(2050.0, 1000.0, 1000.0)));
        assert!(a.differs_from(&shadow_range(4000.0, 1000.0, 1000.0)));
    }

    #[test]
    fn occlusion_follows_distance_within_real_limits() {
        let near = occlusion_scale(500.0, 1000.0);
        let far = occlusion_scale(20_000.0, 1000.0);
        assert!(near.ao_radius < far.ao_radius);
        // 実寸 1 cm〜50 cm
        for d in [0.0, 1.0, 1e9] {
            let s = occlusion_scale(d, 1000.0);
            assert!((10.0..=500.0).contains(&s.ao_radius), "{s:?}");
            assert!(s.ao_thickness > 0.0);
        }
        assert!(!near.differs_from(&occlusion_scale(510.0, 1000.0)));
        assert!(near.differs_from(&far));
    }
}
