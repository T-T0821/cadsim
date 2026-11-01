//! 取り込んだファイルの更新を見つける (開いたまま、ファイルを直すと読み直す)
//!
//! ファイルの印 (更新時刻と大きさなど) を外側が定期的に調べて渡すと、読み直すべきファイルを返す。
//! 書き込みの途中で読まないよう、印が変わってから次に調べたときも同じなら (落ち着いたら) 読み直す。
//! ファイルシステムには触らない。印の型も外側が決める。

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
struct Entry<S> {
    /// 最後に読んだ (または見張り始めた) ときの印
    known: Option<S>,
    /// 変わったのを見つけたが、まだ落ち着いていない印
    pending: Option<S>,
}

/// 見張っているファイルと、その印
#[derive(Clone, Debug, PartialEq)]
pub struct FileWatch<K: Ord, S> {
    files: BTreeMap<K, Entry<S>>,
}

impl<K: Ord, S> Default for FileWatch<K, S> {
    fn default() -> Self {
        Self {
            files: BTreeMap::new(),
        }
    }
}

impl<K: Ord + Clone, S: PartialEq + Clone> FileWatch<K, S> {
    /// 見張っているファイル
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.files.keys()
    }

    pub fn is_watching(&self, key: &K) -> bool {
        self.files.contains_key(key)
    }

    /// 見張るファイルを `keys` にそろえる。新しく加わったものは `stamp` (いまの印) から見張り始める
    pub fn set_keys(&mut self, keys: impl IntoIterator<Item = K>, stamp: impl Fn(&K) -> Option<S>) {
        let keys: Vec<K> = keys.into_iter().collect();
        self.files.retain(|k, _| keys.contains(k));
        for key in keys {
            if let std::collections::btree_map::Entry::Vacant(slot) = self.files.entry(key) {
                let known = stamp(slot.key());
                slot.insert(Entry {
                    known,
                    pending: None,
                });
            }
        }
    }

    /// いまの印を渡す。読み直すべきとき (変わって、落ち着いた) に true。
    /// ファイルが見つからない (`None`) あいだは、消えたのか書き込み中なのか分からないので何もしない
    pub fn observe(&mut self, key: &K, stamp: Option<S>) -> bool {
        let Some(entry) = self.files.get_mut(key) else {
            return false;
        };
        let Some(stamp) = stamp else {
            return false;
        };
        if entry.known.as_ref() == Some(&stamp) {
            entry.pending = None;
            return false;
        }
        if entry.pending.as_ref() == Some(&stamp) {
            entry.known = Some(stamp);
            entry.pending = None;
            return true;
        }
        entry.pending = Some(stamp);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch(stamp: u32) -> FileWatch<&'static str, u32> {
        let mut w = FileWatch::default();
        w.set_keys(["a.glb"], |_| Some(stamp));
        w
    }

    #[test]
    fn reloads_once_the_change_settles() {
        let mut w = watch(1);
        assert!(!w.observe(&"a.glb", Some(1)));
        // 変わった直後は待ち、次も同じなら読み直す
        assert!(!w.observe(&"a.glb", Some(2)));
        assert!(w.observe(&"a.glb", Some(2)));
        // 一度読み直したら、同じ印ではもう読まない
        assert!(!w.observe(&"a.glb", Some(2)));
    }

    #[test]
    fn keeps_waiting_while_the_file_is_being_written() {
        let mut w = watch(1);
        assert!(!w.observe(&"a.glb", Some(2)));
        assert!(!w.observe(&"a.glb", Some(3)));
        assert!(!w.observe(&"a.glb", None));
        assert!(!w.observe(&"a.glb", Some(4)));
        assert!(w.observe(&"a.glb", Some(4)));
    }

    #[test]
    fn change_back_to_the_known_stamp_is_not_a_change() {
        let mut w = watch(1);
        assert!(!w.observe(&"a.glb", Some(2)));
        assert!(!w.observe(&"a.glb", Some(1)));
        assert!(!w.observe(&"a.glb", Some(1)));
    }

    #[test]
    fn set_keys_adds_and_drops_files() {
        let mut w = watch(1);
        w.set_keys(["a.glb", "b.stl"], |_| Some(7));
        assert!(w.is_watching(&"b.stl"));
        // 見張り続けているファイルの印は引き継ぐ
        assert!(!w.observe(&"a.glb", Some(1)));
        w.set_keys(["b.stl"], |_| None);
        assert!(!w.is_watching(&"a.glb"));
        assert!(!w.observe(&"a.glb", Some(5)));
        assert_eq!(w.keys().count(), 1);
    }
}
