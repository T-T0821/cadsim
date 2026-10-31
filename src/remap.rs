//! 形状の差し替え: 古いアセンブリと新しいアセンブリのノードを名前と構造で対応づける
//!
//! 設計変更で STEP を取り込み直したとき、関節やセンサー、スクリプトなどの設定は
//! 古いノードに付いている。新しい形状のノードを古いノードに対応づけられれば、
//! 古いノードの ID を残したまま形状と配置だけを入れ替えて、設定を引き継げる。
//!
//! 対応づけは次の順に試し、先に決まったものを優先する。
//! 1. 一番上からの名前の並び (パス) が同じ
//! 2. インスタンス名が同じで、まだ決まっていないノードの中で1つずつしかない
//! 3. 製品名が同じで、まだ決まっていないノードの中で1つずつしかない
//!
//! 一番上のノード同士は常に対応づける (ファイル名が変わっても差し替えられるように)。
//! Bevy にもファイル形式にも依存しない。

use std::collections::HashMap;

/// 対応づけに使うノードの情報。`parent` は同じ一覧の中の番号。
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineNode {
    pub name: String,
    pub product: Option<String>,
    pub parent: Option<usize>,
    /// 形状を持つ部品か (アセンブリなら false)
    pub is_part: bool,
}

/// どうやって対応づけたか
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchReason {
    /// 一番上のノード同士
    Root,
    /// 名前の並びが同じ
    Path,
    /// インスタンス名が同じ
    Name,
    /// 製品名が同じ
    Product,
}

impl MatchReason {
    pub fn label(self) -> &'static str {
        match self {
            MatchReason::Root => "最上位",
            MatchReason::Path => "パスが一致",
            MatchReason::Name => "名前が一致",
            MatchReason::Product => "製品名が一致",
        }
    }
}

/// 対応づけの結果。番号は渡した一覧の中の位置。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemapPlan {
    /// (古い番号, 新しい番号, 理由)。古い番号の順に並ぶ。
    pub matched: Vec<(usize, usize, MatchReason)>,
    /// 新しい側に見つからなかった古いノード (形状は古いまま残る)
    pub unmatched_old: Vec<usize>,
    /// 古い側に見つからなかった新しいノード (新しく加わる)
    pub unmatched_new: Vec<usize>,
}

impl RemapPlan {
    /// 古い番号に対応づけた新しい番号
    pub fn new_for(&self, old: usize) -> Option<usize> {
        self.matched
            .iter()
            .find(|(o, _, _)| *o == old)
            .map(|(_, n, _)| *n)
    }
}

/// 一番上から自分までの名前の並び (一番上自身の名前は含めない)
fn path_of(nodes: &[OutlineNode], mut i: usize) -> Vec<&str> {
    let mut path = Vec::new();
    while let Some(parent) = nodes[i].parent {
        path.push(nodes[i].name.as_str());
        i = parent;
    }
    path.reverse();
    path
}

/// 古い一覧と新しい一覧を対応づける。どちらも先頭 (番号 0) が一番上のノード。
pub fn plan_remap(old: &[OutlineNode], new: &[OutlineNode]) -> RemapPlan {
    let mut old_to_new: Vec<Option<(usize, MatchReason)>> = vec![None; old.len()];
    let mut new_taken = vec![false; new.len()];
    let mut take = |o: usize, n: usize, reason, old_to_new: &mut Vec<_>| {
        old_to_new[o] = Some((n, reason));
        new_taken[n] = true;
    };

    if !old.is_empty() && !new.is_empty() {
        take(0, 0, MatchReason::Root, &mut old_to_new);
    }

    // 1. パスが同じ (同じパスが複数あれば、出てきた順に組にする)
    let mut new_by_path: HashMap<Vec<&str>, Vec<usize>> = HashMap::new();
    for n in 1..new.len() {
        new_by_path.entry(path_of(new, n)).or_default().push(n);
    }
    for o in 1..old.len() {
        if let Some(list) = new_by_path.get_mut(&path_of(old, o))
            && !list.is_empty()
        {
            let n = list.remove(0);
            take(o, n, MatchReason::Path, &mut old_to_new);
        }
    }

    // 2. 名前、3. 製品名が、残りの中で1つずつしかない
    type Key = fn(&OutlineNode) -> Option<&str>;
    let keys: [(Key, MatchReason); 2] = [
        (|n| Some(n.name.as_str()), MatchReason::Name),
        (|n| n.product.as_deref(), MatchReason::Product),
    ];
    for (key, reason) in keys {
        let count = |nodes: &[OutlineNode], free: &dyn Fn(usize) -> bool| {
            let mut map: HashMap<(&str, bool), Vec<usize>> = HashMap::new();
            for (i, node) in nodes.iter().enumerate().skip(1) {
                if free(i)
                    && let Some(k) = key(node)
                {
                    map.entry((k, node.is_part)).or_default().push(i);
                }
            }
            map
        };
        let olds = count(old, &|i| old_to_new[i].is_none());
        let news = count(new, &|i| !new_taken[i]);
        let mut pairs: Vec<(usize, usize)> = olds
            .iter()
            .filter_map(|(k, o)| match (o.as_slice(), news.get(k).map(Vec::as_slice)) {
                ([o], Some([n])) => Some((*o, *n)),
                _ => None,
            })
            .collect();
        pairs.sort();
        for (o, n) in pairs {
            take(o, n, reason, &mut old_to_new);
        }
    }

    let mut plan = RemapPlan::default();
    for (o, m) in old_to_new.iter().enumerate() {
        match m {
            Some((n, reason)) => plan.matched.push((o, *n, *reason)),
            None => plan.unmatched_old.push(o),
        }
    }
    plan.unmatched_new = (0..new.len()).filter(|&n| !new_taken[n]).collect();
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, product: &str, parent: Option<usize>, is_part: bool) -> OutlineNode {
        OutlineNode {
            name: name.into(),
            product: Some(product.into()),
            parent,
            is_part,
        }
    }

    /// Root ─ Arm (asm) ─ Link1, Link2 / Base
    fn old_tree() -> Vec<OutlineNode> {
        vec![
            node("robot_v1", "robot", None, false),
            node("Arm", "arm", Some(0), false),
            node("Link1", "link", Some(1), true),
            node("Link2", "link", Some(1), true),
            node("Base", "base", Some(0), true),
        ]
    }

    #[test]
    fn same_structure_matches_everything_by_path() {
        let mut new = old_tree();
        new[0].name = "robot_v2".into();
        let plan = plan_remap(&old_tree(), &new);
        assert_eq!(plan.matched.len(), 5);
        assert_eq!(plan.matched[0].2, MatchReason::Root);
        assert!(plan.matched[1..].iter().all(|m| m.2 == MatchReason::Path));
        assert!(plan.unmatched_old.is_empty() && plan.unmatched_new.is_empty());
    }

    #[test]
    fn moved_part_matches_by_name_and_new_parts_are_listed() {
        // Base がサブアセンブリの下に移り、Sensor が増え、Link2 がなくなった
        let new = vec![
            node("robot_v2", "robot", None, false),
            node("Arm", "arm", Some(0), false),
            node("Link1", "link", Some(1), true),
            node("Frame", "frame", Some(0), false),
            node("Base", "base", Some(3), true),
            node("Sensor", "sensor", Some(3), true),
        ];
        let plan = plan_remap(&old_tree(), &new);
        assert_eq!(plan.new_for(2), Some(2));
        assert_eq!(plan.new_for(4), Some(4));
        assert!(plan.matched.contains(&(4, 4, MatchReason::Name)));
        assert_eq!(plan.unmatched_old, vec![3]);
        assert_eq!(plan.unmatched_new, vec![3, 5]);
    }

    #[test]
    fn renamed_instance_matches_by_unique_product() {
        let mut new = old_tree();
        new[4].name = "Base_rev2".into();
        let plan = plan_remap(&old_tree(), &new);
        assert!(plan.matched.contains(&(4, 4, MatchReason::Product)));
    }

    #[test]
    fn ambiguous_names_are_left_unmatched() {
        // Link1/Link2 が両方とも別名になり、製品名 link は2つあるので決められない
        let mut new = old_tree();
        new[2].name = "L_a".into();
        new[3].name = "L_b".into();
        let plan = plan_remap(&old_tree(), &new);
        assert_eq!(plan.unmatched_old, vec![2, 3]);
        assert_eq!(plan.unmatched_new, vec![2, 3]);
    }

    #[test]
    fn duplicate_paths_pair_in_order() {
        let tree = vec![
            node("top", "top", None, false),
            node("Bolt", "bolt", Some(0), true),
            node("Bolt", "bolt", Some(0), true),
        ];
        let plan = plan_remap(&tree, &tree);
        assert_eq!(
            plan.matched,
            vec![
                (0, 0, MatchReason::Root),
                (1, 1, MatchReason::Path),
                (2, 2, MatchReason::Path)
            ]
        );
    }
}
