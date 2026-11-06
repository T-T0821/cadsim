//! 3D ビューで部品を平面で切って中を見る「断面」道具の状態と計算
//!
//! 流れは「プロパティの『断面』を押す → 選んだ部品 (かモデル全体) の中心に平面が出る →
//! 平面の矢印の先をドラッグで位置 → X / Y / Z で向き、面をクリックでその面に平行 → Esc で閉じる」。
//! ここには平面の状態 ([`SectionTool`]) と、ドラッグ位置から平面の位置を決める計算、
//! 切り口の線を集める計算 ([`section_lines`]) を置く。描き方とマウスの読み方はアプリ側 (`features/modeling`)。

use std::hash::{DefaultHasher, Hash, Hasher};

use glam::DVec3;

use super::SolidStore;
use super::ops::section;
use crate::math::Aabb;
use crate::model::{Model, NodeId, NodeKind};

/// 平面の向き
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SectionAxis {
    X,
    Y,
    Z,
}

impl SectionAxis {
    pub const ALL: [SectionAxis; 3] = [SectionAxis::X, SectionAxis::Y, SectionAxis::Z];

    pub fn label(self) -> &'static str {
        match self {
            SectionAxis::X => "X",
            SectionAxis::Y => "Y",
            SectionAxis::Z => "Z",
        }
    }

    pub fn dir(self) -> DVec3 {
        match self {
            SectionAxis::X => DVec3::X,
            SectionAxis::Y => DVec3::Y,
            SectionAxis::Z => DVec3::Z,
        }
    }
}

/// 断面の道具 (ワールド座標)
#[derive(bevy_ecs::prelude::Resource, Clone, PartialEq, Debug)]
pub struct SectionTool {
    pub active: bool,
    /// 切る部品かアセンブリ。None ならモデル全体
    pub target: Option<NodeId>,
    /// 切る対象の外接箱の中心。平面の位置はここからの距離で持つ
    pub origin: DVec3,
    /// 平面の法線 (単位ベクトル)。この向きの側が「切り取って見せない側」
    pub normal: DVec3,
    /// `origin` から法線の向きに測った平面の位置
    pub offset: f64,
    /// 外接箱の大きさ (平面と矢印を描く大きさ)
    pub size: f64,
    /// 外接箱を法線に投影した範囲 (offset の動ける範囲)
    pub range: (f64, f64),
    /// 矢印の先にカーソルが乗っている
    pub hovered: bool,
    /// 矢印の先をドラッグ中
    pub dragging: bool,
    /// 切り口の線 (ワールド座標)
    pub lines: Vec<Vec<DVec3>>,
    /// 断面を出せなかった部品の数
    pub skipped: usize,
    /// `lines` を作ったときの平面とモデルの鍵
    computed: Option<u64>,
    /// 切る対象の外接箱
    bounds: Option<Aabb>,
}

impl Default for SectionTool {
    fn default() -> Self {
        Self {
            active: false,
            target: None,
            origin: DVec3::ZERO,
            normal: DVec3::X,
            offset: 0.0,
            size: 1.0,
            range: (-1.0, 1.0),
            hovered: false,
            dragging: false,
            lines: Vec::new(),
            skipped: 0,
            computed: None,
            bounds: None,
        }
    }
}

impl SectionTool {
    /// `target` (の外接箱 `bounds`) の中心に、X に垂直な平面を出す
    pub fn start(&mut self, target: Option<NodeId>, bounds: Aabb) {
        *self = SectionTool {
            active: true,
            target,
            origin: bounds.center(),
            size: bounds.size().length().max(1e-6),
            bounds: Some(bounds),
            ..SectionTool::default()
        };
        self.set_normal(DVec3::X);
    }

    /// 道具をやめる
    pub fn stop(&mut self) {
        *self = SectionTool::default();
    }

    /// 平面の向きを軸にそろえ、対象の中心に戻す
    pub fn set_axis(&mut self, axis: SectionAxis) {
        self.set_normal(axis.dir());
        self.offset = 0.0;
    }

    /// いまの向きが軸 `axis` (向きは問わない) か
    pub fn is_axis(&self, axis: SectionAxis) -> bool {
        self.normal.dot(axis.dir()).abs() > 1.0 - 1e-9
    }

    /// 法線を反対にする (見せる側を入れかえる)。平面の位置はそのまま
    pub fn flip(&mut self) {
        self.normal = -self.normal;
        self.offset = -self.offset;
        self.range = (-self.range.1, -self.range.0);
    }

    /// クリックした面 (点 `point`、法線 `normal`) に平行で、その点を通る平面にする
    pub fn align_to_face(&mut self, point: DVec3, normal: DVec3) {
        if normal.length_squared() < 1e-12 {
            return;
        }
        self.set_normal(normal.normalize());
        self.offset = (point - self.origin).dot(self.normal);
        self.clamp();
    }

    fn set_normal(&mut self, normal: DVec3) {
        self.normal = normal;
        self.range = match self.bounds {
            Some(b) => b
                .corners()
                .iter()
                .map(|&c| (c - self.origin).dot(normal))
                .fold((f64::MAX, f64::MIN), |(lo, hi), d| (lo.min(d), hi.max(d))),
            None => (-self.size, self.size),
        };
        self.clamp();
    }

    fn clamp(&mut self) {
        self.offset = self.offset.clamp(self.range.0, self.range.1);
    }

    /// 平面の上の点 (対象の中心を平面に落とした点)
    pub fn point(&self) -> DVec3 {
        self.origin + self.normal * self.offset
    }

    /// 平面から出す矢印の長さ
    pub fn arrow_length(&self) -> f64 {
        self.size * 0.25
    }

    /// 矢印の先 (ドラッグするつまみ) の位置
    pub fn handle_position(&self) -> DVec3 {
        self.point() + self.normal * self.arrow_length()
    }

    /// 法線の上の点 `p` に矢印の先を合わせて、平面を動かす。`step` があればその倍数に丸める
    pub fn set_offset_from_point(&mut self, p: DVec3, step: Option<f64>) {
        let offset = (p - self.origin).dot(self.normal) - self.arrow_length();
        self.offset = match step {
            Some(s) if s > 0.0 => (offset / s).round() * s,
            _ => offset,
        };
        self.clamp();
    }

    /// 平面を描く四角の角 (対象より少し大きく)
    pub fn plane_corners(&self) -> [DVec3; 4] {
        let (u, v) = self.normal.any_orthonormal_pair();
        let h = self.size * 0.6;
        let c = self.point();
        [
            c + (u + v) * h,
            c + (u - v) * h,
            c + (-u - v) * h,
            c + (-u + v) * h,
        ]
    }

    /// 画面下に出す案内
    pub fn hint(&self) -> &'static str {
        if self.dragging {
            "ドラッグで断面を動かす (Ctrl で刻みにそろえる)"
        } else {
            "矢印の先をドラッグで断面を動かす・面をクリックでその面に平行・X / Y / Z で向き・Esc で閉じる"
        }
    }

    /// 案内に、切れなかった部品の数を添えたもの
    pub fn full_hint(&self) -> String {
        if self.skipped > 0 && !self.dragging {
            format!(
                "{} (形を持たない部品 {} 個は切れません)",
                self.hint(),
                self.skipped
            )
        } else {
            self.hint().to_owned()
        }
    }

    /// 平面かモデルが変わっていれば、切り口の線を作り直す
    pub fn refresh(&mut self, model: &Model, store: &dyn SolidStore) {
        if !self.active {
            return;
        }
        let key = self.key(model);
        if self.computed == Some(key) {
            return;
        }
        let (lines, skipped) = section_lines(model, store, self.target, self.point(), self.normal);
        self.lines = lines;
        self.skipped = skipped;
        self.computed = Some(key);
    }

    /// 平面と、切る部品の形・配置をまとめた鍵
    fn key(&self, model: &Model) -> u64 {
        let mut h = DefaultHasher::new();
        self.target.hash(&mut h);
        for v in self
            .point()
            .to_array()
            .iter()
            .chain(&self.normal.to_array())
        {
            v.to_bits().hash(&mut h);
        }
        model.visit_parts(|part| {
            if self.target.is_some_and(|t| !part.path.contains(&t)) {
                return;
            }
            part.id.hash(&mut h);
            for v in part.world.to_cols_array() {
                v.to_bits().hash(&mut h);
            }
            match model.get(part.id).map(|n| &n.kind) {
                Some(NodeKind::Imported { shape, .. } | NodeKind::StandardBody { shape, .. }) => {
                    shape.id.hash(&mut h)
                }
                Some(NodeKind::Box(b)) => {
                    for v in b.size.iter().chain(&b.position).chain(&b.rotation_deg) {
                        v.to_bits().hash(&mut h);
                    }
                }
                _ => {}
            }
        });
        h.finish()
    }
}

/// `target` (None ならモデル全体) の部品を、`point` を通り `normal` に垂直な平面で切った線。
/// 平面にかからない部品は飛ばす。戻り値の数は、形を持たないなどで切れなかった部品の数
pub fn section_lines(
    model: &Model,
    store: &dyn SolidStore,
    target: Option<NodeId>,
    point: DVec3,
    normal: DVec3,
) -> (Vec<Vec<DVec3>>, usize) {
    let mut parts = Vec::new();
    model.visit_parts(|part| {
        if target.is_some_and(|t| !part.path.contains(&t)) || !part.shown {
            return;
        }
        // 外接箱の角が平面の両側にあるものだけ切る
        let crosses = part.local_bounds.is_some_and(|b| {
            let sides = b
                .transformed(part.world)
                .corners()
                .map(|c| (c - point).dot(normal));
            sides.iter().any(|&d| d <= 0.0) && sides.iter().any(|&d| d >= 0.0)
        });
        if crosses {
            parts.push(part.id);
        }
    });
    let mut lines = Vec::new();
    let mut skipped = 0;
    for id in parts {
        match section(model, store, id, point, normal) {
            Ok(l) => lines.extend(l),
            Err(_) => skipped += 1,
        }
    }
    (lines, skipped)
}

#[cfg(test)]
mod tests {
    use super::super::tests::MemoryStore;
    use super::*;

    fn cube_bounds() -> Aabb {
        Aabb {
            min: DVec3::splat(-50.0),
            max: DVec3::splat(50.0),
        }
    }

    #[test]
    fn dragging_the_arrow_moves_the_plane_within_the_part() {
        let mut tool = SectionTool::default();
        tool.start(Some(1), cube_bounds());
        assert!(tool.active);
        assert_eq!(tool.point(), DVec3::ZERO);
        assert_eq!(tool.range, (-50.0, 50.0));
        // 矢印の先を X = 20 + 矢印の長さ に動かすと、平面は X = 20
        let tip = DVec3::new(20.0 + tool.arrow_length(), 7.0, 3.0);
        tool.set_offset_from_point(tip, None);
        assert!((tool.point().x - 20.0).abs() < 1e-9);
        // 刻みにそろえる
        tool.set_offset_from_point(DVec3::new(23.0 + tool.arrow_length(), 0.0, 0.0), Some(10.0));
        assert!((tool.offset - 20.0).abs() < 1e-9);
        // 部品の外までは行かない
        tool.set_offset_from_point(DVec3::new(500.0, 0.0, 0.0), None);
        assert_eq!(tool.offset, 50.0);
        // 軸を変えると中心に戻る
        tool.set_axis(SectionAxis::Y);
        assert!(tool.is_axis(SectionAxis::Y));
        assert_eq!(tool.point(), DVec3::ZERO);
        tool.stop();
        assert!(!tool.active);
    }

    #[test]
    fn align_to_a_face_and_flip() {
        let mut tool = SectionTool::default();
        tool.start(None, cube_bounds());
        tool.align_to_face(DVec3::new(0.0, 0.0, 50.0), DVec3::new(0.0, 0.0, 2.0));
        assert!(tool.is_axis(SectionAxis::Z));
        assert_eq!(tool.offset, 50.0);
        tool.flip();
        assert_eq!(tool.normal, -DVec3::Z);
        assert_eq!(tool.point(), DVec3::new(0.0, 0.0, 50.0));
        // 斜めの面: 範囲は外接箱の角を投影したもの
        tool.align_to_face(DVec3::ZERO, DVec3::new(1.0, 1.0, 0.0));
        let r = 50.0 * 2f64.sqrt();
        assert!((tool.range.1 - r).abs() < 1e-9);
    }

    #[test]
    fn cut_a_box_and_skip_parts_away_from_the_plane() {
        let store = MemoryStore::default();
        let mut model = Model::default();
        let a = model.add_box([100.0, 100.0, 100.0], [0.0; 3]);
        // 平面にかからない箱
        model.add_box([10.0, 10.0, 10.0], [500.0, 0.0, 0.0]);
        let mut tool = SectionTool::default();
        tool.start(None, model.scene_bounds().unwrap());
        tool.set_axis(SectionAxis::X);
        tool.align_to_face(DVec3::ZERO, DVec3::X);
        tool.refresh(&model, &store);
        assert_eq!(tool.skipped, 0);
        // 箱を X = 0 で切ると、100 × 100 の正方形
        let points: Vec<DVec3> = tool.lines.iter().flatten().copied().collect();
        assert!(!points.is_empty());
        assert!(points.iter().all(|p| p.x.abs() < 1e-6));
        assert!(points.iter().any(|p| (p.y - 50.0).abs() < 1e-6));
        assert!(points.iter().all(|p| p.y.abs() <= 50.0 + 1e-6));
        // 同じ平面なら作り直さない。箱を動かすと作り直す
        let before = tool.lines.clone();
        tool.refresh(&model, &store);
        assert_eq!(tool.lines, before);
        if let Some(NodeKind::Box(b)) = model.get_mut(a).map(|n| &mut n.kind) {
            b.size = [100.0, 40.0, 100.0];
        }
        tool.refresh(&model, &store);
        let points: Vec<DVec3> = tool.lines.iter().flatten().copied().collect();
        assert!(points.iter().all(|p| p.y.abs() <= 20.0 + 1e-6));
    }
}
