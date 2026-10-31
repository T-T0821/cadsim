//! 画面での動作確認の自動化 (UI チェック)
//!
//! 手順書 (シナリオ) に書いた操作を1フレームずつ流し、決めた場面で画面写真を撮って
//! 基準画像と見比べる。ここには Bevy・egui・画像形式に依存しない部分を置く。
//!
//! - [`scenario`] 手順書の書き方と読み取り
//! - [`runner`]   手順を1フレームずつ進め、アプリに「何を入力するか・いつ撮るか」を返す
//! - [`widget`]   画面上の部品 (ボタンなど) を表示名で探す
//! - [`compare`]  2枚の画像を見比べ、違う画素に印を付けた差分画像を作る
//! - [`report`]   写真ごとの判定 (同じ・違う・基準なし) と手順書ごとの結果
//!
//! アプリ側 (`src/ui_check/`) は、返された入力を Bevy のメッセージにして流し、
//! 画面を撮り、`cadsim_io::ui_check` で PNG と報告書を書くだけにする。

pub mod compare;
pub mod report;
pub mod runner;
pub mod scenario;
pub mod widget;

/// 画面上の長方形 (左上が原点、単位はウィンドウの論理ピクセル)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}
