//! STEP を読み込んだときの形状の修復の結果を、部品ごとにまとめて案内する
//!
//! 修復そのものは読み込みで自動 (`formats::step`): 面を縫い合わせても閉じなかったシェルは、
//! すき間を直し、だめなら許容誤差を広げて縫い直す。ここでは結果 ([`ShapeRepair`]) を
//! 読み込んだ木から集め、ステータスバーの「○ 件のすき間を閉じました」と、
//! クリックでその部品へ順に飛ぶための一覧 ([`RepairNotice`]) にする。

use crate::model::{Model, NodeId, NodeKind};
use crate::shape::ShapeRepair;

/// 部品の形の修復の結果。修復していない部品や形を持たないノードは None
pub fn part_repair(model: &Model, id: NodeId) -> Option<ShapeRepair> {
    match &model.get(id)?.kind {
        NodeKind::Imported { shape, .. } if !shape.repair.is_empty() => Some(shape.repair),
        _ => None,
    }
}

/// 読み込んだ木の中で形を直した部品の一覧
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepairReport {
    /// すき間を閉じた部品 (直せなかったシェルもある部品は `open` の方に入る)
    pub healed: Vec<NodeId>,
    /// 直しても閉じなかった部品
    pub open: Vec<NodeId>,
    /// 閉じたシェルの数
    pub healed_shells: usize,
}

impl RepairReport {
    /// `root` 以下の部品の修復の結果を集める
    pub fn collect(model: &Model, root: NodeId) -> RepairReport {
        let mut report = RepairReport::default();
        for id in model.subtree(root) {
            let Some(r) = part_repair(model, id) else {
                continue;
            };
            report.healed_shells += r.healed;
            if r.open > 0 {
                report.open.push(id);
            } else {
                report.healed.push(id);
            }
        }
        report
    }

    pub fn is_empty(&self) -> bool {
        self.healed.is_empty() && self.open.is_empty()
    }

    /// ステータスバーに出す一行
    pub fn message(&self) -> String {
        match (self.healed_shells, self.open.len()) {
            (0, open) => format!("閉じていない部品が {open} 個あります (質量は計算できません)"),
            (healed, 0) => format!("形状のすき間を {healed} 件閉じました"),
            (healed, open) => format!(
                "形状のすき間を {healed} 件閉じました。閉じていない部品が {open} 個あります"
            ),
        }
    }

    /// 案内する部品の並び (閉じなかった部品を先に)
    pub fn parts(&self) -> Vec<NodeId> {
        self.open.iter().chain(&self.healed).copied().collect()
    }
}

/// ステータスバーに出している修復の案内。クリックするたびに次の部品へ飛ぶ
#[derive(bevy_ecs::prelude::Resource, Clone, Debug, Default)]
pub struct RepairNotice {
    pub report: Option<RepairReport>,
    /// 次に飛ぶ部品の番号 (`report.parts()` の中)
    next: usize,
}

impl RepairNotice {
    /// 読み込んだ木 `root` の結果を出す。直したものが無ければ前の案内を消す
    pub fn show(&mut self, model: &Model, root: NodeId) {
        let report = RepairReport::collect(model, root);
        self.report = (!report.is_empty()).then_some(report);
        self.next = 0;
    }

    pub fn dismiss(&mut self) {
        self.report = None;
        self.next = 0;
    }

    /// 次に見せる部品 (削除された部品は飛ばす)。押すたびに順に回る
    pub fn next_part(&mut self, model: &Model) -> Option<NodeId> {
        let parts: Vec<NodeId> = self
            .report
            .as_ref()?
            .parts()
            .into_iter()
            .filter(|&id| model.get(id).is_some())
            .collect();
        if parts.is_empty() {
            self.dismiss();
            return None;
        }
        let id = parts[self.next % parts.len()];
        self.next = (self.next + 1) % parts.len();
        Some(id)
    }
}

/// 部品の修復の結果を、プロパティに出す説明にする
pub fn describe(repair: ShapeRepair) -> String {
    match (repair.healed, repair.open) {
        (h, 0) => format!(
            "読み込みで形状のすき間を {h} か所閉じました。質量・断面・干渉チェックはそのまま使えます"
        ),
        (0, o) => format!(
            "{o} か所の形状が閉じていません (面が欠けているか、すき間が大きすぎます)。\
             表示・断面は使えますが、質量は計算できません。元の CAD で「ソリッドとして書き出す」設定にすると直ることがあります"
        ),
        (h, o) => format!(
            "すき間を {h} か所閉じましたが、{o} か所は閉じていません。閉じていない形は質量の計算に入りません"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::model::StepSource;
    use crate::shape::ImportedShape;
    use glam::DMat4;

    fn add_part(model: &mut Model, parent: NodeId, name: &str, repair: ShapeRepair) -> NodeId {
        let mut shape = ImportedShape::new(Vec::new(), Vec::new(), Vec::new(), None);
        shape.repair = repair;
        model.add_node(
            Some(parent),
            name.into(),
            DMat4::IDENTITY,
            NodeKind::Imported {
                product: name.into(),
                shape: Arc::new(shape),
                source: StepSource::Sample,
            },
        )
    }

    #[test]
    fn collect_healed_and_open_parts() {
        let mut model = Model::default();
        let root = model.add_node(
            None,
            "file.step".into(),
            DMat4::IDENTITY,
            NodeKind::Assembly { product: None },
        );
        add_part(&mut model, root, "ok", ShapeRepair::default());
        let healed = add_part(
            &mut model,
            root,
            "healed",
            ShapeRepair { healed: 2, open: 0 },
        );
        let open = add_part(&mut model, root, "open", ShapeRepair { healed: 1, open: 1 });
        let report = RepairReport::collect(&model, root);
        assert_eq!(report.healed, vec![healed]);
        assert_eq!(report.open, vec![open]);
        assert_eq!(report.healed_shells, 3);
        assert!(report.message().contains("3 件閉じました"));
        assert!(report.message().contains("1 個"));

        // クリックで閉じなかった部品から順に回る
        let mut notice = RepairNotice::default();
        notice.show(&model, root);
        assert_eq!(notice.next_part(&model), Some(open));
        assert_eq!(notice.next_part(&model), Some(healed));
        assert_eq!(notice.next_part(&model), Some(open));
        // 部品が消えたら飛ばし、全部消えたら案内も消す
        model.remove(open);
        assert_eq!(notice.next_part(&model), Some(healed));
        model.remove(healed);
        assert_eq!(notice.next_part(&model), None);
        assert!(notice.report.is_none());
    }

    #[test]
    fn nothing_to_report() {
        let mut model = Model::default();
        let root = model.add_box([1.0; 3], [0.0; 3]);
        let mut notice = RepairNotice::default();
        notice.show(&model, root);
        assert!(notice.report.is_none());
        assert!(describe(ShapeRepair { healed: 0, open: 2 }).contains("質量は計算できません"));
    }
}
