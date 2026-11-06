//! 3D ビューでのマウスの読み方と、ビューに重ねる案内
//!
//! - [`ClickWatch`][]: 左クリックの連打からダブルクリックを見つける。
//!   右ボタンは「押して、動かさずに離した」ときだけ右クリック (メニュー) にし、
//!   動かしたら見回し (ドラッグ) として扱う。
//! - [`ViewOverlay`][]: 右クリックで開いた部品のメニューと、カーソルの下の部品の名前。
//! - [`ViewHint`][]: どの道具を使っていても同じ場所 (ビューの下) に出す、次の操作の案内。
//!
//! - [`PointerLock`][]: 3D でクリックを使う道具 (穴あけ・部品の役割・フィレットなど) が
//!   「いまクリックは自分のもの」と宣言する。持たれている間は、クリックでの選択・ギズモ・
//!   右クリックメニューを止める。
//!
//! 画面に描くのはアプリの `ui/view_hints.rs`・`ui/context_menu.rs`、マウスを読むのは
//! `viewport/picking.rs`。ここは判定だけで Bevy・egui に依存しない。

use crate::model::NodeId;

/// 2回目の押下がこの秒数以内ならダブルクリック
pub const DOUBLE_CLICK_SECONDS: f64 = 0.4;
/// クリックとみなす、押してから離すまでの動き (論理ピクセル)
pub const CLICK_SLOP: f32 = 6.0;
/// カーソルがこの秒数止まったら部品の名前を出す
pub const HOVER_NAME_SECONDS: f64 = 0.35;

/// 左クリックの連打と、右ボタンの押し始めを覚えておく
#[derive(Clone, Copy, Debug, Default, PartialEq, bevy_ecs::prelude::Resource)]
pub struct ClickWatch {
    last_left: Option<(f64, [f32; 2])>,
    right_down: Option<[f32; 2]>,
}

impl ClickWatch {
    /// 左ボタンを押した。直前の押下から近い時間・近い場所なら true (ダブルクリック)。
    /// ダブルクリックを返したあとは数え直すので、3連打で2回続けて返すことはない。
    pub fn left_pressed(&mut self, at: [f32; 2], time: f64) -> bool {
        let double = self.last_left.is_some_and(|(t, p)| {
            time - t <= DOUBLE_CLICK_SECONDS && distance(p, at) <= CLICK_SLOP
        });
        self.last_left = if double { None } else { Some((time, at)) };
        double
    }

    /// 右ボタンを押した
    pub fn right_pressed(&mut self, at: [f32; 2]) {
        self.right_down = Some(at);
    }

    /// 右ボタンを離した。押した場所からほとんど動いていなければその場所 (右クリック)。
    /// 動いていれば見回しだったので None。
    pub fn right_released(&mut self, at: [f32; 2]) -> Option<[f32; 2]> {
        let down = self.right_down.take()?;
        (distance(down, at) <= CLICK_SLOP).then_some(at)
    }

    /// 右ボタンを押したまま動かしている (見回し) か
    pub fn right_dragging(&self, at: [f32; 2]) -> bool {
        self.right_down
            .is_some_and(|down| distance(down, at) > CLICK_SLOP)
    }
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// 右クリックで開いたメニュー
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartMenu {
    /// 右クリックした部品。何もないところなら None (ビューのメニュー)
    pub node: Option<NodeId>,
    /// 開く場所 (ウィンドウ座標、論理ピクセル)
    pub at: [f32; 2],
}

/// カーソルの下に止まっている部品
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoverName {
    pub node: NodeId,
    /// 止まり始めた時刻 (秒)
    pub since: f64,
    pub at: [f32; 2],
}

/// 3D ビューに重ねるもの (メニューと部品名)
#[derive(Clone, Copy, Debug, Default, PartialEq, bevy_ecs::prelude::Resource)]
pub struct ViewOverlay {
    pub menu: Option<PartMenu>,
    pub hover: Option<HoverName>,
}

impl ViewOverlay {
    /// カーソルの下の部品を毎フレーム知らせる。部品が変わるか動いたら時刻を取り直す。
    /// 名前を出してよければその部品を返す (止まってから [`HOVER_NAME_SECONDS`] 経過)。
    pub fn hovering(&mut self, node: Option<NodeId>, at: [f32; 2], time: f64) -> Option<NodeId> {
        let Some(node) = node else {
            self.hover = None;
            return None;
        };
        let same = self
            .hover
            .is_some_and(|h| h.node == node && distance(h.at, at) <= CLICK_SLOP);
        if !same {
            self.hover = Some(HoverName {
                node,
                since: time,
                at,
            });
        }
        self.hover
            .filter(|h| time - h.since >= HOVER_NAME_SECONDS)
            .map(|h| h.node)
    }
}

/// ビューの下に出す案内。道具が毎フレーム書き、描く側は読むだけ。
#[derive(Clone, Debug, Default, PartialEq, bevy_ecs::prelude::Resource)]
pub struct ViewHint {
    pub text: Option<String>,
    /// エラー (赤く出す)
    pub error: bool,
}

impl ViewHint {
    pub fn set(&mut self, text: impl Into<String>) {
        self.text = Some(text.into());
        self.error = false;
    }

    pub fn set_error(&mut self, text: impl Into<String>) {
        self.text = Some(text.into());
        self.error = true;
    }

    pub fn clear(&mut self) {
        self.text = None;
        self.error = false;
    }
}

/// 3D ビューのクリックを使っている道具。道具を始めるときに [`hold`](Self::hold)、
/// やめるとき (Esc や選択が外れたときも) に [`release`](Self::release) する。
#[derive(bevy_ecs::prelude::Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PointerLock {
    held_by: Option<&'static str>,
}

impl PointerLock {
    /// クリックを道具のものにする (別の道具が持っていても奪う。道具は一度に1つだけのため)
    pub fn hold(&mut self, tool: &'static str) {
        self.held_by = Some(tool);
    }

    /// その道具が持っていれば解放する (別の道具が持っていればそのまま)
    pub fn release(&mut self, tool: &'static str) {
        if self.held_by == Some(tool) {
            self.held_by = None;
        }
    }

    pub fn is_free(&self) -> bool {
        self.held_by.is_none()
    }

    pub fn held_by(&self) -> Option<&'static str> {
        self.held_by
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_released_only_by_its_holder() {
        let mut lock = PointerLock::default();
        assert!(lock.is_free());
        lock.hold("hole");
        assert!(!lock.is_free());
        lock.release("fillet");
        assert_eq!(lock.held_by(), Some("hole"));
        lock.hold("fillet");
        assert_eq!(lock.held_by(), Some("fillet"));
        lock.release("fillet");
        assert!(lock.is_free());
    }

    #[test]
    fn double_click_needs_quick_second_press_nearby() {
        let mut w = ClickWatch::default();
        assert!(!w.left_pressed([10.0, 10.0], 0.0));
        assert!(w.left_pressed([12.0, 11.0], 0.2));
        // ダブルクリックの直後の3回目は数え直し
        assert!(!w.left_pressed([12.0, 11.0], 0.3));
        // 遅い・遠い
        assert!(!w.left_pressed([12.0, 11.0], 1.0));
        assert!(!w.left_pressed([40.0, 11.0], 1.1));
    }

    #[test]
    fn right_click_is_press_and_release_without_moving() {
        let mut w = ClickWatch::default();
        w.right_pressed([100.0, 100.0]);
        assert!(!w.right_dragging([102.0, 101.0]));
        assert_eq!(w.right_released([102.0, 101.0]), Some([102.0, 101.0]));
        w.right_pressed([100.0, 100.0]);
        assert!(w.right_dragging([130.0, 100.0]));
        assert_eq!(w.right_released([130.0, 100.0]), None);
        // 押していないのに離した
        assert_eq!(w.right_released([130.0, 100.0]), None);
    }

    #[test]
    fn hover_name_waits_until_the_cursor_rests() {
        let mut o = ViewOverlay::default();
        let a: NodeId = 1;
        assert_eq!(o.hovering(Some(a), [0.0, 0.0], 0.0), None);
        assert_eq!(o.hovering(Some(a), [1.0, 0.0], 0.2), None);
        assert_eq!(o.hovering(Some(a), [1.0, 0.0], 0.5), Some(a));
        // 動かすと数え直し
        assert_eq!(o.hovering(Some(a), [50.0, 0.0], 0.6), None);
        assert_eq!(o.hovering(Some(a), [50.0, 0.0], 1.0), Some(a));
        // 離れたら消える
        assert_eq!(o.hovering(None, [50.0, 0.0], 1.1), None);
        assert_eq!(o.hover, None);
    }

    #[test]
    fn hint_text_and_error() {
        let mut h = ViewHint::default();
        h.set("クリック");
        assert_eq!(h.text.as_deref(), Some("クリック"));
        assert!(!h.error);
        h.set_error("だめ");
        assert!(h.error);
        h.clear();
        assert_eq!(h, ViewHint::default());
    }
}
