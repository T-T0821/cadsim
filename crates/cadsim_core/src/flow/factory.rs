//! 工場: ラインのまとまりと、ラインごとの集計 (Bevy に依存しない)
//!
//! 工場は1つの [`FlowModel`] で表す。要素に [`Element::line`] を付けてラインにまとめ、
//! ライン間は「搬送」(フォークリフト・手運び) やバッファ (倉庫・部品置き場) でつなぐ。
//! 作業者の名簿は工場で1つなので、持ち場のラインの手が空いた人は別のラインを応援できる。
//!
//! 別の .cadsim の工程を、1本のラインとして取り込める ([`FlowModel::import_line`])。

use std::collections::HashMap;

use super::model::{
    Element, ElementId, ElementKind, Energy, Failure, FlowModel, KindTag, Labor, ProductId,
    ProductTime,
};
use super::sim::{Report, State};
use crate::staffing::model::{AssignRule, Break, Shift, Skill, Staffing, Worker};

/// ラインの ID。工場の中で一意。
pub type LineId = u32;

/// ライン1本
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub id: LineId,
    pub name: String,
}

impl FlowModel {
    /// ラインを足す。名前が空なら「ライン1」のように付ける。
    pub fn add_line(&mut self, name: &str) -> LineId {
        let id = self.lines.iter().map(|l| l.id).max().unwrap_or(0) + 1;
        let name = name.trim();
        let name = if name.is_empty() {
            format!("ライン{}", self.lines.len() + 1)
        } else {
            name.to_owned()
        };
        self.lines.push(Line { id, name });
        id
    }

    pub fn line(&self, id: LineId) -> Option<&Line> {
        self.lines.iter().find(|l| l.id == id)
    }

    /// ラインを消す。要素は消さずに、どのラインにも入っていない状態にする。
    pub fn remove_line(&mut self, id: LineId) {
        self.lines.retain(|l| l.id != id);
        for e in &mut self.elements {
            if e.line == Some(id) {
                e.line = None;
            }
        }
    }

    /// 要素が属するラインの名前
    pub fn line_name(&self, element: &Element) -> Option<&str> {
        element
            .line
            .and_then(|l| self.line(l))
            .map(|l| l.name.as_str())
    }

    /// どのラインにも入っていない要素を、新しいラインにまとめる。要素がなければ None。
    pub fn group_unassigned(&mut self, name: &str) -> Option<LineId> {
        if !self.elements.iter().any(|e| e.line.is_none()) {
            return None;
        }
        let line = self.add_line(name);
        for e in &mut self.elements {
            if e.line.is_none() {
                e.line = Some(line);
            }
        }
        Some(line)
    }

    /// 別の工程モデルを、1本のラインとして末尾に取り込む。ID は振り直し、送り先もつなぎ直す。
    ///
    /// 3D の部品・AGV の搬送・タイミングチャートの対応は別のファイルのものなので外す。
    /// 品種は名前で突き合わせ、ない品種は足す (注文は取り込まない)。
    /// `with_staff` なら名簿も取り込み (スキル・持ち場は新しい ID に直す)、
    /// 同じ中身のシフトは今あるものを使う。
    /// 今の工程にラインがまだなければ、先に今の要素を「ライン1」にまとめる。
    pub fn import_line(&mut self, other: &FlowModel, name: &str, with_staff: bool) -> LineId {
        if self.lines.is_empty() {
            self.group_unassigned("");
        }
        let line = self.add_line(name);
        // 品種は名前で突き合わせ、なければ足す
        let mut products = HashMap::new();
        for p in &other.products {
            let id = match self.products.iter().find(|q| q.name == p.name) {
                Some(q) => q.id,
                None => self.add_product(&p.name),
            };
            products.insert(p.id, id);
        }
        let product = |p: &ProductId| products.get(p).copied();
        let mut ids: HashMap<ElementId, ElementId> = HashMap::new();
        for e in &other.elements {
            let id = self.insert(Element {
                next: Vec::new(),
                part: None,
                carrier: None,
                starts_chart: false,
                line: Some(line),
                product: e.product.as_ref().and_then(product),
                product_times: e
                    .product_times
                    .iter()
                    .filter_map(|t| {
                        Some(ProductTime {
                            product: product(&t.product)?,
                            cycle_time: t.cycle_time,
                        })
                    })
                    .collect(),
                accepts: e.accepts.iter().filter_map(product).collect(),
                ..e.clone()
            });
            ids.insert(e.id, id);
        }
        for e in &other.elements {
            let next = e.next.iter().filter_map(|n| ids.get(n).copied()).collect();
            if let Some(new) = self.get_mut(ids[&e.id]) {
                new.next = next;
            }
        }
        if with_staff {
            let mut shifts = HashMap::new();
            for s in &other.staffing.shifts {
                let same = self.staffing.shifts.iter().find(|t| {
                    t.name == s.name && t.start == s.start && t.end == s.end && t.breaks == s.breaks
                });
                let id = match same {
                    Some(t) => t.id,
                    None => self.staffing.insert_shift(s.clone()),
                };
                shifts.insert(s.id, id);
            }
            for w in &other.staffing.workers {
                self.staffing.insert_worker(Worker {
                    shift: w.shift.and_then(|s| shifts.get(&s).copied()),
                    skills: w
                        .skills
                        .iter()
                        .filter_map(|k| {
                            Some(Skill {
                                element: *ids.get(&k.element)?,
                                speed: k.speed,
                            })
                        })
                        .collect(),
                    primary: w.primary.and_then(|p| ids.get(&p).copied()),
                    ..w.clone()
                });
            }
        }
        line
    }
}

/// ライン1本分の集計
#[derive(Clone, Debug, PartialEq)]
pub struct LineSummary {
    /// None は「どのラインにも入れていない要素」(ライン間の搬送・倉庫など)
    pub line: Option<LineId>,
    pub name: String,
    /// ラインから出ていった数 (排出に入った数と、ラインの外へ送った数)
    pub output: u32,
    pub throughput_per_hour: f64,
    /// ラインの中でいちばん忙しい工程 (稼働 + 故障)
    pub bottleneck: Option<ElementId>,
    /// ボトルネックの稼働率 (0〜1)
    pub bottleneck_busy: f64,
    /// ラインの中のワーク数の時間平均
    pub wip_average: f64,
    /// 人が要る工程の人待ちの割合の平均 (0〜1)。人が要る工程がなければ None
    pub no_operator_ratio: Option<f64>,
}

/// ラインごとの集計。ラインの並び順で、最後にどのラインにも入っていない要素 (あれば)。
pub fn line_summaries(model: &FlowModel, report: &Report) -> Vec<LineSummary> {
    let mut groups: Vec<(Option<LineId>, String)> = model
        .lines
        .iter()
        .map(|l| (Some(l.id), l.name.clone()))
        .collect();
    if model.elements.iter().any(|e| e.line.is_none()) {
        groups.push((None, "ライン間 (どのラインにも入れていない要素)".into()));
    }
    groups
        .into_iter()
        .map(|(line, name)| {
            let members: Vec<&Element> = model.elements.iter().filter(|e| e.line == line).collect();
            let stats = |e: &Element| report.element(e.id);
            let output = members
                .iter()
                .filter(|e| {
                    matches!(e.kind, ElementKind::Sink)
                        || e.next
                            .iter()
                            .any(|n| model.get(*n).is_none_or(|t| t.line != line))
                })
                .filter_map(|e| stats(e))
                .map(|s| s.count)
                .sum();
            let busiest = members
                .iter()
                .filter(|e| e.kind.tag() == KindTag::Process)
                .filter_map(|e| stats(e))
                .filter(|s| s.busy_ratio() > 0.0)
                .max_by(|a, b| a.busy_ratio().total_cmp(&b.busy_ratio()));
            let waits: Vec<f64> = members
                .iter()
                .filter(|e| e.labor.needs_operator())
                .filter_map(|e| stats(e))
                .map(|s| s.times.ratio(State::NoOperator))
                .collect();
            LineSummary {
                line,
                name,
                output,
                throughput_per_hour: if report.measured > 0.0 {
                    f64::from(output) * 3600.0 / report.measured
                } else {
                    0.0
                },
                bottleneck: busiest.map(|s| s.id),
                bottleneck_busy: busiest.map_or(0.0, |s| s.busy_ratio()),
                wip_average: members
                    .iter()
                    .filter_map(|e| stats(e))
                    .map(|s| s.content_average)
                    .sum(),
                no_operator_ratio: (!waits.is_empty())
                    .then(|| waits.iter().sum::<f64>() / waits.len() as f64),
            }
        })
        .collect()
}

/// 動作確認用の工場: 加工ライン → フォークリフト → 組立ライン。
/// 組立は 2 台並べ、2 台目には持ち場の人を置かない。日勤 4 人がそれぞれ持ち場を持ち、
/// 手が空けば 2 台目の組立 (加工ラインの人にとっては別のラインの応援) に入る。
pub fn sample_factory() -> FlowModel {
    const H: f64 = 3600.0;
    let mut m = FlowModel::default();
    let machining = m.add_line("加工ライン");
    let assembly = m.add_line("組立ライン");
    let add = |m: &mut FlowModel, line, name: &str, kind, labor, energy| {
        let id = m.add(KindTag::Sink);
        let e = m.get_mut(id).unwrap();
        e.name = name.into();
        e.kind = kind;
        e.labor = labor;
        e.energy = energy;
        e.line = line;
        id
    };
    let process = |cycle_time, spread, failure| ElementKind::Process {
        cycle_time,
        spread,
        failure,
    };
    let kw = |rated_kw, standby_kw| Energy {
        rated_kw,
        standby_kw,
        air_l_per_min: 0.0,
    };
    let none = Energy::default();
    let l1 = Some(machining);
    let l2 = Some(assembly);
    let ids = [
        add(
            &mut m,
            l1,
            "材料",
            ElementKind::Source {
                interval: 30.0,
                spread: 0.1,
                limit: None,
            },
            Labor::None,
            none,
        ),
        add(
            &mut m,
            l1,
            "切削",
            process(
                28.0,
                0.05,
                Some(Failure {
                    mtbf: 2.0 * H,
                    mttr: 600.0,
                }),
            ),
            Labor::Load { seconds: 6.0 },
            kw(11.0, 2.0),
        ),
        add(
            &mut m,
            l1,
            "洗浄",
            process(20.0, 0.0, None),
            Labor::None,
            kw(4.0, 1.0),
        ),
        add(
            &mut m,
            l1,
            "出口バッファ",
            ElementKind::Buffer { capacity: 10 },
            Labor::None,
            none,
        ),
        add(
            &mut m,
            None,
            "フォークリフト",
            ElementKind::Transport {
                travel_time: 120.0,
                capacity: 5,
            },
            Labor::None,
            none,
        ),
        add(
            &mut m,
            l2,
            "部品倉庫",
            ElementKind::Buffer { capacity: 200 },
            Labor::None,
            none,
        ),
        add(
            &mut m,
            l2,
            "組立1",
            process(55.0, 0.1, None),
            Labor::Full,
            kw(1.5, 0.3),
        ),
        add(
            &mut m,
            l2,
            "組立2",
            process(55.0, 0.1, None),
            Labor::Full,
            kw(1.5, 0.3),
        ),
        add(
            &mut m,
            l2,
            "検査",
            process(15.0, 0.0, None),
            Labor::Full,
            kw(0.8, 0.3),
        ),
        add(
            &mut m,
            l2,
            "梱包",
            process(20.0, 0.1, None),
            Labor::Full,
            none,
        ),
        add(&mut m, l2, "出荷", ElementKind::Sink, Labor::None, none),
    ];
    let [
        material,
        cutting,
        washing,
        exit,
        forklift,
        store,
        assemble1,
        assemble2,
        inspect,
        pack,
        ship,
    ] = ids;
    for (from, to) in [
        (material, vec![cutting]),
        (cutting, vec![washing]),
        (washing, vec![exit]),
        (exit, vec![forklift]),
        (forklift, vec![store]),
        // 組立は 2 台並べて、空いている方へ送る
        (store, vec![assemble1, assemble2]),
        (assemble1, vec![inspect]),
        (assemble2, vec![inspect]),
        (inspect, vec![pack]),
        (pack, vec![ship]),
    ] {
        m.get_mut(from).unwrap().next = to;
    }
    let mut staffing = Staffing::default();
    staffing.walk_seconds = 15.0;
    staffing.rule = AssignRule::Primary;
    let day = staffing.insert_shift(Shift {
        id: 0,
        name: "日勤".into(),
        start: 8.0 * H,
        end: 17.0 * H,
        breaks: vec![
            Break {
                start: 10.0 * H,
                end: 10.0 * H + 600.0,
            },
            Break {
                start: 12.0 * H,
                end: 13.0 * H,
            },
            Break {
                start: 15.0 * H,
                end: 15.0 * H + 600.0,
            },
        ],
    });
    let skill = |element, speed| Skill { element, speed };
    for (name, skills, primary) in [
        (
            "Aさん",
            vec![skill(cutting, 1.0), skill(assemble2, 0.9)],
            cutting,
        ),
        (
            "Bさん",
            vec![skill(assemble1, 1.0), skill(assemble2, 1.0)],
            assemble1,
        ),
        (
            "Cさん",
            vec![skill(inspect, 1.1), skill(pack, 1.0)],
            inspect,
        ),
        ("Dさん", vec![skill(pack, 1.0), skill(assemble2, 1.0)], pack),
    ] {
        staffing.insert_worker(Worker {
            id: 0,
            name: name.into(),
            shift: Some(day),
            absent: false,
            skills,
            primary: Some(primary),
        });
    }
    m.staffing = staffing;
    m
}

#[cfg(test)]
mod tests {
    use super::super::model::{apply_sample_staffing, sample_line};
    use super::super::sim::Simulation;
    use super::*;

    #[test]
    fn the_sample_factory_runs_and_reports_each_line() {
        let m = sample_factory();
        assert!(m.problems().is_empty(), "{:?}", m.problems());
        let r = Simulation::run(&m).unwrap();
        let lines = line_summaries(&m, &r);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].name, "加工ライン");
        assert_eq!(lines[1].name, "組立ライン");
        assert_eq!(lines[2].line, None);
        // 加工ラインから出た数 = フォークリフトが運んだ数 + 運んでいる途中 ≥ 組立ラインの出荷
        assert!(lines[0].output >= lines[2].output);
        assert!(lines[2].output >= lines[1].output);
        assert_eq!(lines[1].output, r.output);
        assert!(r.output > 100, "{}", r.output);
        // A さんは切削の合間に組立を応援する
        let a = r.workers.iter().find(|w| w.name == "Aさん").unwrap();
        assert!(a.support_tasks > 0, "{a:?}");
        let c = r.workers.iter().find(|w| w.name == "Cさん").unwrap();
        assert!(c.support_tasks <= c.tasks);
    }

    #[test]
    fn importing_a_line_renumbers_and_keeps_its_staff() {
        let mut factory = sample_line();
        let mut other = sample_line();
        apply_sample_staffing(&mut other);
        let shifts_before = other.staffing.shifts.len();
        let line = factory.import_line(&other, "第2ライン", true);
        // 今の要素は先に「ライン1」にまとまる
        assert_eq!(factory.lines.len(), 2);
        assert_eq!(factory.lines[0].name, "ライン1");
        assert_eq!(factory.line(line).unwrap().name, "第2ライン");
        assert_eq!(factory.elements.len(), 12);
        let imported: Vec<&Element> = factory
            .elements
            .iter()
            .filter(|e| e.line == Some(line))
            .collect();
        assert_eq!(imported.len(), 6);
        // 送り先は取り込んだライン の中だけを指す
        for e in &imported {
            for n in &e.next {
                assert_eq!(factory.get(*n).unwrap().line, Some(line));
            }
        }
        assert_eq!(factory.staffing.workers.len(), 3);
        assert_eq!(factory.staffing.shifts.len(), shifts_before);
        let assembly = imported.iter().find(|e| e.name == "組立").unwrap().id;
        let b = &factory.staffing.workers[1];
        assert_eq!(b.primary, Some(assembly));
        assert!(factory.problems().is_empty(), "{:?}", factory.problems());
        // 同じ名簿をもう一度取り込んでも、同じ中身のシフトは増えない
        factory.import_line(&other, "", true);
        assert_eq!(factory.staffing.shifts.len(), shifts_before);
        assert_eq!(factory.lines[2].name, "ライン3");
        let r = Simulation::run(&factory).unwrap();
        let lines = line_summaries(&factory, &r);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines.iter().map(|l| l.output).sum::<u32>(), r.output);
    }

    #[test]
    fn transport_limits_how_many_are_on_the_way() {
        // 生成 10 秒ごと → 搬送 100 秒・同時に 2 個 → 排出。運べるのは 100 秒に 2 個まで
        let mut m = FlowModel::default();
        let s = m.append(KindTag::Source);
        if let ElementKind::Source { interval, .. } = &mut m.get_mut(s).unwrap().kind {
            *interval = 10.0;
        }
        let t = m.append(KindTag::Transport);
        m.get_mut(t).unwrap().kind = ElementKind::Transport {
            travel_time: 100.0,
            capacity: 2,
        };
        m.append(KindTag::Sink);
        m.settings.duration = 3600.0;
        let r = Simulation::run(&m).unwrap();
        // 100 秒ごとに 2 個届く (3600 秒ちょうどに届く 1 個も数える)
        assert_eq!(r.output, 71);
        assert!(r.lead_time.unwrap() >= 100.0);
        let stats = r.element(t).unwrap();
        assert!(stats.times.ratio(State::Working) > 0.99);
        assert_eq!(stats.content_max, 2);
    }

    #[test]
    fn removing_a_line_keeps_its_elements() {
        let mut m = sample_factory();
        let first = m.lines[0].id;
        m.remove_line(first);
        assert_eq!(m.lines.len(), 1);
        assert_eq!(m.elements.len(), 11);
        assert!(m.elements.iter().filter(|e| e.line.is_none()).count() >= 5);
        assert!(m.problems().is_empty());
    }
}
