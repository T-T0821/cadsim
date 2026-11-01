//! 下書きを使える形に整える
//!
//! 生成 AI の出力は形が崩れていることがあるので、配置する前にここで直す。
//! 直したところは警告の文にして返し、画面に出す。

use std::collections::HashSet;

use super::model::{EquipmentKind, LineDraft, Verb};

/// 設備の数の上限 (これより多い分は捨てる)
pub const MAX_EQUIPMENT: usize = 40;
/// ステップの数の上限
pub const MAX_STEPS: usize = 60;
/// 動作時間の範囲 (秒)
const SECONDS_RANGE: [f64; 2] = [0.1, 120.0];
/// コンベアの長さの範囲 (mm)
const LENGTH_RANGE: [f64; 2] = [300.0, 20_000.0];

/// 整えた結果
#[derive(Clone, Debug, PartialEq)]
pub struct Checked {
    pub draft: LineDraft,
    /// 直したところの説明
    pub warnings: Vec<String>,
}

/// 下書きを整える。設備が1つもなければ Err。
pub fn normalize(mut draft: LineDraft) -> Result<Checked, String> {
    let mut warnings = Vec::new();

    if draft.title.trim().is_empty() {
        draft.title = "ライン".into();
    }
    draft.title = draft.title.trim().to_string();

    if draft.equipment.len() > MAX_EQUIPMENT {
        warnings.push(format!(
            "設備が多すぎるので、最初の {MAX_EQUIPMENT} 台だけ使います"
        ));
        draft.equipment.truncate(MAX_EQUIPMENT);
    }
    if draft.equipment.is_empty() {
        return Err("文から設備 (コンベア・ロボットなど) を読み取れませんでした".into());
    }

    // 名前: 空なら種類 + 番号、重なったら番号を足す
    let mut used = HashSet::new();
    for i in 0..draft.equipment.len() {
        let kind = draft.equipment[i].kind;
        let mut name = draft.equipment[i].name.trim().to_string();
        if name.is_empty() {
            name = format!("{}{}", kind.label(), i + 1);
        }
        if used.contains(&name) {
            let base = name.clone();
            let mut n = 2;
            while used.contains(&format!("{base}-{n}")) {
                n += 1;
            }
            name = format!("{base}-{n}");
            warnings.push(format!("同じ名前の設備があったので「{name}」にしました"));
        }
        used.insert(name.clone());
        let e = &mut draft.equipment[i];
        e.name = name;
        if let Some(p) = e.position
            && !(p[0].is_finite() && p[1].is_finite())
        {
            e.position = None;
        }
        e.length = e
            .length
            .filter(|l| l.is_finite() && *l > 0.0)
            .map(|l| l.clamp(LENGTH_RANGE[0], LENGTH_RANGE[1]));
    }

    if draft.steps.len() > MAX_STEPS {
        warnings.push(format!(
            "ステップが多すぎるので、最初の {MAX_STEPS} 個だけ使います"
        ));
        draft.steps.truncate(MAX_STEPS);
    }

    let kind_of = |draft: &LineDraft, name: &str| draft.find(name).map(|e| e.kind);
    let mut steps = std::mem::take(&mut draft.steps);
    for (index, step) in steps.iter_mut().enumerate() {
        let number = index + 1;
        step.comment = step.comment.trim().to_string();
        if !(step.dwell.is_finite() && step.dwell >= 0.0) {
            step.dwell = 0.0;
        }
        step.dwell = step.dwell.min(SECONDS_RANGE[1]);

        let mut actions = Vec::new();
        for mut action in std::mem::take(&mut step.actions) {
            let Some(kind) = kind_of(&draft, &action.equipment) else {
                warnings.push(format!(
                    "ステップ {number}: 「{}」という設備がないので、その動作を外しました",
                    action.equipment
                ));
                continue;
            };
            if action.verb.applies_to() != kind {
                warnings.push(format!(
                    "ステップ {number}: {}「{}」は「{}」できないので、その動作を外しました",
                    kind.label(),
                    action.equipment,
                    action.verb.label()
                ));
                continue;
            }
            if action.verb.needs_target() {
                let target_ok = action
                    .target
                    .as_deref()
                    .and_then(|t| kind_of(&draft, t))
                    .is_some_and(is_place_kind);
                if !target_ok {
                    match default_target(&draft, action.verb) {
                        Some(t) => {
                            warnings.push(format!(
                                "ステップ {number}: {}場所が分からないので「{t}」にしました",
                                if action.verb == Verb::Pick {
                                    "取る"
                                } else {
                                    "置く"
                                }
                            ));
                            action.target = Some(t);
                        }
                        None => {
                            warnings.push(format!(
                                "ステップ {number}: 取る・置く場所 (コンベアか作業台) がないので、その動作を外しました"
                            ));
                            continue;
                        }
                    }
                }
            } else {
                action.target = None;
            }
            if !(action.seconds.is_finite() && action.seconds > 0.0) {
                action.seconds = action.verb.default_seconds();
            }
            action.seconds = action.seconds.clamp(SECONDS_RANGE[0], SECONDS_RANGE[1]);
            // 同じ設備への2つ目の動作は同時にできないので外す
            if actions
                .iter()
                .any(|a: &super::model::DraftAction| a.equipment == action.equipment)
            {
                warnings.push(format!(
                    "ステップ {number}: 「{}」に同時に2つの動作があったので、1つ目だけ残しました",
                    action.equipment
                ));
                continue;
            }
            actions.push(action);
        }
        step.actions = actions;

        if let Some((sensor, _)) = &step.wait_for
            && kind_of(&draft, sensor) != Some(EquipmentKind::Sensor)
        {
            warnings.push(format!(
                "ステップ {number}: 「{sensor}」はセンサーではないので、開始条件を外しました"
            ));
            step.wait_for = None;
        }
        if step.comment.is_empty() {
            step.comment = describe(step);
        }
    }
    let before = steps.len();
    steps.retain(|s| !s.actions.is_empty() || s.wait_for.is_some() || s.dwell > 0.0);
    if steps.len() < before {
        warnings.push(format!(
            "何もしないステップを {} 個外しました",
            before - steps.len()
        ));
    }
    draft.steps = steps;
    draft.notes.retain(|n| !n.trim().is_empty());

    Ok(Checked { draft, warnings })
}

/// ロボットが取る・置く場所になれる設備
fn is_place_kind(kind: EquipmentKind) -> bool {
    matches!(kind, EquipmentKind::Conveyor | EquipmentKind::Table)
}

/// 場所の指定がないときに使う設備: 取るのは最初の、置くのは最後のコンベア / 作業台
fn default_target(draft: &LineDraft, verb: Verb) -> Option<String> {
    let mut places = draft.equipment.iter().filter(|e| is_place_kind(e.kind));
    let found = if verb == Verb::Pick {
        places.next()
    } else {
        places.next_back()
    };
    found.map(|e| e.name.clone())
}

/// 説明のないステップに付ける説明 ("ロボット1 取る / コンベア1 搬送")
fn describe(step: &super::model::DraftStep) -> String {
    let parts: Vec<String> = step
        .actions
        .iter()
        .map(|a| format!("{} {}", a.equipment, a.verb.label()))
        .collect();
    if parts.is_empty() {
        if let Some((sensor, on)) = &step.wait_for {
            return format!("{sensor} {} 待ち", if *on { "ON" } else { "OFF" });
        }
        return "待ち".into();
    }
    parts.join(" / ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_draft::model::{DraftAction, DraftStep, Equipment};

    fn base() -> LineDraft {
        LineDraft {
            title: " 箱詰め ".into(),
            equipment: vec![
                Equipment::new("製品コンベア", EquipmentKind::Conveyor),
                Equipment::new("ロボット1", EquipmentKind::Robot),
                Equipment::new("箱コンベア", EquipmentKind::Conveyor),
                Equipment::new("ワーク検出", EquipmentKind::Sensor),
            ],
            steps: Vec::new(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn empty_equipment_is_an_error() {
        assert!(normalize(LineDraft::default()).is_err());
    }

    #[test]
    fn trims_title_and_renames_duplicates() {
        let mut d = base();
        d.equipment
            .push(Equipment::new("ロボット1", EquipmentKind::Robot));
        d.equipment.push(Equipment::new("", EquipmentKind::Table));
        let c = normalize(d).unwrap();
        assert_eq!(c.draft.title, "箱詰め");
        assert_eq!(c.draft.equipment[4].name, "ロボット1-2");
        assert_eq!(c.draft.equipment[5].name, "作業台6");
        assert_eq!(c.warnings.len(), 1);
    }

    #[test]
    fn drops_unknown_and_mismatched_actions() {
        let mut d = base();
        d.steps.push(DraftStep {
            comment: "x".into(),
            actions: vec![
                DraftAction::new("ないもの", Verb::Run),
                DraftAction::new("ロボット1", Verb::Run),
                DraftAction::new("製品コンベア", Verb::Run),
            ],
            ..Default::default()
        });
        let c = normalize(d).unwrap();
        assert_eq!(c.draft.steps[0].actions.len(), 1);
        assert_eq!(c.warnings.len(), 2);
    }

    #[test]
    fn fills_missing_pick_and_place_targets() {
        let mut d = base();
        d.steps.push(DraftStep {
            actions: vec![DraftAction::new("ロボット1", Verb::Pick)],
            ..Default::default()
        });
        d.steps.push(DraftStep {
            actions: vec![DraftAction::new("ロボット1", Verb::Place).at("ワーク検出")],
            ..Default::default()
        });
        let c = normalize(d).unwrap();
        assert_eq!(
            c.draft.steps[0].actions[0].target.as_deref(),
            Some("製品コンベア")
        );
        assert_eq!(
            c.draft.steps[1].actions[0].target.as_deref(),
            Some("箱コンベア")
        );
        assert_eq!(c.draft.steps[0].comment, "ロボット1 取る");
    }

    #[test]
    fn fixes_bad_seconds_and_drops_empty_steps() {
        let mut d = base();
        let mut a = DraftAction::new("製品コンベア", Verb::Run);
        a.seconds = f64::NAN;
        let mut b = DraftAction::new("箱コンベア", Verb::Run);
        b.seconds = 1e9;
        d.steps.push(DraftStep {
            actions: vec![a, b],
            ..Default::default()
        });
        d.steps.push(DraftStep::default());
        let c = normalize(d).unwrap();
        assert_eq!(c.draft.steps.len(), 1);
        assert_eq!(c.draft.steps[0].actions[0].seconds, 3.0);
        assert_eq!(c.draft.steps[0].actions[1].seconds, 120.0);
    }

    #[test]
    fn wait_must_name_a_sensor() {
        let mut d = base();
        d.steps.push(DraftStep {
            wait_for: Some(("ワーク検出".into(), true)),
            ..Default::default()
        });
        d.steps.push(DraftStep {
            wait_for: Some(("ロボット1".into(), true)),
            dwell: 1.0,
            ..Default::default()
        });
        let c = normalize(d).unwrap();
        assert_eq!(c.draft.steps[0].comment, "ワーク検出 ON 待ち");
        assert!(c.draft.steps[1].wait_for.is_none());
    }

    #[test]
    fn one_action_per_equipment_in_a_step() {
        let mut d = base();
        d.steps.push(DraftStep {
            actions: vec![
                DraftAction::new("ロボット1", Verb::Pick),
                DraftAction::new("ロボット1", Verb::Place),
            ],
            ..Default::default()
        });
        let c = normalize(d).unwrap();
        assert_eq!(c.draft.steps[0].actions.len(), 1);
    }
}
