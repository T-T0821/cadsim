//! 次に押すボタンを光らせる案内
//!
//! どのボタンを押す順か (`gamify::guide_for`) と、いま光らせるボタンの選び方 (`gamify::next_target`) は
//! core にある。ここは egui のアクセシビリティ情報から「画面に出ている部品の表示名と位置」を集め、
//! 選ばれたボタンのまわりに光る枠を描くだけにする。

use bevy::prelude::*;
use bevy_egui::{
    EguiOutput, PrimaryEguiContext,
    egui::{self, Align2, Color32, CornerRadius, Stroke, pos2, vec2},
};

use super::theme::palette;
use super::tutorial::TutorialUi;
use plantwright_core::gamify::{Mission, next_target};
use plantwright_core::ui_check::Rect as WidgetRect;
use plantwright_core::ui_check::widget::{self, Widget};

/// 案内に使う、前のフレームの画面の部品 (`TutorialUi` の中に持つ)
#[derive(Default)]
pub struct GuideUi {
    /// 部品を集める (ミッションカードを出している間だけ)
    wanted: bool,
    shown: Vec<Widget>,
    /// 光らせているボタンが、カードの手順の何番目か
    pub current_step: Option<usize>,
}

impl GuideUi {
    /// 部品を集めるかを決める。集める間だけ egui にアクセシビリティ情報を作らせる
    pub fn set_wanted(&mut self, ctx: &egui::Context, wanted: bool) {
        if wanted == self.wanted {
            return;
        }
        self.wanted = wanted;
        if wanted {
            ctx.enable_accesskit();
        } else {
            // 画面の自動確認も同じ情報で部品を探すので、そのときは止めない
            if !crate::features::ui_check::running() {
                ctx.disable_accesskit();
            }
            self.shown.clear();
            self.current_step = None;
        }
    }

    /// 表示名に合う、画面に出ている部品のうちいちばん上のもの
    /// (タイミングチャートは画面の下にあるので、同じ名前なら上に浮かぶ道具のボタンを選ぶ)
    fn find(&self, matches: impl Fn(&str) -> bool) -> Option<WidgetRect> {
        self.shown
            .iter()
            .filter(|w| w.rect.w > 0.0 && w.rect.h > 0.0 && matches(&w.label))
            .map(|w| w.rect)
            .min_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)))
    }
}

/// egui が出したアクセシビリティ情報から、表示名と位置のある部品を集める
pub fn collect_shown(
    mut tutorial: ResMut<TutorialUi>,
    outputs: Query<&EguiOutput, With<PrimaryEguiContext>>,
) {
    let guide = &mut tutorial.guide;
    if !guide.wanted {
        return;
    }
    let Ok(output) = outputs.single() else {
        return;
    };
    let Some(update) = &output.platform_output.accesskit_update else {
        return;
    };
    let raw = update
        .nodes
        .iter()
        .filter_map(|(_, node)| {
            let label = node.label().or(node.value())?.trim().to_string();
            let b = node.bounds()?;
            Some(Widget {
                label,
                role: format!("{:?}", node.role()),
                rect: WidgetRect::new(
                    b.x0 as f32,
                    b.y0 as f32,
                    (b.x1 - b.x0) as f32,
                    (b.y1 - b.y0) as f32,
                ),
            })
        })
        .collect();
    guide.shown = widget::normalize(raw);
}

/// 今のミッションで次に押すボタンを光らせる。光らせたボタンの手順を `current_step` に覚える
pub fn glow_next_button(
    ctx: &egui::Context,
    now: f64,
    next: &'static Mission,
    tool_open: bool,
    guide: &mut GuideUi,
) {
    let target = next_target(next.id, tool_open, |t| {
        guide.find(|label| t.matches(label)).is_some()
    });
    guide.current_step = target.map(|t| t.step);
    let Some(target) = target else {
        return;
    };
    let Some(r) = guide.find(|label| target.matches(label)) else {
        return;
    };
    let rect = egui::Rect::from_min_size(pos2(r.x, r.y), vec2(r.w, r.h));
    draw_glow(ctx, now, rect, target.step + 1);
}

/// ボタンのまわりの、脈打つ光の枠と「ここを押す」の札
fn draw_glow(ctx: &egui::Context, now: f64, rect: egui::Rect, step: usize) {
    // 前面のメニューや浮かぶウィンドウの上にも出す
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        "tutorial_glow".into(),
    ));
    let pulse = 0.5 + 0.5 * (now as f32 * 4.0).sin();
    // 外側ほど薄い枠を重ねて、ぼんやり光って見せる
    for i in 0..4 {
        let grow = 2.0 + i as f32 * 2.5 + pulse * 2.0;
        let alpha = (0.55 - i as f32 * 0.12) * (0.6 + 0.4 * pulse);
        painter.rect_stroke(
            rect.expand(grow),
            CornerRadius::same(6 + i * 2),
            Stroke::new(2.5, palette::YELLOW.gamma_multiply(alpha.max(0.05))),
            egui::StrokeKind::Outside,
        );
    }
    painter.rect_stroke(
        rect.expand(1.5),
        CornerRadius::same(5),
        Stroke::new(2.0, palette::YELLOW),
        egui::StrokeKind::Outside,
    );

    // 札は画面の端からはみ出さない側に出す (ふつうはボタンの下、下が足りなければ上)
    let screen = ctx.content_rect();
    let font = egui::FontId::proportional(13.0);
    let tag_height = painter
        .layout_no_wrap("▲".to_owned(), font.clone(), palette::NAVY_DEEP)
        .size()
        .y
        + 6.0;
    let bob = pulse * 3.0;
    let below = rect.bottom() + 8.0 + tag_height < screen.bottom();
    // 札の三角はボタンの方を向ける
    let text = if below {
        format!("▲ {step} ここを押す")
    } else {
        format!("▼ {step} ここを押す")
    };
    let galley = painter.layout_no_wrap(text, font, palette::NAVY_DEEP);
    let size = galley.size() + vec2(12.0, 6.0);
    let (anchor, align) = if below {
        (
            rect.center_bottom() + vec2(0.0, 8.0 + bob),
            Align2::CENTER_TOP,
        )
    } else {
        (
            rect.center_top() - vec2(0.0, 8.0 + bob),
            Align2::CENTER_BOTTOM,
        )
    };
    let mut tag = align.anchor_size(anchor, size);
    // 左右ははみ出したぶんだけ戻す
    let shift = (screen.left() + 4.0 - tag.left()).max(0.0)
        - (tag.right() - (screen.right() - 4.0)).max(0.0);
    tag = tag.translate(vec2(shift, 0.0));
    painter.rect_filled(tag, CornerRadius::same(7), palette::YELLOW);
    painter.galley(tag.min + vec2(6.0, 3.0), galley, Color32::WHITE);
    ctx.request_repaint();
}
