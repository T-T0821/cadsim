//! ゲーム風の UI の中身: チュートリアルのミッション、経験値とレベル、達成時の紙吹雪、ミッションの表示の設定
//!
//! Bevy・egui・ファイル形式に依存しない。UI 層は毎フレーム [`Observation`] を作って
//! [`Progress::observe`] に渡し、返ってきた達成を画面に出すだけにする。

/// チュートリアルのミッション。並び順がおすすめの進め方。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MissionId {
    SelectPart,
    OrbitView,
    EditPart,
    Undo,
    SwitchProjection,
    Measure,
    FindInterference,
    Annotate,
    PlayChart,
    Save,
}

pub struct Mission {
    pub id: MissionId,
    /// 保存ファイルで使う名前 (変えない)
    pub key: &'static str,
    pub title: &'static str,
    /// 画面のどこで何をすればよいか
    pub hint: &'static str,
    pub xp: u32,
}

pub const MISSIONS: &[Mission] = &[
    Mission {
        id: MissionId::SelectPart,
        key: "select_part",
        title: "部品をえらぼう",
        hint: "3D ビューの部品か、左のツリーの行をクリック",
        xp: 50,
    },
    Mission {
        id: MissionId::OrbitView,
        key: "orbit_view",
        title: "ぐるっと見まわそう",
        hint: "Alt を押しながら左ドラッグ (右ドラッグでも見まわせる)",
        xp: 50,
    },
    Mission {
        id: MissionId::EditPart,
        key: "edit_part",
        title: "部品を動かそう",
        hint: "部品をえらんで W を押し、出てきた矢印をドラッグ",
        xp: 80,
    },
    Mission {
        id: MissionId::Undo,
        key: "undo",
        title: "時間を巻き戻そう",
        hint: "Ctrl+Z で元に戻す (Ctrl+Y でやり直し)",
        xp: 60,
    },
    Mission {
        id: MissionId::SwitchProjection,
        key: "switch_projection",
        title: "図面モードに切り替えよう",
        hint: "5 キーか、ツールバーの「平行」で平行投影に",
        xp: 50,
    },
    Mission {
        id: MissionId::Measure,
        key: "measure",
        title: "距離をはかろう",
        hint: "M を押して、部品の表面を2回クリック",
        xp: 80,
    },
    Mission {
        id: MissionId::FindInterference,
        key: "find_interference",
        title: "ぶつかりを見つけよう",
        hint: "部品を動かして別の部品に重ねると、赤く光る",
        xp: 100,
    },
    Mission {
        id: MissionId::Annotate,
        key: "annotate",
        title: "寸法を書きこもう",
        hint: "「寸法・注記」ウィンドウで点間距離などを置く",
        xp: 80,
    },
    Mission {
        id: MissionId::PlayChart,
        key: "play_chart",
        title: "設備を動かそう",
        hint: "下のタイミングチャートの「▶ 再生」を押す",
        xp: 100,
    },
    Mission {
        id: MissionId::Save,
        key: "save",
        title: "セーブしよう",
        hint: "Ctrl+S でプロジェクトを保存",
        xp: 80,
    },
];

pub fn mission(id: MissionId) -> &'static Mission {
    MISSIONS
        .iter()
        .find(|m| m.id == id)
        .expect("すべての MissionId は MISSIONS にある")
}

/// 1レベル上がるのに要る経験値
pub const XP_PER_LEVEL: u32 = 150;

/// 見まわしたとみなすカメラの回転量 (ラジアン、ヨーとピッチの変化の合計)
const ORBIT_THRESHOLD: f64 = 0.6;

/// その時点の画面の状態。UI 層が毎フレーム集める。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Observation {
    pub selected: bool,
    /// カメラのヨーとピッチ (ラジアン)
    pub camera_angles: [f64; 2],
    /// 3D ビューの上でマウスのボタンを押している (カメラの自動の動きと区別する)
    pub dragging_view: bool,
    pub orthographic: bool,
    /// いちばん新しい「元に戻す」の名前
    pub last_undo_label: Option<String>,
    pub can_redo: bool,
    pub measurement_count: usize,
    /// 干渉している部品の組の数
    pub interfering_pairs: usize,
    pub annotation_count: usize,
    pub playing: bool,
    /// ファイルに結びついたプロジェクトで、未保存の変更がない
    pub saved_to_file: bool,
}

/// ミッションを1つ達成したときの知らせ
#[derive(Clone, Debug, PartialEq)]
pub struct Achievement {
    pub mission: MissionId,
    pub xp: u32,
    /// レベルが上がったら、新しいレベル
    pub level_up: Option<u32>,
    /// これで全部のミッションが終わった
    pub all_complete: bool,
}

/// レベルの表示用
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub level: u32,
    /// 今のレベルに入ってから得た経験値
    pub into_level: u32,
    pub needed: u32,
}

impl Level {
    pub fn of(xp: u32) -> Self {
        Self {
            level: 1 + xp / XP_PER_LEVEL,
            into_level: xp % XP_PER_LEVEL,
            needed: XP_PER_LEVEL,
        }
    }

    pub fn fraction(&self) -> f32 {
        self.into_level as f32 / self.needed as f32
    }

    /// レベルに応じた称号
    pub fn title(&self) -> &'static str {
        level_title(self.level)
    }
}

/// レベルに応じた称号
pub fn level_title(level: u32) -> &'static str {
    match level {
        1 => "見習いエンジニア",
        2 => "かけだし設計者",
        3 => "ライン設計者",
        4 => "ベテラン設計者",
        5 => "設備マイスター",
        _ => "伝説のマイスター",
    }
}

/// ミッションの進み具合と経験値
#[derive(Clone, Debug, Default)]
pub struct Progress {
    completed: Vec<MissionId>,
    xp: u32,
    /// 直前の状態 (変化を見つけるのに使う)。最初の観測では何も達成しない。
    last: Option<Observation>,
    /// 見まわしたカメラの回転量の合計
    orbit_travel: f64,
    /// 起動してから部品を編集したか (最初からある干渉を数えないため)
    edited: bool,
}

impl Progress {
    /// 保存しておいた進み具合から作る。知らない名前は無視する。
    pub fn restore<'a>(keys: impl IntoIterator<Item = &'a str>, xp: u32) -> Self {
        let mut completed = Vec::new();
        for key in keys {
            if let Some(m) = MISSIONS.iter().find(|m| m.key == key)
                && !completed.contains(&m.id)
            {
                completed.push(m.id);
            }
        }
        Self {
            completed,
            xp,
            ..Default::default()
        }
    }

    pub fn completed_keys(&self) -> Vec<&'static str> {
        self.completed.iter().map(|&id| mission(id).key).collect()
    }

    pub fn xp(&self) -> u32 {
        self.xp
    }

    pub fn level(&self) -> Level {
        Level::of(self.xp)
    }

    pub fn is_done(&self, id: MissionId) -> bool {
        self.completed.contains(&id)
    }

    pub fn done_count(&self) -> usize {
        self.completed.len()
    }

    pub fn all_complete(&self) -> bool {
        self.completed.len() == MISSIONS.len()
    }

    /// 次に挑戦するミッション (おすすめ順で最初の未達成)
    pub fn next(&self) -> Option<&'static Mission> {
        MISSIONS.iter().find(|m| !self.is_done(m.id))
    }

    /// 最初からやり直す
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 新しい状態を見て、このフレームで達成したミッションを返す
    pub fn observe(&mut self, now: &Observation) -> Vec<Achievement> {
        let Some(before) = self.last.replace(now.clone()) else {
            return Vec::new();
        };
        if now.dragging_view && before.dragging_view {
            self.orbit_travel += (now.camera_angles[0] - before.camera_angles[0]).abs()
                + (now.camera_angles[1] - before.camera_angles[1]).abs();
        }

        let edited = now.last_undo_label != before.last_undo_label
            && now.last_undo_label.as_deref().is_some_and(is_edit_label);
        self.edited |= edited;
        let reached = [
            (MissionId::SelectPart, now.selected && !before.selected),
            (MissionId::OrbitView, self.orbit_travel > ORBIT_THRESHOLD),
            (MissionId::EditPart, edited),
            (MissionId::Undo, now.can_redo && !before.can_redo),
            (
                MissionId::SwitchProjection,
                now.orthographic != before.orthographic,
            ),
            (
                MissionId::Measure,
                now.measurement_count > before.measurement_count,
            ),
            (
                MissionId::FindInterference,
                self.edited && now.interfering_pairs > before.interfering_pairs,
            ),
            (
                MissionId::Annotate,
                now.annotation_count > before.annotation_count,
            ),
            (MissionId::PlayChart, now.playing && !before.playing),
            (MissionId::Save, now.saved_to_file && !before.saved_to_file),
        ];

        let mut achievements = Vec::new();
        for (id, hit) in reached {
            if hit && !self.is_done(id) {
                achievements.push(self.complete(id));
            }
        }
        achievements
    }

    fn complete(&mut self, id: MissionId) -> Achievement {
        let before = self.level().level;
        let xp = mission(id).xp;
        self.completed.push(id);
        self.xp += xp;
        let after = self.level().level;
        Achievement {
            mission: id,
            xp,
            level_up: (after > before).then_some(after),
            all_complete: self.all_complete(),
        }
    }
}

/// 部品の編集・追加による履歴か (読み込みは数えない)。
/// 履歴の名前は `history` が「◯◯ を編集」「◯◯ を追加」の形で付ける。
fn is_edit_label(label: &str) -> bool {
    label.ends_with("を編集") || label.ends_with("を追加")
}

/// ミッションの表示と進み具合。アプリを閉じても覚えておく。
#[derive(Clone, Debug)]
pub struct Preferences {
    pub show_missions: bool,
    pub progress: Progress,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            show_missions: true,
            progress: Progress::default(),
        }
    }
}

/// 紙吹雪の1片。位置は表示領域の幅・高さを 1 とした割合。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConfettiPiece {
    pub x0: f32,
    pub vx: f32,
    pub vy: f32,
    pub spin: f32,
    pub color: usize,
}

/// 紙吹雪の重力 (表示領域の高さ / 秒²)
const CONFETTI_GRAVITY: f32 = 1.4;

impl ConfettiPiece {
    /// 打ち上げてから `t` 秒後の位置 (x, y) と回転角
    pub fn at(&self, t: f32) -> (f32, f32, f32) {
        let x = self.x0 + self.vx * t;
        let y = self.vy * t + 0.5 * CONFETTI_GRAVITY * t * t;
        (x, y, self.spin * t)
    }
}

/// 上端から降らせる紙吹雪を `count` 片作る。同じ `seed` なら同じ並び。
pub fn confetti(seed: u64, count: usize, colors: usize) -> Vec<ConfettiPiece> {
    let mut state = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut next = || {
        // xorshift64*
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        (state.wrapping_mul(0x2545F4914F6CDD1D) >> 40) as f32 / (1u64 << 24) as f32
    };
    (0..count)
        .map(|_| ConfettiPiece {
            x0: next(),
            vx: (next() - 0.5) * 0.3,
            vy: -0.2 - next() * 0.4,
            spin: (next() - 0.5) * 12.0,
            color: (next() * colors.max(1) as f32) as usize % colors.max(1),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observe(progress: &mut Progress, edit: impl FnOnce(&mut Observation)) -> Vec<MissionId> {
        let mut now = progress.last.clone().unwrap_or_default();
        edit(&mut now);
        progress
            .observe(&now)
            .into_iter()
            .map(|a| a.mission)
            .collect()
    }

    #[test]
    fn first_observation_only_sets_baseline() {
        let mut progress = Progress::default();
        let start = Observation {
            selected: true,
            interfering_pairs: 3,
            ..Default::default()
        };
        assert!(progress.observe(&start).is_empty());
        assert_eq!(progress.done_count(), 0);
    }

    #[test]
    fn missions_complete_on_change() {
        let mut progress = Progress::default();
        observe(&mut progress, |_| {});
        assert_eq!(
            observe(&mut progress, |o| o.selected = true),
            [MissionId::SelectPart]
        );
        assert_eq!(
            observe(&mut progress, |o| o.measurement_count = 1),
            [MissionId::Measure]
        );
        assert_eq!(
            observe(&mut progress, |o| o.orthographic = true),
            [MissionId::SwitchProjection]
        );
        // 2回目は数えない
        assert!(observe(&mut progress, |o| o.selected = false).is_empty());
        assert!(observe(&mut progress, |o| o.selected = true).is_empty());
        assert_eq!(progress.xp(), 50 + 80 + 50);
    }

    #[test]
    fn loading_a_file_is_not_an_edit() {
        let mut progress = Progress::default();
        observe(&mut progress, |_| {});
        assert!(
            observe(&mut progress, |o| o.last_undo_label =
                Some("a.step を読み込み".into()))
            .is_empty()
        );
        assert_eq!(
            observe(&mut progress, |o| o.last_undo_label =
                Some("箱1 を編集".into())),
            [MissionId::EditPart]
        );
    }

    #[test]
    fn orbit_needs_enough_rotation() {
        let mut progress = Progress::default();
        observe(&mut progress, |_| {});
        // 全体表示などでカメラが自動で動いても数えない
        assert!(observe(&mut progress, |o| o.camera_angles = [2.0, 1.0]).is_empty());
        observe(&mut progress, |o| o.dragging_view = true);
        assert!(observe(&mut progress, |o| o.camera_angles = [2.3, 1.0]).is_empty());
        assert_eq!(
            observe(&mut progress, |o| o.camera_angles = [2.3, 1.4]),
            [MissionId::OrbitView]
        );
    }

    #[test]
    fn interference_counts_only_after_editing() {
        let mut progress = Progress::default();
        observe(&mut progress, |_| {});
        // 読み込んだ直後からある干渉は数えない
        assert!(observe(&mut progress, |o| o.interfering_pairs = 2).is_empty());
        assert_eq!(
            observe(&mut progress, |o| o.last_undo_label =
                Some("箱1 を編集".into())),
            [MissionId::EditPart]
        );
        assert_eq!(
            observe(&mut progress, |o| o.interfering_pairs = 3),
            [MissionId::FindInterference]
        );
    }

    #[test]
    fn level_up_and_all_complete() {
        let mut progress = Progress::default();
        let mut last = None;
        for m in MISSIONS {
            last = Some(progress.complete(m.id));
        }
        let last = last.unwrap();
        assert!(last.all_complete);
        assert!(progress.next().is_none());
        let total: u32 = MISSIONS.iter().map(|m| m.xp).sum();
        assert_eq!(progress.xp(), total);
        assert_eq!(progress.level().level, 1 + total / XP_PER_LEVEL);

        let mut fresh = Progress::default();
        let first = fresh.complete(MissionId::PlayChart);
        assert_eq!(first.level_up, None);
        let second = fresh.complete(MissionId::FindInterference);
        assert_eq!(second.level_up, Some(2));
    }

    #[test]
    fn restore_round_trip() {
        let mut progress = Progress::default();
        progress.complete(MissionId::Save);
        progress.complete(MissionId::Undo);
        let keys = progress.completed_keys();
        let restored = Progress::restore(keys.iter().copied().chain(["unknown"]), progress.xp());
        assert!(restored.is_done(MissionId::Save));
        assert!(restored.is_done(MissionId::Undo));
        assert_eq!(restored.done_count(), 2);
        assert_eq!(restored.next().unwrap().id, MissionId::SelectPart);
    }

    #[test]
    fn mission_keys_are_unique() {
        for (i, a) in MISSIONS.iter().enumerate() {
            for b in &MISSIONS[i + 1..] {
                assert_ne!(a.key, b.key);
                assert_ne!(a.id, b.id);
            }
        }
    }

    #[test]
    fn confetti_falls_and_is_deterministic() {
        let a = confetti(7, 40, 5);
        assert_eq!(a, confetti(7, 40, 5));
        assert_eq!(a.len(), 40);
        for piece in &a {
            assert!((0.0..1.0).contains(&piece.x0));
            assert!(piece.color < 5);
            // 最初は上に飛び、やがて下に落ちる
            assert!(piece.at(0.05).1 < 0.0);
            assert!(piece.at(3.0).1 > 1.0);
        }
    }
}
