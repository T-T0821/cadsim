//! 現場のデータ (ドメイン): 床に敷いた平面図
//!
//! 部品のツリーとは別に、プロジェクトに属する「周りの状況」を持つ。
//! `Model` の `site` に入れ、プロジェクトファイルにも一緒に保存する。

use super::underlay::Underlay;

#[derive(Clone, Default)]
pub struct Site {
    pub underlays: Vec<Underlay>,
    /// 下敷きを足したり消したりするたびに1増える (表示側が作り直すかの目印)
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

    pub fn is_empty(&self) -> bool {
        self.underlays.is_empty()
    }
}
