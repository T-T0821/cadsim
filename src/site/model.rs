//! 現場のデータ (ドメイン): 床に敷いた平面図と、計測した点群
//!
//! 部品のツリーとは別に、プロジェクトに属する「周りの状況」を持つ。
//! `Model` の `site` に入れ、プロジェクトファイルにも一緒に保存する。

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
}
