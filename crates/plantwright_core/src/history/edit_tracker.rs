//! 文書の変更を「元に戻す」の1件にまとめる見張り役
//!
//! タイミングチャートや工程モデルのように、編集する場所が画面のあちこちにある文書は、
//! 編集のたびに履歴へ積む代わりに「前に確定した状態」と今の状態を比べて変更を見つける。
//! ドラッグや文字入力の途中は確定せず、手を離したとき (`idle`) に1件にする。

/// 1つの文書の、最後に確定した状態を覚えておく
#[derive(Clone, Debug)]
pub struct EditTracker<T> {
    committed: Option<T>,
}

impl<T> Default for EditTracker<T> {
    fn default() -> Self {
        Self { committed: None }
    }
}

impl<T: Clone> EditTracker<T> {
    /// 今の状態を見る。前に確定した状態と違えば (変更前, 変更後) を返し、今の状態を確定する。
    /// 初めて見たときは、それを確定するだけで変更とはみなさない。
    pub fn observe(&mut self, current: T, same: impl Fn(&T, &T) -> bool) -> Option<(T, T)> {
        match self.committed.take() {
            Some(before) if !same(&before, &current) => {
                self.committed = Some(current.clone());
                Some((before, current))
            }
            _ => {
                self.committed = Some(current);
                None
            }
        }
    }

    /// 元に戻す・やり直しで状態を書き換えたあとに呼ぶ。今の状態を変更なしとして確定する。
    pub fn sync(&mut self, current: T) {
        self.committed = Some(current);
    }

    /// 新規作成・開くのあとに呼ぶ。次に見た状態を最初の状態にする。
    pub fn reset(&mut self) {
        self.committed = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eq(a: &i32, b: &i32) -> bool {
        a == b
    }

    #[test]
    fn first_look_is_not_a_change() {
        let mut t = EditTracker::default();
        assert_eq!(t.observe(1, eq), None);
        assert_eq!(t.observe(1, eq), None);
        assert_eq!(t.observe(3, eq), Some((1, 3)));
        assert_eq!(t.observe(3, eq), None);
    }

    #[test]
    fn sync_and_reset() {
        let mut t = EditTracker::default();
        t.observe(1, eq);
        // 元に戻して 0 にした: 変更として積み直さない
        t.sync(0);
        assert_eq!(t.observe(0, eq), None);
        assert_eq!(t.observe(5, eq), Some((0, 5)));
        // 別のプロジェクトを開いた: 開いた状態から数える
        t.reset();
        assert_eq!(t.observe(9, eq), None);
    }

    #[test]
    fn same_can_ignore_fields() {
        let mut t = EditTracker::default();
        let same = |a: &(i32, u64), b: &(i32, u64)| a.0 == b.0;
        t.observe((1, 0), same);
        // 2つ目 (表示用の番号など) だけ変わっても変更にしない
        assert_eq!(t.observe((1, 7), same), None);
        assert_eq!(t.observe((2, 7), same), Some(((1, 7), (2, 7))));
    }
}
