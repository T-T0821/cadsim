//! 「AI に聞く」ウィンドウ: 使い方を会話で質問する
//!
//! 質問の組み立てと返事の読み取りは `cadsim_core::usecase::help`。
//! ここは画面の様子を集めて渡し、答えと「開く」ボタンを出すだけにする。
//! 他の画面からは [`HelpChat::ask`] で質問文を入れた状態で開ける。

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use super::AiAssistant;
use super::job::LlmJob;
use crate::camera::ViewportRect;
use crate::model::{Model, Selection};
use crate::motion::TimingChart;
use crate::ui::dock::{self, Dock};
use crate::ui::playful::PlayfulUi;
use crate::ui::theme::palette;
use cadsim_core::usecase::help::{self, Answer, HelpContext};
use cadsim_core::usecase::llm::LlmMessage;
use cadsim_core::workspace::Tool;

/// 質問の例 (会話が空のときにボタンで出す)
const EXAMPLES: [&str; 4] = [
    "シリンダを置いて動かすには?",
    "部品どうしがぶつかっていないか調べたい",
    "非常停止で設備が止まるか確かめたい",
    "ラインの稼働率を見たい",
];

enum Entry {
    User(String),
    Ai(Answer),
    /// キーがないときの手がかり
    Offline(Vec<Tool>),
    Error(String),
}

/// 「AI に聞く」の会話
#[derive(Resource, Default)]
pub struct HelpChat {
    pub open: bool,
    input: String,
    entries: Vec<Entry>,
    job: Option<LlmJob<Answer>>,
    /// 次のフレームで質問欄にフォーカスを移す
    focus_input: bool,
}

impl HelpChat {
    /// 開く (閉じていれば)
    pub fn open(&mut self) {
        if !self.open {
            self.open = true;
            self.focus_input = true;
        }
    }

    pub fn toggle(&mut self) {
        if self.open {
            self.open = false;
        } else {
            self.open();
        }
    }

    /// 質問欄に文面を入れた状態で開く (送るのはユーザーが押してから)。
    /// ミッションカードなど、ほかの画面から「この操作を聞く」ときの入り口。
    #[allow(dead_code)]
    pub fn ask(&mut self, question: impl Into<String>) {
        self.input = question.into();
        self.open = true;
        self.focus_input = true;
    }

    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    /// これまでの会話 (AI に渡す分)
    fn history(&self) -> Vec<LlmMessage> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                Entry::User(q) => Some(LlmMessage::user(q.clone())),
                Entry::Ai(a) => Some(LlmMessage::assistant(a.text.clone())),
                _ => None,
            })
            .collect()
    }

    fn send(&mut self, ai: &AiAssistant, context: HelpContext) {
        let question = self.input.trim().to_owned();
        if question.is_empty() || self.is_busy() {
            return;
        }
        self.input.clear();
        if !ai.has_key() {
            self.entries.push(Entry::User(question.clone()));
            self.entries
                .push(Entry::Offline(help::suggest_tools(&question)));
            return;
        }
        // 質問は答えが返ってきてから履歴に入れる (失敗したときに会話が崩れないように)
        let history = self.history();
        self.entries.push(Entry::User(question.clone()));
        self.job = Some(LlmJob::start(ai.client(), move |client| {
            help::ask(client, &history, &question, &context)
        }));
    }

    fn poll(&mut self) {
        if let Some(result) = self.job.as_ref().and_then(LlmJob::poll) {
            self.job = None;
            self.entries.push(match result {
                Ok(answer) => Entry::Ai(answer),
                Err(err) => Entry::Error(err.to_string()),
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn help_window(
    mut contexts: EguiContexts,
    mut chat: ResMut<HelpChat>,
    mut ai: ResMut<AiAssistant>,
    mut dock: ResMut<Dock>,
    model: Res<Model>,
    selection: Res<Selection>,
    chart: Res<TimingChart>,
    playful: Res<PlayfulUi>,
    viewport: Res<ViewportRect>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
        chat.toggle();
    }
    if !chat.open {
        return Ok(());
    }
    let chat = chat.as_mut();
    chat.poll();
    if chat.is_busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    let mut open = true;
    let mut window = egui::Window::new("💬  AI に聞く")
        .id(egui::Id::new("help_chat"))
        .open(&mut open)
        .default_width(360.0)
        .resizable(true)
        .collapsible(true);
    if let Some(view) = viewport.rect {
        let rect = egui::Rect::from_min_max(
            egui::pos2(view.min.x, view.min.y),
            egui::pos2(view.max.x, view.max.y),
        );
        // ビューキューブの下、3D ビューの右端に出す。ビューの外 (チャートの上) にははみ出さない
        window = window
            .pivot(egui::Align2::RIGHT_TOP)
            .default_pos([rect.max.x - 10.0, rect.min.y + 150.0])
            .default_height((rect.height() - 170.0).clamp(240.0, 460.0))
            .constrain_to(rect.shrink(2.0));
    }
    let mut send = false;
    let mut open_tool = None;
    window.show(&ctx, |ui| {
        if !ai.has_key() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(palette::YELLOW, "⚠");
                ui.label(
                    egui::RichText::new(
                        "AI の API キーが未設定です。キーなしでも、質問の言葉から道具を探して案内します。",
                    )
                    .small(),
                );
                if ui.small_button("AI の設定…").clicked() {
                    ai.settings_open = true;
                }
            });
            ui.separator();
        }
        let input_height = 64.0;
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - input_height).max(120.0))
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if chat.entries.is_empty() {
                    ui.label("操作で迷ったら、したいことを文で聞いてください。画面の様子を見て、手順と使う道具を答えます。");
                    ui.add_space(6.0);
                    for example in EXAMPLES {
                        if ui.button(format!("💡 {example}")).clicked() {
                            chat.input = example.into();
                            send = true;
                        }
                    }
                }
                for entry in &chat.entries {
                    entry_ui(ui, entry, &mut open_tool);
                }
                if let Some(job) = &chat.job {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak(format!("考え中… {} 秒", job.elapsed_secs()));
                    });
                }
            });
        ui.separator();
        ui.horizontal(|ui| {
            let edit = ui.add(
                egui::TextEdit::multiline(&mut chat.input)
                    .hint_text("例: コンベアの上でワークを流したい")
                    .desired_rows(2)
                    .desired_width(ui.available_width() - 64.0),
            );
            if chat.focus_input {
                edit.request_focus();
                chat.focus_input = false;
            }
            // Enter で送る (Shift+Enter は改行)
            if edit.has_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift)
            {
                send = true;
            }
            ui.vertical(|ui| {
                let can_send = !chat.is_busy() && !chat.input.trim().is_empty();
                if ui
                    .add_enabled(can_send, egui::Button::new("送る"))
                    .on_hover_text("Enter で送る / Shift+Enter で改行")
                    .clicked()
                {
                    send = true;
                }
                if ui
                    .add_enabled(!chat.entries.is_empty(), egui::Button::new("消す").small())
                    .on_hover_text("会話を最初からにする")
                    .clicked()
                {
                    chat.entries.clear();
                }
            });
        });
    });
    if send {
        // Enter の改行が入っていれば取り除いてから送る
        let trimmed = chat.input.trim_end_matches('\n').to_owned();
        chat.input = trimmed;
        let context = HelpContext {
            part_count: model
                .roots
                .iter()
                .map(|&r| model.measure(r).part_count)
                .sum(),
            selected: selection
                .node
                .filter(|&id| model.get(id).is_some())
                .map(|id| model.path_label(id)),
            open_tools: dock.open_tools().to_vec(),
            chart_steps: chart.steps.len(),
            mission: playful.prefs.progress.next().map(|m| m.title.to_owned()),
        };
        chat.send(&ai, context);
    }
    if let Some(tool) = open_tool {
        dock.set_open(tool, true);
    }
    if !open {
        chat.open = false;
    }
    Ok(())
}

fn entry_ui(ui: &mut egui::Ui, entry: &Entry, open_tool: &mut Option<Tool>) {
    ui.add_space(4.0);
    match entry {
        Entry::User(text) => {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                bubble(ui, palette::PURPLE.gamma_multiply(0.45), |ui| {
                    ui.label(egui::RichText::new(text).color(palette::TEXT_STRONG));
                });
            });
        }
        Entry::Ai(answer) => {
            bubble(ui, palette::NAVY_BUTTON, |ui| {
                ui.label(&answer.text);
                tool_buttons(ui, &answer.tools, open_tool);
            });
        }
        Entry::Offline(tools) => {
            bubble(ui, palette::NAVY_BUTTON, |ui| {
                if tools.is_empty() {
                    ui.label("近い道具が見つかりませんでした。左のツールレールのボタンにカーソルを乗せると、それぞれの説明が出ます。");
                } else {
                    ui.label("この道具が近そうです:");
                    tool_buttons(ui, tools, open_tool);
                }
            });
        }
        Entry::Error(text) => {
            ui.colored_label(palette::PINK, format!("⚠ {text}"));
        }
    }
}

fn tool_buttons(ui: &mut egui::Ui, tools: &[Tool], open_tool: &mut Option<Tool>) {
    if tools.is_empty() {
        return;
    }
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        for &tool in tools {
            if ui
                .button(format!("{} を開く", dock::title(tool)))
                .on_hover_text(tool.hint())
                .clicked()
            {
                *open_tool = Some(tool);
            }
        }
    });
}

fn bubble(ui: &mut egui::Ui, fill: egui::Color32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_max_width(290.0);
            add(ui);
        });
}
