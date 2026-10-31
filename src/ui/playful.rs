//! ゲーム風の UI: 見た目の反映、ミッションカードとマスコット、達成時のお祝い (紙吹雪)
//!
//! ミッションの判定や経験値の計算は `gamify` にあり、ここは
//! 「画面の状態を集めて渡す → 達成を受け取って演出する」だけをする。

use std::collections::VecDeque;
use std::path::PathBuf;

use bevy::prelude::*;
use bevy_egui::{
    EguiContexts,
    egui::{self, Align2, Color32, CornerRadius, Pos2, Stroke, Vec2, pos2, vec2},
};

use super::dock::Dock;
use super::theme::{self, palette};
use super::tutorial::{self, TutorialUi};
use crate::annotation::Annotations;
use crate::assistant::help::HelpChat;
use crate::camera::{OrbitCamera, ViewportRect};
use crate::flow::FlowRun;
use crate::history::History;
use crate::model::NodeKind;
use crate::model::{Model, Selection};
use crate::motion::Playback;
use crate::preferences_file;
use crate::project::ProjectState;
use crate::simulation::InterferenceCheck;
use crate::simulation::interference::PairStatus;
use crate::simulation::measure::MeasureTool;
use cadsim_core::gamify::{
    Achievement, Chapter, ConfettiPiece, MissionId, Observation, Preferences, confetti,
    level_title, mission,
};

/// お祝いの表示時間 (秒)
const TOAST_SECONDS: f64 = 3.2;
const CONFETTI_SECONDS: f32 = 3.0;
const CONFETTI_PIECES: usize = 90;
const CARD_WIDTH: f32 = 228.0;

/// ゲーム風 UI の設定と、演出の途中経過
#[derive(Resource)]
pub struct PlayfulUi {
    pub prefs: Preferences,
    path: Option<PathBuf>,
    /// 設定ファイルに書き出していない変更がある
    dirty: bool,
    /// egui に見た目を反映済みか
    applied: bool,
    toasts: VecDeque<Achievement>,
    /// 先頭のお祝いを出し始めた時刻 (egui の時刻)
    toast_started: Option<f64>,
    confetti: Vec<ConfettiPiece>,
    confetti_started: Option<f64>,
    bursts: u64,
    show_list: bool,
    /// ミッションカードに今のミッションの手順を出す
    pub show_steps: bool,
}

impl PlayfulUi {
    fn load() -> Self {
        let path = preferences_file::default_path();
        let prefs = path
            .as_deref()
            .map(preferences_file::load)
            .unwrap_or_default();
        Self {
            prefs,
            path,
            dirty: false,
            applied: false,
            toasts: VecDeque::new(),
            toast_started: None,
            confetti: Vec::new(),
            confetti_started: None,
            bursts: 0,
            show_list: false,
            show_steps: true,
        }
    }

    pub fn set_show_missions(&mut self, show: bool) {
        if self.prefs.show_missions != show {
            self.prefs.show_missions = show;
            self.dirty = true;
        }
    }

    /// ようこそ画面を閉じた
    pub fn mark_welcomed(&mut self) {
        if !self.prefs.welcomed {
            self.prefs.welcomed = true;
            self.dirty = true;
        }
    }

    pub fn skip_mission(&mut self, id: MissionId) {
        self.prefs.progress.skip(id);
        self.dirty = true;
    }

    pub fn retry_mission(&mut self, id: MissionId) {
        self.prefs.progress.retry(id);
        self.dirty = true;
    }

    pub fn reset_missions(&mut self) {
        self.prefs.progress.reset();
        self.prefs.show_missions = true;
        self.toasts.clear();
        self.toast_started = None;
        self.dirty = true;
    }

    fn celebrate(&mut self, achievement: Achievement) {
        self.bursts += 1;
        let count = if achievement.level_up.is_some()
            || achievement.chapter_complete.is_some()
            || achievement.all_complete
        {
            CONFETTI_PIECES * 2
        } else {
            CONFETTI_PIECES
        };
        self.confetti = confetti(self.bursts, count, palette::CONFETTI.len());
        self.confetti_started = None;
        self.toasts.push_back(achievement);
    }
}

pub struct PlayfulPlugin;

impl Plugin for PlayfulPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(PlayfulUi::load())
            .init_resource::<TutorialUi>()
            .add_systems(Update, (track_missions, save_preferences).chain());
    }
}

/// 画面の状態を集めてミッションの達成を調べる
#[allow(clippy::too_many_arguments)]
fn track_missions(
    mut ui: ResMut<PlayfulUi>,
    model: Res<Model>,
    selection: Res<Selection>,
    orbit: Res<OrbitCamera>,
    history: Res<History>,
    measure: Res<MeasureTool>,
    check: Res<InterferenceCheck>,
    annotations: Res<Annotations>,
    playback: Res<Playback>,
    project: Res<ProjectState>,
    viewport: Res<ViewportRect>,
    mouse: Res<ButtonInput<MouseButton>>,
    flow: Res<FlowRun>,
) {
    if ui.prefs.progress.all_complete() {
        return;
    }
    let interfering_pairs = check
        .report
        .pairs
        .iter()
        .filter(|pair| matches!(pair.status, PairStatus::Interfering { .. }))
        .count();
    let saved_to_file = project.has_file() && !project.unsaved();
    let now = Observation {
        selected: selection.node.is_some_and(|id| model.get(id).is_some()),
        camera_angles: [orbit.yaw as f64, orbit.pitch as f64],
        dragging_view: !viewport.pointer_on_ui
            && mouse.any_pressed([MouseButton::Left, MouseButton::Right, MouseButton::Middle]),
        orthographic: orbit.orthographic,
        last_undo_label: history.undo_label(),
        can_redo: history.redo_label().is_some(),
        measurement_count: measure.measurements.len(),
        interfering_pairs,
        annotation_count: annotations.items.len(),
        playing: playback.playing,
        saved_to_file,
        library_part_count: model
            .all_nodes()
            .into_iter()
            .filter(|&id| {
                model
                    .get(id)
                    .is_some_and(|n| matches!(n.kind, NodeKind::Standard(_)))
            })
            .count(),
        flow_has_result: flow.report.is_some(),
    };
    let achievements = ui.prefs.progress.observe(&now);
    if !achievements.is_empty() {
        ui.dirty = true;
    }
    for achievement in achievements {
        ui.celebrate(achievement);
    }
}

fn save_preferences(mut ui: ResMut<PlayfulUi>) {
    if !ui.dirty {
        return;
    }
    ui.dirty = false;
    if let Some(path) = &ui.path
        && let Err(err) = preferences_file::save(path, &ui.prefs)
    {
        warn!("設定を保存できませんでした ({}): {err}", path.display());
    }
}

/// 最初のフレームで見た目を egui と 3D ビューの背景に反映する
pub fn apply_theme(
    mut contexts: EguiContexts,
    mut ui: ResMut<PlayfulUi>,
    mut clear: ResMut<ClearColor>,
) -> Result {
    if ui.applied {
        return Ok(());
    }
    theme::apply(contexts.ctx_mut()?);
    clear.0 = theme::CLEAR_COLOR;
    ui.applied = true;
    Ok(())
}

/// ミッションカード・お祝い・紙吹雪を 3D ビューの上に描く
pub fn draw_overlay(
    mut contexts: EguiContexts,
    mut ui: ResMut<PlayfulUi>,
    mut tutorial: ResMut<TutorialUi>,
    mut playback: ResMut<Playback>,
    mut help: ResMut<HelpChat>,
    mut dock: ResMut<Dock>,
    viewport: Res<ViewportRect>,
) -> Result {
    let Some(rect) = viewport.rect else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?.clone();
    let view = egui::Rect::from_min_max(pos2(rect.min.x, rect.min.y), pos2(rect.max.x, rect.max.y));
    let now = ctx.input(|i| i.time);

    // ようこそ画面を出している間はカードを隠す (tutorial::draw がようこそ画面を描く)
    if ui.prefs.show_missions && ui.prefs.welcomed && !tutorial.welcome_requested {
        let mut card = tutorial::CardCtx {
            tutorial: &mut tutorial,
            playback: &mut playback,
            help: &mut help,
            dock: &mut dock,
        };
        mission_card(&ctx, view, now, &mut ui, &mut card);
    }
    draw_toast(&ctx, view, now, &mut ui);
    draw_confetti(&ctx, view, now, &mut ui);
    Ok(())
}

fn mission_card(
    ctx: &egui::Context,
    view: egui::Rect,
    now: f64,
    state: &mut PlayfulUi,
    card: &mut tutorial::CardCtx,
) {
    let celebrating = !state.toasts.is_empty();
    egui::Area::new("mission_card".into())
        // 3D ビューの左下、座標軸の右に置く。背景をドラッグすると動かせる
        .pivot(Align2::LEFT_BOTTOM)
        .default_pos(pos2(view.min.x + 96.0, view.max.y - 10.0))
        .constrain_to(view)
        .show(ctx, |ui| {
            egui::Frame::window(ui.style())
                .stroke(Stroke::new(1.5, palette::PURPLE.gamma_multiply(0.7)))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    // 手順を出すときは読みやすいよう広げる
                    let steps = state.show_steps && state.prefs.progress.next().is_some();
                    ui.set_width(if steps {
                        tutorial::CARD_WIDTH_WITH_STEPS
                    } else {
                        CARD_WIDTH
                    });
                    ui.horizontal(|ui| {
                        mascot(ui, 42.0, now, celebrating);
                        level_header(ui, state);
                    });
                    ui.add_space(2.0);
                    current_mission(ui, state, card);
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        let arrow = if state.show_list { "▼" } else { "▶" };
                        if ui.small_button(format!("{arrow} 一覧")).clicked() {
                            state.show_list = !state.show_list;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button("隠す")
                                .on_hover_text("ツールバー右のレベル表示を押すとまた出せます")
                                .clicked()
                            {
                                state.set_show_missions(false);
                            }
                        });
                    });
                    if state.show_list {
                        mission_list(ui, state);
                    }
                });
        });
}

fn level_header(ui: &mut egui::Ui, state: &PlayfulUi) {
    let level = state.prefs.progress.level();
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("Lv.{}", level.level))
                    .strong()
                    .color(palette::YELLOW)
                    .size(18.0),
            );
            ui.label(egui::RichText::new(level.title()).color(palette::TEXT_STRONG));
        });
        ui.add(
            egui::ProgressBar::new(level.fraction())
                .desired_width(CARD_WIDTH - 56.0)
                .desired_height(14.0)
                .fill(palette::TEAL)
                .corner_radius(CornerRadius::same(7))
                .text(
                    egui::RichText::new(format!("{} / {} XP", level.into_level, level.needed))
                        .size(10.5)
                        .color(palette::TEXT_STRONG),
                ),
        );
    });
}

fn current_mission(ui: &mut egui::Ui, state: &mut PlayfulUi, card: &mut tutorial::CardCtx) {
    match state.prefs.progress.next() {
        Some(next) => tutorial::mission_body(ui, next, state, card),
        None if !state.prefs.progress.all_complete() => {
            ui.label("残りはとばしたミッションだけです。");
            ui.label(
                egui::RichText::new("「▶ 一覧」の「戻す」で、もう一度挑戦できます。")
                    .small()
                    .color(palette::TEXT),
            );
        }
        None => {
            ui.label(
                egui::RichText::new("🏆 全ミッション達成! もう立派なマイスターです")
                    .strong()
                    .color(palette::YELLOW),
            );
            if ui.small_button("最初から遊ぶ").clicked() {
                state.reset_missions();
            }
        }
    }
}

fn mission_list(ui: &mut egui::Ui, state: &mut PlayfulUi) {
    let mut retry = None;
    egui::Grid::new("mission_list")
        .num_columns(3)
        .spacing([6.0, 3.0])
        .show(ui, |ui| {
            for chapter in Chapter::ALL {
                ui.label(
                    egui::RichText::new(format!("第{}章 {}", chapter.number(), chapter.title()))
                        .small()
                        .color(palette::TEAL),
                );
                ui.end_row();
                for m in chapter.missions() {
                    let progress = &state.prefs.progress;
                    if progress.is_done(m.id) {
                        ui.colored_label(palette::GREEN, "✔");
                        ui.weak(m.title);
                        ui.weak(format!("{} XP", m.xp));
                    } else if progress.is_skipped(m.id) {
                        ui.weak("↷");
                        ui.weak(m.title).on_hover_text(m.hint);
                        if ui
                            .small_button("戻す")
                            .on_hover_text("次のミッションの候補に戻す")
                            .clicked()
                        {
                            retry = Some(m.id);
                        }
                    } else {
                        ui.weak("○");
                        ui.label(m.title).on_hover_text(m.hint);
                        ui.weak(format!("{} XP", m.xp));
                    }
                    ui.end_row();
                }
            }
        });
    if let Some(id) = retry {
        state.retry_mission(id);
    }
}

/// 案内役のマスコット (立方体のキャラクター)。喜んでいるときは跳ねて目を細める。
pub(super) fn mascot(ui: &mut egui::Ui, size: f32, now: f64, happy: bool) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::click());
    let t = now as f32;
    let bounce = if happy {
        -(t * 9.0).sin().abs() * 6.0
    } else {
        (t * 2.0).sin() * 1.5
    };
    let body = egui::Rect::from_center_size(
        rect.center() + vec2(0.0, bounce + 2.0),
        Vec2::splat(size * 0.78),
    );
    let painter = ui.painter_at(rect.expand(8.0));
    // 影
    painter.rect_filled(
        egui::Rect::from_center_size(
            pos2(rect.center().x, rect.bottom() - 1.0),
            vec2(size * 0.6 - bounce.abs(), 4.0),
        ),
        CornerRadius::same(2),
        Color32::from_black_alpha(70),
    );
    painter.rect(
        body,
        CornerRadius::same(10),
        palette::ORANGE,
        Stroke::new(2.0, Color32::from_rgb(0xc8, 0x6a, 0x1c)),
        egui::StrokeKind::Inside,
    );
    // 上面のつや
    painter.rect_filled(
        egui::Rect::from_min_size(body.min + vec2(6.0, 4.0), vec2(body.width() * 0.4, 4.0)),
        CornerRadius::same(2),
        Color32::from_white_alpha(90),
    );
    let eye_y = body.center().y - body.height() * 0.08;
    let eye_dx = body.width() * 0.2;
    // 4 秒に一度まばたき
    let blinking = (t % 4.0) < 0.12;
    let dark = Color32::from_rgb(0x2a, 0x1a, 0x10);
    for side in [-1.0, 1.0] {
        let eye = pos2(body.center().x + side * eye_dx, eye_y);
        if happy {
            // ^ ^ の目
            let w = 4.5;
            painter.line_segment(
                [eye + vec2(-w, 2.0), eye + vec2(0.0, -2.5)],
                Stroke::new(2.2, dark),
            );
            painter.line_segment(
                [eye + vec2(0.0, -2.5), eye + vec2(w, 2.0)],
                Stroke::new(2.2, dark),
            );
        } else if blinking {
            painter.line_segment(
                [eye + vec2(-4.0, 0.0), eye + vec2(4.0, 0.0)],
                Stroke::new(2.2, dark),
            );
        } else {
            painter.circle_filled(eye, 5.0, Color32::WHITE);
            painter.circle_filled(eye + vec2(0.8, 0.8), 2.6, dark);
        }
        // ほっぺ
        painter.circle_filled(
            pos2(eye.x + side * 4.0, eye_y + 8.0),
            3.0,
            Color32::from_rgba_unmultiplied(0xff, 0x6b, 0x9a, 150),
        );
    }
    // 口 (笑顔の弧)
    let mouth_c = pos2(body.center().x, body.center().y + body.height() * 0.12);
    let open = if happy { 6.0 } else { 4.0 };
    let points: Vec<Pos2> = (0..=8)
        .map(|i| {
            let a = std::f32::consts::PI * (i as f32 / 8.0);
            mouth_c + vec2(-a.cos() * open, a.sin() * open * 0.7)
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(2.2, dark)));
    response.on_hover_text("いっしょに覚えよう!");
}

fn draw_toast(ctx: &egui::Context, view: egui::Rect, now: f64, state: &mut PlayfulUi) {
    let Some(achievement) = state.toasts.front().cloned() else {
        return;
    };
    let started = *state.toast_started.get_or_insert(now);
    let age = now - started;
    if age > TOAST_SECONDS {
        state.toasts.pop_front();
        state.toast_started = None;
        return;
    }
    // 出るときは少し大きくなってから戻り、最後はふわっと消える
    let pop = ease_out_back((age / 0.35).min(1.0) as f32);
    let fade = (((TOAST_SECONDS - age) / 0.5).min(1.0)) as f32;
    let rise = (1.0 - fade) * -16.0;

    egui::Area::new("mission_toast".into())
        .order(egui::Order::Foreground)
        .interactable(false)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(view.center_top() + vec2(0.0, 56.0 + rise))
        .show(ctx, |ui| {
            ui.multiply_opacity(fade);
            // 大きくなる途中で折り返さないようにする
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            let frame = egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(22, 12))
                .fill(palette::NAVY_LIGHT)
                .stroke(Stroke::new(2.5, palette::YELLOW))
                .corner_radius(CornerRadius::same(18));
            frame.show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    let size = |base: f32| base * pop.max(0.2);
                    let headline = if achievement.all_complete {
                        "🏆 全ミッション制覇!".to_owned()
                    } else if let Some(chapter) = achievement.chapter_complete {
                        format!("📘 第{}章 クリア!", chapter.number())
                    } else {
                        "🎉 ミッション達成!".to_owned()
                    };
                    ui.label(
                        egui::RichText::new(headline)
                            .strong()
                            .size(size(24.0))
                            .color(palette::YELLOW),
                    );
                    ui.label(
                        egui::RichText::new(mission(achievement.mission).title).size(size(16.0)),
                    );
                    let xp = egui::RichText::new(format!("+{} XP", achievement.xp))
                        .strong()
                        .size(size(18.0));
                    ui.label(xp.color(palette::TEAL));
                    if let Some(level) = achievement.level_up {
                        let title = level_title(level);
                        let text =
                            egui::RichText::new(format!("⬆ レベルアップ! Lv.{level} {title}"))
                                .strong()
                                .size(size(17.0));
                        ui.label(text.color(palette::PINK));
                    }
                });
            });
        });
}

fn draw_confetti(ctx: &egui::Context, view: egui::Rect, now: f64, state: &mut PlayfulUi) {
    if state.confetti.is_empty() {
        return;
    }
    let started = *state.confetti_started.get_or_insert(now);
    let t = (now - started) as f32;
    if t > CONFETTI_SECONDS {
        state.confetti.clear();
        state.confetti_started = None;
        return;
    }
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            "confetti".into(),
        ))
        .with_clip_rect(view);
    let fade = ((CONFETTI_SECONDS - t) / 0.6).min(1.0);
    for piece in &state.confetti {
        let (x, y, angle) = piece.at(t);
        let center = pos2(
            view.min.x + x * view.width(),
            view.min.y + y * view.height(),
        );
        let color = palette::CONFETTI[piece.color].gamma_multiply(fade);
        // 回転しながらひらひらする長方形 (横幅を cos で縮めて裏返る感じを出す)
        let half = vec2(5.0 * (angle * 0.7).cos().abs().max(0.25), 3.0);
        let (s, c) = angle.sin_cos();
        let rotate = |v: Vec2| center + vec2(v.x * c - v.y * s, v.x * s + v.y * c);
        let points = vec![
            rotate(vec2(-half.x, -half.y)),
            rotate(vec2(half.x, -half.y)),
            rotate(vec2(half.x, half.y)),
            rotate(vec2(-half.x, half.y)),
        ];
        painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
    }
    ctx.request_repaint();
}

fn ease_out_back(x: f32) -> f32 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
}
