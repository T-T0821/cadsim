//! 3D ビューの床に形を描いて押し出す道具の状態と計算
//!
//! 流れは「床をクリックして頂点を置いていく → 最初の点をクリック (か Enter) で閉じる →
//! 上の点をドラッグで高さ → Enter で確定」。ここにはその状態 ([`ExtrudeTool`]) と、
//! クリック位置・ドラッグ位置から形と高さを決める計算だけを置く。
//! 画面への描き方とマウスの読み方はアプリ側 (`features/modeling`)。
//!
//! 頂点は床 (Y = [`ExtrudeTool::floor_y`]) の上のワールド座標で持ち、確定するときに
//! [`ExtrudeTool::outline`] で [`super::extrude`] に渡す上面図の座標 (右 +X・上 -Z) に直す。

use glam::{DVec2, DVec3};

use super::hole_tool::{round3, snap};

/// 高さの下限 (モデル単位)
const MIN_HEIGHT: f64 = 0.001;

/// ドラッグで動かすつまみ
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    /// 形の真ん中の上の点。ドラッグで高さ
    Height,
    /// 置いた頂点。ドラッグで動かす
    Vertex(usize),
}

/// 道具の状態
#[derive(bevy_ecs::prelude::Resource, Clone, PartialEq, Debug)]
pub struct ExtrudeTool {
    pub active: bool,
    /// 床の高さ (Y)
    pub floor_y: f64,
    /// 置いた頂点 (ワールド座標、床の上)
    pub points: Vec<DVec3>,
    /// 形を閉じた (これ以上頂点を足さず、高さを決める)
    pub closed: bool,
    pub height: f64,
    /// カーソルの下の床の点 (次の頂点の候補。閉じたら使わない)
    pub hover_point: Option<DVec3>,
    pub dragging: Option<Handle>,
    pub hovered: Option<Handle>,
    /// 前回の高さ (次の初期値)
    pub last_height: f64,
    /// 直前の押し出しのエラー (形は残す)
    pub error: Option<String>,
}

impl Default for ExtrudeTool {
    fn default() -> Self {
        Self {
            active: false,
            floor_y: 0.0,
            points: Vec::new(),
            closed: false,
            height: 0.0,
            hover_point: None,
            dragging: None,
            hovered: None,
            last_height: 0.0,
            error: None,
        }
    }
}

impl ExtrudeTool {
    pub fn start(&mut self, floor_y: f64) {
        self.active = true;
        self.floor_y = floor_y;
        self.points.clear();
        self.closed = false;
        self.height = 0.0;
        self.hover_point = None;
        self.dragging = None;
        self.hovered = None;
        self.error = None;
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.points.clear();
        self.closed = false;
        self.hover_point = None;
        self.dragging = None;
        self.hovered = None;
    }

    /// 押し出した後: 高さを次の初期値にして道具をやめる
    pub fn finish(&mut self) {
        if self.height > 0.0 {
            self.last_height = self.height;
        }
        self.error = None;
        self.stop();
    }

    /// 頂点を足す (床の上の点に丸める)。`step` があればその倍数に丸める
    pub fn add_point(&mut self, p: DVec3, step: Option<f64>) {
        if self.closed {
            return;
        }
        let p = self.on_floor(p, step);
        if self.points.last().is_none_or(|q| q.distance(p) > 1e-9) {
            self.points.push(p);
        }
    }

    /// 頂点を動かす
    pub fn move_point(&mut self, index: usize, p: DVec3, step: Option<f64>) {
        let p = self.on_floor(p, step);
        if let Some(q) = self.points.get_mut(index) {
            *q = p;
        }
    }

    /// 床の上に落として、刻みにそろえる
    fn on_floor(&self, p: DVec3, step: Option<f64>) -> DVec3 {
        DVec3::new(snap(p.x, step), self.floor_y, snap(p.z, step))
    }

    /// 形を閉じる。頂点が3つ未満なら閉じない (false)。
    /// 閉じたら高さを初期値 (前回の高さ、初回は形の大きさから) にする
    pub fn close(&mut self) -> bool {
        if self.points.len() < 3 {
            return false;
        }
        self.closed = true;
        self.hover_point = None;
        if self.height <= 0.0 {
            self.height = if self.last_height > 0.0 {
                self.last_height
            } else {
                (self.extent() * 0.5).max(MIN_HEIGHT)
            };
        }
        true
    }

    /// 形の大きさ (外接の長い方の辺)
    fn extent(&self) -> f64 {
        let (min, max) = self.bounds_xz();
        (max - min).max_element()
    }

    fn bounds_xz(&self) -> (DVec2, DVec2) {
        let mut min = DVec2::splat(f64::INFINITY);
        let mut max = DVec2::splat(f64::NEG_INFINITY);
        for p in &self.points {
            min = min.min(DVec2::new(p.x, p.z));
            max = max.max(DVec2::new(p.x, p.z));
        }
        if self.points.is_empty() {
            (DVec2::ZERO, DVec2::ZERO)
        } else {
            (min, max)
        }
    }

    /// 形の真ん中 (頂点の平均、床の上)
    pub fn center(&self) -> DVec3 {
        if self.points.is_empty() {
            return DVec3::new(0.0, self.floor_y, 0.0);
        }
        let sum: DVec3 = self.points.iter().sum();
        let c = sum / self.points.len() as f64;
        DVec3::new(c.x, self.floor_y, c.z)
    }

    /// つまみの位置
    pub fn handle_position(&self, handle: Handle) -> DVec3 {
        match handle {
            Handle::Height => self.center() + DVec3::Y * self.height,
            Handle::Vertex(i) => self.points.get(i).copied().unwrap_or(self.center()),
        }
    }

    /// 真ん中を通る縦の線の上の点 `p` に高さを合わせる
    pub fn set_height_from_point(&mut self, p: DVec3, step: Option<f64>) {
        let h = snap(p.y - self.floor_y, step);
        self.height = h.max(MIN_HEIGHT);
    }

    /// 数値で高さを決める
    pub fn set_height(&mut self, height: f64) {
        self.height = height.max(MIN_HEIGHT);
    }

    /// Esc: 閉じていれば開き直し、頂点があれば最後の1つを消し、なければ道具をやめる
    pub fn cancel(&mut self) {
        self.dragging = None;
        if self.closed {
            self.closed = false;
        } else if !self.points.is_empty() {
            self.points.pop();
        } else {
            self.stop();
        }
    }

    /// 押し出しに渡す上面図の座標 (右 +X・上 -Z) と、部品を置く位置。
    /// 頂点の X・Z の最小の角を原点にする (部品の位置がその角になる)
    pub fn outline(&self) -> (Vec<DVec2>, DVec3) {
        let (min, _) = self.bounds_xz();
        let origin = DVec3::new(min.x, self.floor_y, min.y);
        let outline = self
            .points
            .iter()
            .map(|p| DVec2::new(p.x - origin.x, -(p.z - origin.z)))
            .collect();
        (outline, origin)
    }

    /// 上の面の頂点 (描画用)
    pub fn top_points(&self) -> Vec<DVec3> {
        self.points
            .iter()
            .map(|p| *p + DVec3::Y * self.height)
            .collect()
    }

    /// 最後の頂点からカーソルまでの長さ (描画用)。頂点がないか閉じていれば None
    pub fn pending_length(&self) -> Option<f64> {
        if self.closed {
            return None;
        }
        Some(self.points.last()?.distance(self.hover_point?))
    }

    /// 画面に出す寸法の文字
    pub fn summary(&self) -> String {
        if self.closed {
            format!("{} 頂点 / 高さ {}", self.points.len(), round3(self.height))
        } else {
            format!("{} 頂点", self.points.len())
        }
    }

    /// 画面の下に出す案内
    pub fn hint(&self) -> &'static str {
        match (self.points.len(), self.closed, self.dragging) {
            (_, true, Some(Handle::Height)) => "ドラッグで高さを決めます (Ctrl で刻みにそろえる)",
            (_, true, Some(Handle::Vertex(_))) => "ドラッグで頂点を動かします",
            (_, true, None) => {
                "上の点をドラッグで高さ、頂点をドラッグで形を直す。Enter で押し出す・Esc で描き直し"
            }
            (0, false, _) => "床をクリックして形の1つ目の頂点を置いてください (Esc でやめる)",
            (1 | 2, false, _) => "続けて頂点をクリックしてください (Esc で1つ戻る)",
            (_, false, _) => "最初の点をクリックか Enter で形を閉じます (Esc で1つ戻る)",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(tool: &mut ExtrudeTool) {
        tool.add_point(DVec3::new(10.0, 0.0, 10.0), None);
        tool.add_point(DVec3::new(30.0, 0.0, 10.0), None);
        tool.add_point(DVec3::new(30.0, 0.0, 30.0), None);
        tool.add_point(DVec3::new(10.0, 0.0, 30.0), None);
    }

    #[test]
    fn points_land_on_the_floor_and_snap() {
        let mut tool = ExtrudeTool::default();
        tool.start(5.0);
        tool.add_point(DVec3::new(10.4, 99.0, 20.6), Some(1.0));
        assert_eq!(tool.points, vec![DVec3::new(10.0, 5.0, 21.0)]);
        // 同じ点は増えない
        tool.add_point(DVec3::new(10.0, 5.0, 21.0), None);
        assert_eq!(tool.points.len(), 1);
    }

    #[test]
    fn closing_needs_three_points_and_sets_a_height() {
        let mut tool = ExtrudeTool::default();
        tool.start(0.0);
        tool.add_point(DVec3::ZERO, None);
        tool.add_point(DVec3::X, None);
        assert!(!tool.close());
        assert!(!tool.closed);
        tool.add_point(DVec3::Z, None);
        assert!(tool.close());
        assert!(tool.closed);
        assert!((tool.height - 0.5).abs() < 1e-9, "初回は形の大きさの半分");
        // 閉じたら頂点は増えない
        tool.add_point(DVec3::new(5.0, 0.0, 5.0), None);
        assert_eq!(tool.points.len(), 3);
    }

    #[test]
    fn outline_is_top_view_with_min_corner_as_origin() {
        let mut tool = ExtrudeTool::default();
        tool.start(0.0);
        square(&mut tool);
        let (outline, origin) = tool.outline();
        assert_eq!(origin, DVec3::new(10.0, 0.0, 10.0));
        assert_eq!(
            outline,
            vec![
                DVec2::new(0.0, 0.0),
                DVec2::new(20.0, 0.0),
                DVec2::new(20.0, -20.0),
                DVec2::new(0.0, -20.0),
            ],
            "上面図では +Z が下 (v が負)"
        );
    }

    #[test]
    fn height_handle_sits_above_center() {
        let mut tool = ExtrudeTool::default();
        tool.start(0.0);
        square(&mut tool);
        tool.close();
        tool.set_height_from_point(DVec3::new(99.0, 12.3, 99.0), None);
        assert_eq!(tool.height, 12.3);
        assert_eq!(
            tool.handle_position(Handle::Height),
            DVec3::new(20.0, 12.3, 20.0)
        );
        tool.set_height_from_point(DVec3::new(0.0, -5.0, 0.0), None);
        assert_eq!(tool.height, MIN_HEIGHT, "0 以下にはならない");
        tool.set_height_from_point(DVec3::new(0.0, 7.6, 0.0), Some(5.0));
        assert_eq!(tool.height, 10.0);
    }

    #[test]
    fn cancel_steps_back_then_stops() {
        let mut tool = ExtrudeTool::default();
        tool.start(0.0);
        square(&mut tool);
        tool.close();
        tool.cancel();
        assert!(!tool.closed && tool.points.len() == 4);
        tool.cancel();
        assert_eq!(tool.points.len(), 3);
        tool.cancel();
        tool.cancel();
        tool.cancel();
        assert!(tool.points.is_empty() && tool.active);
        tool.cancel();
        assert!(!tool.active);
    }

    #[test]
    fn finish_remembers_height_for_next_time() {
        let mut tool = ExtrudeTool::default();
        tool.start(0.0);
        square(&mut tool);
        tool.close();
        tool.set_height(40.0);
        tool.finish();
        assert!(!tool.active);
        tool.start(0.0);
        square(&mut tool);
        tool.close();
        assert_eq!(tool.height, 40.0);
    }

    #[test]
    fn pending_length_and_summary() {
        let mut tool = ExtrudeTool::default();
        tool.start(0.0);
        assert_eq!(tool.pending_length(), None);
        tool.add_point(DVec3::ZERO, None);
        tool.hover_point = Some(DVec3::new(3.0, 0.0, 4.0));
        assert_eq!(tool.pending_length(), Some(5.0));
        assert_eq!(tool.summary(), "1 頂点");
        square(&mut tool);
        tool.close();
        tool.set_height(2.5);
        assert_eq!(tool.summary(), "5 頂点 / 高さ 2.5");
        assert_eq!(tool.pending_length(), None);
    }
}
