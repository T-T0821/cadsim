//! 「工程シミュレーション」ウィンドウの「案の比較」タブ
//!
//! - 今の設定 (工程・名簿・回す条件) を名前を付けて案として取っておく
//! - 案の読み込み・名前の変更・削除
//! - 繰り返しの回数を決めて、今の設定とすべての案を回して並べる
//!
//! 計算は `cadsim_core::flow::scenario` にあり、ここは入力を集めて結果を表に写すだけ。

use bevy_egui::egui::{self, Color32};

use super::model::FlowModel;
use super::scenario::{MAX_REPLICATIONS, ScenarioComparison, Summary, compare_scenarios};

/// 案の比較タブだけが使う状態
pub struct ScenarioUiState {
    /// 次に取っておく案の名前
    new_name: String,
    replications: u32,
    result: Option<Result<ScenarioComparison, Vec<String>>>,
    /// 比べたときのモデル (案も含む)。今と違えば結果は古い。
    compared_with: Option<FlowModel>,
}

impl Default for ScenarioUiState {
    fn default() -> Self {
        Self {
            new_name: String::new(),
            replications: 10,
            result: None,
            compared_with: None,
        }
    }
}

const GREEN: Color32 = Color32::from_rgb(90, 190, 110);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const ERROR: Color32 = Color32::from_rgb(230, 90, 90);

/// 今の設定の行の名前
const CURRENT: &str = "今の設定";

/// タブを描く。案を読み込んで今の設定を置き換えたら true。
pub fn scenario_tab(ui: &mut egui::Ui, flow: &mut FlowModel, st: &mut ScenarioUiState) -> bool {
    let mut replaced = false;
    egui::ScrollArea::vertical()
        .id_salt("flow_scenarios")
        .show(ui, |ui| {
            ui.strong("案");
            ui.weak("今の工程・人員・回す条件をまるごと写して取っておけます。設定を変えては取っておき、まとめて比べます。");
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut st.new_name)
                        .hint_text(format!("案{}", flow.scenarios.len() + 1))
                        .desired_width(160.0),
                );
                if ui
                    .button("今の設定を案として取っておく")
                    .on_hover_text("名前が空なら「案1」のように付ける")
                    .clicked()
                {
                    flow.save_scenario(&st.new_name);
                    st.new_name.clear();
                }
            });
            replaced = scenario_list(ui, flow);
            ui.separator();
            comparison_section(ui, flow, st);
        });
    replaced
}

fn scenario_list(ui: &mut egui::Ui, flow: &mut FlowModel) -> bool {
    if flow.scenarios.is_empty() {
        ui.weak("まだ案はありません。");
        return false;
    }
    let mut load = None;
    let mut remove = None;
    egui::Grid::new("flow_scenario_list")
        .striped(true)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            for (i, s) in flow.scenarios.iter_mut().enumerate() {
                ui.add_sized([180.0, 20.0], egui::TextEdit::singleline(&mut s.name))
                    .on_hover_text("名前を変えられる");
                let m = &s.model;
                ui.weak(format!(
                    "工程 {} ・ 作業者 {} 人 ・ {:.0} 時間",
                    m.elements.len(),
                    m.staffing.present_count(),
                    m.settings.duration / 3600.0
                ));
                if ui
                    .button("読み込む")
                    .on_hover_text("この案の設定を今の設定にする (今の設定は上書きされる)")
                    .clicked()
                {
                    load = Some(i);
                }
                if ui.button("消す").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
    if let Some(i) = remove {
        flow.scenarios.remove(i);
    }
    if let Some(i) = load {
        flow.load_scenario(i);
        return true;
    }
    false
}

fn comparison_section(ui: &mut egui::Ui, flow: &FlowModel, st: &mut ScenarioUiState) {
    ui.strong("比べる");
    ui.horizontal_wrapped(|ui| {
        ui.label("繰り返し");
        ui.add(
            egui::DragValue::new(&mut st.replications)
                .speed(0.2)
                .range(1..=MAX_REPLICATIONS)
                .suffix(" 回"),
        )
        .on_hover_text("乱数の種を 1 つずつずらして回す回数。故障やばらつきがあるときは多めに");
        if ui
            .button("▶ 今の設定と案を比べる")
            .on_hover_text("今の設定とすべての案を、同じ回数ずつ回して並べる")
            .clicked()
        {
            let mut cases: Vec<(String, &FlowModel)> = vec![(CURRENT.into(), flow)];
            cases.extend(flow.scenarios.iter().map(|s| (s.name.clone(), &s.model)));
            st.result = Some(compare_scenarios(&cases, st.replications));
            st.compared_with = Some(flow.clone());
        }
    });
    let Some(result) = &st.result else {
        ui.weak("故障やばらつきがあると 1 回の結果は偶然に左右されるので、何回か回して平均と幅 (いちばん少ない〜多い) を出します。");
        return;
    };
    if st.compared_with.as_ref().is_some_and(|m| m != flow) {
        ui.colored_label(WARN, "設定か案が変わりました。結果は前のものです。");
    }
    let c = match result {
        Ok(c) => c,
        Err(problems) => {
            for p in problems {
                ui.colored_label(ERROR, format!("⚠ {p}"));
            }
            return;
        }
    };
    let best = c.best();
    egui::Grid::new("flow_scenario_compare")
        .striped(true)
        .spacing([14.0, 4.0])
        .show(ui, |ui| {
            for title in [
                "案",
                "人数",
                "排出数",
                "スループット",
                "作業者の稼働率",
                "工程の人待ち",
                "仕掛数 (平均)",
                "リードタイム",
                "電力量/個",
                "ボトルネック",
            ] {
                ui.strong(title);
            }
            ui.end_row();
            for (i, r) in c.rows.iter().enumerate() {
                let text = |s: String| {
                    if best == Some(i) {
                        egui::RichText::new(s).color(GREEN).strong()
                    } else {
                        egui::RichText::new(s)
                    }
                };
                ui.label(text(r.name.clone()));
                ui.label(text(format!("{} 人", r.headcount)));
                ui.label(text(range(&r.output, 0, 1.0, " 個")));
                ui.label(text(range(&r.throughput_per_hour, 1, 1.0, " 個/時")));
                ui.label(text(opt(&r.worker_utilization, 0, 100.0, "%")));
                ui.label(text(opt(&r.no_operator_ratio, 1, 100.0, "%")));
                ui.label(text(range(&r.wip_average, 2, 1.0, " 個")));
                ui.label(text(opt(&r.lead_time, 0, 1.0, " 秒")));
                ui.label(text(opt(&r.energy_per_work_kwh, 3, 1.0, " kWh")));
                ui.label(text(r.bottleneck.clone().unwrap_or_else(|| "-".into())));
                ui.end_row();
            }
        });
    ui.weak(format!(
        "それぞれ {} 回回した平均です。( ) はいちばん少ない〜多い回。排出数の平均がいちばん多い案を緑にしています。",
        c.replications
    ));
}

/// 「平均 (最小〜最大)」。幅がなければ平均だけ。
fn range(s: &Summary, digits: usize, scale: f64, unit: &str) -> String {
    let (mean, min, max) = (s.mean * scale, s.min * scale, s.max * scale);
    if (max - min).abs() < 1e-9 {
        format!("{mean:.digits$}{unit}")
    } else {
        format!("{mean:.digits$}{unit} ({min:.digits$}〜{max:.digits$})")
    }
}

fn opt(s: &Option<Summary>, digits: usize, scale: f64, unit: &str) -> String {
    s.as_ref()
        .map_or("-".into(), |s| range(s, digits, scale, unit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_hides_a_zero_width() {
        let s = Summary {
            mean: 10.0,
            min: 10.0,
            max: 10.0,
        };
        assert_eq!(range(&s, 0, 1.0, " 個"), "10 個");
        let s = Summary {
            mean: 0.5,
            min: 0.4,
            max: 0.6,
        };
        assert_eq!(range(&s, 0, 100.0, "%"), "50% (40〜60)");
    }
}
