//! 「PLC 連携」ウィンドウ: モードの切り替え、模擬 PLC の運転、外部 PLC (Modbus TCP / OPC UA / 三菱 SLMP) の接続、I/O モニタ

use crate::ui::dock::{self, Dock};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use plantwright_core::plc::melsec::Device;
use plantwright_core::plc::slmp::{Encoding, FrameKind, Transport};
use plantwright_core::workspace::Tool;

use super::{Plc, Protocol};
use crate::document::selection::Selection;
use crate::features::motion::Playback;
use crate::features::scripting::{ScriptRun, script_controls};
use plantwright_core::model::Model;
use plantwright_core::motion::chart::TimingChart;
use plantwright_core::plc::device_map::{DeviceArea, DeviceMap};
use plantwright_core::plc::io::{AddressPlan, IoPoint};
use plantwright_core::plc::io_csv::CsvReport;
use plantwright_core::plc::modbus::{ModbusClient, ModbusSettings};
use plantwright_core::plc::opcua::{OpcUaClient, OpcUaSettings};
use plantwright_core::plc::session::{LinkStatus, PlcLink, PlcMode, PlcSession};
use plantwright_core::plc::slmp_link::{SlmpLink, SlmpSettings};

const OK_COLOR: egui::Color32 = egui::Color32::from_rgb(90, 200, 120);
const WARN_COLOR: egui::Color32 = egui::Color32::from_rgb(240, 170, 60);
const ON_COLOR: egui::Color32 = egui::Color32::from_rgb(60, 200, 90);

#[allow(clippy::too_many_arguments)]
pub fn plc_window(
    mut contexts: EguiContexts,
    chart: Res<TimingChart>,
    mut plc: ResMut<Plc>,
    mut playback: ResMut<Playback>,
    model: Res<Model>,
    mut selection: ResMut<Selection>,
    mut scripts: ResMut<ScriptRun>,
    mut dock: ResMut<Dock>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let Some(window) = dock::panel(ctx, Tool::Plc, &dock) else {
        return Ok(());
    };
    let mut mode = plc.session.mode;

    let mut open = true;
    window.open(&mut open).show(ctx, |ui| {
        ui.horizontal_wrapped(|ui| {
            for m in [
                PlcMode::Off,
                PlcMode::Simulated,
                PlcMode::External,
                PlcMode::LogReplay,
                PlcMode::Script,
            ] {
                ui.radio_value(&mut mode, m, mode_label(m));
            }
        });
        ui.separator();
        let Plc {
            session,
            protocol,
            modbus,
            opcua,
            slmp,
            notice,
        } = &mut *plc;
        match session.mode {
            PlcMode::Off => {
                ui.label("タイミングチャートの時刻表どおりに再生します。");
                ui.label("PLC の出力で動かすときは、上でモードを選んでください。");
            }
            PlcMode::Simulated => simulated_controls(ui, &chart, session),
            PlcMode::LogReplay => log_controls(ui, &chart, session, notice),
            PlcMode::Script => {
                if script_controls(ui, &model, &mut selection, &mut scripts) {
                    session.reset_plant();
                }
            }
            PlcMode::External => {
                ui.horizontal(|ui| {
                    ui.label("通信方式");
                    // つないでいる間は切り替えない
                    ui.add_enabled_ui(!session.is_connected(), |ui| {
                        ui.radio_value(protocol, Protocol::ModbusTcp, "Modbus TCP");
                        ui.radio_value(protocol, Protocol::OpcUa, "OPC UA");
                        ui.radio_value(protocol, Protocol::Slmp, "三菱 (SLMP)");
                    });
                });
                match protocol {
                    Protocol::ModbusTcp => modbus_controls(ui, session, modbus),
                    Protocol::OpcUa => opcua_controls(ui, session, opcua),
                    Protocol::Slmp => slmp_controls(ui, session, slmp),
                }
            }
        }
        if session.is_active() {
            ui.separator();
            address_plan_editor(ui, &mut session.plan);
            io_csv_buttons(ui, &chart, session, notice);
            // 三菱 PLC とつなぐときは、I/O モニタと一覧に信号を置いたデバイスも出す
            let devices = (session.mode == PlcMode::External && *protocol == Protocol::Slmp)
                .then_some(&slmp.devices);
            io_monitor(ui, session, devices);
        }
    });
    if !open {
        dock.set_open(Tool::Plc, false);
    }

    if mode != plc.session.mode {
        plc.session.set_mode(mode);
        scripts.stop();
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
        PlcMode::External => "外部PLC",
        PlcMode::LogReplay => "ログ再生",
        PlcMode::Script => "部品スクリプト",
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

fn log_controls(
    ui: &mut egui::Ui,
    chart: &TimingChart,
    plc: &mut PlcSession,
    notice: &mut Option<String>,
) {
    ui.label("実機で記録した PLC の出力 (GX LogViewer などの CSV) を時刻どおりに流し、設備を動かします。");
    if ui
        .button("ログ CSV を読み込み…")
        .on_hover_text("列見出しは I/O 表の「ログ列名」(空なら信号名) と突き合わせます")
        .clicked()
        && let Some(path) = rfd::FileDialog::new()
            .add_filter("CSV", &["csv", "txt"])
            .pick_file()
    {
        *notice = Some(match std::fs::read(&path) {
            Ok(bytes) => match plc.load_log(&decode_text(&bytes)) {
                Ok(log) => {
                    let mut text = format!(
                        "{} 行・{:.1} 秒・{} 信号を読み込みました。",
                        log.rows(),
                        log.duration(),
                        log.signals()
                    );
                    if !log.unused_columns.is_empty() {
                        text += &format!(" 使わない列: {}。", log.unused_columns.join("、"));
                    }
                    text
                }
                Err(e) => e,
            },
            Err(e) => format!("読み込めませんでした: {e}"),
        });
    }

    let mut seek = None;
    let Some(player) = &mut plc.log else {
        ui.weak("まだログを読み込んでいません。");
        return;
    };
    ui.horizontal(|ui| {
        if ui.button("⏮").on_hover_text("先頭へ").clicked() {
            seek = Some(0.0);
        }
        let label = if player.playing {
            "⏸ 一時停止"
        } else {
            "▶ 再生"
        };
        if ui.button(label).clicked() {
            if !player.playing && player.time >= player.log.duration() {
                seek = Some(0.0);
            }
            player.playing = !player.playing;
        }
        ui.checkbox(&mut player.looping, "繰り返し");
        egui::ComboBox::from_id_salt("plc_log_speed")
            .width(60.0)
            .selected_text(format!("x{}", player.speed))
            .show_ui(ui, |ui| {
                for speed in [0.1, 0.25, 0.5, 1.0, 2.0, 5.0] {
                    ui.selectable_value(&mut player.speed, speed, format!("x{speed}"));
                }
            });
    });
    let mut time = player.time;
    let slider = egui::Slider::new(&mut time, 0.0..=player.log.duration())
        .suffix(" s")
        .fixed_decimals(2);
    if ui.add(slider).changed() {
        seek = Some(time);
    }
    if let Some(t) = seek {
        plc.seek_log(chart, t);
    }
}

/// ファイルの中身を文字列にする。UTF-8 でなければ Shift_JIS (日本語版 Windows のツールが書き出す CSV) とみなす。
fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => encoding_rs::SHIFT_JIS.decode(bytes).0.into_owned(),
    }
}

fn opcua_controls(ui: &mut egui::Ui, plc: &mut PlcSession, settings: &mut OpcUaSettings) {
    let connected = plc.is_connected();
    ui.add_enabled_ui(!connected, |ui| {
        egui::Grid::new("opcua_settings")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("エンドポイント");
                ui.text_edit_singleline(&mut settings.endpoint);
                ui.end_row();
                ui.label("通信周期");
                ui.add(
                    egui::DragValue::new(&mut settings.interval_ms)
                        .range(10..=1000)
                        .suffix(" ms"),
                );
                ui.end_row();
            });
    });
    let with_node = plc
        .table
        .points
        .iter()
        .filter(|p| !p.node.is_empty())
        .count();
    let text = format!(
        "OPC UA ノードを決めた信号: {with_node} / {}",
        plc.table.points.len()
    );
    if with_node == 0 {
        ui.colored_label(
            WARN_COLOR,
            text + "。I/O 表を CSV に書き出し、「OPC UAノード」列を埋めて読み込んでください。",
        );
    } else {
        ui.label(text);
    }
    connect_row(ui, plc, || Box::new(OpcUaClient::start(settings.clone())));
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
    connect_row(ui, plc, || Box::new(ModbusClient::start(settings.clone())));
}

fn slmp_controls(ui: &mut egui::Ui, plc: &mut PlcSession, settings: &mut SlmpSettings) {
    let connected = plc.is_connected();
    ui.add_enabled_ui(!connected, |ui| {
        egui::Grid::new("slmp_settings")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("IPアドレス");
                ui.text_edit_singleline(&mut settings.host);
                ui.end_row();
                ui.label("ポート");
                ui.add(egui::DragValue::new(&mut settings.port));
                ui.end_row();
                ui.label("通信");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut settings.transport, Transport::Tcp, "TCP");
                    ui.radio_value(&mut settings.transport, Transport::Udp, "UDP");
                });
                ui.end_row();
                ui.label("フレーム");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut settings.frame, FrameKind::E3, "3E");
                    ui.radio_value(&mut settings.frame, FrameKind::E4, "4E");
                });
                ui.end_row();
                ui.label("コード");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut settings.encoding, Encoding::Binary, "バイナリ");
                    ui.radio_value(&mut settings.encoding, Encoding::Ascii, "ASCII");
                });
                ui.end_row();
                ui.label("通信周期");
                ui.add(
                    egui::DragValue::new(&mut settings.poll_ms)
                        .range(5..=1000)
                        .suffix(" ms"),
                );
                ui.end_row();
            });
        device_map_editor(ui, &mut settings.devices);
    });
    for problem in settings.devices.problems(&plc.table) {
        ui.colored_label(WARN_COLOR, problem);
    }
    ui.weak("PLC 側は GX Works の「Ethernet 設定」で SLMP の交信用ポートを開き、通信方式・コードをここと合わせてください。");
    connect_row(ui, plc, || Box::new(SlmpLink::start(settings.clone())));
}

/// 4つの領域を三菱 PLC のどのデバイスの何番から置くか
fn device_map_editor(ui: &mut egui::Ui, devices: &mut DeviceMap) {
    egui::CollapsingHeader::new("デバイスの割り当て")
        .id_salt("slmp_devices")
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new("slmp_device_map")
                .num_columns(2)
                .show(ui, |ui| {
                    for (label, output, word) in [
                        ("出力ビット (PLC → 設備)", true, false),
                        ("出力ワード (PLC → 設備)", true, true),
                        ("入力ビット (設備 → PLC)", false, false),
                        ("入力ワード (設備 → PLC)", false, true),
                    ] {
                        ui.label(label);
                        let area = devices.area_mut(output, word);
                        ui.horizontal(|ui| device_area_editor(ui, label, word, area));
                        ui.end_row();
                    }
                });
            ui.weak("各領域の先頭の信号をこのデバイスに置き、続く信号を順に並べます。X・Y・B・W の番号は 16 進数です。X・Y は入出力ユニットのない番号を使ってください (ふつうは M・D か B・W)。");
        });
}

fn device_area_editor(ui: &mut egui::Ui, id: &str, word: bool, area: &mut DeviceArea) {
    let choices: &[Device] = if word { &Device::WORDS } else { &Device::BITS };
    egui::ComboBox::from_id_salt(("slmp_device", id))
        .width(50.0)
        .selected_text(area.device.name())
        .show_ui(ui, |ui| {
            for &d in choices {
                ui.selectable_value(
                    &mut area.device,
                    d,
                    format!("{}  {}", d.name(), d.description()),
                );
            }
        });
    let number = egui::DragValue::new(&mut area.head).range(0..=0xFF_FFFF);
    if area.device.is_hex() {
        ui.add(number.hexadecimal(1, false, true));
    } else {
        ui.add(number);
    }
}

/// 接続 / 切断のボタンと通信の状態
fn connect_row(ui: &mut egui::Ui, plc: &mut PlcSession, start: impl FnOnce() -> Box<dyn PlcLink>) {
    ui.horizontal(|ui| {
        if plc.is_connected() {
            if ui.button("切断").clicked() {
                plc.disconnect();
            }
        } else if ui.button("接続").clicked() {
            plc.connect(start());
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
            if !plan.custom.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(format!("CSV で個別に決めた信号: {} 件", plan.custom.len()));
                    if ui.small_button("個別設定を消す").clicked() {
                        plan.custom.clear();
                    }
                });
            }
        });
}

/// I/O 対応表の CSV 書き出し・読み込みのボタンと、読み込み結果のお知らせ
fn io_csv_buttons(
    ui: &mut egui::Ui,
    chart: &TimingChart,
    plc: &mut PlcSession,
    notice: &mut Option<String>,
) {
    ui.horizontal(|ui| {
        if ui
            .button("I/O 表を CSV に書き出し…")
            .on_hover_text("Excel で番号・スケール・OPC UA ノードを編集して読み戻せます")
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("CSV", &["csv"])
                .set_file_name("io_table.csv")
                .save_file()
        {
            *notice = Some(
                match std::fs::write(
                    &path,
                    plantwright_core::formats::csv_text::write(plc.export_io_rows()),
                ) {
                    Ok(()) => format!("{} に書き出しました", path.display()),
                    Err(e) => format!("書き出せませんでした: {e}"),
                },
            );
        }
        if ui.button("CSV を読み込み…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("CSV", &["csv"])
                .pick_file()
        {
            *notice = Some(match std::fs::read(&path) {
                Ok(bytes) => {
                    let rows = plantwright_core::formats::csv_text::parse(
                        &String::from_utf8_lossy(&bytes),
                    );
                    match plc.import_io_rows(&rows, chart) {
                        Ok(report) => import_summary(&report),
                        Err(e) => e,
                    }
                }
                Err(e) => format!("読み込めませんでした: {e}"),
            });
        }
    });
    if let Some(text) = notice.clone() {
        ui.horizontal(|ui| {
            ui.label(text);
            if ui.small_button("×").clicked() {
                *notice = None;
            }
        });
    }
}

fn import_summary(report: &CsvReport) -> String {
    let mut text = format!("{} 件の信号に反映しました。", report.applied);
    if !report.unknown.is_empty() {
        text += &format!(" 一覧にない信号: {}。", report.unknown.join("、"));
    }
    for e in &report.errors {
        text += &format!(" {e}。");
    }
    text
}

fn io_monitor(ui: &mut egui::Ui, plc: &mut PlcSession, devices: Option<&DeviceMap>) {
    let located = devices.map(|d| d.locate_all(&plc.table));
    ui.horizontal(|ui| {
        ui.strong("I/O モニタ");
        if ui
            .small_button("一覧をコピー")
            .on_hover_text("信号名と番号をタブ区切りでコピーする (Excel や PLC のコメントに貼れる)")
            .clicked()
        {
            ui.ctx()
                .copy_text(io_list_text(&plc.table.points, located.as_deref()));
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
                    for (i, p) in table.points.iter().enumerate() {
                        let point = p.point;
                        ui.label(if point.is_output() {
                            "出力"
                        } else {
                            "入力"
                        });
                        ui.label(&p.name);
                        match &located {
                            Some(located) => ui.label(device_label(located[i])),
                            None => ui.label(p.address_label()),
                        };
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

fn device_label(device: Option<plantwright_core::plc::melsec::DeviceRef>) -> String {
    device.map_or_else(|| "(置けません)".to_string(), |d| d.to_string())
}

/// I/O 一覧をタブ区切りの文字列にする。`devices` があれば三菱 PLC のデバイスの列も付ける。
fn io_list_text(
    points: &[IoPoint],
    devices: Option<&[Option<plantwright_core::plc::melsec::DeviceRef>]>,
) -> String {
    let mut text = String::from("向き\t信号\t種類\t番号\tModbus番地");
    if devices.is_some() {
        text += "\tデバイス";
    }
    text += "\n";
    for (i, p) in points.iter().enumerate() {
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
            "{direction}\t{}\t{kind}\t{}\t{modbus:05}",
            p.name, p.address
        );
        if let Some(devices) = devices {
            text += &format!("\t{}", device_label(devices[i]));
        }
        text += "\n";
    }
    text
}
