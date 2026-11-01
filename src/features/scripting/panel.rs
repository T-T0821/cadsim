//! 部品スクリプトの画面: プロパティ欄のエディタと、PLC 連携ウィンドウの実行操作

use bevy_egui::egui;

use super::{ScriptRun, check, scripts_of};
use crate::document::history::History;
use crate::document::selection::Selection;
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::scripting::rhai_engine::API_HELP;
use plantwright_core::scripting::runner::ScriptState;

const OK_COLOR: egui::Color32 = egui::Color32::from_rgb(90, 200, 120);
const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(235, 100, 90);

/// 新しくスクリプトを付けたときのひな形
const TEMPLATE: &str = "\
// 例: ワークが来たらシリンダを出して戻す
loop {
    wait_signal(\"ワーク有\", true);
    move_joint(\"シリンダ1\", 1);
    wait(0.5);
    move_joint(\"シリンダ1\", 0);
}
";

/// プロパティ欄の「スクリプト」。付ける・外す・編集する。
pub fn script_section(ui: &mut egui::Ui, model: &mut Model, history: &mut History, id: NodeId) {
    let Some(node) = model.get(id) else {
        return;
    };
    let before = node.script.clone();
    let mut script = before.clone();
    match &mut script {
        None => {
            ui.weak("PLC 連携の「部品スクリプト」モードで、この部品の動きを Rhai で書けます。");
            if ui.button("スクリプトを付ける").clicked() {
                script = Some(TEMPLATE.to_owned());
            }
        }
        Some(source) => {
            ui.add(
                egui::TextEdit::multiline(source)
                    .code_editor()
                    .desired_rows(10)
                    .desired_width(f32::INFINITY),
            );
            match check(source) {
                Ok(()) => ui.colored_label(OK_COLOR, "書き方の誤りはありません"),
                Err(e) => ui.colored_label(ERROR_COLOR, e),
            };
            egui::CollapsingHeader::new("使える関数")
                .id_salt("script_help")
                .show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(API_HELP).monospace()).wrap());
                    ui.weak(
                        "信号名は PLC 連携の I/O 一覧の名前 (例: シリンダ1.出端) かセンサー名。\
                         それ以外の名前を set すると、スクリプトどうしで共有する内部信号になります。",
                    );
                });
            if ui.button("スクリプトを外す").clicked() {
                script = None;
            }
        }
    }
    if script != before {
        history.begin_edit(model, id);
        if let Some(node) = model.get_mut(id) {
            node.script = script;
        }
    }
}

/// PLC 連携ウィンドウの「部品スクリプト」モードの操作。
/// 実行を始めたら true を返す (呼び出し側で設備を原点に戻す)。
pub fn script_controls(
    ui: &mut egui::Ui,
    model: &Model,
    selection: &mut Selection,
    run: &mut ScriptRun,
) -> bool {
    ui.label("部品に付けたスクリプトが、PLC の代わりに I/O を読み書きして設備を動かします。");
    let count = scripts_of(model).len();
    if count == 0 {
        ui.weak("スクリプトの付いた部品がありません。部品を選び、プロパティの「スクリプト」で付けてください。");
    }
    let mut started = false;
    ui.horizontal(|ui| {
        let running = run.running().is_some_and(|c| c.is_running());
        if running {
            if ui.button("■ 停止").clicked() {
                run.stop();
            }
        } else if ui
            .add_enabled(count > 0, egui::Button::new("▶ 実行"))
            .on_hover_text("設備を原点に戻し、すべてのスクリプトを最初から動かす")
            .clicked()
        {
            run.start(model);
            started = true;
        }
        if let Some(c) = run.running() {
            ui.label(format!("{:.1} 秒", c.time));
        }
    });
    let Some(controller) = run.running() else {
        return started;
    };
    egui::Grid::new("script_states")
        .num_columns(2)
        .spacing([12.0, 2.0])
        .show(ui, |ui| {
            for script in &controller.scripts {
                if ui
                    .link(&script.name)
                    .on_hover_text("クリックで部品を選ぶ")
                    .clicked()
                    && model.get(script.node).is_some()
                {
                    selection.select(script.node);
                    selection.reveal_in_tree = true;
                }
                match script.state() {
                    ScriptState::Running => ui.colored_label(OK_COLOR, "実行中"),
                    ScriptState::Finished => ui.label("終了"),
                    ScriptState::Failed(_) => ui.colored_label(ERROR_COLOR, "エラー"),
                };
                ui.end_row();
            }
        });
    if !controller.internal.is_empty() {
        ui.collapsing("内部信号", |ui| {
            for (name, on) in &controller.internal {
                ui.label(format!("{name}: {}", if *on { "ON" } else { "OFF" }));
            }
        });
    }
    ui.label("ログ");
    egui::ScrollArea::vertical()
        .id_salt("script_log")
        .max_height(120.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if controller.log.is_empty() {
                ui.weak("(log や print の出力とエラーがここに出ます)");
            }
            for line in &controller.log {
                ui.monospace(line);
            }
        });
    started
}
