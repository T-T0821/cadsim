//! 言葉でラインを組む (UI 層)
//!
//! ツールレールの「言葉で」(言葉でラインを組む) で開くウィンドウ。文を入れて「下書きを作る」を押すと、
//! 別スレッドでユースケース [`compose_line`] を呼び、できた下書きを上から見た図と
//! ステップの一覧で見せる。「モデルとチャートに置く」で [`apply::apply`] を呼んで反映する。
//!
//! - `apply.rs` … 下書きをモデル・タイミングチャート・ワーク搬送の設定に置く (Bevy 非依存)
//!
//! 生成 AI は「ツール → AI の設定」の API キーがあるときだけ使い (共通の [`AiAssistant`])、
//! なければ・失敗したら文のキーワードから作る。

pub mod apply;

use crate::ui::dock::{self, Dock};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use plantwright_core::workspace::Tool;

use plantwright_core::line_draft::EquipmentKind;
use plantwright_core::line_draft::compose::{ComposedLine, LineDraftAi, compose_line};
use plantwright_core::line_draft::json::LlmLineDrafter;

use crate::assistant::AiAssistant;
use crate::assistant::job::LlmJob;

use crate::conveyor::ConveyorConfigRes;
use crate::history::History;
use crate::model::Selection;
use plantwright_core::model::Model;
use plantwright_core::motion::chart::TimingChart;

pub struct LineComposePlugin;

impl Plugin for LineComposePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ComposeUi>()
            .add_systems(EguiPrimaryContextPass, compose_window);
    }
}

/// 例として入力欄の下に並べる文
const EXAMPLES: [&str; 3] = [
    "コンベア2本とロボット1台で箱詰め",
    "コンベア1本、センサー1個、ストッパで外観検査",
    "投入と排出のコンベア2本、ロボット2台で移載",
];

/// 「言葉でラインを組む」ウィンドウの状態
#[derive(Resource)]
pub struct ComposeUi {
    text: String,
    /// 作っている最中の下書き (別スレッドで作る)
    job: Option<LlmJob<Result<ComposedLine, String>>>,
    result: Option<ComposedLine>,
    error: Option<String>,
    /// 置いたあとの知らせ
    message: Option<String>,
    /// モデルとチャートに置いた回数 (チュートリアルの達成に使う)
    placed: u32,
}

impl Default for ComposeUi {
    fn default() -> Self {
        Self {
            text: EXAMPLES[0].into(),
            job: None,
            result: None,
            error: None,
            message: None,
            placed: 0,
        }
    }
}

impl ComposeUi {
    /// モデルとチャートに置いた回数
    pub fn placed_count(&self) -> u32 {
        self.placed
    }

    /// 別スレッドで下書きを作り始める。`use_ai` なら生成 AI に、そうでなければキーワードから。
    fn start(&mut self, assistant: &AiAssistant) {
        let text = self.text.clone();
        let use_ai = assistant.has_key();
        self.job = Some(LlmJob::start(assistant.client(), move |client| {
            let drafter = LlmLineDrafter { client };
            let ai = use_ai.then_some(&drafter as &dyn LineDraftAi);
            Ok(compose_line(&text, ai))
        }));
        self.error = None;
        self.message = None;
    }

    /// 別スレッドの結果が届いていれば受け取る
    fn poll(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        let Some(result) = job.poll() else {
            return;
        };
        self.job = None;
        match result.map_err(|e| e.to_string()).and_then(|r| r) {
            Ok(line) => {
                self.result = Some(line);
                self.error = None;
            }
            Err(e) => {
                self.result = None;
                self.error = Some(e);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn compose_window(
    mut contexts: EguiContexts,
    mut state: ResMut<ComposeUi>,
    mut model: ResMut<Model>,
    mut chart: ResMut<TimingChart>,
    mut conveyors: ResMut<ConveyorConfigRes>,
    mut history: ResMut<History>,
    mut selection: ResMut<Selection>,
    mut assistant: ResMut<AiAssistant>,
    mut dock: ResMut<Dock>,
) -> Result {
    let Some(window) = dock::workspace_window(Tool::Compose, &dock, [520.0, 560.0]) else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let state = state.bypass_change_detection();
    state.poll();
    if state.job.is_some() {
        ctx.request_repaint();
    }

    let mut open = true;
    let mut place = false;
    window
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label("作りたいラインを文で書いてください。設備の種類と台数、作業の内容を入れると下書きになります。");
            ui.add(
                egui::TextEdit::multiline(&mut state.text)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text(EXAMPLES[0]),
            );
            ui.horizontal_wrapped(|ui| {
                ui.weak("例:");
                for example in EXAMPLES {
                    if ui.small_button(example).clicked() {
                        state.text = example.into();
                    }
                }
            });
            ui.horizontal(|ui| {
                let busy = state.job.is_some();
                if ui
                    .add_enabled(!busy, egui::Button::new("下書きを作る"))
                    .clicked()
                {
                    state.start(&assistant);
                }
                if let Some(job) = &state.job {
                    ui.spinner();
                    ui.label(format!("作っています… {} 秒", job.elapsed_secs()));
                }
            });
            ui.horizontal(|ui| {
                if assistant.has_key() {
                    ui.weak("生成 AI (Claude) で作ります。使えないときは文のキーワードから作ります");
                } else {
                    ui.weak("API キーが未設定なので、文のキーワードから作ります");
                    if ui.small_button("AI の設定…").clicked() {
                        assistant.settings_open = true;
                    }
                }
            });
            if let Some(error) = &state.error {
                ui.colored_label(egui::Color32::from_rgb(230, 80, 80), error);
            }
            if let Some(message) = &state.message {
                ui.colored_label(egui::Color32::from_rgb(80, 190, 110), message);
            }
            if let Some(line) = &state.result {
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    preview(ui, line);
                });
                ui.separator();
                place = ui
                    .button("モデルとチャートに置く")
                    .on_hover_text(
                        "設備を箱で置いてアセンブリにまとめ、タイミングチャートの後ろにステップを足す。Ctrl+Z で戻せる",
                    )
                    .clicked();
            }
        });
    if !open {
        dock.set_open(Tool::Compose, false);
    }

    if place && let Some(line) = state.result.take() {
        let offset = apply::free_offset(&model, &line);
        let applied = apply::apply(&mut model, &mut chart, &mut conveyors, &line, offset);
        let label = format!("{} を追加", model.path_label(applied.root));
        history.record_added(&model, applied.root, label);
        selection.select(applied.root);
        selection.reveal_in_tree = true;
        state.placed += 1;
        state.message = Some(format!(
            "置きました: アクチュエータ {} 個、センサー {} 個、ステップ {} 個をチャートに足しました",
            applied.actuators, applied.sensors, applied.steps
        ));
    }
    Ok(())
}

/// 下書きの中身: 上から見た図・設備・ステップ・メモ
fn preview(ui: &mut egui::Ui, line: &ComposedLine) {
    let draft = &line.draft;
    ui.horizontal(|ui| {
        ui.strong(&draft.title);
        ui.weak(format!(
            "({}で作成・1サイクル 約 {:.1} 秒)",
            line.origin.label(),
            draft.cycle_seconds()
        ));
    });
    for w in &line.warnings {
        ui.colored_label(egui::Color32::from_rgb(230, 170, 60), format!("⚠ {w}"));
    }
    plan_view(ui, line);

    egui::CollapsingHeader::new(format!("設備 ({} 台)", draft.equipment.len()))
        .default_open(true)
        .show(ui, |ui| {
            for kind in EquipmentKind::ALL {
                let names: Vec<&str> = draft
                    .equipment
                    .iter()
                    .filter(|e| e.kind == kind)
                    .map(|e| e.name.as_str())
                    .collect();
                if !names.is_empty() {
                    ui.label(format!("{}: {}", kind.label(), names.join("、")));
                }
            }
        });
    egui::CollapsingHeader::new(format!("ステップ ({} 個)", draft.steps.len()))
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new("compose_steps")
                .striped(true)
                .num_columns(3)
                .show(ui, |ui| {
                    for (i, step) in draft.steps.iter().enumerate() {
                        ui.label(format!("{}", i + 1));
                        ui.label(&step.comment);
                        let mut detail: Vec<String> = step
                            .actions
                            .iter()
                            .map(|a| match &a.target {
                                Some(t) => format!(
                                    "{} {} ({t}) {:.1}秒",
                                    a.equipment,
                                    a.verb.label(),
                                    a.seconds
                                ),
                                None => {
                                    format!("{} {} {:.1}秒", a.equipment, a.verb.label(), a.seconds)
                                }
                            })
                            .collect();
                        if let Some((sensor, on)) = &step.wait_for {
                            detail.insert(
                                0,
                                format!("条件: {sensor} {}", if *on { "ON" } else { "OFF" }),
                            );
                        }
                        if step.dwell > 0.0 {
                            detail.push(format!("待ち {:.1}秒", step.dwell));
                        }
                        ui.weak(detail.join(" / "));
                        ui.end_row();
                    }
                });
        });
    if !draft.notes.is_empty() {
        egui::CollapsingHeader::new("メモ")
            .default_open(true)
            .show(ui, |ui| {
                for note in &draft.notes {
                    ui.label(format!("・{note}"));
                }
            });
    }
}

/// 上から見た配置図 (X が右、Z が下)
fn plan_view(ui: &mut egui::Ui, line: &ComposedLine) {
    let draft = &line.draft;
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for p in &line.placed {
        for k in 0..2 {
            let half = p.size[k * 2] / 2.0;
            min[k] = min[k].min(p.center[k] - half);
            max[k] = max[k].max(p.center[k] + half);
        }
    }
    if !(min[0].is_finite() && max[0].is_finite()) {
        return;
    }
    let width = ui.available_width().min(480.0);
    let span = [(max[0] - min[0]).max(1.0), (max[1] - min[1]).max(1.0)];
    let margin = 12.0;
    let scale = ((width - margin * 2.0) as f64 / span[0]).min(180.0 / span[1]);
    let height = (span[1] * scale) as f32 + margin * 2.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    let to_screen = |x: f64, z: f64| {
        egui::pos2(
            rect.left() + margin + ((x - min[0]) * scale) as f32,
            rect.top() + margin + ((z - min[1]) * scale) as f32,
        )
    };
    for (e, p) in draft.equipment.iter().zip(&line.placed) {
        let a = to_screen(p.center[0] - p.size[0] / 2.0, p.center[1] - p.size[2] / 2.0);
        let b = to_screen(p.center[0] + p.size[0] / 2.0, p.center[1] + p.size[2] / 2.0);
        let color = match e.kind {
            EquipmentKind::Conveyor => egui::Color32::from_rgb(90, 102, 122),
            EquipmentKind::Robot => egui::Color32::from_rgb(242, 140, 38),
            EquipmentKind::Cylinder => egui::Color32::from_rgb(76, 140, 217),
            EquipmentKind::Sensor => egui::Color32::from_rgb(230, 204, 51),
            EquipmentKind::Table => egui::Color32::from_rgb(158, 133, 102),
        };
        painter.rect_filled(egui::Rect::from_two_pos(a, b), 2.0, color);
        if matches!(
            e.kind,
            EquipmentKind::Conveyor | EquipmentKind::Robot | EquipmentKind::Table
        ) {
            painter.text(
                to_screen(p.center[0], p.center[1]),
                egui::Align2::CENTER_CENTER,
                &e.name,
                egui::FontId::proportional(11.0),
                egui::Color32::WHITE,
            );
        }
    }
}
