//! 現場のデータ: 床に敷いた平面図と、計測した点群
//!
//! 部品のツリーとは別に、プロジェクトに属する「周りの状況」を持つ。
//! `Model` の `site` に入れ、プロジェクトファイルにも一緒に保存する。

use std::sync::Arc;

use super::pointcloud::PlacedCloud;
use super::underlay::Underlay;

#[derive(Clone, Default)]
pub struct Site {
    pub underlays: Vec<Underlay>,
    pub clouds: Vec<PlacedCloud>,
    /// 下敷きや点群を足したり消したりするたびに1増える (表示側が作り直すかの目印)
    pub revision: u64,
}

impl Site {
    pub fn add_underlay(&mut self, underlay: Underlay) -> usize {
        self.underlays.push(underlay);
        self.revision += 1;
        self.underlays.len() - 1
    }

    pub fn remove_underlay(&mut self, index: usize) {
        if index < self.underlays.len() {
            self.underlays.remove(index);
            self.revision += 1;
        }
    }

    pub fn add_cloud(&mut self, cloud: PlacedCloud) -> usize {
        self.clouds.push(cloud);
        self.revision += 1;
        self.clouds.len() - 1
    }

    pub fn remove_cloud(&mut self, index: usize) {
        if index < self.clouds.len() {
            self.clouds.remove(index);
            self.revision += 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.underlays.is_empty() && self.clouds.is_empty()
    }

    /// 配置が同じか (元に戻す履歴で変更を見つけるため)。
    /// 図面や点の中身は読み込んだときのものを共有しているので、同じものかどうかだけを見る。
    pub fn same_layout(&self, other: &Site) -> bool {
        let same_underlay = |a: &Underlay, b: &Underlay| {
            Arc::ptr_eq(&a.content, &b.content)
                && a.name == b.name
                && a.source == b.source
                && a.scale == b.scale
                && a.origin == b.origin
                && a.rotation_deg == b.rotation_deg
                && a.elevation == b.elevation
                && a.opacity == b.opacity
                && a.visible == b.visible
        };
        let same_cloud = |a: &PlacedCloud, b: &PlacedCloud| {
            Arc::ptr_eq(&a.cloud, &b.cloud)
                && a.name == b.name
                && a.source == b.source
                && a.offset == b.offset
                && a.rotation_deg == b.rotation_deg
                && a.visible == b.visible
                && a.settings == b.settings
        };
        self.underlays.len() == other.underlays.len()
            && self.clouds.len() == other.clouds.len()
            && self
                .underlays
                .iter()
                .zip(&other.underlays)
                .all(|(a, b)| same_underlay(a, b))
            && self
                .clouds
                .iter()
                .zip(&other.clouds)
                .all(|(a, b)| same_cloud(a, b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::site::underlay::PlanContent;

    #[test]
    fn same_layout_compares_placement_but_shares_content() {
        let mut site = Site::default();
        site.add_underlay(Underlay::new(
            "floor.dxf".into(),
            "floor.dxf".into(),
            PlanContent::default(),
            1.0,
        ));
        let copy = site.clone();
        assert!(site.same_layout(&copy));
        // 表示用の番号だけ違っても同じ
        let mut bumped = copy.clone();
        bumped.revision += 5;
        assert!(site.same_layout(&bumped));
        let mut moved = copy.clone();
        moved.underlays[0].origin = [100.0, 0.0];
        assert!(!site.same_layout(&moved));
        // 同じファイルを読み直したもの (中身が別) は違う
        let mut reloaded = Site::default();
        reloaded.add_underlay(Underlay::new(
            "floor.dxf".into(),
            "floor.dxf".into(),
            PlanContent::default(),
            1.0,
        ));
        assert!(!site.same_layout(&reloaded));
        assert!(!site.same_layout(&Site::default()));
    }
}
