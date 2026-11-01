//! 「工程シミュレーション」ウィンドウの「工場」タブ
//!
//! - ラインの一覧 (足す・名前を変える・消す)、ラインに入れていない要素をまとめる
//! - 別の .plantwright の工程を1本のラインとして取り込む (名簿も取り込むか選べる)
//! - サンプル工場
//! - 回した結果のラインごとの表
//!
//! 計算は `plantwright_core::flow::factory` にあり、ここは入力を集めて結果を表に写すだけ。

use std::path::Path;

use bevy_egui::egui::{self, Color32};

use super::FlowRun;
use plantwright_core::flow::factory::{line_summaries, sample_factory};
use plantwright_core::flow::model::FlowModel;
use plantwright_core::flow::record::from_record;

/// 工場タブだけが使う状態
pub struct FactoryUiState {
    /// 取り込むときに名簿も取り込む
    with_staff: bool,
    /// 取り込みの結果・失敗の知らせ
    message: Option<(bool, String)>,
}

impl Default for FactoryUiState {
    fn default() -> Self {
        Self {
            with_staff: true,
            message: None,
        }
    }
}

const GREEN: Color32 = Color32::from_rgb(90, 190, 110);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const ERROR: Color32 = Color32::from_rgb(230, 90, 90);

/// タブを描く。工程をまるごと置き換えたら true。
pub fn factory_tab(
    ui: &mut egui::Ui,
    flow: &mut FlowModel,
    run: &FlowRun,
    st: &mut FactoryUiState,
) -> bool {
    let mut replaced = false;
    egui::ScrollArea::vertical()
        .id_salt("flow_factory")
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("サンプル工場")
                    .on_hover_text(
                        "加工ライン → フォークリフト → 組立ライン (組立 2 台) と、日勤 4 人の工場に置き換える",
                    )
                    .clicked()
                {
                    let scenarios = std::mem::take(&mut flow.scenarios);
                    *flow = sample_factory();
                    flow.scenarios = scenarios;
                    st.message = None;
                    replaced = true;
                }
                ui.separator();
                if ui
                    .button("別のファイルのラインを取り込む…")
                    .on_hover_text("ほかの .plantwright の工程を、1 本のラインとしてこの工場に足す")
                    .clicked()
                {
                    st.message = pick_and_import(flow, st.with_staff);
                }
                ui.checkbox(&mut st.with_staff, "名簿も取り込む")
                    .on_hover_text("取り込むファイルの作業者とシフトも足す。同じ中身のシフトは今あるものを使う");
            });
            if let Some((ok, text)) = &st.message {
                ui.colored_label(if *ok { GREEN } else { ERROR }, text);
            }
            ui.add_space(4.0);
            lines_section(ui, flow);
            ui.separator();
            results_section(ui, flow, run);
        });
    replaced
}

/// ファイルを選んで取り込む。選ばなければ何もしない。
fn pick_and_import(flow: &mut FlowModel, with_staff: bool) -> Option<(bool, String)> {
    let path = rfd::FileDialog::new()
        .set_title("取り込むラインのプロジェクト")
        .add_filter("plantwright プロジェクト", crate::project::OPEN_EXTENSIONS)
        .pick_file()?;
    Some(match read_flow(&path) {
        Ok(other) if other.is_empty() => (false, "このファイルには工程がありません".into()),
        Ok(other) => {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let count = other.elements.len();
            let line = flow.import_line(&other, &name, with_staff);
            let name = flow.line(line).map_or(name, |l| l.name.clone());
            (
                true,
                format!(
                    "「{name}」として {count} 個の要素を取り込みました。ライン間の送り先は「工程」タブでつないでください。"
                ),
            )
        }
        Err(e) => (false, e),
    })
}

/// .plantwright から工程モデルだけを読む (3D の部品は取り込まないので対応付けは見ない)
fn read_flow(path: &Path) -> Result<FlowModel, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("読めません: {e}"))?;
    let file = crate::project::ProjectFile::from_json(&text).map_err(|e| e.to_string())?;
    let record = file.flow.ok_or("このファイルには工程がありません")?;
    Ok(from_record(&record, |_| None).0)
}

fn lines_section(ui: &mut egui::Ui, flow: &mut FlowModel) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("ライン");
        if ui.button("+ ライン").clicked() {
            flow.add_line("");
        }
        if flow.lines.is_empty()
            && !flow.is_empty()
            && ui
                .button("今の要素を 1 本のラインにまとめる")
                .on_hover_text("今ある要素をすべて「ライン1」に入れる")
                .clicked()
        {
            flow.group_unassigned("");
        }
    });
    if flow.lines.is_empty() {
        ui.weak("ラインに分けると、ラインごとの出来高・ボトルネック・人待ちが出ます。要素をどのラインに入れるかは「工程」タブで選びます。ライン間の搬送や倉庫は、どのラインにも入れずにおけます。");
        return;
    }
    let mut remove = None;
    egui::Grid::new("flow_lines")
        .striped(true)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            let counts: Vec<usize> = flow
                .lines
                .iter()
                .map(|l| {
                    flow.elements
                        .iter()
                        .filter(|e| e.line == Some(l.id))
                        .count()
                })
                .collect();
            for (line, count) in flow.lines.iter_mut().zip(counts) {
                ui.add_sized([180.0, 20.0], egui::TextEdit::singleline(&mut line.name))
                    .on_hover_text("名前を変えられる");
                ui.weak(format!("要素 {count} 個"));
                if ui
                    .button("消す")
                    .on_hover_text(
                        "ラインだけを消す (要素は残り、どのラインにも入っていない状態になる)",
                    )
                    .clicked()
                {
                    remove = Some(line.id);
                }
                ui.end_row();
            }
        });
    if let Some(id) = remove {
        flow.remove_line(id);
    }
}

fn results_section(ui: &mut egui::Ui, flow: &FlowModel, run: &FlowRun) {
    ui.strong("ラインごとの結果");
    let Some(report) = &run.report else {
        ui.weak("▶ 実行 か ⏩ 最後まで を押すと、ラインごとの結果が出ます。");
        return;
    };
    // 結果は回したときの工程で数える (今の工程と違えば知らせる)
    let model = run.ran_with.as_ref().unwrap_or(flow);
    if run.is_stale(flow) {
        ui.colored_label(WARN, "設定が変わりました。結果は前の設定のものです。");
    }
    let lines = line_summaries(model, report);
    let worst = lines
        .iter()
        .filter(|l| l.line.is_some())
        .min_by(|a, b| a.throughput_per_hour.total_cmp(&b.throughput_per_hour))
        .and_then(|l| l.line);
    egui::Grid::new("flow_line_results")
        .striped(true)
        .spacing([14.0, 4.0])
        .show(ui, |ui| {
            for title in [
                "ライン",
                "出来高",
                "スループット",
                "ボトルネック",
                "仕掛数 (平均)",
                "人待ち",
            ] {
                ui.strong(title);
            }
            ui.end_row();
            for l in &lines {
                let text = |s: String| {
                    if l.line.is_some() && l.line == worst && lines.len() > 1 {
                        egui::RichText::new(s).color(WARN)
                    } else {
                        egui::RichText::new(s)
                    }
                };
                ui.label(text(l.name.clone()));
                ui.label(text(format!("{} 個", l.output)));
                ui.label(text(format!("{:.1} 個/時", l.throughput_per_hour)));
                ui.label(text(
                    l.bottleneck
                        .and_then(|id| report.element(id))
                        .map_or("-".into(), |e| {
                            format!("{} ({:.0}%)", e.name, l.bottleneck_busy * 100.0)
                        }),
                ));
                ui.label(text(format!("{:.2} 個", l.wip_average)));
                ui.label(text(
                    l.no_operator_ratio
                        .map_or("-".into(), |r| format!("{:.1}%", r * 100.0)),
                ));
                ui.end_row();
            }
        });
    ui.weak("出来高はラインから出ていった数 (排出に入った数と、ほかのラインや搬送へ送った数) です。いちばん少ないラインを黄色にしています。");
    if report.workers.iter().any(|w| w.support_tasks > 0) {
        ui.add_space(4.0);
        ui.strong("ほかのラインの応援");
        for w in report.workers.iter().filter(|w| w.support_tasks > 0) {
            ui.label(format!(
                "{}: 作業 {} 回のうち {} 回 ({:.0}%)",
                w.name,
                w.tasks,
                w.support_tasks,
                f64::from(w.support_tasks) * 100.0 / f64::from(w.tasks.max(1))
            ));
        }
    }
}
