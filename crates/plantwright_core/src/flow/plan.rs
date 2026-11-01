//! 品種と生産計画 (Bevy に依存しない)
//!
//! - **品種** ([`Product`]) ごとに、工程の加工時間 ([`Element::product_times`]) と通る工程
//!   ([`Element::accepts`]) を変えられる。品種が変わると工程は段取り替え
//!   ([`Element::setup_time`]) をしてから加工する。人が要る工程なら段取り替えも人の作業になる。
//! - **生産計画** は注文 ([`Order`]: 品種・数量・納期) の並び。注文のある生成は、並び順に
//!   注文の数だけワークを出して止まる。排出に入った数を注文ごとに数え、納期に間に合ったかを出す。
//!
//! 表 (Excel・CSV の行) から計画を作る [`plan_from_rows`] もここに置く。ファイルを読んで
//! 文字の表にするのはアダプター (`plantwright_core`) の仕事。

use super::model::{Element, ElementId, FlowModel};

/// 品種の ID。工場の中で一意。
pub type ProductId = u32;

/// 品種
#[derive(Clone, Debug, PartialEq)]
pub struct Product {
    pub id: ProductId,
    pub name: String,
}

/// 品種ごとの加工時間 (工程の既定のサイクルタイムの代わり)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProductTime {
    pub product: ProductId,
    /// 1個あたりの加工時間 (秒)
    pub cycle_time: f64,
}

/// 注文1件
#[derive(Clone, Debug, PartialEq)]
pub struct Order {
    pub name: String,
    pub product: ProductId,
    pub quantity: u32,
    /// 納期 (シミュレーションの開始からの秒)
    pub due: f64,
    /// 投入する生成。None ならいちばん上の生成
    pub source: Option<ElementId>,
}

impl FlowModel {
    /// 品種を足す。名前が空なら「品種1」のように付ける。
    pub fn add_product(&mut self, name: &str) -> ProductId {
        let id = self.products.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        let name = name.trim();
        let name = if name.is_empty() {
            format!("品種{}", self.products.len() + 1)
        } else {
            name.to_owned()
        };
        self.products.push(Product { id, name });
        id
    }

    pub fn product(&self, id: ProductId) -> Option<&Product> {
        self.products.iter().find(|p| p.id == id)
    }

    pub fn product_name(&self, id: Option<ProductId>) -> &str {
        id.and_then(|id| self.product(id))
            .map_or("品種なし", |p| p.name.as_str())
    }

    /// 品種を消す。その品種の加工時間・通る工程・注文も消す。
    pub fn remove_product(&mut self, id: ProductId) {
        self.products.retain(|p| p.id != id);
        for e in &mut self.elements {
            if e.product == Some(id) {
                e.product = None;
            }
            e.product_times.retain(|t| t.product != id);
            e.accepts.retain(|&p| p != id);
        }
        self.orders.retain(|o| o.product != id);
    }

    /// 注文を投入する生成 (要素の ID)。生成がなければ None。
    pub fn order_source(&self, order: &Order) -> Option<ElementId> {
        let is_source = |e: &&Element| e.kind.tag() == super::model::KindTag::Source;
        order
            .source
            .filter(|id| self.get(*id).is_some_and(|e| is_source(&e)))
            .or_else(|| self.elements.iter().find(is_source).map(|e| e.id))
    }

    /// 計画の数量の合計
    pub fn planned_quantity(&self) -> u32 {
        self.orders.iter().map(|o| o.quantity).sum()
    }

    pub(crate) fn plan_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (k, o) in self.orders.iter().enumerate() {
            let name = if o.name.is_empty() {
                format!("注文{}", k + 1)
            } else {
                o.name.clone()
            };
            if self.product(o.product).is_none() {
                out.push(format!("{name}: 品種がありません"));
            }
            if !(o.due.is_finite() && o.due >= 0.0) {
                out.push(format!("{name}: 納期は 0 時間以上にしてください"));
            }
        }
        for e in &self.elements {
            if !(e.setup_time.is_finite() && e.setup_time >= 0.0) {
                out.push(format!(
                    "{}: 段取り替えの時間は 0 秒以上にしてください",
                    e.name
                ));
            }
            for t in &e.product_times {
                if t.cycle_time.is_nan() || t.cycle_time < super::model::MIN_TIME {
                    out.push(format!(
                        "{}: {}の加工時間は {} 秒以上にしてください",
                        e.name,
                        self.product_name(Some(t.product)),
                        super::model::MIN_TIME
                    ));
                }
            }
        }
        out
    }
}

impl Element {
    /// この品種の加工時間。品種ごとの時間がなければ `base`
    pub fn cycle_for(&self, product: Option<ProductId>, base: f64) -> f64 {
        product
            .and_then(|p| self.product_times.iter().find(|t| t.product == p))
            .map_or(base, |t| t.cycle_time)
    }

    /// この品種を受け取るか (通す品種を決めていなければ何でも受け取る)
    pub fn accepts_product(&self, product: Option<ProductId>) -> bool {
        self.accepts.is_empty() || product.is_some_and(|p| self.accepts.contains(&p))
    }
}

/// 注文の結果
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderStatus {
    /// 納期までにすべて排出した
    OnTime,
    /// 納期を過ぎてから排出し終えた、または納期を過ぎてもまだ終わっていない
    Late,
    /// まだ終わっていないが、納期はまだ先 (回す時間が納期より短い)
    Open,
}

impl OrderStatus {
    pub fn label(self) -> &'static str {
        match self {
            OrderStatus::OnTime => "間に合った",
            OrderStatus::Late => "遅れ",
            OrderStatus::Open => "未完了 (納期前)",
        }
    }
}

/// 注文1件分の集計
#[derive(Clone, Debug, PartialEq)]
pub struct OrderStats {
    pub name: String,
    pub product: ProductId,
    pub quantity: u32,
    /// 投入した数
    pub released: u32,
    /// 排出した数
    pub done: u32,
    /// 最後の1個を排出した時刻 (秒)
    pub finished: Option<f64>,
    pub due: f64,
    pub status: OrderStatus,
}

impl OrderStats {
    /// 遅れ (秒)。間に合ったか、まだ納期前なら None
    pub fn lateness(&self, now: f64) -> Option<f64> {
        (self.status == OrderStatus::Late).then(|| self.finished.unwrap_or(now) - self.due)
    }
}

/// 納期達成率 (納期の来た注文のうち、間に合った割合)。納期の来た注文がなければ None
pub fn on_time_ratio(orders: &[OrderStats]) -> Option<f64> {
    let judged: Vec<&OrderStats> = orders
        .iter()
        .filter(|o| o.status != OrderStatus::Open)
        .collect();
    (!judged.is_empty()).then(|| {
        judged
            .iter()
            .filter(|o| o.status == OrderStatus::OnTime)
            .count() as f64
            / judged.len() as f64
    })
}

/// 表の行から注文を作る。1 行目は見出しで、「品種」「数量」「納期」の列を探す
/// (「注文」「名前」の列があれば注文の名前、「生成」「投入」の列があれば投入する生成の名前)。
/// 納期は開始からの時間 (数)。知らない品種は足す。読めなかった行は理由を返す。
pub fn plan_from_rows(model: &mut FlowModel, rows: &[Vec<String>]) -> (Vec<Order>, Vec<String>) {
    let mut warnings = Vec::new();
    let Some((header, body)) = rows.split_first() else {
        return (Vec::new(), vec!["表が空です".into()]);
    };
    let find = |keys: &[&str]| {
        header
            .iter()
            .position(|h| keys.iter().any(|k| h.trim().contains(k)))
    };
    let (Some(product_col), Some(quantity_col), Some(due_col)) = (
        find(&["品種", "品番", "製品"]),
        find(&["数量", "個数"]),
        find(&["納期"]),
    ) else {
        return (
            Vec::new(),
            vec!["1 行目に「品種」「数量」「納期」の見出しが要ります".into()],
        );
    };
    let name_col = find(&["注文", "名前", "オーダ"]);
    let source_col = find(&["生成", "投入"]);
    let mut orders = Vec::new();
    for (k, row) in body.iter().enumerate() {
        let line = k + 2;
        let cell = |c: usize| row.get(c).map_or("", |s| s.trim());
        if row.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        let product_name = cell(product_col);
        if product_name.is_empty() {
            warnings.push(format!("{line} 行目: 品種が空です"));
            continue;
        }
        let Ok(quantity) = cell(quantity_col).replace(',', "").parse::<f64>() else {
            warnings.push(format!(
                "{line} 行目: 数量「{}」が読めません",
                cell(quantity_col)
            ));
            continue;
        };
        let Ok(due) = cell(due_col).parse::<f64>() else {
            warnings.push(format!(
                "{line} 行目: 納期「{}」が読めません",
                cell(due_col)
            ));
            continue;
        };
        if !(quantity >= 0.0 && quantity <= f64::from(u32::MAX) && due >= 0.0) {
            warnings.push(format!("{line} 行目: 数量と納期は 0 以上にしてください"));
            continue;
        }
        let product = match model.products.iter().find(|p| p.name == product_name) {
            Some(p) => p.id,
            None => model.add_product(product_name),
        };
        let source = source_col.and_then(|c| {
            let name = cell(c);
            if name.is_empty() {
                return None;
            }
            let found = model
                .elements
                .iter()
                .find(|e| e.name == name && e.kind.tag() == super::model::KindTag::Source)
                .map(|e| e.id);
            if found.is_none() {
                warnings.push(format!(
                    "{line} 行目: 生成「{name}」がないので、いちばん上の生成から投入します"
                ));
            }
            found
        });
        orders.push(Order {
            name: name_col
                .map(|c| cell(c).to_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("注文{}", orders.len() + 1)),
            product,
            quantity: quantity.round() as u32,
            due: due * 3600.0,
            source,
        });
    }
    (orders, warnings)
}

/// サンプル工場に品種と計画を入れる: 標準品と特注品、切削の段取り替え 10 分、注文 3 件
/// (最後の注文は少し遅れる)。
/// 工程の名前で探すので、[`super::factory::sample_factory`] のあとに呼ぶ。
pub fn apply_sample_plan(m: &mut FlowModel) {
    const H: f64 = 3600.0;
    let find = |m: &FlowModel, name: &str| m.elements.iter().find(|e| e.name == name).map(|e| e.id);
    let standard = m.add_product("標準品");
    let custom = m.add_product("特注品");
    for (name, custom_time, setup) in [
        ("切削", 40.0, 600.0),
        ("組立1", 70.0, 0.0),
        ("組立2", 70.0, 0.0),
    ] {
        if let Some(id) = find(m, name) {
            let e = m.get_mut(id).unwrap();
            e.product_times = vec![ProductTime {
                product: custom,
                cycle_time: custom_time,
            }];
            e.setup_time = setup;
        }
    }
    m.orders = vec![
        Order {
            name: "No.101".into(),
            product: standard,
            quantity: 150,
            due: 2.5 * H,
            source: None,
        },
        Order {
            name: "No.102".into(),
            product: custom,
            quantity: 80,
            due: 4.5 * H,
            source: None,
        },
        Order {
            name: "No.103".into(),
            product: standard,
            quantity: 200,
            due: 7.0 * H,
            source: None,
        },
    ];
}

#[cfg(test)]
mod tests {
    use super::super::factory::sample_factory;
    use super::super::model::{ElementKind, KindTag};
    use super::super::sim::Simulation;
    use super::*;

    fn rows(text: &str) -> Vec<Vec<String>> {
        text.lines()
            .map(|l| l.split('\t').map(str::to_owned).collect())
            .collect()
    }

    /// 生成 (1 秒ごと) → 工程 → 排出。工程は 10 秒、品種 B は 20 秒、段取り替え 30 秒
    fn line_with_products() -> (FlowModel, ProductId, ProductId) {
        let mut m = FlowModel::default();
        let s = m.append(KindTag::Source);
        if let ElementKind::Source { interval, .. } = &mut m.get_mut(s).unwrap().kind {
            *interval = 1.0;
        }
        let p = m.append(KindTag::Process);
        if let ElementKind::Process { cycle_time, .. } = &mut m.get_mut(p).unwrap().kind {
            *cycle_time = 10.0;
        }
        m.append(KindTag::Sink);
        let a = m.add_product("A");
        let b = m.add_product("B");
        let e = m.get_mut(p).unwrap();
        e.product_times = vec![ProductTime {
            product: b,
            cycle_time: 20.0,
        }];
        e.setup_time = 30.0;
        (m, a, b)
    }

    #[test]
    fn orders_are_released_in_turn_with_setups_between_products() {
        let (mut m, a, b) = line_with_products();
        let order = |name: &str, product, quantity, due_h: f64| Order {
            name: name.into(),
            product,
            quantity,
            due: due_h * 3600.0,
            source: None,
        };
        m.orders = vec![
            order("1", a, 3, 1.0),
            order("2", b, 2, 0.01),
            order("3", a, 1, 1.0),
        ];
        m.settings.duration = 3600.0;
        let r = Simulation::run(&m).unwrap();
        // A 3 個 (30 秒) → 段取り 30 → B 2 個 (40 秒) → 段取り 30 → A 1 個 (10 秒) で 140 秒
        assert_eq!(r.output, 6);
        let p = &r.elements[1];
        assert_eq!(p.setups, 2);
        assert!((p.setup_seconds - 60.0).abs() < 1e-9);
        assert_eq!(r.orders.len(), 3);
        assert_eq!(r.orders[0].done, 3);
        assert_eq!(r.orders[0].status, OrderStatus::OnTime);
        // 2 番目は納期 36 秒で、終わるのは 100 秒過ぎ
        assert_eq!(r.orders[1].status, OrderStatus::Late);
        assert!(r.orders[1].lateness(r.time).unwrap() > 60.0);
        let last = r.orders[2].finished.unwrap();
        assert!((last - 140.0).abs() < 1e-6, "{last}");
        assert_eq!(on_time_ratio(&r.orders), Some(2.0 / 3.0));
        assert_eq!(r.product_output, vec![(Some(a), 4), (Some(b), 2)]);
    }

    #[test]
    fn unfinished_orders_are_late_only_after_their_due() {
        let (mut m, a, _) = line_with_products();
        m.orders = vec![Order {
            name: "大口".into(),
            product: a,
            quantity: 1000,
            due: 100.0 * 3600.0,
            source: None,
        }];
        m.settings.duration = 600.0;
        let r = Simulation::run(&m).unwrap();
        assert_eq!(r.orders[0].status, OrderStatus::Open);
        assert_eq!(on_time_ratio(&r.orders), None);
        m.orders[0].due = 300.0;
        let r = Simulation::run(&m).unwrap();
        assert_eq!(r.orders[0].status, OrderStatus::Late);
    }

    #[test]
    fn products_take_their_own_route() {
        // 生成 → [塗装 (B だけ), 素通し] → 排出。A は塗装を通らない
        let (mut m, a, b) = line_with_products();
        let source = m.elements[0].id;
        let process = m.elements[1].id;
        let sink = m.elements[2].id;
        let paint = m.add(KindTag::Process);
        m.get_mut(paint).unwrap().accepts = vec![b];
        m.get_mut(paint).unwrap().next = vec![sink];
        m.get_mut(process).unwrap().accepts = vec![a];
        m.get_mut(source).unwrap().next = vec![paint, process];
        m.orders = vec![
            Order {
                name: "A".into(),
                product: a,
                quantity: 3,
                due: 3600.0,
                source: None,
            },
            Order {
                name: "B".into(),
                product: b,
                quantity: 2,
                due: 3600.0,
                source: None,
            },
        ];
        let r = Simulation::run(&m).unwrap();
        assert_eq!(r.output, 5);
        assert_eq!(r.element(paint).unwrap().count, 2);
        assert_eq!(r.element(process).unwrap().count, 3);
    }

    #[test]
    fn a_source_without_orders_makes_its_own_product() {
        let (mut m, _, b) = line_with_products();
        let source = m.elements[0].id;
        m.get_mut(source).unwrap().product = Some(b);
        m.settings.duration = 200.0;
        let r = Simulation::run(&m).unwrap();
        assert_eq!(r.product_output, vec![(Some(b), r.output)]);
        assert_eq!(r.elements[1].setups, 0);
    }

    #[test]
    fn rows_become_orders_and_new_products() {
        let (mut m, a, _) = line_with_products();
        let text = "注文番号\t品種\t数量\t納期 (時間)\n101\tA\t1,200\t8\n102\tC\t50\t4.5\n\n103\t\t1\t1\n104\tA\tたくさん\t1";
        let (orders, warnings) = plan_from_rows(&mut m, &rows(text));
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0].name, "101");
        assert_eq!(orders[0].product, a);
        assert_eq!(orders[0].quantity, 1200);
        assert_eq!(orders[1].due, 4.5 * 3600.0);
        assert_eq!(m.product_name(Some(orders[1].product)), "C");
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].starts_with("5 行目"));
        let (none, warnings) = plan_from_rows(&mut m, &rows("a\tb\n1\t2"));
        assert!(none.is_empty());
        assert!(warnings[0].contains("見出し"));
    }

    #[test]
    fn removing_a_product_cleans_up() {
        let (mut m, a, b) = line_with_products();
        m.orders = vec![Order {
            name: "x".into(),
            product: b,
            quantity: 1,
            due: 1.0,
            source: None,
        }];
        m.elements[1].accepts = vec![a, b];
        m.remove_product(b);
        assert!(m.orders.is_empty());
        assert!(m.elements[1].product_times.is_empty());
        assert_eq!(m.elements[1].accepts, vec![a]);
    }

    #[test]
    fn the_sample_plan_runs() {
        let mut m = sample_factory();
        apply_sample_plan(&mut m);
        assert!(m.problems().is_empty(), "{:?}", m.problems());
        let r = Simulation::run(&m).unwrap();
        assert_eq!(r.orders.len(), 3);
        let cutting = m.elements.iter().find(|e| e.name == "切削").unwrap().id;
        assert_eq!(r.element(cutting).unwrap().setups, 2);
        assert!(r.output <= m.planned_quantity());
        assert!(on_time_ratio(&r.orders).is_some());
    }
}
