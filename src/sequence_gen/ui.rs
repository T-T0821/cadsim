//! 「チャート・ラダーの自動生成」ウィンドウ

use crate::ui::dock::{self, Dock};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use cadsim_core::workspace::Tool;

use super::ladder::{self, Direction};
use super::log_events;
use super::spec::{self, ApplyMode};
use super::usecase::{self, ExplainSource};
use super::{SequenceGen, SourceKind, Tab};
use crate::assistant::AiAssistant;
use crate::assistant::job::LlmJob;
use crate::motion::TimingChart;

/// ログから取り出す変化の上限
const MAX_LOG_EVENTS: usize = 3000;

const DESCRIPTION_HINT: &str = "例: ワークが光電センサーで検知されたら、クランプで固定する。\
リフタを上げて 0.5 秒待ち、X軸で 300 mm 先の検査位置へ運ぶ。\
安全扉が開いているときは動かさない。最後は全部元の位置に戻す。";

pub fn sequence_gen_window(
    mut contexts: EguiContexts,
    mut state: ResMut<SequenceGen>,
    mut chart: ResMut<TimingChart>,
    mut ai: ResMut<AiAssistant>,
    mut dock: ResMut<Dock>,
) -> Result {
    state.poll_jobs();
    let Some(window) = dock::workspace_window(Tool::SequenceGen, &dock, [560.0, 520.0]) else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    if state.generating.is_some() || state.explaining.is_some() {
        // 経過秒数の表示を進め、終わったらすぐ受け取る
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    let mut open = true;
    window.open(&mut open).show(ctx, |ui| {
        let state = &mut *state;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut state.tab, Tab::Generate, "説明文から作る");
            ui.selectable_value(&mut state.tab, Tab::Ladder, "ラダーを書き出す");
            ui.selectable_value(&mut state.tab, Tab::Explain, "動作を説明させる");
        });
        ui.separator();
        match state.tab {
            Tab::Generate => generate_tab(ui, state, &mut chart, &mut ai),
            Tab::Ladder => ladder_tab(ui, state, &chart),
            Tab::Explain => explain_tab(ui, state, &chart, &mut ai),
        }
        if let Some(text) = state.notice.clone() {
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label(text);
                if ui.small_button("×").clicked() {
                    state.notice = None;
                }
            });
        }
    });
    if !open {
        dock.set_open(Tool::SequenceGen, false);
    }
    Ok(())
}

/// API キーがなければ設定を促す。キーがあれば true。
fn key_ready(ui: &mut egui::Ui, ai: &mut AiAssistant) -> bool {
    if ai.has_key() {
        return true;
    }
    ui.horizontal(|ui| {
        ui.label("生成 AI を使うには API キーが要ります。");
        if ui.button("AI の設定…").clicked() {
            ai.settings_open = true;
        }
    });
    false
}

fn busy(ui: &mut egui::Ui, label: &str, seconds: u64) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(format!("{label}… {seconds} 秒"));
    });
}

fn generate_tab(
    ui: &mut egui::Ui,
    state: &mut SequenceGen,
    // 反映するときだけ書き換える (毎フレーム「変わった」にしない)
    chart: &mut ResMut<TimingChart>,
    ai: &mut AiAssistant,
) {
    ui.label("設備の動きを文章で書くと、生成 AI がステップ・開始条件 (インターロック)・装置とセンサーを下書きします。");
    ui.small("今のチャートにある装置は同じ名前で使います。下書きは確かめてから反映できます (Ctrl+Z で戻せます)。");
    ui.add(
        egui::TextEdit::multiline(&mut state.description)
            .hint_text(DESCRIPTION_HINT)
            .desired_rows(6)
            .desired_width(f32::INFINITY),
    );
    ui.horizontal(|ui| {
        ui.radio_value(&mut state.mode, ApplyMode::Replace, "ステップを置き換える");
        ui.radio_value(&mut state.mode, ApplyMode::Append, "後ろに足す");
    });
    let ready = key_ready(ui, ai);
    if let Some(job) = &state.generating {
        busy(ui, "下書きを作っています", job.elapsed_secs());
    } else if ui
        .add_enabled(
            ready && !state.description.trim().is_empty(),
            egui::Button::new("下書きを作る"),
        )
        .clicked()
    {
        let description = state.description.clone();
        let current = spec::from_chart(chart.as_ref());
        state.notice = None;
        state.generating = Some(LlmJob::start(ai.client(), move |llm| {
            usecase::generate(llm, &description, &current)
        }));
    }

    let Some(draft) = state.draft.clone() else {
        return;
    };
    ui.separator();
    // 反映したらどうなるかを、チャートの写しで先に見せる
    let mut preview = chart.as_ref().clone();
    let report = spec::apply(&draft, &mut preview, state.mode);
    ui.label(egui::RichText::new(format!("下書き ({} ステップ)", draft.steps.len())).strong());
    egui::ScrollArea::vertical()
        .id_salt("draft")
        .max_height(220.0)
        .show(ui, |ui| {
            ui.monospace(spec::describe(&draft));
        });
    if !report.added_actuators.is_empty() || !report.added_sensors.is_empty() {
        let mut added: Vec<String> = report.added_actuators.clone();
        added.extend(report.added_sensors.clone());
        ui.label(format!("新しく足す装置・センサー: {}", added.join("、")));
    }
    for warning in &report.warnings {
        ui.colored_label(egui::Color32::from_rgb(240, 170, 60), warning);
    }
    egui::CollapsingHeader::new("反映後の I/O 表")
        .id_salt("draft_io")
        .show(ui, |ui| {
            io_grid(ui, &ladder::generate(&preview), "draft_io_grid")
        });
    ui.horizontal(|ui| {
        if ui.button("チャートに反映").clicked() {
            let report = spec::apply(&draft, chart.as_mut(), state.mode);
            state.notice = Some(report.summary());
            state.draft = None;
        }
        if ui.button("捨てる").clicked() {
            state.draft = None;
        }
    });
}

fn io_grid(ui: &mut egui::Ui, ladder: &ladder::Ladder, id: &str) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(200.0)
        .show(ui, |ui| {
            egui::Grid::new(id).striped(true).show(ui, |ui| {
                for d in ladder
                    .devices
                    .iter()
                    .filter(|d| d.direction != Direction::Internal)
                {
                    ui.monospace(&d.device);
                    ui.label(&d.name);
                    ui.label(d.direction.label());
                    ui.end_row();
                }
            });
        });
}

fn save_dialog(name: &str, ext: &str, label: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new()
        .add_filter(label, &[ext])
        .set_file_name(name)
        .save_file()
}

fn write_result(path: &std::path::Path, bytes: &[u8]) -> String {
    match std::fs::write(path, bytes) {
        Ok(()) => format!("{} に書き出しました", path.display()),
        Err(e) => format!("書き出せませんでした: {e}"),
    }
}

fn ladder_tab(ui: &mut egui::Ui, state: &mut SequenceGen, chart: &TimingChart) {
    ui.label("今のタイミングチャートから、三菱 PLC (MELSEC Q / iQ-R) 向けのステップラダーと I/O 表を作ります。生成 AI は使いません。");
    let program = ladder::generate(chart);
    let count = |dir| {
        program
            .devices
            .iter()
            .filter(|d| d.direction == dir)
            .count()
    };
    ui.label(format!(
        "{} ステップ / 命令 {} 個 / 入力 {} 点 / 出力 {} 点",
        chart.steps.len(),
        program.instruction_count(),
        count(Direction::Input),
        count(Direction::Output),
    ));
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("命令リストを保存…")
            .on_hover_text("デバイスのコメント付きのテキスト。内容の確認や説明に使います")
            .clicked()
            && let Some(path) = save_dialog("ladder.txt", "txt", "テキスト")
        {
            state.notice = Some(write_result(&path, ladder::to_text(&program).as_bytes()));
        }
        if ui
            .button("GX Works 用 CSV を保存…")
            .on_hover_text(
                "命令リストの CSV (Shift_JIS)。GX Works の「CSV ファイルを読み出し」で取り込みます",
            )
            .clicked()
            && let Some(path) = save_dialog("ladder_gxworks.csv", "csv", "CSV")
        {
            let csv = ladder::to_gx_csv(&program);
            let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(&csv);
            state.notice = Some(write_result(&path, &bytes));
        }
        if ui
            .button("I/O 表を保存…")
            .on_hover_text(
                "デバイス・信号名・向き・備考の CSV。先頭 2 列はデバイスコメントに使えます",
            )
            .clicked()
            && let Some(path) = save_dialog("io_list.csv", "csv", "CSV")
        {
            state.notice = Some(write_result(&path, ladder::io_csv(&program).as_bytes()));
        }
        if ui.button("I/O 表をコピー").clicked() {
            ui.ctx().copy_text(ladder::io_tsv(&program));
            state.notice = Some("I/O 表をコピーしました (タブ区切り)".into());
        }
    });
    egui::CollapsingHeader::new("I/O 表")
        .id_salt("ladder_io")
        .default_open(true)
        .show(ui, |ui| io_grid(ui, &program, "ladder_io_grid"));
    egui::CollapsingHeader::new("命令リスト")
        .id_salt("ladder_text")
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("ladder_text_scroll")
                .max_height(300.0)
                .show(ui, |ui| ui.monospace(ladder::to_text(&program)));
        });
    ui.small("ラダーは決まった型で作った下書きです。実機に入れる前に、デバイスの割り付けと安全回路を必ず確認してください。");
}

/// ファイルの中身を文字列にする。UTF-8 でなければ Shift_JIS とみなす。
fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => encoding_rs::SHIFT_JIS.decode(bytes).0.into_owned(),
    }
}

fn explain_tab(
    ui: &mut egui::Ui,
    state: &mut SequenceGen,
    chart: &TimingChart,
    ai: &mut AiAssistant,
) {
    ui.label("タイミングチャート・ラダー・PLC のログを生成 AI に読ませて、設備の動きを文章で説明させます。");
    ui.horizontal(|ui| {
        ui.radio_value(&mut state.source, SourceKind::Chart, "今のチャート");
        ui.radio_value(&mut state.source, SourceKind::Ladder, "ラダー");
        ui.radio_value(&mut state.source, SourceKind::Log, "PLC のログ");
    });
    let source = match state.source {
        SourceKind::Chart => {
            ui.label(format!(
                "{} ステップのチャートを説明させます。",
                chart.steps.len()
            ));
            Some(ExplainSource::Chart(spec::from_chart(chart)))
        }
        SourceKind::Ladder => {
            ui.horizontal(|ui| {
                if ui.button("ファイルを開く…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("ラダー", &["txt", "csv", "il"])
                        .pick_file()
                {
                    match std::fs::read(&path) {
                        Ok(bytes) => state.ladder_text = decode_text(&bytes),
                        Err(e) => state.notice = Some(format!("読み込めませんでした: {e}")),
                    }
                }
                if ui
                    .button("今のチャートのラダーを入れる")
                    .on_hover_text("「ラダーを書き出す」と同じ命令リストを入れる")
                    .clicked()
                {
                    state.ladder_text = ladder::to_text(&ladder::generate(chart));
                }
            });
            egui::ScrollArea::vertical()
                .id_salt("ladder_input")
                .max_height(200.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut state.ladder_text)
                            .code_editor()
                            .hint_text("命令リスト (LD X0 / OUT Y20 …) やラダーをテキストにしたものを貼り付け")
                            .desired_rows(8)
                            .desired_width(f32::INFINITY),
                    );
                });
            Some(ExplainSource::Ladder(state.ladder_text.clone()))
        }
        SourceKind::Log => {
            if ui.button("ログ CSV を開く…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("CSV", &["csv", "txt"])
                    .pick_file()
            {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        match log_events::summarize(&decode_text(&bytes), MAX_LOG_EVENTS) {
                            Ok(summary) => state.log = Some((name, summary)),
                            Err(e) => state.notice = Some(format!("ログを読めませんでした: {e}")),
                        }
                    }
                    Err(e) => state.notice = Some(format!("読み込めませんでした: {e}")),
                }
            }
            match &state.log {
                Some((name, log)) => {
                    let mut text = format!(
                        "{name}: {} 行、{} 信号、変化 {} 件",
                        log.rows, log.signals, log.events
                    );
                    if log.omitted > 0 {
                        text += &format!(" (長いため後半の {} 件は送りません)", log.omitted);
                    }
                    ui.label(text);
                    Some(ExplainSource::Log(log.clone()))
                }
                None => {
                    ui.small("GX LogViewer などで書き出した CSV。信号が変わった所だけを取り出して送ります。");
                    None
                }
            }
        }
    };

    let ready = key_ready(ui, ai);
    if let Some(job) = &state.explaining {
        busy(ui, "読んでいます", job.elapsed_secs());
    } else if ui
        .add_enabled(ready && source.is_some(), egui::Button::new("説明させる"))
        .clicked()
        && let Some(source) = source
    {
        state.notice = None;
        state.explaining = Some(LlmJob::start(ai.client(), move |llm| {
            usecase::explain(llm, &source)
        }));
    }

    if !state.explanation.is_empty() {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("説明").strong());
            if ui.small_button("コピー").clicked() {
                ui.ctx().copy_text(state.explanation.clone());
            }
            if ui.small_button("消す").clicked() {
                state.explanation.clear();
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("explanation")
            .max_height(260.0)
            .show(ui, |ui| {
                ui.add(egui::Label::new(&state.explanation).selectable(true));
            });
    }
}
