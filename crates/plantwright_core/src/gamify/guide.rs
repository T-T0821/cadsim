//! 次に押すボタンの案内
//!
//! ミッションごとに、押す順にボタンの表示名を並べておく。UI 層は画面に出ている部品の表示名を集め、
//! [`next_target`] で「いま押すボタン」を選んで光らせる。後ろの手順のボタンほど優先する
//! (メニューを開けばその中の項目、道具のパネルを開けばその中のボタンへ、案内が先へ進む)。

use super::MissionId;

/// いつそのボタンを案内するか
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum When {
    /// いつでも
    Always,
    /// ミッションの道具のパネルが閉じているときだけ (パネルを開くツールレールのボタン。
    /// 開いたあとに押すと閉じてしまうので、開いたら案内しない)
    ToolClosed,
    /// ミッションの道具のパネルが開いているときだけ (同じ表示名のボタンがほかの画面にもあるため)
    ToolOpen,
}

/// 押してほしいボタン
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GuideTarget {
    /// カードの手順の何番目か (0 始まり)
    pub step: usize,
    /// ボタンの表示名。末尾が `*` なら、その前までで始まる表示名に合う
    pub label: &'static str,
    pub when: When,
}

impl GuideTarget {
    /// 画面の部品の表示名がこのボタンか
    pub fn matches(&self, shown: &str) -> bool {
        let shown = shown.trim();
        match self.label.strip_suffix('*') {
            Some(prefix) => shown.starts_with(prefix),
            None => shown == self.label,
        }
    }

    fn allowed(&self, tool_open: bool) -> bool {
        match self.when {
            When::Always => true,
            When::ToolClosed => !tool_open,
            When::ToolOpen => tool_open,
        }
    }
}

const fn at(step: usize, label: &'static str) -> GuideTarget {
    GuideTarget {
        step,
        label,
        when: When::Always,
    }
}

const fn open(label: &'static str) -> GuideTarget {
    GuideTarget {
        step: 0,
        label,
        when: When::ToolClosed,
    }
}

const fn inside(step: usize, label: &'static str) -> GuideTarget {
    GuideTarget {
        step,
        label,
        when: When::ToolOpen,
    }
}

/// ミッションの、押す順のボタン。3D ビューの中の操作やキーだけのミッションは空。
pub fn guide_for(id: MissionId) -> &'static [GuideTarget] {
    use MissionId::*;
    match id {
        SelectPart | OrbitView | Measure => const { &[] },
        PlayChart => const { &[at(0, "⏸ 一時停止"), at(1, "▶ 再生")] },
        SwitchProjection => const { &[at(0, "平行")] },
        EditPart => const { &[at(0, "⏮ 先頭へ"), at(1, "✚ 移動")] },
        Undo => const { &[at(0, "⟲ 戻す")] },
        FindInterference => const { &[at(0, "✚ 移動"), open("干渉")] },
        PlaceLibraryPart => const { &[open("部品")] },
        RunFlow => const { &[open("工程"), inside(1, "サンプル工程"), inside(2, "▶ 実行")] },
        Annotate => const { &[open("寸法"), inside(1, "点間距離")] },
        Save => const { &[at(0, "💾 保存")] },
        AddActuator => {
            const {
                &[
                    at(1, "+ アクチュエータ"),
                    at(1, "シリンダ"),
                    at(2, "+ 選択中の「*"),
                ]
            }
        }
        AddStep => const { &[at(0, "+ ステップ")] },
        AddSensor => const { &[at(0, "+ センサー"), at(0, "光電センサー")] },
        EmergencyStop => {
            const {
                &[
                    open("安全"),
                    inside(1, "非常停止ボタン"),
                    inside(2, "押す"),
                    inside(2, "遮光"),
                ]
            }
        }
        RunPlc => const { &[open("PLC"), inside(1, "内蔵の模擬PLC"), inside(2, "▶ 運転")] },
        RunLineLink => {
            const {
                &[
                    open("連携"),
                    inside(1, "サンプルのラインを置く"),
                    inside(2, "▶ 再生"),
                ]
            }
        }
        ComposeLine => {
            const {
                &[
                    open("言葉で"),
                    inside(1, "下書きを作る"),
                    inside(2, "モデルとチャートに置く"),
                ]
            }
        }
        ExportLadder => {
            const {
                &[
                    open("ラダー"),
                    inside(1, "ラダーを書き出す"),
                    inside(2, "I/O 表をコピー"),
                ]
            }
        }
        WalkAround => const { &[at(0, "🎥 ビュー"), at(0, "🚶 一人称で歩く")] },
        ExportVideo => {
            const {
                &[
                    at(0, "ファイル"),
                    at(0, "書き出し ⏵"),
                    at(0, "シミュレーションを動画で書き出し…"),
                    at(1, "● 書き出し開始…"),
                ]
            }
        }
    }
}

/// いま光らせるボタン: 案内してよいもののうち、画面に出ている (`shown` が true の) いちばん後ろのもの
pub fn next_target(
    id: MissionId,
    tool_open: bool,
    shown: impl Fn(&GuideTarget) -> bool,
) -> Option<&'static GuideTarget> {
    guide_for(id)
        .iter()
        .rev()
        .find(|t| t.allowed(tool_open) && shown(t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamify::MISSIONS;

    #[test]
    fn label_matching() {
        assert!(at(0, "▶ 運転").matches(" ▶ 運転 "));
        assert!(!at(0, "▶ 運転").matches("▶ 運転中"));
        let prefix = at(0, "+ 選択中の「*");
        assert!(prefix.matches("+ 選択中の「ベルト」を追加"));
        assert!(!prefix.matches("+ 選択中の部品を追加 (ツリーかビューで選択)"));
    }

    #[test]
    fn later_buttons_win_as_they_appear() {
        let shown = |labels: &'static [&'static str]| {
            move |t: &GuideTarget| labels.iter().any(|l| t.matches(l))
        };
        // パネルを開く前はツールレールのボタン
        let t = next_target(MissionId::RunPlc, false, shown(&["PLC", "▶ 再生"])).unwrap();
        assert_eq!((t.step, t.label), (0, "PLC"));
        // 開いたら、レールのボタンは押させない (押すと閉じる)。中のボタンへ進む
        let t = next_target(MissionId::RunPlc, true, shown(&["PLC", "内蔵の模擬PLC"])).unwrap();
        assert_eq!((t.step, t.label), (1, "内蔵の模擬PLC"));
        let t = next_target(
            MissionId::RunPlc,
            true,
            shown(&["PLC", "内蔵の模擬PLC", "▶ 運転"]),
        )
        .unwrap();
        assert_eq!((t.step, t.label), (2, "▶ 運転"));
        // 同じ表示名のボタンがほかの画面にあっても、道具が閉じていれば中のボタンは案内しない
        let t = next_target(MissionId::RunLineLink, false, shown(&["連携", "▶ 再生"])).unwrap();
        assert_eq!(t.label, "連携");
        // 何も出ていなければ案内しない
        assert!(next_target(MissionId::RunPlc, true, shown(&["PLC"])).is_none());
        assert!(next_target(MissionId::SelectPart, false, |_| true).is_none());
    }

    #[test]
    fn guide_steps_fit_the_mission_steps() {
        for m in MISSIONS {
            let targets = guide_for(m.id);
            let mut last = 0;
            for t in targets {
                assert!(t.step < m.steps.len(), "{} の案内の手順がない", m.key);
                assert!(t.step >= last, "{} の案内が手順を戻る", m.key);
                last = t.step;
                if t.when != When::Always {
                    assert!(m.tool.is_some(), "{} に道具がない", m.key);
                }
            }
        }
    }
}
