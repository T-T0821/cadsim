//! 人数の比較: 出勤する人数を 1〜N 人に変えて同じ条件で回し、結果を並べる (Bevy に依存しない)
//!
//! 何人目までを出すかは名簿の上から順 ([`Staffing::with_headcount`])。
//! 名簿の並びを変えれば、誰を残すかも変えられる。

use super::model::{AssignRule, Staffing};
use crate::flow::model::FlowModel;
use crate::flow::sim::{Report, Simulation};

/// 人数1通り分の結果
#[derive(Clone, Debug, PartialEq)]
pub struct HeadcountRow {
    pub headcount: usize,
    /// 集計期間の排出数
    pub output: u32,
    pub throughput_per_hour: f64,
    /// 作業者の稼働率の平均 (0〜1)
    pub worker_utilization: f64,
    /// 人が要る工程の人待ちの割合の平均 (0〜1)
    pub no_operator_ratio: f64,
    pub wip_average: f64,
    pub lead_time: Option<f64>,
    /// 人が要るのに、出勤している人の誰もできない工程の名前
    pub uncovered: Vec<String>,
}

/// 比べた結果
#[derive(Clone, Debug, PartialEq)]
pub struct HeadcountComparison {
    pub rows: Vec<HeadcountRow>,
    /// 目標のスループット (個/時)。None なら目標なし。
    pub target_per_hour: Option<f64>,
}

impl HeadcountComparison {
    /// 目標を満たすいちばん少ない人数。目標がない・どの人数でも届かなければ None。
    pub fn smallest_meeting_target(&self) -> Option<usize> {
        let target = self.target_per_hour?;
        self.rows
            .iter()
            .find(|r| r.throughput_per_hour >= target)
            .map(|r| r.headcount)
    }
}

/// 1〜(出勤している人数) 人で回して比べる。回せないモデルなら理由を返す。
pub fn compare_headcount(
    model: &FlowModel,
    target_per_hour: Option<f64>,
) -> Result<HeadcountComparison, Vec<String>> {
    let problems = model.problems();
    if !problems.is_empty() {
        return Err(problems);
    }
    let max = model.staffing.present_count();
    if max == 0 {
        return Err(vec!["出勤する作業者がいません".into()]);
    }
    let rows = (1..=max)
        .map(|headcount| {
            let mut m = model.clone();
            m.staffing = model.staffing.with_headcount(headcount);
            run_row(&m).map(|(row, _)| row)
        })
        .collect::<Result<Vec<_>, Vec<String>>>()?;
    Ok(HeadcountComparison {
        rows,
        target_per_hour,
    })
}

/// 今の名簿のまま回して1行にする
fn run_row(m: &FlowModel) -> Result<(HeadcountRow, Report), Vec<String>> {
    let r = Simulation::run(m)?;
    let uncovered = m
        .elements
        .iter()
        .filter(|e| e.labor.needs_operator())
        .filter(|e| {
            !m.staffing
                .workers
                .iter()
                .any(|w| !w.absent && w.speed_for(e.id).is_some())
        })
        .map(|e| e.name.clone())
        .collect();
    let row = HeadcountRow {
        headcount: m.staffing.present_count(),
        output: r.output,
        throughput_per_hour: r.throughput_per_hour,
        worker_utilization: r.worker_utilization().unwrap_or(0.0),
        no_operator_ratio: r.no_operator_ratio(m).unwrap_or(0.0),
        wip_average: r.wip_average,
        lead_time: r.lead_time,
        uncovered,
    };
    Ok((row, r))
}

/// 割り当てのルール1つ分の結果
#[derive(Clone, Debug, PartialEq)]
pub struct RuleRow {
    pub rule: AssignRule,
    pub row: HeadcountRow,
    /// 作業者の稼働率のいちばん低い人・高い人 (0〜1)。偏りの目安。
    pub utilization_min: f64,
    pub utilization_max: f64,
    /// 全員が別の工程へ歩いた回数の合計
    pub walks: u32,
}

/// 今の名簿・人数のまま、割り当てのルールだけを変えて回して比べる
pub fn compare_rules(model: &FlowModel) -> Result<Vec<RuleRow>, Vec<String>> {
    let problems = model.problems();
    if !problems.is_empty() {
        return Err(problems);
    }
    if model.staffing.present_count() == 0 {
        return Err(vec!["出勤する作業者がいません".into()]);
    }
    AssignRule::ALL
        .into_iter()
        .map(|rule| {
            let mut m = model.clone();
            m.staffing.rule = rule;
            let (row, report) = run_row(&m)?;
            let util = report.workers.iter().map(|w| w.utilization());
            Ok(RuleRow {
                rule,
                row,
                utilization_min: util.clone().fold(f64::INFINITY, f64::min),
                utilization_max: util.fold(0.0, f64::max),
                walks: report.workers.iter().map(|w| w.walks).sum(),
            })
        })
        .collect()
}

/// 名簿の並びで「上から何人」を決めるので、並べ替えを用意する (`up` なら前へ)
pub fn move_worker(staffing: &mut Staffing, id: super::model::WorkerId, up: bool) {
    let Some(i) = staffing.workers.iter().position(|w| w.id == id) else {
        return;
    };
    let j = if up { i.checked_sub(1) } else { Some(i + 1) };
    if let Some(j) = j.filter(|&j| j < staffing.workers.len()) {
        staffing.workers.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::model::{apply_sample_staffing, sample_line};

    #[test]
    fn more_people_never_hurt_and_the_target_is_found() {
        let mut m = sample_line();
        apply_sample_staffing(&mut m);
        let c = compare_headcount(&m, Some(80.0)).unwrap();
        assert_eq!(c.rows.len(), 3);
        // A さん 1 人では検査ができない
        assert_eq!(c.rows[0].uncovered, ["検査"]);
        assert!(c.rows[2].uncovered.is_empty());
        // 人が増えるほど人待ちは減り、排出は減らない
        for pair in c.rows.windows(2) {
            assert!(pair[1].output + 2 >= pair[0].output, "{pair:?}");
            assert!(pair[1].no_operator_ratio <= pair[0].no_operator_ratio + 0.02);
        }
        assert!(c.rows[0].no_operator_ratio > c.rows[2].no_operator_ratio);
        let need = c.smallest_meeting_target();
        if let Some(n) = need {
            assert!(c.rows[n - 1].throughput_per_hour >= 80.0);
        }
    }

    #[test]
    fn rules_are_compared_with_the_same_people() {
        let mut m = sample_line();
        apply_sample_staffing(&mut m);
        let rows = compare_rules(&m).unwrap();
        assert_eq!(rows.len(), 3);
        for r in &rows {
            assert_eq!(r.row.headcount, 3);
            assert!(r.utilization_min <= r.utilization_max);
        }
        // 持ち場の人が先なら、速い人ルールより歩く回数が少ない
        let walks = |rule| rows.iter().find(|r| r.rule == rule).unwrap().walks;
        assert!(walks(AssignRule::Primary) < walks(AssignRule::Fastest));
    }

    #[test]
    fn nobody_present_is_an_error() {
        let mut m = sample_line();
        apply_sample_staffing(&mut m);
        for w in &mut m.staffing.workers {
            w.absent = true;
        }
        assert!(compare_headcount(&m, None).is_err());
    }

    #[test]
    fn move_worker_reorders_the_roster() {
        let mut s = Staffing::default();
        let a = s.add_worker();
        let b = s.add_worker();
        move_worker(&mut s, b, true);
        assert_eq!(s.workers[0].id, b);
        assert_eq!(s.workers[1].id, a);
    }
}
