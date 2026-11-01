//! 「工程シミュレーション」ウィンドウの「計画」タブ
//!
//! - 品種の一覧 (足す・名前を変える・消す)
//! - 生産計画: 注文 (品種・数量・納期・投入する生成) の表、Excel / CSV からの読み込み
//! - サンプル計画
//! - 回した結果: 納期達成率、注文ごとの完了時刻と遅れ、品種ごとの排出数、段取り替えの回数
//!
//! 品種ごとの加工時間・段取り替え・通る工程は「工程」タブの要素の設定で入れる。
//! 計算は `plantwright_core::flow::plan` にあり、ここは入力を集めて結果を表に写すだけ。

use bevy_egui::egui::{self, Color32};
use plantwright_core::staffing::model::clock_label;

use super::super::FlowRun;
use plantwright_core::flow::factory::sample_factory;
use plantwright_core::flow::model::{FlowModel, KindTag, Order};
use plantwright_core::flow::plan::{OrderStatus, apply_sample_plan, on_time_ratio, plan_from_rows};
use plantwright_core::flow::plan_table::read_table;

/// 計画タブだけが使う状態
#[derive(Default)]
pub struct PlanUiState {
    /// 読み込みの結果・失敗の知らせ (true なら成功)
    messages: Vec<(bool, String)>,
}

const GREEN: Color32 = Color32::from_rgb(90, 190, 110);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const ERROR: Color32 = Color32::from_rgb(230, 90, 90);

/// タブを描く。工程をまるごと置き換えたら true。
pub fn plan_tab(
    ui: &mut egui::Ui,
    flow: &mut FlowModel,
    run: &FlowRun,
    st: &mut PlanUiState,
) -> bool {
    let mut replaced = false;
    egui::ScrollArea::vertical()
        .id_salt("flow_plan")
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("サンプル計画")
                    .on_hover_text("サンプル工場に、標準品と特注品の 3 件の注文と、切削の段取り替え (10 分) を入れる")
                    .clicked()
                {
                    let scenarios = std::mem::take(&mut flow.scenarios);
                    *flow = sample_factory();
                    apply_sample_plan(flow);
                    flow.scenarios = scenarios;
                    st.messages.clear();
                    replaced = true;
                }
                ui.separator();
                if ui
                    .button("Excel / CSV から読み込む…")
                    .on_hover_text("1 行目に「品種」「数量」「納期 (開始からの時間)」の見出しがある表を読み、今の注文と置き換える")
                    .clicked()
                {
                    st.messages = pick_and_read(flow);
                }
            });
            for (ok, text) in &st.messages {
                ui.colored_label(if *ok { GREEN } else { WARN }, text);
            }
            ui.add_space(4.0);
            products_section(ui, flow);
            ui.separator();
            orders_section(ui, flow);
            ui.separator();
            results_section(ui, flow, run);
        });
    replaced
}

/// 表のファイルを選んで注文を読み込む。選ばなければ何もしない。
fn pick_and_read(flow: &mut FlowModel) -> Vec<(bool, String)> {
    let Some(path) = rfd::FileDialog::new()
        .set_title("生産計画の表")
        .add_filter(
            "Excel / CSV",
            &["xlsx", "xlsm", "xls", "ods", "csv", "tsv", "txt"],
        )
        .pick_file()
    else {
        return Vec::new();
    };
    let rows = match read_table(&path) {
        Ok(rows) => rows,
        Err(e) => return vec![(false, e)],
    };
    let (orders, warnings) = plan_from_rows(flow, &rows);
    let mut out: Vec<(bool, String)> = warnings.into_iter().map(|w| (false, w)).collect();
    if !orders.is_empty() {
        out.insert(
            0,
            (
                true,
                format!(
                    "{} 件の注文を読み込みました (合計 {} 個)",
                    orders.len(),
                    orders.iter().map(|o| o.quantity).sum::<u32>()
                ),
            ),
        );
        flow.orders = orders;
    }
    out
}

fn products_section(ui: &mut egui::Ui, flow: &mut FlowModel) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("品種");
        if ui.button("+ 品種").clicked() {
            flow.add_product("");
        }
    });
    if flow.products.is_empty() {
        ui.weak(
            "品種を足すと、「工程」タブで品種ごとの加工時間・段取り替え・通る工程を入れられます。",
        );
        return;
    }
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        for p in &mut flow.products {
            ui.add_sized([120.0, 20.0], egui::TextEdit::singleline(&mut p.name))
                .on_hover_text("名前を変えられる");
            if ui
                .small_button("消す")
                .on_hover_text("品種を消す (その品種の加工時間・注文も消える)")
                .clicked()
            {
                remove = Some(p.id);
            }
            ui.add_space(8.0);
        }
    });
    if let Some(id) = remove {
        flow.remove_product(id);
    }
}

fn orders_section(ui: &mut egui::Ui, flow: &mut FlowModel) {
    ui.horizontal_wrapped(|ui| {
        ui.strong("生産計画 (注文)");
        let can_add = !flow.products.is_empty();
        if ui
            .add_enabled(can_add, egui::Button::new("+ 注文"))
            .on_disabled_hover_text("先に品種を足してください")
            .clicked()
        {
            let product = flow.products[0].id;
            let due = flow.settings.duration;
            let n = flow.orders.len() + 1;
            flow.orders.push(Order {
                name: format!("注文{n}"),
                product,
                quantity: 100,
                due,
                source: None,
            });
        }
        if !flow.orders.is_empty() && ui.button("すべて消す").clicked() {
            flow.orders.clear();
        }
    });
    if flow.orders.is_empty() {
        ui.weak("注文がなければ、生成は間隔どおりに出し続けます。注文を入れると、生成は上の注文から順に、その数だけ出して止まります。");
        return;
    }
    let sources: Vec<(u32, String)> = flow
        .elements
        .iter()
        .filter(|e| e.kind.tag() == KindTag::Source)
        .map(|e| (e.id, e.name.clone()))
        .collect();
    let products: Vec<(u32, String)> = flow
        .products
        .iter()
        .map(|p| (p.id, p.name.clone()))
        .collect();
    let start = flow.staffing.start_clock;
    let mut remove = None;
    let mut swap = None;
    let count = flow.orders.len();
    egui::Grid::new("flow_orders")
        .striped(true)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            for title in [
                "注文",
                "品種",
                "数量",
                "納期 (開始から)",
                "",
                "投入する生成",
                "",
            ] {
                ui.strong(title);
            }
            ui.end_row();
            for (k, o) in flow.orders.iter_mut().enumerate() {
                ui.add_sized([90.0, 20.0], egui::TextEdit::singleline(&mut o.name));
                egui::ComboBox::from_id_salt(("flow_order_product", k))
                    .selected_text(
                        products
                            .iter()
                            .find(|(id, _)| *id == o.product)
                            .map_or("-", |(_, n)| n.as_str()),
                    )
                    .show_ui(ui, |ui| {
                        for (id, name) in &products {
                            ui.selectable_value(&mut o.product, *id, name);
                        }
                    });
                ui.add(
                    egui::DragValue::new(&mut o.quantity)
                        .range(0..=10_000_000)
                        .suffix(" 個"),
                );
                let mut h = o.due / 3600.0;
                if ui
                    .add(
                        egui::DragValue::new(&mut h)
                            .speed(0.1)
                            .range(0.0..=1000.0)
                            .max_decimals(2)
                            .suffix(" 時間"),
                    )
                    .changed()
                {
                    o.due = h * 3600.0;
                }
                ui.weak(clock_label(start + o.due))
                    .on_hover_text("時計の時刻 (人員タブの「始める時刻」から数える)");
                if sources.len() > 1 {
                    let current = o
                        .source
                        .and_then(|s| sources.iter().find(|(id, _)| *id == s))
                        .map_or("いちばん上の生成", |(_, n)| n.as_str());
                    egui::ComboBox::from_id_salt(("flow_order_source", k))
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut o.source, None, "いちばん上の生成");
                            for (id, name) in &sources {
                                ui.selectable_value(&mut o.source, Some(*id), name);
                            }
                        });
                } else {
                    ui.weak(sources.first().map_or("-", |(_, n)| n.as_str()));
                }
                ui.horizontal(|ui| {
                    if ui.small_button("↑").on_hover_text("先に投入する").clicked() && k > 0
                    {
                        swap = Some((k, k - 1));
                    }
                    if ui.small_button("↓").on_hover_text("後に投入する").clicked() && k + 1 < count
                    {
                        swap = Some((k, k + 1));
                    }
                    if ui.small_button("消す").clicked() {
                        remove = Some(k);
                    }
                });
                ui.end_row();
            }
        });
    if let Some((a, b)) = swap {
        flow.orders.swap(a, b);
    }
    if let Some(k) = remove {
        flow.orders.remove(k);
    }
    let total = flow.planned_quantity();
    ui.weak(format!("合計 {total} 個。上の注文から順に投入します。"));
}

fn results_section(ui: &mut egui::Ui, flow: &FlowModel, run: &FlowRun) {
    ui.strong("計画の結果");
    let Some(report) = &run.report else {
        ui.weak("▶ 実行 か ⏩ 最後まで を押すと、注文ごとに間に合ったかが出ます。");
        return;
    };
    let model = run.ran_with.as_ref().unwrap_or(flow);
    if run.is_stale(flow) {
        ui.colored_label(WARN, "設定が変わりました。結果は前の設定のものです。");
    }
    if report.orders.is_empty() && report.product_output.iter().all(|(p, _)| p.is_none()) {
        ui.weak("注文も品種もない設定で回した結果です。");
        return;
    }
    if !report.orders.is_empty() {
        match on_time_ratio(&report.orders) {
            Some(r) => {
                let late = report
                    .orders
                    .iter()
                    .filter(|o| o.status == OrderStatus::Late)
                    .count();
                ui.horizontal(|ui| {
                    ui.label("納期達成率");
                    ui.colored_label(
                        if late == 0 { GREEN } else { ERROR },
                        egui::RichText::new(format!("{:.0}%", r * 100.0)).strong(),
                    );
                    if late > 0 {
                        ui.colored_label(ERROR, format!("遅れ {late} 件"));
                    }
                });
            }
            None => {
                ui.weak("まだ納期の来た注文がありません (回す時間を延ばすと判定できます)。");
            }
        }
        let start = model.staffing.start_clock;
        let time = |t: f64| format!("{:.2} 時間 ({})", t / 3600.0, clock_label(start + t));
        egui::Grid::new("flow_order_results")
            .striped(true)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                for title in [
                    "注文",
                    "品種",
                    "排出 / 数量",
                    "納期",
                    "完了",
                    "遅れ",
                    "結果",
                ] {
                    ui.strong(title);
                }
                ui.end_row();
                for o in &report.orders {
                    let c = match o.status {
                        OrderStatus::OnTime => GREEN,
                        OrderStatus::Late => ERROR,
                        OrderStatus::Open => WARN,
                    };
                    ui.label(&o.name);
                    ui.label(model.product_name(Some(o.product)));
                    ui.label(format!("{} / {} 個", o.done, o.quantity));
                    ui.label(time(o.due));
                    ui.label(o.finished.map_or("-".into(), time));
                    ui.label(
                        o.lateness(report.time)
                            .map_or("-".into(), |l| format!("{:.0} 分", l / 60.0)),
                    );
                    ui.colored_label(c, o.status.label());
                    ui.end_row();
                }
            });
    }
    let products: Vec<String> = report
        .product_output
        .iter()
        .map(|(p, n)| format!("{} {n} 個", model.product_name(*p)))
        .collect();
    if !products.is_empty() {
        ui.label(format!("品種ごとの排出: {}", products.join("、")));
    }
    let setups: Vec<String> = report
        .elements
        .iter()
        .filter(|e| e.setups > 0)
        .map(|e| {
            format!(
                "{} {} 回 ({:.0} 分)",
                e.name,
                e.setups,
                e.setup_seconds / 60.0
            )
        })
        .collect();
    if !setups.is_empty() {
        ui.label(format!("段取り替え: {}", setups.join("、")));
    }
}
