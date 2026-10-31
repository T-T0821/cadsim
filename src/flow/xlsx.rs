//! 工程シミュレーションの結果を Excel (xlsx) に書き出す (Bevy に依存しない)
//!
//! シートは3枚:
//! - 概要: ラインの集計 (排出数・スループット・サイクルタイム・仕掛数・電力量など) と回した条件
//! - 設備別: 要素ごとの稼働・待ち・詰まり・故障の割合、電力量・エア消費量と、割合の積み上げ横棒グラフ
//! - 推移: 一定間隔の排出数の累計・仕掛数と、その折れ線グラフ

use rust_xlsxwriter::{
    Chart, ChartFormat, ChartSolidFill, ChartType, Color, Format, FormatAlign, Workbook, Worksheet,
    XlsxError,
};

use super::model::{ElementKind, FlowModel, KindTag};
use super::sim::{Report, State};

const SUMMARY: &str = "概要";
const ELEMENTS: &str = "設備別";
const SERIES: &str = "推移";

/// 状態の色 (3D ビューの色分けと同じ)
fn state_color(state: State) -> Color {
    let [r, g, b] = super::palette::state_rgb(state);
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    Color::RGB((c(r) << 16) | (c(g) << 8) | c(b))
}

/// 結果を xlsx のバイト列にする
pub fn report_to_xlsx(model: &FlowModel, report: &Report) -> Result<Vec<u8>, XlsxError> {
    let mut workbook = Workbook::new();
    let header = Format::new()
        .set_bold()
        .set_background_color(Color::RGB(0xDDE4EE))
        .set_align(FormatAlign::Center);
    let bold = Format::new().set_bold();
    let percent = Format::new().set_num_format("0.0%");
    let decimal = Format::new().set_num_format("0.00");

    summary_sheet(
        workbook.add_worksheet(),
        model,
        report,
        &header,
        &bold,
        &decimal,
    )?;
    elements_sheet(
        workbook.add_worksheet(),
        report,
        &header,
        &percent,
        &decimal,
    )?;
    series_sheet(workbook.add_worksheet(), report, &header, &decimal)?;
    workbook.save_to_buffer()
}

fn summary_sheet(
    sheet: &mut Worksheet,
    model: &FlowModel,
    report: &Report,
    header: &Format,
    bold: &Format,
    decimal: &Format,
) -> Result<(), XlsxError> {
    sheet.set_name(SUMMARY)?;
    sheet.set_column_width(0, 28)?;
    sheet.set_column_width(1, 16)?;
    sheet.set_column_width(2, 14)?;
    sheet.write_string_with_format(0, 0, "ラインの集計", bold)?;
    let bottleneck = report
        .bottleneck
        .and_then(|id| report.element(id))
        .map_or("-".to_owned(), |e| e.name.clone());
    let rows: Vec<(&str, Option<f64>, &str)> = vec![
        ("回した時間", Some(report.time / 3600.0), "時間"),
        (
            "集計した時間 (立ち上がりを除く)",
            Some(report.measured / 3600.0),
            "時間",
        ),
        ("排出数", Some(report.output as f64), "個"),
        ("スループット", Some(report.throughput_per_hour), "個/時"),
        ("サイクルタイム", report.cycle_time, "秒/個"),
        ("リードタイム (生成→排出)", report.lead_time, "秒"),
        ("仕掛数 (平均)", Some(report.wip_average), "個"),
        ("仕掛数 (最大)", Some(report.wip_max as f64), "個"),
        ("電力量", Some(report.energy_kwh), "kWh"),
        ("1個あたりの電力量", report.energy_per_work_kwh, "kWh/個"),
        ("エア消費量", Some(report.air_m3), "m³ (ANR)"),
    ];
    let mut row = 1;
    for (label, value, unit) in rows {
        sheet.write_string(row, 0, label)?;
        match value {
            Some(v) => sheet.write_number_with_format(row, 1, v, decimal)?,
            None => sheet.write_string(row, 1, "-")?,
        };
        sheet.write_string(row, 2, unit)?;
        row += 1;
    }
    sheet.write_string(row, 0, "ボトルネック")?;
    sheet.write_string(row, 1, &bottleneck)?;
    row += 2;

    sheet.write_string_with_format(row, 0, "回した条件", bold)?;
    row += 1;
    let s = model.settings;
    for (label, value) in [
        ("回す時間 (時間)", s.duration / 3600.0),
        ("立ち上がり (時間)", s.warmup / 3600.0),
        ("乱数の種", s.seed as f64),
    ] {
        sheet.write_string(row, 0, label)?;
        sheet.write_number(row, 1, value)?;
        row += 1;
    }
    row += 1;
    for (col, title) in [
        "要素",
        "種類",
        "時間 (秒)",
        "ばらつき",
        "容量・上限",
        "MTBF (分)",
        "MTTR (分)",
        "定格 (kW)",
        "待機 (kW)",
        "エア (L/min)",
    ]
    .iter()
    .enumerate()
    {
        sheet.write_string_with_format(row, col as u16, *title, header)?;
    }
    row += 1;
    for e in &model.elements {
        sheet.write_string(row, 0, &e.name)?;
        sheet.write_string(row, 1, e.kind.tag().label())?;
        match &e.kind {
            ElementKind::Source {
                interval,
                spread,
                limit,
            } => {
                sheet.write_number(row, 2, *interval)?;
                sheet.write_number(row, 3, *spread)?;
                if let Some(l) = limit {
                    sheet.write_number(row, 4, *l)?;
                }
            }
            ElementKind::Process {
                cycle_time,
                spread,
                failure,
            } => {
                sheet.write_number(row, 2, *cycle_time)?;
                sheet.write_number(row, 3, *spread)?;
                if let Some(f) = failure {
                    sheet.write_number(row, 5, f.mtbf / 60.0)?;
                    sheet.write_number(row, 6, f.mttr / 60.0)?;
                }
            }
            ElementKind::Buffer { capacity } => {
                sheet.write_number(row, 4, *capacity)?;
            }
            ElementKind::Sink => {}
        }
        sheet.write_number(row, 7, e.energy.rated_kw)?;
        sheet.write_number(row, 8, e.energy.standby_kw)?;
        sheet.write_number(row, 9, e.energy.air_l_per_min)?;
        row += 1;
    }
    Ok(())
}

fn elements_sheet(
    sheet: &mut Worksheet,
    report: &Report,
    header: &Format,
    percent: &Format,
    decimal: &Format,
) -> Result<(), XlsxError> {
    sheet.set_name(ELEMENTS)?;
    let titles = [
        "要素",
        "種類",
        "稼働",
        "待ち",
        "詰まり",
        "故障",
        "通過数",
        "故障回数",
        "平均加工時間 (秒)",
        "平均の中身 (個)",
        "最大の中身 (個)",
        "電力量 (kWh)",
        "エア消費量 (m³)",
        "ボトルネック",
    ];
    for (col, title) in titles.iter().enumerate() {
        sheet.write_string_with_format(0, col as u16, *title, header)?;
    }
    sheet.set_column_width(0, 16)?;
    sheet.set_column_range_width(8, 12, 16)?;
    for (i, e) in report.elements.iter().enumerate() {
        let row = i as u32 + 1;
        sheet.write_string(row, 0, &e.name)?;
        sheet.write_string(row, 1, e.kind.label())?;
        // 状態の割合は生成と工程だけ (バッファ・排出は空欄)
        if matches!(e.kind, KindTag::Source | KindTag::Process) {
            for (k, state) in State::ALL.into_iter().enumerate() {
                sheet.write_number_with_format(row, 2 + k as u16, e.times.ratio(state), percent)?;
            }
        }
        sheet.write_number(row, 6, e.count)?;
        sheet.write_number(row, 7, e.failures)?;
        if let Some(c) = e.average_cycle {
            sheet.write_number_with_format(row, 8, c, decimal)?;
        }
        sheet.write_number_with_format(row, 9, e.content_average, decimal)?;
        sheet.write_number(row, 10, e.content_max as f64)?;
        sheet.write_number_with_format(row, 11, e.energy_kwh, decimal)?;
        sheet.write_number_with_format(row, 12, e.air_m3, decimal)?;
        if report.bottleneck == Some(e.id) {
            sheet.write_string(row, 13, "★")?;
        }
    }
    let last = report.elements.len() as u32;
    let total = last + 1;
    sheet.write_string_with_format(total, 0, "合計", header)?;
    sheet.write_number_with_format(total, 11, report.energy_kwh, decimal)?;
    sheet.write_number_with_format(total, 12, report.air_m3, decimal)?;

    if last > 0 {
        let mut chart = Chart::new(ChartType::BarPercentStacked);
        chart.title().set_name("設備ごとの状態の割合");
        for (k, state) in State::ALL.into_iter().enumerate() {
            let col = 2 + k as u16;
            chart
                .add_series()
                .set_name((ELEMENTS, 0, col))
                .set_categories((ELEMENTS, 1, 0, last, 0))
                .set_values((ELEMENTS, 1, col, last, col))
                .set_format(
                    ChartFormat::new()
                        .set_solid_fill(ChartSolidFill::new().set_color(state_color(state))),
                );
        }
        // 上から要素の並び順に見えるように、項目軸を逆向きにする
        chart.y_axis().set_reverse();
        sheet.insert_chart(total + 2, 0, &chart)?;
    }
    Ok(())
}

fn series_sheet(
    sheet: &mut Worksheet,
    report: &Report,
    header: &Format,
    decimal: &Format,
) -> Result<(), XlsxError> {
    sheet.set_name(SERIES)?;
    for (col, title) in ["時刻 (時間)", "排出数 (累計)", "仕掛数"]
        .iter()
        .enumerate()
    {
        sheet.write_string_with_format(0, col as u16, *title, header)?;
    }
    sheet.set_column_range_width(0, 2, 14)?;
    for (i, s) in report.samples.iter().enumerate() {
        let row = i as u32 + 1;
        sheet.write_number_with_format(row, 0, s.time / 3600.0, decimal)?;
        sheet.write_number(row, 1, s.output)?;
        sheet.write_number(row, 2, s.wip as f64)?;
    }
    let last = report.samples.len() as u32;
    if last > 1 {
        let mut chart = Chart::new(ChartType::ScatterStraight);
        chart.title().set_name("排出数と仕掛数の推移");
        chart
            .add_series()
            .set_name((SERIES, 0, 1))
            .set_categories((SERIES, 1, 0, last, 0))
            .set_values((SERIES, 1, 1, last, 1));
        chart
            .add_series()
            .set_name((SERIES, 0, 2))
            .set_categories((SERIES, 1, 0, last, 0))
            .set_values((SERIES, 1, 2, last, 2))
            .set_secondary_axis(true);
        chart.x_axis().set_name("時刻 (時間)");
        chart.y_axis().set_name("排出数");
        chart.y2_axis().set_name("仕掛数");
        sheet.insert_chart(1, 4, &chart)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::model::sample_line;
    use super::super::sim::Simulation;
    use super::*;

    #[test]
    fn writes_a_zip_with_three_sheets() {
        let model = sample_line();
        let report = Simulation::run(&model).unwrap();
        let bytes = report_to_xlsx(&model, &report).unwrap();
        // xlsx は zip。先頭は "PK"
        assert_eq!(&bytes[..2], b"PK");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("xl/worksheets/sheet3.xml"));
        assert!(text.contains("xl/charts/chart1.xml"));
    }
}
