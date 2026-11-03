//! ゲーム風の UI の中身: チュートリアルの章とミッション、経験値とレベル、達成時の紙吹雪、ミッションの表示の設定
//!
//! Bevy・egui・ファイル形式に依存しない。アプリ側は毎フレーム [`Observation`] を作って
//! [`Progress::observe`] に渡し、返ってきた達成を画面に出すだけにする。

mod confetti;
mod guide;
mod missions;

pub use confetti::{ConfettiPiece, confetti};
pub use guide::{GuideTarget, When, guide_for, next_target};
pub use missions::MISSIONS;

use crate::workspace::Tool;

/// チュートリアルの章。基本編は「見る → さわる → ラインを組む → 仕上げる」、
/// 応用編は「動きを付ける → ラインとして動かす → 使いこなす」の順にまとめる。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Chapter {
    Look,
    Touch,
    BuildLine,
    Finish,
    Motion,
    RunLine,
    Master,
}

/// 章のまとまり。基本編で主な操作をひと回りし、応用編で設備の動き・PLC・AI などを覚える。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Part {
    Basic,
    Advanced,
}

impl Part {
    pub const ALL: [Part; 2] = [Part::Basic, Part::Advanced];

    pub fn title(self) -> &'static str {
        match self {
            Part::Basic => "基本編",
            Part::Advanced => "応用編",
        }
    }

    /// ようこそ画面に出す説明
    pub fn summary(self) -> &'static str {
        match self {
            Part::Basic => "見る → さわる → ラインを組む → 保存する (5 分ほど)",
            Part::Advanced => "動きを付ける → PLC・ライン連携で動かす → AI・一人称・動画",
        }
    }

    /// このまとまりの章
    pub fn chapters(self) -> impl Iterator<Item = Chapter> {
        Chapter::ALL.into_iter().filter(move |c| c.part() == self)
    }
}

impl Chapter {
    pub const ALL: [Chapter; 7] = [
        Chapter::Look,
        Chapter::Touch,
        Chapter::BuildLine,
        Chapter::Finish,
        Chapter::Motion,
        Chapter::RunLine,
        Chapter::Master,
    ];

    pub fn part(self) -> Part {
        match self {
            Chapter::Look | Chapter::Touch | Chapter::BuildLine | Chapter::Finish => Part::Basic,
            Chapter::Motion | Chapter::RunLine | Chapter::Master => Part::Advanced,
        }
    }

    /// 1 始まりの章番号
    pub fn number(self) -> usize {
        Self::ALL.iter().position(|&c| c == self).unwrap_or(0) + 1
    }

    pub fn title(self) -> &'static str {
        match self {
            Chapter::Look => "動くラインを見てみよう",
            Chapter::Touch => "部品にさわってみよう",
            Chapter::BuildLine => "ラインを組んでみよう",
            Chapter::Finish => "仕上げて残そう",
            Chapter::Motion => "設備に動きを付けよう",
            Chapter::RunLine => "ラインとして動かそう",
            Chapter::Master => "もっと使いこなそう",
        }
    }

    /// この章のミッション (おすすめ順)
    pub fn missions(self) -> impl Iterator<Item = &'static Mission> {
        MISSIONS.iter().filter(move |m| m.chapter == self)
    }
}

/// 操作する場所。アプリ側はここを光らせたり矢印で指したりする。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    /// 3D ビューの中
    Viewport,
    /// 左のアセンブリツリー
    Tree,
    /// 画面下のタイミングチャート
    Chart,
    /// 上のメニューバー
    Menu,
    /// 画面左端のツールレール (道具のボタン)
    Rail,
    /// キーボード
    Keyboard,
}

/// チュートリアルのミッション。並び順がおすすめの進め方。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MissionId {
    SelectPart,
    OrbitView,
    PlayChart,
    SwitchProjection,
    EditPart,
    Undo,
    Measure,
    FindInterference,
    PlaceLibraryPart,
    RunFlow,
    Annotate,
    Save,
    AddActuator,
    AddStep,
    AddSensor,
    EmergencyStop,
    RunPlc,
    RunLineLink,
    ComposeLine,
    ExportLadder,
    WalkAround,
    ExportVideo,
}

pub struct Mission {
    pub id: MissionId,
    /// 保存ファイルで使う名前 (変えない)
    pub key: &'static str,
    pub chapter: Chapter,
    pub title: &'static str,
    /// 画面のどこで何をすればよいか (1行)
    pub hint: &'static str,
    /// チュートリアルの手順。上から順にやればクリアできる
    pub steps: &'static [&'static str],
    /// この操作が何の役に立つか
    pub why: &'static str,
    /// 操作する場所
    pub place: Place,
    /// 設備が動いている間はできない (ギズモでの編集は設計位置で止まっているときだけ)
    pub needs_stopped_line: bool,
    /// 使うキー (カードにキーの形で出す)
    pub keys: &'static [&'static str],
    /// 使う道具。カードに「開く」ボタンを出す
    pub tool: Option<Tool>,
    pub xp: u32,
}

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

/// その時点の画面の状態。アプリ側が毎フレーム集める。
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
    /// 標準部品ライブラリから置いた部品の数
    pub library_part_count: usize,
    /// 工程シミュレーションの結果が出ている
    pub flow_has_result: bool,
    /// タイミングチャートのアクチュエータに割り当てた部品の数 (全アクチュエータの合計)
    pub actuated_part_count: usize,
    /// タイミングチャートのステップの数
    pub step_count: usize,
    /// タイミングチャートのセンサーの数
    pub sensor_count: usize,
    /// 安全パネルで非常停止を押しているか、センサーを遮光している
    pub safety_tripped: bool,
    /// 内蔵の模擬 PLC で運転している
    pub plc_running: bool,
    /// ライン連携でラインが動いている
    pub line_running: bool,
    /// 「言葉でラインを組む」でモデルに置いた回数
    pub composed_count: u32,
    /// ラダーか I/O 表を書き出した (コピーした) 回数
    pub ladder_export_count: u32,
    /// 一人称で歩いている
    pub walking: bool,
    /// 動画を書き出し終えた回数
    pub video_export_count: u32,
}

/// ミッションを1つ達成したときの知らせ
#[derive(Clone, Debug, PartialEq)]
pub struct Achievement {
    pub mission: MissionId,
    pub xp: u32,
    /// レベルが上がったら、新しいレベル
    pub level_up: Option<u32>,
    /// これで章のミッションが全部終わったら、その章
    pub chapter_complete: Option<Chapter>,
    /// これで基本編・応用編のミッションが全部終わったら、そのまとまり
    pub part_complete: Option<Part>,
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
    /// 「とばす」を押したミッション。あとで達成すれば経験値がもらえる
    skipped: Vec<MissionId>,
    xp: u32,
    /// 直前の状態 (変化を見つけるのに使う)。最初の観測では何も達成しない。
    last: Option<Observation>,
    /// 見まわしたカメラの回転量の合計
    orbit_travel: f64,
    /// 起動してから部品を編集したか (最初からある干渉を数えないため)
    edited: bool,
    /// 一覧の「挑戦」でえらんだミッション。おすすめ順より先にこれを出す (保存しない)
    focus: Option<MissionId>,
}

impl Progress {
    /// 保存しておいた進み具合から作る。知らない名前は無視する。
    pub fn restore<'a>(keys: impl IntoIterator<Item = &'a str>, xp: u32) -> Self {
        Self {
            completed: ids_of(keys, &[]),
            xp,
            ..Default::default()
        }
    }

    /// とばしたミッションも戻す。達成済みのものは達成のままにする。
    pub fn with_skipped<'a>(mut self, keys: impl IntoIterator<Item = &'a str>) -> Self {
        self.skipped = ids_of(keys, &self.completed);
        self
    }

    pub fn completed_keys(&self) -> Vec<&'static str> {
        self.completed.iter().map(|&id| mission(id).key).collect()
    }

    pub fn skipped_keys(&self) -> Vec<&'static str> {
        self.skipped.iter().map(|&id| mission(id).key).collect()
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

    pub fn is_skipped(&self, id: MissionId) -> bool {
        self.skipped.contains(&id)
    }

    /// 達成したか、とばした
    fn is_settled(&self, id: MissionId) -> bool {
        self.is_done(id) || self.is_skipped(id)
    }

    pub fn done_count(&self) -> usize {
        self.completed.len()
    }

    /// 達成したかとばしたミッションの数 (「ミッション 3/12」の表示に使う)
    pub fn settled_count(&self) -> usize {
        self.completed.len() + self.skipped.len()
    }

    /// 全部のミッションを達成した (とばしたものは含めない)
    pub fn all_complete(&self) -> bool {
        self.completed.len() == MISSIONS.len()
    }

    /// 次に挑戦するミッション。一覧でえらんだものがあればそれ、
    /// なければおすすめ順で最初の、達成もとばしもしていないもの
    pub fn next(&self) -> Option<&'static Mission> {
        if let Some(id) = self.focus.filter(|&id| !self.is_done(id)) {
            return Some(mission(id));
        }
        MISSIONS.iter().find(|m| !self.is_settled(m.id))
    }

    /// 一覧でえらんだミッションに今すぐ挑戦する (とばしていたら候補に戻す)
    pub fn focus(&mut self, id: MissionId) {
        if self.is_done(id) {
            return;
        }
        self.skipped.retain(|&s| s != id);
        self.focus = Some(id);
    }

    /// 一覧でえらんで挑戦しているミッション
    pub fn focused(&self) -> Option<MissionId> {
        self.focus.filter(|&id| !self.is_done(id))
    }

    /// 章の (達成した数, ミッションの数)
    pub fn chapter_progress(&self, chapter: Chapter) -> (usize, usize) {
        let total = chapter.missions().count();
        let done = chapter.missions().filter(|m| self.is_done(m.id)).count();
        (done, total)
    }

    /// 今のミッションをとばして次へ進む。経験値はもらえない。
    pub fn skip(&mut self, id: MissionId) {
        if self.focus == Some(id) {
            self.focus = None;
        }
        if !self.is_settled(id) {
            self.skipped.push(id);
        }
    }

    /// とばしたミッションをもう一度「次のミッション」の候補に戻す
    pub fn retry(&mut self, id: MissionId) {
        self.skipped.retain(|&s| s != id);
    }

    /// 次の観測を基準にし直す。プロジェクトを開いたときなど、操作によらず画面が変わるときに呼ぶ
    /// (開いたファイルに寸法や標準部品があっても、ミッション達成にしない)。
    pub fn rebase(&mut self) {
        self.last = None;
        self.orbit_travel = 0.0;
        self.edited = false;
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
            (
                MissionId::PlaceLibraryPart,
                now.library_part_count > before.library_part_count,
            ),
            (
                MissionId::RunFlow,
                now.flow_has_result && !before.flow_has_result,
            ),
            (
                MissionId::AddActuator,
                added_one(before.actuated_part_count, now.actuated_part_count),
            ),
            (
                MissionId::AddStep,
                added_one(before.step_count, now.step_count),
            ),
            (
                MissionId::AddSensor,
                added_one(before.sensor_count, now.sensor_count),
            ),
            (
                MissionId::EmergencyStop,
                now.safety_tripped && !before.safety_tripped,
            ),
            (MissionId::RunPlc, now.plc_running && !before.plc_running),
            (
                MissionId::RunLineLink,
                now.line_running && !before.line_running,
            ),
            (
                MissionId::ComposeLine,
                now.composed_count > before.composed_count,
            ),
            (
                MissionId::ExportLadder,
                now.ladder_export_count > before.ladder_export_count,
            ),
            (MissionId::WalkAround, now.walking && !before.walking),
            (
                MissionId::ExportVideo,
                now.video_export_count > before.video_export_count,
            ),
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
        let m = mission(id);
        self.skipped.retain(|&s| s != id);
        if self.focus == Some(id) {
            self.focus = None;
        }
        self.completed.push(id);
        self.xp += m.xp;
        let after = self.level().level;
        let (done, total) = self.chapter_progress(m.chapter);
        let part = m.chapter.part();
        let part_done = MISSIONS
            .iter()
            .filter(|other| other.chapter.part() == part)
            .all(|other| self.is_done(other.id));
        Achievement {
            mission: id,
            xp: m.xp,
            level_up: (after > before).then_some(after),
            chapter_complete: (done == total).then_some(m.chapter),
            part_complete: part_done.then_some(part),
            all_complete: self.all_complete(),
        }
    }
}

/// 保存名からミッションを引く。知らない名前・重複・`except` にあるものは除く。
fn ids_of<'a>(keys: impl IntoIterator<Item = &'a str>, except: &[MissionId]) -> Vec<MissionId> {
    let mut ids = Vec::new();
    for key in keys {
        if let Some(m) = MISSIONS.iter().find(|m| m.key == key)
            && !ids.contains(&m.id)
            && !except.contains(&m.id)
        {
            ids.push(m.id);
        }
    }
    ids
}

/// ちょうど1つ増えたか。手で足すと1つずつ増える。サンプル装置の読み込みなどで
/// まとめて増えたときは、ミッションの操作をしたことにしない
fn added_one(before: usize, now: usize) -> bool {
    now == before + 1
}

/// 部品の編集・追加による履歴か (読み込みは数えない)。
/// 履歴の名前は `history` が「◯◯ を編集」「◯◯ を追加」の形で付ける。
fn is_edit_label(label: &str) -> bool {
    label.ends_with("を編集") || label.ends_with("を追加")
}

/// ミッションの表示と進み具合、画面の好み。アプリを閉じても覚えておく。
#[derive(Clone, Debug)]
pub struct Preferences {
    pub show_missions: bool,
    pub progress: Progress,
    /// 初回のようこそ画面を閉じた
    pub welcomed: bool,
    /// 3D ビューに浮かぶウィンドウ (音の設定など) を、カーソルが乗っていない間は透かす
    pub see_through_panels: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            show_missions: true,
            progress: Progress::default(),
            welcomed: false,
            see_through_panels: true,
        }
    }
}

#[cfg(test)]
mod tests;
