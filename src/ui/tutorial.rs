//! 最初の5分の体験: ようこそ画面、ミッションカードの手順、操作する場所を指す矢印
//!
//! ミッションの中身 (章・手順・使うキー・操作する場所) は `cadsim_core::gamify` にあり、
//! ここはそれを画面に出し、ボタンの操作を `PlayfulUi` と各 Resource に伝えるだけにする。

use bevy::prelude::*;
use bevy_egui::{
    EguiContexts,
    egui::{self, Align2, Color32, CornerRadius, Stroke, pos2, vec2},
};

use super::dock::{self, Dock, RAIL_WIDTH};
use super::logo;
use super::playful::{PlayfulUi, mascot};
use super::theme::palette;
use crate::assistant::help::HelpChat;
use crate::motion::Playback;
use crate::project::{ProjectAction, ProjectState};
use crate::view::camera::ViewportRect;
use cadsim_core::gamify::{Chapter, MISSIONS, Mission, Part, Place};

/// 手順を出しているときのミッションカードの幅
pub const CARD_WIDTH_WITH_STEPS: f32 = 310.0;
/// ようこそ画面の幅 (後ろのデモラインを隠しすぎない)
const WELCOME_WIDTH: f32 = 360.0;
/// 新しいミッションになってから矢印を出しておく時間 (秒)
const POINTER_SECONDS: f64 = 8.0;

/// ようこそ画面と矢印の状態
#[derive(Resource, Default)]
pub struct TutorialUi {
    /// 「ヘルプ → ようこそ画面」で開き直した
    pub welcome_requested: bool,
    /// 矢印を出し始めた時刻 (egui の時刻) と、そのときのミッション
    pointer: Option<(f64, &'static str)>,
    /// 「場所を教えて」が押された
    point_now: bool,
    /// ようこそ画面に出すロゴ (初めて出すときに作る)
    logo: Option<egui::TextureHandle>,
}

impl TutorialUi {
    /// 矢印をもう一度出す
    pub fn point(&mut self) {
        self.point_now = true;
    }
}

/// ようこそ画面と、今のミッションの操作する場所を指す矢印を描く
pub fn draw(
    mut contexts: EguiContexts,
    mut playful: ResMut<PlayfulUi>,
    mut tutorial: ResMut<TutorialUi>,
    mut project: ResMut<ProjectState>,
    viewport: Res<ViewportRect>,
) -> Result {
    let Some(rect) = viewport.rect else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?.clone();
    let view = egui::Rect::from_min_max(pos2(rect.min.x, rect.min.y), pos2(rect.max.x, rect.max.y));
    let now = ctx.input(|i| i.time);

    if !playful.prefs.welcomed || tutorial.welcome_requested {
        welcome(&ctx, view, now, &mut playful, &mut tutorial, &mut project);
        return Ok(());
    }
    if playful.prefs.show_missions
        && let Some(next) = playful.prefs.progress.next()
    {
        pointer(&ctx, view, now, next, &mut tutorial);
    }
    Ok(())
}

/// 「plantwright」の文字。wright (つくる人) を紫にする
fn wordmark(size: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    for (text, color) in [("plant", palette::TEXT_STRONG), ("wright", palette::PURPLE)] {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(size),
                color,
                ..Default::default()
            },
        );
    }
    job
}

/// 初めて起動したときの案内。デモラインが後ろで動いている。
fn welcome(
    ctx: &egui::Context,
    view: egui::Rect,
    now: f64,
    playful: &mut PlayfulUi,
    tutorial: &mut TutorialUi,
    project: &mut ProjectState,
) {
    let mut choice = None;
    // 後ろで動いているデモラインが見えるよう、3D ビューの左に寄せて出す
    egui::Area::new("welcome".into())
        .order(egui::Order::Foreground)
        .pivot(Align2::LEFT_CENTER)
        .fixed_pos(pos2(view.min.x + 16.0, view.center().y))
        .show(ctx, |ui| {
            egui::Frame::window(ui.style())
                .fill(palette::NAVY_LIGHT)
                .stroke(Stroke::new(2.5, palette::PURPLE))
                .corner_radius(CornerRadius::same(18))
                .inner_margin(egui::Margin::symmetric(24, 18))
                .show(ui, |ui| {
                    ui.set_width(WELCOME_WIDTH);
                    let logo = tutorial
                        .logo
                        .get_or_insert_with(|| logo::mark_texture(ctx))
                        .id();
                    ui.horizontal(|ui| {
                        ui.add(egui::Image::new((logo, vec2(40.0, 40.0))));
                        ui.label(wordmark(30.0));
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        mascot(ui, 64.0, now, true);
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("ようこそ!")
                                    .strong()
                                    .size(26.0)
                                    .color(palette::YELLOW),
                            );
                            ui.label("後ろで動いているのはデモのラインです。");
                        });
                    });
                    ui.add_space(6.0);
                    ui.label(
                        "ロボットがワークをつかんでコンベアからコンベアへ移し、\
                         安全柵やセンサー、AGV、作業者もいっしょに置いてあります。",
                    );
                    ui.add_space(4.0);
                    ui.label(format!(
                        "チュートリアルは {} つの章・{} 個のミッション。\
                         基本編で主な操作をひと回りしたら、応用編で動き・PLC・AI まで覚えられます。",
                        Chapter::ALL.len(),
                        MISSIONS.len()
                    ));
                    ui.add_space(4.0);
                    for part in Part::ALL {
                        ui.label(
                            egui::RichText::new(format!("{}  {}", part.title(), part.summary()))
                                .strong()
                                .color(palette::YELLOW),
                        );
                        for chapter in part.chapters() {
                            ui.label(
                                egui::RichText::new(format!(
                                    "  第{}章  {}",
                                    chapter.number(),
                                    chapter.title()
                                ))
                                .color(palette::TEAL),
                            );
                        }
                    }
                    ui.add_space(10.0);
                    ui.vertical_centered_justified(|ui| {
                        let start = egui::Button::new(
                            egui::RichText::new("▶ チュートリアルを始める")
                                .strong()
                                .size(17.0)
                                .color(palette::NAVY_DEEP),
                        )
                        .fill(palette::YELLOW)
                        .min_size(vec2(0.0, 34.0));
                        if ui.add(start).clicked() {
                            choice = Some(Welcome::Tutorial);
                        }
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            let half = (ui.available_width() - 8.0) / 2.0;
                            if ui
                                .add_sized([half, 26.0], egui::Button::new("自由にさわる"))
                                .on_hover_text(
                                    "ミッションカードは左下に残ります。いつでも続きから始められます",
                                )
                                .clicked()
                            {
                                choice = Some(Welcome::Explore);
                            }
                            if ui
                                .add_sized([half, 26.0], egui::Button::new("空のプロジェクトから"))
                                .on_hover_text("デモラインを閉じて、何もない状態から始めます")
                                .clicked()
                            {
                                choice = Some(Welcome::Empty);
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.vertical_centered(|ui| {
                        ui.weak("この画面は「ヘルプ → ようこそ画面」でまた開けます");
                    });
                });
        });

    let Some(choice) = choice else {
        return;
    };
    playful.mark_welcomed();
    tutorial.welcome_requested = false;
    match choice {
        Welcome::Tutorial => {
            playful.set_show_missions(true);
            playful.show_steps = true;
            tutorial.point();
        }
        Welcome::Explore => {
            playful.set_show_missions(true);
            playful.show_steps = false;
        }
        Welcome::Empty => {
            playful.set_show_missions(true);
            project.request(ProjectAction::New);
        }
    }
}

enum Welcome {
    Tutorial,
    Explore,
    Empty,
}

/// ミッションカードのボタンが書き換えるもの
pub struct CardCtx<'a> {
    pub tutorial: &'a mut TutorialUi,
    pub playback: &'a mut Playback,
    pub help: &'a mut HelpChat,
    pub dock: &'a mut Dock,
}

/// 「AI に聞く」の質問欄に入れる文面
fn question_for(next: &Mission) -> String {
    format!(
        "チュートリアルのミッション「{}」のやり方がわかりません。いまの画面のどこで何をすればよいか、順番に教えてください。",
        next.title
    )
}

/// ミッションカードの中の、今のミッションの章・手順・キー・ボタン
pub fn mission_body(
    ui: &mut egui::Ui,
    next: &'static Mission,
    playful: &mut PlayfulUi,
    card: &mut CardCtx,
) {
    let CardCtx {
        tutorial,
        playback,
        help,
        dock,
    } = card;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "第{}章 {}",
                next.chapter.number(),
                next.chapter.title()
            ))
            .color(palette::TEAL),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("+{} XP", next.xp))
                    .strong()
                    .color(palette::ORANGE),
            );
        });
    });
    chapter_dots(ui, next, playful);
    ui.label(
        egui::RichText::new(next.title)
            .strong()
            .size(17.0)
            .color(Color32::WHITE),
    );
    if !next.keys.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for key in next.keys {
                keycap(ui, key);
            }
        });
    }

    if playful.show_steps {
        for (i, step) in next.steps.iter().enumerate() {
            ui.horizontal_top(|ui| {
                ui.label(
                    egui::RichText::new(format!("{}", i + 1))
                        .strong()
                        .color(palette::YELLOW),
                );
                ui.add(egui::Label::new(*step).wrap());
            });
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("💡 {}", next.why))
                    .small()
                    .weak(),
            )
            .wrap(),
        );
    } else {
        ui.label(egui::RichText::new(format!("ヒント: {}", next.hint)).small());
    }

    // 使う道具はその場で開けるようにする (ツールレールを探さなくてよいように)
    if let Some(tool) = next.tool
        && !dock.is_open(tool)
    {
        let label = format!("{} {}を開く", dock::icon(tool), tool.title());
        if ui
            .button(label)
            .on_hover_text("左端のツールレールのボタンと同じです")
            .clicked()
        {
            dock.set_open(tool, true);
        }
    }

    // ギズモでの編集は設備が止まって設計位置にあるときだけできるので、その場で止められるようにする
    let moving = playback.playing || playback.time != 0.0;
    if next.needs_stopped_line && moving {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(palette::ORANGE, "設備が動いている間は部品を動かせません");
            if ui.small_button("⏮ 止めて元の位置へ").clicked() {
                playback.playing = false;
                playback.time = 0.0;
            }
        });
    }

    ui.horizontal(|ui| {
        let label = if playful.show_steps {
            "▲ 手順をたたむ"
        } else {
            "▼ 手順を見る"
        };
        if ui.small_button(label).clicked() {
            playful.show_steps = !playful.show_steps;
        }
        if ui
            .small_button("👉 どこ?")
            .on_hover_text("操作する場所を矢印で指します")
            .clicked()
        {
            tutorial.point();
        }
    });
    ui.horizontal(|ui| {
        if ui
            .small_button("💬 AI に聞く")
            .on_hover_text(
                "このミッションのやり方を AI に聞く画面を開きます (送るのはあなたが押してから)",
            )
            .clicked()
        {
            help.ask(question_for(next));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("とばす ⏭")
                .on_hover_text("あとでやっても経験値はもらえます。一覧から戻せます")
                .clicked()
            {
                playful.skip_mission(next.id);
                tutorial.point();
            }
        });
    });
}

/// 章の中の進み具合を ● ○ で出す
fn chapter_dots(ui: &mut egui::Ui, next: &Mission, playful: &PlayfulUi) {
    let progress = &playful.prefs.progress;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for m in next.chapter.missions() {
            let (text, color) = if progress.is_done(m.id) {
                ("●", palette::GREEN)
            } else if m.id == next.id {
                ("◉", palette::YELLOW)
            } else if progress.is_skipped(m.id) {
                ("◌", Color32::GRAY)
            } else {
                ("○", Color32::GRAY)
            };
            ui.label(egui::RichText::new(text).color(color))
                .on_hover_text(m.title);
        }
        ui.weak(format!(
            "全体 {}/{}",
            progress.settled_count() + 1,
            MISSIONS.len()
        ));
    });
}

/// キーボードのキーの形
fn keycap(ui: &mut egui::Ui, key: &str) {
    egui::Frame::new()
        .fill(palette::NAVY_DEEP)
        .stroke(Stroke::new(1.0, Color32::from_gray(140)))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(key)
                    .monospace()
                    .strong()
                    .color(Color32::WHITE),
            );
        });
}

/// 今のミッションの操作する場所を、点滅する矢印や枠で指す
fn pointer(
    ctx: &egui::Context,
    view: egui::Rect,
    now: f64,
    next: &'static Mission,
    tutorial: &mut TutorialUi,
) {
    let fresh = tutorial.pointer.is_none_or(|(_, key)| key != next.key);
    if fresh || tutorial.point_now {
        tutorial.pointer = Some((now, next.key));
        tutorial.point_now = false;
    }
    let Some((started, _)) = tutorial.pointer else {
        return;
    };
    let age = now - started;
    if age > POINTER_SECONDS {
        return;
    }
    let fade = ((POINTER_SECONDS - age) / 1.0).min(1.0) as f32;
    let pulse = 0.5 + 0.5 * (age as f32 * 5.0).sin();
    let color = palette::YELLOW.gamma_multiply(fade * (0.55 + 0.45 * pulse));
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        "tutorial_pointer".into(),
    ));
    let nudge = pulse * 10.0;
    let stroke = Stroke::new(5.0, color);
    let label = |at: egui::Pos2, align: Align2, text: &str| {
        let font = egui::FontId::proportional(16.0);
        let galley = painter.layout_no_wrap(text.to_owned(), font, Color32::WHITE);
        let rect = align.anchor_size(at, galley.size()).expand2(vec2(8.0, 4.0));
        painter.rect_filled(
            rect,
            CornerRadius::same(8),
            palette::NAVY_DEEP.gamma_multiply(fade),
        );
        painter.rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(1.5, color),
            egui::StrokeKind::Outside,
        );
        painter.galley(
            rect.min + vec2(8.0, 4.0),
            galley,
            Color32::WHITE.gamma_multiply(fade),
        );
    };
    match next.place {
        Place::Viewport => {
            painter.rect_stroke(
                view.shrink(6.0 + nudge * 0.4),
                CornerRadius::same(12),
                stroke,
                egui::StrokeKind::Inside,
            );
            label(
                view.center_top() + vec2(0.0, 28.0),
                Align2::CENTER_TOP,
                "3D ビューで操作",
            );
        }
        Place::Chart => {
            let tip = pos2(view.center().x, view.max.y - 6.0);
            painter.arrow(tip - vec2(0.0, 70.0 + nudge), vec2(0.0, 60.0), stroke);
            label(
                tip - vec2(0.0, 80.0 + nudge),
                Align2::CENTER_BOTTOM,
                "下のタイミングチャート",
            );
        }
        Place::Tree => {
            let tip = pos2(view.min.x + 6.0, view.center().y);
            painter.arrow(tip + vec2(70.0 + nudge, 0.0), vec2(-60.0, 0.0), stroke);
            label(
                tip + vec2(80.0 + nudge, 0.0),
                Align2::LEFT_CENTER,
                "左のツリー",
            );
        }
        Place::Rail => {
            // ツールレールは画面の左端 (アセンブリツリーのさらに左)。そのボタンの横を指す
            let y = next
                .tool
                .and_then(|tool| dock::rail_button_rect(ctx, tool))
                .map_or(view.center().y, |r| r.center().y);
            let tip = pos2(RAIL_WIDTH + 4.0, y);
            painter.arrow(tip + vec2(80.0 + nudge, 0.0), vec2(-70.0, 0.0), stroke);
            let text = match next.tool {
                Some(tool) => format!("左端の {} {}", dock::icon(tool), tool.short_label()),
                None => "左端のツールレール".to_owned(),
            };
            label(tip + vec2(90.0 + nudge, 0.0), Align2::LEFT_CENTER, &text);
        }
        Place::Menu => {
            let tip = pos2(view.min.x + 40.0, view.min.y + 4.0);
            painter.arrow(tip + vec2(0.0, 70.0 + nudge), vec2(0.0, -60.0), stroke);
            label(tip + vec2(0.0, 80.0 + nudge), Align2::LEFT_TOP, next.hint);
        }
        Place::Keyboard => {
            let keys = if next.keys.is_empty() {
                next.hint.to_owned()
            } else {
                format!("⌨ {}", next.keys.join(" + "))
            };
            label(
                view.center_top() + vec2(0.0, 28.0 + nudge * 0.5),
                Align2::CENTER_TOP,
                &keys,
            );
        }
    }
    ctx.request_repaint();
}
