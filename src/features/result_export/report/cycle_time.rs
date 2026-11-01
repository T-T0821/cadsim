//! サイクルタイムの CSV: タイミングチャートのステップごとの時刻と所要時間

use plantwright_core::motion::chart::Schedule;
use plantwright_core::motion::chart::TimingChart;

use super::csv::{Csv, seconds};

const HEADER: [&str; 9] = [
    "ステップ",
    "コメント",
    "開始[s]",
    "終了[s]",
    "所要時間[s]",
    "動作時間[s]",
    "タイマー[s]",
    "最も長い動作",
    "動作",
];

/// ステップごとに1行、最後にサイクルタイムの行を付けた CSV を作る。
///
/// - 動作時間: そのステップで一番遅い動作の時間 (これが終わるまで次へ進まない)
/// - 最も長い動作: 動作時間を決めているアクチュエータ。短縮を検討するときの手がかりになる。
pub fn cycle_time_csv(chart: &TimingChart, schedule: &Schedule) -> String {
    let mut csv = Csv::with_header(&HEADER);
    for (index, (step, span)) in chart.steps.iter().zip(&schedule.steps).enumerate() {
        let actuator_name = |id: u32| {
            chart
                .actuator(id)
                .map_or_else(|| format!("#{id}"), |a| a.name.clone())
        };
        let slowest = step
            .actions
            .iter()
            .max_by(|a, b| a.duration.total_cmp(&b.duration));
        let motion_time = slowest.map_or(0.0, |a| a.duration.max(0.0));
        let actions: Vec<String> = step
            .actions
            .iter()
            .map(|action| {
                let target = chart.actuator(action.actuator).map_or_else(
                    || action.target.to_string(),
                    |a| a.format_value(action.target),
                );
                format!(
                    "{} → {target} ({}s)",
                    actuator_name(action.actuator),
                    seconds(action.duration)
                )
            })
            .collect();
        csv.row([
            (index + 1).to_string(),
            step.comment.clone(),
            seconds(span.start),
            seconds(span.end),
            seconds(span.end - span.start),
            seconds(motion_time),
            seconds(step.dwell.max(0.0)),
            slowest.map_or_else(String::new, |a| actuator_name(a.actuator)),
            actions.join(" / "),
        ]);
    }
    csv.row([
        "サイクルタイム".to_string(),
        String::new(),
        seconds(0.0),
        seconds(schedule.cycle_time),
        seconds(schedule.cycle_time),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    ]);
    csv.into_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use plantwright_core::motion::chart::Action;
    use plantwright_core::motion::chart::ActuatorKind;
    use plantwright_core::motion::chart::Step;

    #[test]
    fn one_row_per_step_and_a_total() {
        let mut chart = TimingChart::default();
        let cylinder = chart.add_actuator(ActuatorKind::Cylinder);
        let axis = chart.add_actuator(ActuatorKind::LinearAxis);
        chart.actuator_mut(axis).unwrap().name = "横行軸".into();
        chart.steps.push(Step {
            comment: "ワーク取り".into(),
            actions: vec![
                Action {
                    actuator: cylinder,
                    target: 1.0,
                    duration: 0.4,
                },
                Action {
                    actuator: axis,
                    target: 2.0,
                    duration: 1.0,
                },
            ],
            dwell: 0.5,
            ..Default::default()
        });
        chart.steps.push(Step {
            comment: "戻り".into(),
            actions: vec![Action {
                actuator: cylinder,
                target: 0.0,
                duration: 0.4,
            }],
            dwell: 0.0,
            ..Default::default()
        });

        let text = cycle_time_csv(&chart, &chart.schedule());
        let lines: Vec<&str> = text.trim_start_matches('\u{feff}').lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(
            lines[1],
            "1,ワーク取り,0.000,1.500,1.500,1.000,0.500,横行軸,シリンダ 1 → 出 (0.400s) / 横行軸 → 2.00 (1.000s)"
        );
        assert_eq!(
            lines[2],
            "2,戻り,1.500,1.900,0.400,0.400,0.000,シリンダ 1,シリンダ 1 → 戻 (0.400s)"
        );
        assert_eq!(lines[3], "サイクルタイム,,0.000,1.900,1.900,,,,");
    }
}
