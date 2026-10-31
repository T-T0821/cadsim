//! 案の比較: 名前を付けて取っておいた設定 (案) を、乱数の種を変えて何回か回し、並べて比べる
//! (Bevy に依存しない)
//!
//! 案は工程・名簿・回す条件をまるごと写したもの ([`Scenario`])。今の設定も「今の設定」として
//! 一緒に比べられる。故障やばらつきがあると 1 回の結果は偶然に左右されるので、
//! 種を 1 つずつずらして `replications` 回回し、平均といちばん少ない〜多い値を出す。

use super::model::FlowModel;
use super::sim::Simulation;

/// 名前を付けて取っておいた設定
#[derive(Clone, Debug, PartialEq)]
pub struct Scenario {
    pub name: String,
    /// 工程・名簿・回す条件 (案の中の `scenarios` は空)
    pub model: FlowModel,
}

/// 繰り返しの集計: 平均と最小・最大
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub mean: f64,
    pub min: f64,
    pub max: f64,
}

impl Summary {
    /// 値の並びから作る。空なら None。
    pub fn of(values: &[f64]) -> Option<Summary> {
        if values.is_empty() {
            return None;
        }
        Some(Summary {
            mean: values.iter().sum::<f64>() / values.len() as f64,
            min: values.iter().copied().fold(f64::INFINITY, f64::min),
            max: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        })
    }
}

/// 案1つ分の結果
#[derive(Clone, Debug, PartialEq)]
pub struct ScenarioRow {
    pub name: String,
    /// 出勤する作業者の数
    pub headcount: usize,
    /// 集計期間の排出数
    pub output: Summary,
    pub throughput_per_hour: Summary,
    /// 作業者の稼働率の平均 (0〜1)。作業者がいなければ None
    pub worker_utilization: Option<Summary>,
    /// 人が要る工程の人待ちの割合 (0〜1)。人が要る工程がなければ None
    pub no_operator_ratio: Option<Summary>,
    pub wip_average: Summary,
    /// リードタイム (秒)。排出がない回は数えない
    pub lead_time: Option<Summary>,
    /// 1 個あたりの電力量 (kWh)
    pub energy_per_work_kwh: Option<Summary>,
    /// いちばん多くの回でボトルネックになった工程の名前
    pub bottleneck: Option<String>,
}

/// 比べた結果
#[derive(Clone, Debug, PartialEq)]
pub struct ScenarioComparison {
    pub rows: Vec<ScenarioRow>,
    /// 1 案あたり何回回したか
    pub replications: u32,
}

impl ScenarioComparison {
    /// 排出数の平均がいちばん多い案の添字
    pub fn best(&self) -> Option<usize> {
        self.rows
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.output.mean.total_cmp(&b.output.mean))
            .map(|(i, _)| i)
    }
}

/// 繰り返しの回数の上限
pub const MAX_REPLICATIONS: u32 = 100;

/// 案をそれぞれ `replications` 回 (種を 1 つずつずらして) 回して比べる。
/// 回せない案があれば、案の名前を付けた理由を返す。
pub fn compare_scenarios(
    cases: &[(String, &FlowModel)],
    replications: u32,
) -> Result<ScenarioComparison, Vec<String>> {
    let replications = replications.clamp(1, MAX_REPLICATIONS);
    let problems: Vec<String> = cases
        .iter()
        .flat_map(|(name, m)| {
            m.problems()
                .into_iter()
                .map(move |p| format!("{name}: {p}"))
        })
        .collect();
    if !problems.is_empty() {
        return Err(problems);
    }
    let rows = cases
        .iter()
        .map(|(name, model)| run_case(name, model, replications))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ScenarioComparison { rows, replications })
}

fn run_case(name: &str, model: &FlowModel, replications: u32) -> Result<ScenarioRow, Vec<String>> {
    let mut m = model.without_scenarios();
    let seed = m.settings.seed;
    let mut output = Vec::new();
    let mut throughput = Vec::new();
    let mut utilization = Vec::new();
    let mut no_operator = Vec::new();
    let mut wip = Vec::new();
    let mut lead = Vec::new();
    let mut energy = Vec::new();
    let mut bottlenecks: Vec<(String, u32)> = Vec::new();
    for k in 0..replications {
        m.settings.seed = seed.wrapping_add(u64::from(k));
        let r = Simulation::run(&m).map_err(|p| {
            p.into_iter()
                .map(|p| format!("{name}: {p}"))
                .collect::<Vec<_>>()
        })?;
        output.push(f64::from(r.output));
        throughput.push(r.throughput_per_hour);
        utilization.extend(r.worker_utilization());
        no_operator.extend(r.no_operator_ratio(&m));
        wip.push(r.wip_average);
        lead.extend(r.lead_time);
        energy.extend(r.energy_per_work_kwh);
        if let Some(b) = r.bottleneck.and_then(|id| r.element(id)) {
            match bottlenecks.iter_mut().find(|(n, _)| *n == b.name) {
                Some((_, count)) => *count += 1,
                None => bottlenecks.push((b.name.clone(), 1)),
            }
        }
    }
    let always = |v: &[f64]| Summary::of(v).expect("at least one replication");
    Ok(ScenarioRow {
        name: name.to_owned(),
        headcount: m.staffing.present_count(),
        output: always(&output),
        throughput_per_hour: always(&throughput),
        worker_utilization: Summary::of(&utilization),
        no_operator_ratio: Summary::of(&no_operator),
        wip_average: always(&wip),
        lead_time: Summary::of(&lead),
        energy_per_work_kwh: Summary::of(&energy),
        // 同じ回数なら先に出たほう
        bottleneck: bottlenecks
            .iter()
            .rev()
            .max_by_key(|(_, c)| *c)
            .map(|(n, _)| n.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::super::model::{ElementKind, apply_sample_staffing, sample_line};
    use super::*;

    #[test]
    fn summary_of_values() {
        let s = Summary::of(&[1.0, 2.0, 6.0]).unwrap();
        assert_eq!((s.mean, s.min, s.max), (3.0, 1.0, 6.0));
        assert!(Summary::of(&[]).is_none());
    }

    #[test]
    fn replications_vary_with_randomness() {
        // サンプルラインには故障とばらつきがあるので、種を変えると排出数がぶれる
        let m = sample_line();
        let c = compare_scenarios(&[("今".into(), &m)], 5).unwrap();
        let row = &c.rows[0];
        assert_eq!(c.replications, 5);
        assert!(row.output.min < row.output.max, "{row:?}");
        assert!(row.output.min <= row.output.mean && row.output.mean <= row.output.max);
        assert_eq!(row.headcount, 0);
        assert!(row.worker_utilization.is_none());
        // 1 回目は今の種そのもの
        let single = compare_scenarios(&[("今".into(), &m)], 1).unwrap();
        let direct = Simulation::run(&m).unwrap();
        assert_eq!(single.rows[0].output.mean, f64::from(direct.output));
    }

    #[test]
    fn the_better_plan_wins() {
        let base = sample_line();
        let mut faster = base.clone();
        // 組立のサイクルタイムを短くした案
        for e in &mut faster.elements {
            if e.name == "加工"
                && let ElementKind::Process { cycle_time, .. } = &mut e.kind
            {
                *cycle_time = 20.0;
            }
        }
        let c =
            compare_scenarios(&[("今".into(), &base), ("加工を速く".into(), &faster)], 3).unwrap();
        assert_eq!(c.best(), Some(1), "{:?}", c.rows);
        assert_eq!(c.rows[0].bottleneck.as_deref(), Some("加工"));
    }

    #[test]
    fn staffed_plans_report_people() {
        let mut m = sample_line();
        apply_sample_staffing(&mut m);
        let c = compare_scenarios(&[("人あり".into(), &m)], 2).unwrap();
        assert_eq!(c.rows[0].headcount, 3);
        assert!(c.rows[0].worker_utilization.is_some());
        assert!(c.rows[0].no_operator_ratio.is_some());
    }

    #[test]
    fn problems_name_the_plan() {
        let broken = FlowModel::default();
        let err = compare_scenarios(&[("空の案".into(), &broken)], 1).unwrap_err();
        assert!(err.iter().all(|p| p.starts_with("空の案: ")), "{err:?}");
    }
}
