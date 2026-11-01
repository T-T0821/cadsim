//! 部品の配置の変更を見つける
//!
//! 「配置の変更も共有する」をオンにすると、自分が動かした部品の配置をほかの人へ送る。
//! 部品は番号ではなくツリーの道順 ("組立 / 部品") で指す。同じプロジェクトを開いていれば、
//! 番号が違っても同じ部品に当たるため。道順が同じ部品が 2 つ以上あるときは、どれか分からないので送らない。

use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct PlacementTracker {
    known: HashMap<String, [f64; 16]>,
    primed: bool,
}

impl PlacementTracker {
    /// 今の配置と前に見た配置を比べ、変わった部品を返す。
    /// 最初の 1 回 (と `reset` の後) は今の配置を覚えるだけで、何も返さない。
    pub fn changes(
        &mut self,
        current: impl IntoIterator<Item = (String, [f64; 16])>,
    ) -> Vec<(String, [f64; 16])> {
        let mut counts: HashMap<String, (usize, [f64; 16])> = HashMap::new();
        for (part, matrix) in current {
            let entry = counts.entry(part).or_insert((0, matrix));
            entry.0 += 1;
        }
        let mut changed = Vec::new();
        let mut next = HashMap::with_capacity(counts.len());
        for (part, (count, matrix)) in counts {
            if count != 1 {
                continue;
            }
            if self.primed && self.known.get(&part).is_none_or(|old| *old != matrix) {
                changed.push((part.clone(), matrix));
            }
            next.insert(part, matrix);
        }
        self.known = next;
        self.primed = true;
        changed.sort_by(|a, b| a.0.cmp(&b.0));
        changed
    }

    /// ほかの人から届いた配置を写したので覚える (送り返さないため)
    pub fn note(&mut self, part: &str, matrix: [f64; 16]) {
        self.known.insert(part.to_string(), matrix);
    }

    /// 覚えた配置を忘れる。次の `changes` は今の配置を覚えるだけになる
    pub fn reset(&mut self) {
        self.known.clear();
        self.primed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(x: f64) -> [f64; 16] {
        let mut m = [0.0; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        m[12] = x;
        m
    }

    #[test]
    fn first_look_only_records_then_changes_are_reported() {
        let mut t = PlacementTracker::default();
        assert!(
            t.changes([("A".into(), m(0.0)), ("B".into(), m(0.0))])
                .is_empty()
        );
        assert!(
            t.changes([("A".into(), m(0.0)), ("B".into(), m(0.0))])
                .is_empty()
        );
        assert_eq!(
            t.changes([("A".into(), m(1.0)), ("B".into(), m(0.0))]),
            vec![("A".to_string(), m(1.0))]
        );
        // 新しく足した部品も送る
        assert_eq!(
            t.changes([
                ("A".into(), m(1.0)),
                ("B".into(), m(0.0)),
                ("C".into(), m(2.0))
            ]),
            vec![("C".to_string(), m(2.0))]
        );
    }

    #[test]
    fn remote_changes_are_not_echoed() {
        let mut t = PlacementTracker::default();
        t.changes([("A".to_string(), m(0.0))]);
        t.note("A", m(5.0));
        assert!(t.changes([("A".to_string(), m(5.0))]).is_empty());
    }

    #[test]
    fn ambiguous_paths_are_skipped() {
        let mut t = PlacementTracker::default();
        t.changes([("A".to_string(), m(0.0)), ("A".to_string(), m(0.0))]);
        assert!(
            t.changes([("A".to_string(), m(1.0)), ("A".to_string(), m(0.0))])
                .is_empty()
        );
        t.reset();
        assert!(t.changes([("B".to_string(), m(3.0))]).is_empty());
    }
}
