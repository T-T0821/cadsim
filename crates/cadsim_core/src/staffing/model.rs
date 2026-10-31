//! 人員配置のデータ: 作業者の名簿・できる工程 (スキル)・シフトと休憩 (Bevy に依存しない)
//!
//! - **作業者** は名前、勤務するシフト、欠勤かどうか、できる工程とその速さを持つ。
//! - **シフト** は始業・終業の時刻と休憩の時間帯。毎日同じ時刻に繰り返す。
//!   終業が始業より早ければ日をまたぐ (例: 22:00〜翌 7:00)。
//! - 工程のほうには「人が要るか」([`Labor`]) を持たせる (`flow::model::Element::labor`)。
//!
//! 時刻はすべて 0:00 からの秒。シミュレーションの 0 秒が何時かは [`Staffing::start_clock`] で決める。

use crate::flow::model::ElementId;

pub type WorkerId = u32;
pub type ShiftId = u32;

/// 1 日の秒数
pub const DAY: f64 = 24.0 * 3600.0;

/// 工程に人が要るか
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Labor {
    /// 人は要らない (自動で動く)
    #[default]
    None,
    /// 加工しているあいだずっと 1 人付く (手作業・組立など)
    Full,
    /// 1 個ごとの最初の `seconds` 秒だけ 1 人付く (投入・段取り・取り出しなど)。
    /// そのあとは人がいなくても機械が加工を続ける。
    Load { seconds: f64 },
}

impl Labor {
    pub fn needs_operator(self) -> bool {
        !matches!(self, Labor::None)
    }

    pub fn label(self) -> &'static str {
        match self {
            Labor::None => "不要 (自動)",
            Labor::Full => "加工中ずっと必要",
            Labor::Load { .. } => "投入・段取りだけ必要",
        }
    }
}

/// 名簿とシフト
#[derive(Clone, Debug, PartialEq)]
pub struct Staffing {
    pub workers: Vec<Worker>,
    pub shifts: Vec<Shift>,
    /// シミュレーションの 0 秒の時刻 (0:00 からの秒)。既定は 8:00。
    pub start_clock: f64,
    /// 別の工程へ移るときの歩行時間 (秒)。同じ工程を続けるときはかからない。
    pub walk_seconds: f64,
    next_worker: WorkerId,
    next_shift: ShiftId,
}

impl Default for Staffing {
    fn default() -> Self {
        Self {
            workers: Vec::new(),
            shifts: Vec::new(),
            start_clock: 8.0 * 3600.0,
            walk_seconds: 0.0,
            next_worker: 1,
            next_shift: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Worker {
    pub id: WorkerId,
    pub name: String,
    /// 勤務するシフト。None なら休みなしで回す時間ずっといる。
    pub shift: Option<ShiftId>,
    /// 欠勤。名簿に残したまま、この人がいない場合を試せる。
    pub absent: bool,
    /// できる工程と速さ
    pub skills: Vec<Skill>,
}

/// できる工程1つ分
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Skill {
    pub element: ElementId,
    /// 作業の速さ。1.0 が標準、1.25 なら標準より 25% 速い (人の作業時間が 1/1.25 になる)。
    pub speed: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shift {
    pub id: ShiftId,
    pub name: String,
    /// 始業の時刻 (0:00 からの秒)
    pub start: f64,
    /// 終業の時刻。始業より早ければ翌日。始業と同じなら 24 時間。
    pub end: f64,
    pub breaks: Vec<Break>,
}

/// 休憩 (時刻は 0:00 からの秒)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Break {
    pub start: f64,
    pub end: f64,
}

/// 作業の速さの範囲
pub const MIN_SPEED: f64 = 0.1;
pub const MAX_SPEED: f64 = 10.0;

impl Staffing {
    pub fn is_empty(&self) -> bool {
        self.workers.is_empty() && self.shifts.is_empty()
    }

    pub fn worker(&self, id: WorkerId) -> Option<&Worker> {
        self.workers.iter().find(|w| w.id == id)
    }

    pub fn worker_mut(&mut self, id: WorkerId) -> Option<&mut Worker> {
        self.workers.iter_mut().find(|w| w.id == id)
    }

    pub fn shift(&self, id: ShiftId) -> Option<&Shift> {
        self.shifts.iter().find(|s| s.id == id)
    }

    pub fn shift_mut(&mut self, id: ShiftId) -> Option<&mut Shift> {
        self.shifts.iter_mut().find(|s| s.id == id)
    }

    /// 作業者を足す。名前は「作業者1」のように番号で付ける。シフトは先頭のもの。
    pub fn add_worker(&mut self) -> WorkerId {
        let name = format!("作業者{}", self.workers.len() + 1);
        let shift = self.shifts.first().map(|s| s.id);
        self.insert_worker(Worker {
            id: 0,
            name,
            shift,
            absent: false,
            skills: Vec::new(),
        })
    }

    /// 作業者をそのまま入れる。ID は振り直す。
    pub fn insert_worker(&mut self, mut worker: Worker) -> WorkerId {
        worker.id = self.next_worker;
        self.next_worker += 1;
        let id = worker.id;
        self.workers.push(worker);
        id
    }

    /// ID を保ったまま入れる (読み込み用)。同じ ID がすでにあれば false。
    pub fn insert_worker_with_id(&mut self, worker: Worker) -> bool {
        if self.worker(worker.id).is_some() {
            return false;
        }
        self.next_worker = self.next_worker.max(worker.id + 1);
        self.workers.push(worker);
        true
    }

    pub fn remove_worker(&mut self, id: WorkerId) {
        self.workers.retain(|w| w.id != id);
    }

    /// シフトを足す。既定は 8:00〜17:00、昼休み 12:00〜13:00。
    pub fn add_shift(&mut self) -> ShiftId {
        let name = format!("シフト{}", self.shifts.len() + 1);
        self.insert_shift(Shift {
            id: 0,
            name,
            start: 8.0 * 3600.0,
            end: 17.0 * 3600.0,
            breaks: vec![Break {
                start: 12.0 * 3600.0,
                end: 13.0 * 3600.0,
            }],
        })
    }

    pub fn insert_shift(&mut self, mut shift: Shift) -> ShiftId {
        shift.id = self.next_shift;
        self.next_shift += 1;
        let id = shift.id;
        self.shifts.push(shift);
        id
    }

    pub fn insert_shift_with_id(&mut self, shift: Shift) -> bool {
        if self.shift(shift.id).is_some() {
            return false;
        }
        self.next_shift = self.next_shift.max(shift.id + 1);
        self.shifts.push(shift);
        true
    }

    /// シフトを消す。そのシフトの作業者は「シフトなし」になる。
    pub fn remove_shift(&mut self, id: ShiftId) {
        self.shifts.retain(|s| s.id != id);
        for w in &mut self.workers {
            if w.shift == Some(id) {
                w.shift = None;
            }
        }
    }

    /// 工程が消えたときに、その工程のスキルを消す
    pub fn forget_element(&mut self, element: ElementId) {
        for w in &mut self.workers {
            w.skills.retain(|s| s.element != element);
        }
    }

    /// 出勤している (欠勤でない) 作業者の数
    pub fn present_count(&self) -> usize {
        self.workers.iter().filter(|w| !w.absent).count()
    }

    /// 出勤している作業者のうち、名簿の上から `headcount` 人だけにした名簿。
    /// 人数を変えて比べるときに使う。
    pub fn with_headcount(&self, headcount: usize) -> Staffing {
        let mut out = self.clone();
        let mut kept = 0;
        for w in &mut out.workers {
            if w.absent {
                continue;
            }
            if kept < headcount {
                kept += 1;
            } else {
                w.absent = true;
            }
        }
        out
    }

    /// 回せない理由
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for w in &self.workers {
            if let Some(s) = w.shift
                && self.shift(s).is_none()
            {
                out.push(format!("{}: シフト #{s} がありません", w.name));
            }
            if w.skills
                .iter()
                .any(|s| !(MIN_SPEED..=MAX_SPEED).contains(&s.speed))
            {
                out.push(format!(
                    "{}: 作業の速さは {:.0}〜{:.0}% にしてください",
                    w.name,
                    MIN_SPEED * 100.0,
                    MAX_SPEED * 100.0
                ));
            }
        }
        for s in &self.shifts {
            let clock = |v: f64| (0.0..DAY).contains(&v);
            if !clock(s.start) || !clock(s.end) {
                out.push(format!("{}: 時刻は 0:00〜23:59 にしてください", s.name));
            }
            for b in &s.breaks {
                if !clock(b.start) || !clock(b.end) {
                    out.push(format!(
                        "{}: 休憩の時刻は 0:00〜23:59 にしてください",
                        s.name
                    ));
                } else if !s.contains_break(b) {
                    out.push(format!(
                        "{}: 休憩 {}〜{} が勤務時間の外にあります",
                        s.name,
                        clock_label(b.start),
                        clock_label(b.end)
                    ));
                }
            }
        }
        if !(self.walk_seconds >= 0.0 && self.walk_seconds.is_finite()) {
            out.push("歩行時間は 0 秒以上にしてください".into());
        }
        if !(0.0..DAY).contains(&self.start_clock) {
            out.push("始める時刻は 0:00〜23:59 にしてください".into());
        }
        out
    }
}

impl Worker {
    /// この工程の作業の速さ。できない工程なら None。
    pub fn speed_for(&self, element: ElementId) -> Option<f64> {
        self.skills
            .iter()
            .find(|s| s.element == element)
            .map(|s| s.speed)
    }

    /// できる工程を足す・外す。足すときの速さは標準 (1.0)。
    pub fn set_skill(&mut self, element: ElementId, can: bool) {
        let has = self.speed_for(element).is_some();
        if can && !has {
            self.skills.push(Skill {
                element,
                speed: 1.0,
            });
        } else if !can {
            self.skills.retain(|s| s.element != element);
        }
    }
}

impl Shift {
    /// 勤務の長さ (秒)
    fn length(&self) -> f64 {
        let len = (self.end - self.start).rem_euclid(DAY);
        if len == 0.0 { DAY } else { len }
    }

    /// 休憩の、始業からの区間 [開始, 終了)
    fn break_span(&self, b: &Break) -> (f64, f64) {
        let from = (b.start - self.start).rem_euclid(DAY);
        let len = (b.end - b.start).rem_euclid(DAY);
        (from, from + len)
    }

    fn contains_break(&self, b: &Break) -> bool {
        let (from, to) = self.break_span(b);
        to <= self.length() && to > from
    }

    /// その時刻に働いているか (勤務時間内で、休憩中でない)
    pub fn on_duty_at(&self, clock: f64) -> bool {
        let offset = (clock - self.start).rem_euclid(DAY);
        if offset >= self.length() {
            return false;
        }
        !self.breaks.iter().any(|b| {
            let (from, to) = self.break_span(b);
            (from..to).contains(&offset)
        })
    }

    /// その時刻から、働いている・いないが次に変わるまでの秒数 (0 より大きい)
    pub fn next_change_after(&self, clock: f64) -> f64 {
        let offset = (clock - self.start).rem_euclid(DAY);
        let len = self.length();
        let mut marks = vec![0.0];
        if len < DAY {
            marks.push(len);
        }
        for b in &self.breaks {
            let (from, to) = self.break_span(b);
            if to <= len {
                marks.push(from);
                marks.push(to);
            }
        }
        marks
            .iter()
            .map(|&m| {
                let d = m - offset;
                if d > 1e-9 { d } else { d + DAY }
            })
            .fold(DAY, f64::min)
    }
}

/// 時刻の表示 (「8:05」)
pub fn clock_label(clock: f64) -> String {
    let minutes = (clock.rem_euclid(DAY) / 60.0).round() as u32;
    format!("{}:{:02}", (minutes / 60) % 24, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: f64 = 3600.0;

    fn day_shift() -> Shift {
        Shift {
            id: 1,
            name: "日勤".into(),
            start: 8.0 * H,
            end: 17.0 * H,
            breaks: vec![Break {
                start: 12.0 * H,
                end: 13.0 * H,
            }],
        }
    }

    #[test]
    fn day_shift_has_lunch_off() {
        let s = day_shift();
        assert!(!s.on_duty_at(7.5 * H));
        assert!(s.on_duty_at(8.0 * H));
        assert!(s.on_duty_at(11.9 * H));
        assert!(!s.on_duty_at(12.5 * H));
        assert!(s.on_duty_at(13.0 * H));
        assert!(!s.on_duty_at(17.0 * H));
        assert_eq!(s.next_change_after(8.0 * H), 4.0 * H);
        assert_eq!(s.next_change_after(12.5 * H), 0.5 * H);
        assert_eq!(s.next_change_after(16.0 * H), 1.0 * H);
        // 終業後は翌朝の始業まで
        assert_eq!(s.next_change_after(20.0 * H), 12.0 * H);
    }

    #[test]
    fn night_shift_crosses_midnight() {
        let s = Shift {
            id: 2,
            name: "夜勤".into(),
            start: 22.0 * H,
            end: 7.0 * H,
            breaks: vec![Break {
                start: 2.0 * H,
                end: 3.0 * H,
            }],
        };
        assert!(s.on_duty_at(23.0 * H));
        assert!(s.on_duty_at(1.0 * H));
        assert!(!s.on_duty_at(2.5 * H));
        assert!(s.on_duty_at(6.9 * H));
        assert!(!s.on_duty_at(7.0 * H));
        assert_eq!(s.next_change_after(23.0 * H), 3.0 * H);
        let mut staffing = Staffing::default();
        staffing.insert_shift(s);
        assert!(staffing.problems().is_empty());
    }

    #[test]
    fn breaks_outside_the_shift_are_reported() {
        let mut s = day_shift();
        s.breaks.push(Break {
            start: 18.0 * H,
            end: 18.5 * H,
        });
        let mut staffing = Staffing::default();
        staffing.insert_shift(s);
        let problems = staffing.problems().join("\n");
        assert!(problems.contains("18:00〜18:30"), "{problems}");
    }

    #[test]
    fn headcount_keeps_the_first_present_workers() {
        let mut s = Staffing::default();
        for _ in 0..4 {
            s.add_worker();
        }
        s.workers[1].absent = true;
        let two = s.with_headcount(2);
        let present: Vec<_> = two
            .workers
            .iter()
            .filter(|w| !w.absent)
            .map(|w| w.name.as_str())
            .collect();
        assert_eq!(present, ["作業者1", "作業者3"]);
        assert_eq!(s.present_count(), 3);
    }

    #[test]
    fn removing_a_shift_frees_its_workers() {
        let mut s = Staffing::default();
        let shift = s.add_shift();
        let w = s.add_worker();
        assert_eq!(s.worker(w).unwrap().shift, Some(shift));
        s.remove_shift(shift);
        assert_eq!(s.worker(w).unwrap().shift, None);
    }

    #[test]
    fn clock_labels() {
        assert_eq!(clock_label(8.0 * H + 5.0 * 60.0), "8:05");
        assert_eq!(clock_label(0.0), "0:00");
    }
}
