//! 生成 AI を使わずに、文のキーワードから下書きを作る
//!
//! API キーがないとき、または生成 AI の呼び出しに失敗したときに使う。
//! 読み取るのは次の2つだけ。
//!
//! - 設備の種類と台数: 「コンベア2本」「ロボット 1台」「2台のロボット」「センサー×3」
//! - 作業の種類: 箱詰め・検査・組立 (どれもなければ移載)
//!
//! 作業の種類ごとに決まった順でステップを並べる。

use super::model::{DraftAction, DraftStep, Equipment, EquipmentKind, LineDraft, Verb};

/// 1種類あたりの台数の上限
const MAX_PER_KIND: usize = 8;

/// 種類ごとのキーワード (小文字で比べる)
const KEYWORDS: [(EquipmentKind, &[&str]); 5] = [
    (
        EquipmentKind::Conveyor,
        &["コンベヤ", "コンベア", "conveyor"],
    ),
    (EquipmentKind::Robot, &["ロボット", "robot", "ロボ"]),
    (
        EquipmentKind::Cylinder,
        &[
            "シリンダ",
            "プッシャ",
            "ストッパ",
            "cylinder",
            "pusher",
            "stopper",
        ],
    ),
    (
        EquipmentKind::Sensor,
        &["センサ", "sensor", "光電", "検出器"],
    ),
    (
        EquipmentKind::Table,
        &["作業台", "置き台", "テーブル", "table", "パレット置き"],
    ),
];

/// 作業の種類
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Task {
    /// 製品を箱に入れる
    Packing,
    /// 検査する
    Inspection,
    /// 組み立てる
    Assembly,
    /// 運んで載せ替える
    Transfer,
}

impl Task {
    pub fn label(self) -> &'static str {
        match self {
            Task::Packing => "箱詰め",
            Task::Inspection => "検査",
            Task::Assembly => "組立",
            Task::Transfer => "移載",
        }
    }
}

/// 文から作業の種類を読み取る
pub fn detect_task(text: &str) -> Task {
    let t = text.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if has(&[
        "箱詰",
        "箱に詰",
        "箱に入れ",
        "梱包",
        "パッキング",
        "packing",
        "pack ",
    ]) {
        Task::Packing
    } else if has(&["検査", "外観", "inspection", "inspect"]) {
        Task::Inspection
    } else if has(&["組立", "組み立", "組付", "assembly", "assemble"]) {
        Task::Assembly
    } else {
        Task::Transfer
    }
}

/// 文から種類ごとの台数を読み取る。キーワードがない種類は 0。
pub fn detect_counts(text: &str) -> Vec<(EquipmentKind, usize)> {
    let lower = text.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    KEYWORDS
        .iter()
        .map(|(kind, words)| {
            let mut total = 0usize;
            let mut taken = vec![false; chars.len()];
            for word in *words {
                let w: Vec<char> = word.chars().collect();
                let mut i = 0;
                while i + w.len() <= chars.len() {
                    if chars[i..i + w.len()] == w[..] && !taken[i] {
                        for flag in &mut taken[i..i + w.len()] {
                            *flag = true;
                        }
                        let n = number_after(&chars, i + w.len())
                            .or_else(|| number_before(&chars, i))
                            .unwrap_or(1);
                        total += n;
                        i += w.len();
                    } else {
                        i += 1;
                    }
                }
            }
            (*kind, total.min(MAX_PER_KIND))
        })
        .collect()
}

/// 数字1文字の値 (半角・全角・漢数字)
fn digit(c: char) -> Option<usize> {
    match c {
        '0'..='9' => Some(c as usize - '0' as usize),
        '０'..='９' => Some(c as usize - '０' as usize),
        '一' => Some(1),
        '二' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        _ => None,
    }
}

/// キーワードのすぐ後ろの数 (「コンベア2本」「ロボット 1台」「センサー×3」)
fn number_after(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start;
    // 長音・空白・つなぎの文字は読み飛ばす
    while i < chars.len()
        && matches!(
            chars[i],
            'ー' | ' ' | '　' | '×' | 'x' | '*' | 'を' | 'が' | 'は' | ':' | '：'
        )
    {
        i += 1;
        if i - start > 3 {
            return None;
        }
    }
    let mut value = None::<usize>;
    while i < chars.len() {
        match digit(chars[i]) {
            Some(d) => value = Some(value.unwrap_or(0) * 10 + d),
            None => break,
        }
        i += 1;
    }
    value.filter(|v| *v > 0)
}

/// キーワードのすぐ前の数 (「2本のコンベア」「2台ロボット」「two」は読まない)
fn number_before(chars: &[char], end: usize) -> Option<usize> {
    let mut i = end;
    // 「の」と助数詞を読み飛ばす
    let skip = ['の', ' ', '　'];
    let counters = ['本', '台', '個', '基', '機', 'つ', '列', '人'];
    while i > 0 && skip.contains(&chars[i - 1]) {
        i -= 1;
    }
    if i > 0 && counters.contains(&chars[i - 1]) {
        i -= 1;
    } else {
        return None;
    }
    let mut value = 0usize;
    let mut scale = 1usize;
    let mut found = false;
    while i > 0 {
        match digit(chars[i - 1]) {
            Some(d) => {
                value += d * scale;
                scale *= 10;
                found = true;
                i -= 1;
            }
            None => break,
        }
    }
    (found && value > 0).then_some(value)
}

/// 文から下書きを作る。設備のキーワードが1つもなければ Err。
pub fn draft_from_text(text: &str) -> Result<LineDraft, String> {
    let task = detect_task(text);
    let counts = detect_counts(text);
    let count = |kind| {
        counts
            .iter()
            .find(|(k, _)| *k == kind)
            .map_or(0, |(_, n)| *n)
    };
    if counts.iter().all(|(_, n)| *n == 0) {
        return Err(
            "設備のキーワードが見つかりませんでした。「コンベア2本とロボット1台で箱詰め」のように、設備と台数を書いてください"
                .into(),
        );
    }

    let mut notes = Vec::new();
    let mut equipment = Vec::new();
    let mut add = |name: String, kind| {
        equipment.push(Equipment::new(name, kind));
    };

    let conveyor_names = conveyor_names(task, count(EquipmentKind::Conveyor));
    for name in &conveyor_names {
        add(name.clone(), EquipmentKind::Conveyor);
    }
    let robots: Vec<String> = numbered("ロボット", count(EquipmentKind::Robot));
    for name in &robots {
        add(name.clone(), EquipmentKind::Robot);
    }
    let mut tables = numbered("作業台", count(EquipmentKind::Table));
    // ロボットが取って置くには場所が2つ要る。足りなければ置き台を足す
    if !robots.is_empty() && conveyor_names.len() + tables.len() < 2 {
        let name = if task == Task::Packing {
            "箱置き台"
        } else {
            "置き台"
        };
        tables.push(name.to_string());
        notes.push(format!(
            "ロボットが置く場所がないので「{name}」を足しました"
        ));
    }
    for name in &tables {
        add(name.clone(), EquipmentKind::Table);
    }
    let cylinders = numbered_with(
        if task == Task::Packing || count(EquipmentKind::Cylinder) == 1 {
            "ストッパ"
        } else {
            "シリンダ"
        },
        count(EquipmentKind::Cylinder),
    );
    for name in &cylinders {
        add(name.clone(), EquipmentKind::Cylinder);
    }
    let sensors = numbered_with("ワーク検出", count(EquipmentKind::Sensor));
    for name in &sensors {
        add(name.clone(), EquipmentKind::Sensor);
    }

    // 取る場所と置く場所
    let mut places: Vec<String> = conveyor_names.clone();
    places.extend(tables.iter().cloned());
    let source = places.first().cloned();
    let destination = places
        .last()
        .cloned()
        .filter(|d| Some(d) != source.as_ref());

    let mut steps = Vec::new();
    // 1. 流してくる
    if let Some(first) = conveyor_names.first() {
        steps.push(step(
            format!("{first}でワークを送る"),
            vec![DraftAction::new(first.clone(), Verb::Run)],
        ));
    }
    // 2. 止める・位置決め (ワークが来るのを待つ)
    let mut wait = sensors.first().map(|s| (s.clone(), true));
    if let Some(cylinder) = cylinders.first() {
        let mut s = step(
            format!("{cylinder}で位置決め"),
            vec![DraftAction::new(cylinder.clone(), Verb::Extend)],
        );
        s.wait_for = wait.take();
        steps.push(s);
    }
    // 3. 作業 (検査・組立はその場で時間を取る)
    match task {
        Task::Inspection => {
            let mut s = step("検査".into(), Vec::new());
            s.dwell = 2.0;
            s.wait_for = wait.take();
            steps.push(s);
        }
        Task::Assembly => {
            let mut s = step("組付け".into(), Vec::new());
            s.dwell = 4.0;
            s.wait_for = wait.take();
            steps.push(s);
        }
        _ => {}
    }
    // 4. ロボットで取って置く (ロボットが複数なら交互に受け持つ)
    if let (Some(src), Some(dst)) = (&source, &destination) {
        for (k, robot) in robots.iter().enumerate() {
            let (pick_label, place_label) = match task {
                Task::Packing => ("製品を取る", "箱に入れる"),
                _ => ("ワークを取る", "ワークを置く"),
            };
            let suffix = if robots.len() > 1 {
                format!(" ({robot})")
            } else {
                String::new()
            };
            let mut pick = step(
                format!("{pick_label}{suffix}"),
                vec![DraftAction::new(robot.clone(), Verb::Pick).at(src.clone())],
            );
            if k == 0 {
                pick.wait_for = wait.take();
            }
            steps.push(pick);
            // 置いている間に位置決めを戻す
            let mut place_actions =
                vec![DraftAction::new(robot.clone(), Verb::Place).at(dst.clone())];
            if k == 0
                && let Some(cylinder) = cylinders.first()
            {
                place_actions.push(DraftAction::new(cylinder.clone(), Verb::Retract));
            }
            steps.push(step(format!("{place_label}{suffix}"), place_actions));
        }
    } else if let Some(cylinder) = cylinders.first() {
        steps.push(step(
            format!("{cylinder}を戻す"),
            vec![DraftAction::new(cylinder.clone(), Verb::Retract)],
        ));
    }
    // 5. 送り出す
    if conveyor_names.len() >= 2 {
        let last = conveyor_names.last().unwrap();
        let label = if task == Task::Packing {
            "箱を送り出す"
        } else {
            "ワークを送り出す"
        };
        let mut a = DraftAction::new(last.clone(), Verb::Run);
        a.seconds = 2.0;
        steps.push(step(format!("{label} ({last})"), vec![a]));
    }
    // 残った開始条件は先頭に付ける
    if let (Some(w), Some(first)) = (wait, steps.first_mut()) {
        first.wait_for.get_or_insert(w);
    }
    if steps.is_empty() {
        notes.push("動かせる設備がないので、ステップは作っていません".into());
    }
    notes.push(format!(
        "生成 AI を使わずに、キーワードから「{}」の流れで作りました",
        task.label()
    ));

    Ok(LineDraft {
        title: format!("{}ライン", task.label()),
        equipment,
        steps,
        notes,
    })
}

fn step(comment: String, actions: Vec<DraftAction>) -> DraftStep {
    DraftStep {
        comment,
        actions,
        ..Default::default()
    }
}

/// "ロボット1", "ロボット2", …
fn numbered(base: &str, n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("{base}{i}")).collect()
}

/// 1台なら番号を付けない
fn numbered_with(base: &str, n: usize) -> Vec<String> {
    if n == 1 {
        vec![base.to_string()]
    } else {
        numbered(base, n)
    }
}

/// コンベアの名前。箱詰めで2本なら製品と箱で分ける。
fn conveyor_names(task: Task, n: usize) -> Vec<String> {
    match (task, n) {
        (Task::Packing, 2) => vec!["製品コンベア".into(), "箱コンベア".into()],
        (Task::Packing, 1) => vec!["製品コンベア".into()],
        (_, 2) => vec!["投入コンベア".into(), "排出コンベア".into()],
        _ => numbered("コンベア", n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_draft::check::normalize;

    fn count_of(text: &str, kind: EquipmentKind) -> usize {
        detect_counts(text)
            .into_iter()
            .find(|(k, _)| *k == kind)
            .unwrap()
            .1
    }

    #[test]
    fn reads_counts_in_various_forms() {
        use EquipmentKind::*;
        let t = "コンベア2本とロボット1台で箱詰め";
        assert_eq!(count_of(t, Conveyor), 2);
        assert_eq!(count_of(t, Robot), 1);
        assert_eq!(count_of(t, Sensor), 0);
        assert_eq!(count_of("2本のコンベアと３台のロボット", Conveyor), 2);
        assert_eq!(count_of("2本のコンベアと３台のロボット", Robot), 3);
        assert_eq!(count_of("コンベヤー三本", Conveyor), 3);
        assert_eq!(count_of("センサー×2 とストッパ", Sensor), 2);
        assert_eq!(count_of("センサー×2 とストッパ", Cylinder), 1);
        assert_eq!(count_of("two conveyors and a robot", Conveyor), 1);
        assert_eq!(count_of("ロボット12台", Robot), MAX_PER_KIND);
    }

    #[test]
    fn detects_task() {
        assert_eq!(detect_task("箱詰めライン"), Task::Packing);
        assert_eq!(detect_task("外観検査をする"), Task::Inspection);
        assert_eq!(detect_task("部品の組み立て"), Task::Assembly);
        assert_eq!(detect_task("ワークを運ぶ"), Task::Transfer);
    }

    #[test]
    fn packing_with_two_conveyors_and_a_robot() {
        let d = draft_from_text("コンベア2本とロボット1台で箱詰め").unwrap();
        let names: Vec<&str> = d.equipment.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["製品コンベア", "箱コンベア", "ロボット1"]);
        let comments: Vec<&str> = d.steps.iter().map(|s| s.comment.as_str()).collect();
        assert_eq!(
            comments,
            [
                "製品コンベアでワークを送る",
                "製品を取る",
                "箱に入れる",
                "箱を送り出す (箱コンベア)"
            ]
        );
        assert_eq!(
            d.steps[1].actions[0].target.as_deref(),
            Some("製品コンベア")
        );
        assert_eq!(d.steps[2].actions[0].target.as_deref(), Some("箱コンベア"));
        // 整えても警告が出ない形になっている
        let c = normalize(d).unwrap();
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    }

    #[test]
    fn sensor_and_stopper_become_interlock_and_positioning() {
        let d = draft_from_text("コンベア1本、ロボット1台、センサー1個、ストッパで箱詰め").unwrap();
        // 置き場所がないので置き台を足す
        assert!(d.find("箱置き台").is_some());
        let s = &d.steps[1];
        assert_eq!(s.comment, "ストッパで位置決め");
        assert_eq!(s.wait_for, Some(("ワーク検出".into(), true)));
        // 置くときにストッパを戻す
        assert!(d.steps[3].actions.iter().any(|a| a.verb == Verb::Retract));
        assert!(normalize(d).unwrap().warnings.is_empty());
    }

    #[test]
    fn inspection_adds_a_dwell_step() {
        let d = draft_from_text("コンベアとセンサーで検査").unwrap();
        let s = d.steps.iter().find(|s| s.comment == "検査").unwrap();
        assert_eq!(s.dwell, 2.0);
        assert_eq!(s.wait_for, Some(("ワーク検出".into(), true)));
    }

    #[test]
    fn two_robots_take_turns() {
        let d = draft_from_text("コンベア2本とロボット2台で移載").unwrap();
        assert_eq!(
            d.steps
                .iter()
                .filter(|s| s.actions.iter().any(|a| a.verb == Verb::Pick))
                .count(),
            2
        );
        assert!(normalize(d).unwrap().warnings.is_empty());
    }

    #[test]
    fn no_keywords_is_an_error() {
        assert!(draft_from_text("いい感じのラインを作って").is_err());
    }
}
