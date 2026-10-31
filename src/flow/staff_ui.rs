//! 「工程シミュレーション」ウィンドウの「人員」タブ
//!
//! - 始める時刻と歩行時間
//! - シフト (始業・終業・休憩)
//! - 作業者の名簿と、できる工程 (スキル表)。マスの数字は作業の速さ (100% が標準)
//! - 人数の比較 (1〜N 人で回して並べる)
//!
//! 計算は `cadsim_core::staffing` にあり、ここは入力を集めて結果を表に写すだけ。

use bevy_egui::egui::{self, Color32};
use cadsim_core::staffing::compare::{HeadcountComparison, compare_headcount, move_worker};
use cadsim_core::staffing::model::{Break, DAY, MAX_SPEED, MIN_SPEED, clock_label};

use super::model::{ElementId, FlowModel, KindTag, apply_sample_staffing, sample_line};

/// 人員タブだけが使う状態
pub struct StaffUiState {
    /// 人数の比較の目標 (個/時)。0 なら目標なし。
    target_per_hour: f64,
    comparison: Option<Result<HeadcountComparison, Vec<String>>>,
    /// 比べたときのモデル。今のモデルと違えば結果は古い。
    compared_with: Option<FlowModel>,
}

impl Default for StaffUiState {
    fn default() -> Self {
        Self {
            target_per_hour: 0.0,
            comparison: None,
            compared_with: None,
        }
    }
}

const GREEN: Color32 = Color32::from_rgb(90, 190, 110);
const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const ERROR: Color32 = Color32::from_rgb(230, 90, 90);

pub fn staff_tab(ui: &mut egui::Ui, flow: &mut FlowModel, st: &mut StaffUiState) -> bool {
    let mut replaced = false;
    egui::ScrollArea::vertical()
        .id_salt("flow_staff")
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("サンプルの人員を入れる")
                    .on_hover_text(
                        "サンプル工程に置き換え、加工 (投入だけ人)・組立・検査 (手作業) と、日勤 3 人を入れる",
                    )
                    .clicked()
                {
                    let mut m = sample_line();
                    apply_sample_staffing(&mut m);
                    *flow = m;
                    st.comparison = None;
                    replaced = true;
                }
                ui.separator();
                ui.label("始める時刻");
                clock_edit(ui, "flow_start_clock", &mut flow.staffing.start_clock)
                    .on_hover_text("シミュレーションの 0 秒が何時か。シフトと休憩はこの時刻から数える");
                ui.label("別の工程への歩行");
                ui.add(
                    egui::DragValue::new(&mut flow.staffing.walk_seconds)
                        .speed(0.5)
                        .range(0.0..=3600.0)
                        .suffix(" 秒"),
                )
                .on_hover_text("作業者が別の工程へ移るときにかかる時間");
            });
            ui.add_space(6.0);
            shifts_section(ui, flow);
            ui.add_space(8.0);
            workers_section(ui, flow);
            ui.add_space(8.0);
            comparison_section(ui, flow, st);
        });
    replaced
}

// ---------------------------------------------------------------------------
// シフト
// ---------------------------------------------------------------------------

fn shifts_section(ui: &mut egui::Ui, flow: &mut FlowModel) {
    ui.horizontal(|ui| {
        ui.strong("シフト");
        if ui
            .button("+ シフト")
            .on_hover_text("8:00〜17:00、昼休み 12:00〜13:00 で追加")
            .clicked()
        {
            flow.staffing.add_shift();
        }
    });
    let staffing = &mut flow.staffing;
    if staffing.shifts.is_empty() {
        ui.weak("シフトがないと、作業者は休みなしで回す時間ずっと働きます。");
        return;
    }
    let mut remove = None;
    egui::Grid::new("flow_shifts")
        .striped(true)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            for title in ["名前", "始業", "終業", "休憩", ""] {
                ui.weak(title);
            }
            ui.end_row();
            for shift in &mut staffing.shifts {
                let id = shift.id;
                ui.add(egui::TextEdit::singleline(&mut shift.name).desired_width(80.0));
                clock_edit(ui, ("shift_start", id), &mut shift.start);
                clock_edit(ui, ("shift_end", id), &mut shift.end)
                    .on_hover_text("始業より早い時刻なら翌日 (夜勤)");
                ui.horizontal_wrapped(|ui| {
                    let mut drop = None;
                    for (k, b) in shift.breaks.iter_mut().enumerate() {
                        clock_edit(ui, ("break_start", id, k), &mut b.start);
                        ui.label("〜");
                        clock_edit(ui, ("break_end", id, k), &mut b.end);
                        if ui
                            .small_button("✕")
                            .on_hover_text("この休憩を消す")
                            .clicked()
                        {
                            drop = Some(k);
                        }
                        ui.add_space(6.0);
                    }
                    if let Some(k) = drop {
                        shift.breaks.remove(k);
                    }
                    if ui.small_button("+ 休憩").clicked() {
                        let start = shift.start + 2.0 * 3600.0;
                        shift.breaks.push(Break {
                            start: start.rem_euclid(DAY),
                            end: (start + 600.0).rem_euclid(DAY),
                        });
                    }
                });
                if ui.small_button("🗑").on_hover_text("シフトを消す").clicked() {
                    remove = Some(id);
                }
                ui.end_row();
            }
        });
    if let Some(id) = remove {
        staffing.remove_shift(id);
    }
}

// ---------------------------------------------------------------------------
// 作業者とスキル表
// ---------------------------------------------------------------------------

fn workers_section(ui: &mut egui::Ui, flow: &mut FlowModel) {
    ui.horizontal(|ui| {
        ui.strong("作業者とできる工程");
        if ui.button("+ 作業者").clicked() {
            flow.staffing.add_worker();
        }
    });
    let processes: Vec<(ElementId, String, bool)> = flow
        .elements
        .iter()
        .filter(|e| e.kind.tag() == KindTag::Process)
        .map(|e| (e.id, e.name.clone(), e.labor.needs_operator()))
        .collect();
    if !processes.iter().any(|(_, _, needs)| *needs) {
        ui.weak("人が要る工程がありません。「工程」タブで工程を選び、「作業者」を「必要」にしてください。");
    }
    if flow.staffing.workers.is_empty() {
        ui.weak("「+ 作業者」で名簿に足し、表のマスにチェックを入れると、その工程を受け持てるようになります。");
        return;
    }
    ui.weak("マスにチェックを入れた工程を受け持ちます。数字は作業の速さで、100% が標準、120% なら手作業が 1.2 倍速くなります。");
    let shifts: Vec<(u32, String)> = flow
        .staffing
        .shifts
        .iter()
        .map(|s| (s.id, s.name.clone()))
        .collect();
    let mut remove = None;
    let mut reorder = None;
    egui::ScrollArea::horizontal()
        .id_salt("flow_skill_matrix")
        .show(ui, |ui| {
            egui::Grid::new("flow_workers")
                .striped(true)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    for title in ["", "名前", "シフト", "欠勤"] {
                        ui.weak(title);
                    }
                    for (_, name, needs) in &processes {
                        if *needs {
                            ui.strong(name);
                        } else {
                            ui.weak(format!("{name} (自動)"))
                                .on_hover_text("人が要らない工程。「工程」タブで「作業者」を必要にすると使われます");
                        }
                    }
                    ui.weak("");
                    ui.end_row();

                    for w in &mut flow.staffing.workers {
                        let id = w.id;
                        ui.horizontal(|ui| {
                            if ui.small_button("↑").on_hover_text("名簿で上へ (人数の比較では上の人から出勤)").clicked() {
                                reorder = Some((id, true));
                            }
                            if ui.small_button("↓").clicked() {
                                reorder = Some((id, false));
                            }
                        });
                        ui.add(egui::TextEdit::singleline(&mut w.name).desired_width(80.0));
                        let shift_name = w
                            .shift
                            .and_then(|s| shifts.iter().find(|(i, _)| *i == s))
                            .map_or("なし (ずっと)", |(_, n)| n.as_str());
                        egui::ComboBox::from_id_salt(("worker_shift", id))
                            .selected_text(shift_name)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut w.shift, None, "なし (ずっと)");
                                for (sid, name) in &shifts {
                                    ui.selectable_value(&mut w.shift, Some(*sid), name);
                                }
                            });
                        ui.checkbox(&mut w.absent, "")
                            .on_hover_text("この人が休んだ場合を試す");
                        for (pid, _, _) in &processes {
                            ui.horizontal(|ui| {
                                let mut can = w.speed_for(*pid).is_some();
                                if ui.checkbox(&mut can, "").changed() {
                                    w.set_skill(*pid, can);
                                }
                                if let Some(skill) = w.skills.iter_mut().find(|s| s.element == *pid) {
                                    let mut p = skill.speed * 100.0;
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut p)
                                                .speed(1.0)
                                                .range(MIN_SPEED * 100.0..=MAX_SPEED * 100.0)
                                                .suffix("%"),
                                        )
                                        .changed()
                                    {
                                        skill.speed = p / 100.0;
                                    }
                                }
                            });
                        }
                        if ui.small_button("🗑").on_hover_text("名簿から消す").clicked() {
                            remove = Some(id);
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(id) = remove {
        flow.staffing.remove_worker(id);
    }
    if let Some((id, up)) = reorder {
        move_worker(&mut flow.staffing, id, up);
    }
    let present = flow.staffing.present_count();
    ui.weak(format!(
        "出勤 {present} 人 / 名簿 {} 人",
        flow.staffing.workers.len()
    ));
}

// ---------------------------------------------------------------------------
// 人数の比較
// ---------------------------------------------------------------------------

fn comparison_section(ui: &mut egui::Ui, flow: &FlowModel, st: &mut StaffUiState) {
    ui.strong("人数の比較");
    ui.horizontal_wrapped(|ui| {
        ui.label("目標のスループット");
        ui.add(
            egui::DragValue::new(&mut st.target_per_hour)
                .speed(1.0)
                .range(0.0..=1_000_000.0)
                .suffix(" 個/時"),
        )
        .on_hover_text("0 なら目標なし。目標を満たすいちばん少ない人数が緑になる");
        if ui
            .button("▶ 人数を変えて比べる")
            .on_hover_text("名簿の上から 1 人、2 人、… と出勤させて同じ条件で回し、結果を並べる")
            .clicked()
        {
            let target = (st.target_per_hour > 0.0).then_some(st.target_per_hour);
            st.comparison = Some(compare_headcount(flow, target));
            st.compared_with = Some(flow.clone());
        }
    });
    let Some(result) = &st.comparison else {
        ui.weak("名簿の上の人から順に出勤させ、1 人〜出勤できる人数まで回して比べます。");
        return;
    };
    if st.compared_with.as_ref().is_some_and(|m| m != flow) {
        ui.colored_label(WARN, "設定が変わりました。結果は前の設定のものです。");
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
    let best = c.smallest_meeting_target();
    egui::Grid::new("flow_headcount")
        .striped(true)
        .spacing([14.0, 4.0])
        .show(ui, |ui| {
            for title in [
                "人数",
                "排出数",
                "スループット",
                "作業者の稼働率",
                "工程の人待ち",
                "仕掛数 (平均)",
                "リードタイム",
            ] {
                ui.strong(title);
            }
            ui.end_row();
            for r in &c.rows {
                let text = |s: String| {
                    if best == Some(r.headcount) {
                        egui::RichText::new(s).color(GREEN).strong()
                    } else {
                        egui::RichText::new(s)
                    }
                };
                ui.label(text(format!("{} 人", r.headcount)));
                ui.label(text(format!("{} 個", r.output)));
                ui.label(text(format!("{:.1} 個/時", r.throughput_per_hour)));
                ui.label(text(format!("{:.0}%", r.worker_utilization * 100.0)));
                ui.label(text(format!("{:.1}%", r.no_operator_ratio * 100.0)));
                ui.label(text(format!("{:.2} 個", r.wip_average)));
                ui.label(text(
                    r.lead_time.map_or("-".into(), |t| format!("{t:.0} 秒")),
                ));
                ui.end_row();
            }
        });
    match (c.target_per_hour, best) {
        (Some(_), Some(n)) => {
            ui.colored_label(GREEN, format!("目標を満たすのは {n} 人からです。"));
        }
        (Some(t), None) => {
            ui.colored_label(
                ERROR,
                format!("どの人数でも {t:.0} 個/時 に届きません。人以外 (設備・投入間隔) が足りない可能性があります。"),
            );
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// 部品
// ---------------------------------------------------------------------------

/// 時刻 (0:00 からの秒) を「時」「分」で編集する
fn clock_edit(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    clock: &mut f64,
) -> egui::Response {
    let total = (clock.rem_euclid(DAY) / 60.0).round() as i64;
    let mut h = total / 60 % 24;
    let mut m = total % 60;
    let inner = ui.push_id(id, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let rh = ui.add(egui::DragValue::new(&mut h).range(0..=23));
            ui.label(":");
            let rm = ui.add(
                egui::DragValue::new(&mut m)
                    .range(0..=59)
                    .custom_formatter(|v, _| format!("{v:02.0}")),
            );
            rh.union(rm)
        })
        .inner
    });
    let response = inner.inner;
    if response.changed() {
        *clock = (h * 3600 + m * 60) as f64;
    }
    response.on_hover_text(clock_label(*clock))
}
