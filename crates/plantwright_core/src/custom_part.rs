//! 置いた自作部品のしるしと公開パラメータ (データだけ)
//!
//! 公開パラメータは、置いたあとで変えられる値 (シリンダのストローク、動作時間、可動範囲、
//! センサーの検出距離)。どれもタイミングチャートのアクチュエータ・センサーの項目を指す。
//! チャートへの読み書きはアプリ側 (`src/custom_part/param.rs`) が受け持つ。

use std::collections::HashMap;

/// パラメータが変える項目。中の数値はアクチュエータ・センサーの id。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ParamTarget {
    /// シリンダのストローク
    Stroke(u32),
    /// 動作時間 (秒)
    Duration(u32),
    /// 可動範囲の下限
    LimitMin(u32),
    /// 可動範囲の上限
    LimitMax(u32),
    /// センサーの光軸の長さ・検出距離
    SensorRange(u32),
}

/// 公開パラメータ1つ
#[derive(Clone, PartialEq, Debug)]
pub struct PartParam {
    /// 画面に出す名前 (例: "ストローク")
    pub label: String,
    pub target: ParamTarget,
    /// 今の値 (保存した部品では既定値)
    pub value: f64,
}

/// 置いた自作部品のしるし。部品の一番上のノードに付ける。
#[derive(Clone, PartialEq, Debug, Default)]
pub struct CustomInstance {
    /// ライブラリでの部品名
    pub part: String,
    /// この部品のアクチュエータ・センサーの名前の頭 (例: "プッシャー 1/")。
    /// 部品のスクリプトは、頭を付けない名前でこの部品の関節・信号を指せる。
    pub prefix: String,
    pub params: Vec<PartParam>,
}

impl ParamTarget {
    /// 部品を置いたときの id の付け替え。写していないものを指していれば None。
    pub fn remap(self, actuators: &HashMap<u32, u32>, sensors: &HashMap<u32, u32>) -> Option<Self> {
        let a = |id: u32| actuators.get(&id).copied();
        Some(match self {
            ParamTarget::Stroke(id) => ParamTarget::Stroke(a(id)?),
            ParamTarget::Duration(id) => ParamTarget::Duration(a(id)?),
            ParamTarget::LimitMin(id) => ParamTarget::LimitMin(a(id)?),
            ParamTarget::LimitMax(id) => ParamTarget::LimitMax(a(id)?),
            ParamTarget::SensorRange(id) => ParamTarget::SensorRange(*sensors.get(&id)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remap_drops_targets_that_were_not_copied() {
        let actuators = HashMap::from([(1, 10)]);
        let sensors = HashMap::from([(5, 50)]);
        assert_eq!(
            ParamTarget::Stroke(1).remap(&actuators, &sensors),
            Some(ParamTarget::Stroke(10))
        );
        assert_eq!(
            ParamTarget::SensorRange(5).remap(&actuators, &sensors),
            Some(ParamTarget::SensorRange(50))
        );
        assert_eq!(ParamTarget::Duration(2).remap(&actuators, &sensors), None);
    }
}
