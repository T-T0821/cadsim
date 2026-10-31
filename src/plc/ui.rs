//! 「PLC 連携」ウィンドウ: モードの切り替え、模擬 PLC の運転、Modbus の接続、I/O モニタ

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use super::Plc;
use super::adapter::modbus::{ModbusClient, ModbusSettings};
use super::domain::io::{AddressPlan, IoPoint};
use super::usecase::{LinkStatus, PlcMode, PlcSession};
use crate::camera::ViewportRect;
use crate::motion::{Playback, TimingChart};

const OK_COLOR: egui::Color32 = egui::Color32::from_rgb(90, 200, 120);
const WARN_COLOR: egui::Color32 = egui::Color32::from_rgb(240, 170, 60);
const ON_COLOR: egui::Color32 = egui::Color32::from_rgb(60, 200, 90);

pub fn plc_window(
    mut contexts: EguiContexts,
    chart: Res<TimingChart>,
    mut plc: ResMut<Plc>,
    mut playback: ResMut<Playback>,
    viewport: Res<ViewportRect>,
) -> Result {
    // 3D ビューの右上 (ビューキューブの下) に出す。ビューの場所が決まるまでは出さない。
    let Some(view) = viewport.rect else {
        return Ok(());
    };
    let top_right = egui::pos2(view.max.x, view.min.y);
    let ctx = contexts.ctx_mut()?;
    let mut mode = plc.session.mode;

    egui::Window::new("PLC連携")
        .pivot(egui::Align2::RIGHT_TOP)
        .default_pos(top_right + egui::vec2(-12.0, 170.0))
        .default_width(380.0)
        .default_open(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for m in [PlcMode::Off, PlcMode::Simulated, PlcMode::External] {
                    ui.radio_value(&mut mode, m, mode_label(m));
                }
            });
            ui.separator();
            let Plc { session, modbus } = &mut *plc;
            match session.mode {
                PlcMode::Off => {
                    ui.label("タイミングチャートの時刻表どおりに再生します。");
                    ui.label("PLC の出力で動かすときは、上でモードを選んでください。");
                }
                PlcMode::Simulated => simulated_controls(ui, &chart, session),
                PlcMode::External => modbus_controls(ui, session, modbus),
            }
            if session.is_active() {
                ui.separator();
                address_plan_editor(ui, &mut session.plan);
                io_monitor(ui, session);
            }
        });

    if mode != plc.session.mode {
        plc.session.set_mode(mode);
        if mode == PlcMode::Off {
            // チャートの再生位置で部品の位置を計算し直させる
            playback.set_changed();
        } else {
            playback.playing = false;
        }
    }
    Ok(())
}

fn mode_label(mode: PlcMode) -> &'static str {
    match mode {
        PlcMode::Off => "オフ (チャート再生)",
        PlcMode::Simulated => "内蔵の模擬PLC",
        PlcMode::External => "Modbus TCP",
    }
}

fn simulated_controls(ui: &mut egui::Ui, chart: &TimingChart, plc: &mut PlcSession) {
    ui.label("タイミングチャートのステップを、出端・戻端・位置決め完了を見ながら順に実行します。");
    ui.horizontal(|ui| {
        let seq = &mut plc.sequencer;
        let label = if seq.running {
            "■ 停止"
        } else {
            "▶ 運転"
        };
        if ui.button(label).clicked() {
            seq.running = !seq.running;
        }
        ui.checkbox(&mut seq.looping, "繰り返し");
        if ui
            .button("リセット")
            .on_hover_text("ステップを先頭に戻し、出力をすべて OFF にする")
            .clicked()
        {
            plc.reset_sequence();
        }
    });
    let step = plc.sequencer.step;
    match chart.steps.get(step) {
        Some(s) => ui.label(format!(
            "ステップ {}/{}  {}",
            step + 1,
            chart.steps.len(),
            s.comment
        )),
        None => ui.label("ステップがありません"),
    };
    if !plc.sequencer.running {
        ui.weak("停止中は下の I/O モニタで出力を手動で切り替えられます。");
    }
}

fn modbus_controls(ui: &mut egui::Ui, plc: &mut PlcSession, settings: &mut ModbusSettings) {
    let connected = plc.is_connected();
    ui.add_enabled_ui(!connected, |ui| {
        egui::Grid::new("modbus_settings")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("IPアドレス");
                ui.text_edit_singleline(&mut settings.host);
                ui.end_row();
                ui.label("ポート");
                ui.add(egui::DragValue::new(&mut settings.port));
                ui.end_row();
                ui.label("ユニットID");
                ui.add(egui::DragValue::new(&mut settings.unit_id));
                ui.end_row();
                ui.label("通信周期");
                ui.add(
                    egui::DragValue::new(&mut settings.poll_ms)
                        .range(5..=1000)
                        .suffix(" ms"),
                );
                ui.end_row();
            });
    });
    ui.horizontal(|ui| {
        if connected {
            if ui.button("切断").clicked() {
                plc.disconnect();
            }
        } else if ui.button("接続").clicked() {
            plc.connect(Box::new(ModbusClient::start(settings.clone())));
        }
        match plc.link_status() {
            LinkStatus::Disconnected => ui.label("未接続"),
            LinkStatus::Connecting => ui.colored_label(WARN_COLOR, "接続中…"),
            LinkStatus::Connected { cycle_ms } => {
                ui.colored_label(OK_COLOR, format!("通信中 ({cycle_ms:.1} ms)"))
            }
            LinkStatus::Error(e) => ui.colored_label(WARN_COLOR, e),
        };
    });
}

fn address_plan_editor(ui: &mut egui::Ui, plan: &mut AddressPlan) {
    egui::CollapsingHeader::new("番号の割り付け (先頭番号)")
        .id_salt("plc_address_plan")
        .show(ui, |ui| {
            egui::Grid::new("plc_plan").num_columns(2).show(ui, |ui| {
                let mut row = |label: &str, value: &mut u16| {
                    ui.label(label);
                    ui.add(egui::DragValue::new(value));
                    ui.end_row();
                };
                row("出力ビット (コイル)", &mut plan.output_bits);
                row("出力ワード (保持レジスタ)", &mut plan.output_words);
                row("入力ビット (コイル)", &mut plan.input_bits);
                row("入力ワード (保持レジスタ)", &mut plan.input_words);
            });
            ui.weak("出力 = PLC → 設備、入力 = 設備 → PLC。ワードは符号付き16ビットで、直動・回転とも 0.1 単位 (0.1 mm、0.1°)。");
        });
}

fn io_monitor(ui: &mut egui::Ui, plc: &mut PlcSession) {
    ui.horizontal(|ui| {
        ui.strong("I/O モニタ");
        if ui
            .small_button("一覧をコピー")
            .on_hover_text("信号名と番号をタブ区切りでコピーする (Excel や PLC のコメントに貼れる)")
            .clicked()
        {
            ui.ctx().copy_text(io_list_text(&plc.table.points));
        }
    });
    for (a, b) in plc.table.address_conflicts() {
        ui.colored_label(WARN_COLOR, format!("番号が重なっています: {a} と {b}"));
    }
    if plc.table.points.is_empty() {
        ui.label("タイミングチャートにアクチュエータを追加すると、ここに信号が並びます。");
        return;
    }

    // 模擬 PLC が止まっている間は、出力を手で切り替えられる
    let manual = plc.mode == PlcMode::Simulated && !plc.sequencer.running;
    let PlcSession { table, io, .. } = plc;
    egui::ScrollArea::vertical()
        .max_height(180.0)
        .show(ui, |ui| {
            egui::Grid::new("plc_io")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    ui.strong("向き");
                    ui.strong("信号");
                    ui.strong("番号");
                    ui.strong("値");
                    ui.end_row();
                    for p in &table.points {
                        let point = p.point;
                        ui.label(if point.is_output() {
                            "出力"
                        } else {
                            "入力"
                        });
                        ui.label(&p.name);
                        ui.label(p.address_label());
                        let editable = manual && point.is_output();
                        if point.is_word() {
                            let mut value = io.word(point);
                            let widget = egui::DragValue::new(&mut value).speed(p.scale * 10.0);
                            if ui.add_enabled(editable, widget).changed() {
                                io.set_word(point, value);
                            }
                        } else {
                            let on = io.bit(point);
                            let text = egui::RichText::new(if on { "● ON" } else { "○ OFF" })
                                .color(if on {
                                    ON_COLOR
                                } else {
                                    ui.visuals().weak_text_color()
                                });
                            if ui.add_enabled(editable, egui::Button::new(text)).clicked() {
                                io.set_bit(point, !on);
                            }
                        }
                        ui.end_row();
                    }
                });
        });
}

/// I/O 一覧をタブ区切りの文字列にする
fn io_list_text(points: &[IoPoint]) -> String {
    let mut text = String::from("向き\t信号\t種類\t番号\tModbus番地\n");
    for p in points {
        let direction = if p.point.is_output() {
            "出力"
        } else {
            "入力"
        };
        let (kind, modbus) = if p.point.is_word() {
            ("保持レジスタ", 40001 + p.address as u32)
        } else {
            ("コイル", 1 + p.address as u32)
        };
        text += &format!(
            "{direction}\t{}\t{kind}\t{}\t{modbus:05}\n",
            p.name, p.address
        );
    }
    text
}
