//! 設備を床に並べる
//!
//! 位置の指定がない設備は、次の決まりで並べる (単位は mm、Y が上、コンベアは +X に流れる)。
//!
//! - コンベアは X 向きに、Z 方向へ間隔をあけて平行に並べる。
//! - ロボットはコンベアとコンベアの間 (1本ならその手前) の、下流の端の近くに置く。
//! - 作業台はロボットの向こう側に置く。
//! - シリンダとセンサーは順にコンベアへ割り当て、下流の端の両脇に付ける。

use super::model::{EquipmentKind, LineDraft, Verb};

/// コンベアの標準の長さ
pub const CONVEYOR_LENGTH: f64 = 2000.0;
pub const CONVEYOR_WIDTH: f64 = 450.0;
/// コンベアの搬送面の高さ
pub const CONVEYOR_HEIGHT: f64 = 750.0;
/// 平行に並べるコンベアの中心の間隔
pub const ROW_GAP: f64 = 1600.0;
pub const TABLE_SIZE: [f64; 3] = [700.0, 700.0, 600.0];
pub const ROBOT_BASE: [f64; 3] = [350.0, 1000.0, 350.0];
pub const CYLINDER_BODY: [f64; 3] = [120.0, 100.0, 250.0];
pub const SENSOR_BODY: [f64; 3] = [40.0, 850.0, 40.0];

/// 並べた設備1台
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    /// 床の上の中心 [x, z]
    pub center: [f64; 2],
    /// 外形 [X, Y(高さ), Z]。ロボットは土台だけの大きさ。
    pub size: [f64; 3],
    /// シリンダ・センサーを付けたコンベアの番号 (設備の並びでの位置)
    pub attached_to: Option<usize>,
    /// シリンダ・センサーがコンベアの +Z 側にあれば true (押す向き・光軸は -Z)
    pub on_far_side: bool,
}

impl Placed {
    /// 外形の中心 (3D)
    pub fn center3(&self) -> [f64; 3] {
        [self.center[0], self.size[1] / 2.0, self.center[1]]
    }

    pub fn top(&self) -> f64 {
        self.size[1]
    }
}

/// 下書きの設備を並べる。結果は `draft.equipment` と同じ順。
pub fn layout(draft: &LineDraft) -> Vec<Placed> {
    let kinds: Vec<EquipmentKind> = draft.equipment.iter().map(|e| e.kind).collect();
    let mut placed: Vec<Option<Placed>> = vec![None; kinds.len()];

    // コンベア
    let mut row = 0usize;
    for (i, e) in draft.equipment.iter().enumerate() {
        if e.kind != EquipmentKind::Conveyor {
            continue;
        }
        let length = e.length.unwrap_or(CONVEYOR_LENGTH);
        let center = e.position.unwrap_or_else(|| {
            let c = [0.0, row as f64 * ROW_GAP];
            row += 1;
            c
        });
        placed[i] = Some(Placed {
            center,
            size: [length, CONVEYOR_HEIGHT, CONVEYOR_WIDTH],
            attached_to: None,
            on_far_side: false,
        });
    }
    let conveyors: Vec<usize> = (0..kinds.len())
        .filter(|&i| kinds[i] == EquipmentKind::Conveyor)
        .collect();
    let rows = row;
    let conveyor_end = conveyors
        .iter()
        .filter_map(|&i| placed[i].as_ref())
        .map(|p| p.center[0] + p.size[0] / 2.0)
        .fold(f64::NEG_INFINITY, f64::max);
    let conveyor_end = if conveyor_end.is_finite() {
        conveyor_end
    } else {
        0.0
    };

    // ロボット
    let gaps = rows.saturating_sub(1).max(1);
    let mut robots = Vec::new();
    for (j, (i, e)) in draft
        .equipment
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == EquipmentKind::Robot)
        .enumerate()
    {
        let center = e.position.unwrap_or_else(|| {
            if rows == 0 {
                [j as f64 * 1500.0, 0.0]
            } else {
                let gap = j % gaps;
                let column = j / gaps;
                [
                    conveyor_end - 450.0 - column as f64 * 1200.0,
                    (gap as f64 + 0.5) * ROW_GAP,
                ]
            }
        });
        robots.push(center);
        placed[i] = Some(Placed {
            center,
            size: ROBOT_BASE,
            attached_to: None,
            on_far_side: false,
        });
    }

    // 作業台: ロボットの向こう側 (+Z)。ロボットがなければコンベアの手前に並べる
    for (t, (i, e)) in draft
        .equipment
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == EquipmentKind::Table)
        .enumerate()
    {
        let center = e.position.unwrap_or_else(|| {
            if robots.is_empty() {
                [t as f64 * 1000.0, -ROW_GAP * 0.6]
            } else {
                let r = robots[t % robots.len()];
                let lap = (t / robots.len()) as f64;
                let z = if rows >= 2 {
                    // コンベアの間のロボットからは、通路の下流側に置く
                    r[1]
                } else {
                    r[1] + 900.0
                };
                let x = if rows >= 2 { r[0] + 900.0 } else { r[0] };
                [x + lap * 900.0, z]
            }
        });
        placed[i] = Some(Placed {
            center,
            size: TABLE_SIZE,
            attached_to: None,
            on_far_side: false,
        });
    }

    // シリンダとセンサー: コンベアの下流の端の両脇
    // [種類][コンベア] ごとに付けた数。センサーとシリンダはそれぞれ1本目のコンベアから順に割り当てる
    let mut per_conveyor = [vec![0usize; conveyors.len()], vec![0usize; conveyors.len()]];
    let mut nth = [0usize; 2];
    let mut loose = 0usize;
    for (i, e) in draft
        .equipment
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, EquipmentKind::Cylinder | EquipmentKind::Sensor))
    {
        let is_sensor = e.kind == EquipmentKind::Sensor;
        let side = usize::from(is_sensor);
        let size = if is_sensor {
            SENSOR_BODY
        } else {
            CYLINDER_BODY
        };
        if conveyors.is_empty() || e.position.is_some() {
            let center = e.position.unwrap_or_else(|| {
                loose += 1;
                [(loose - 1) as f64 * 500.0, -ROW_GAP * 0.6]
            });
            placed[i] = Some(Placed {
                center,
                size,
                attached_to: None,
                on_far_side: false,
            });
            continue;
        }
        let c = nth[side] % conveyors.len();
        nth[side] += 1;
        let conveyor = placed[conveyors[c]].clone().expect("コンベアは並べ済み");
        let k = per_conveyor[side][c];
        per_conveyor[side][c] += 1;
        // センサーは +Z 側、シリンダは -Z 側。同じ側に複数あれば上流へずらす
        let x = conveyor.center[0] + conveyor.size[0] / 2.0 - 150.0 - k as f64 * 250.0;
        let half = conveyor.size[2] / 2.0;
        let z = if is_sensor {
            conveyor.center[1] + half + 60.0
        } else {
            conveyor.center[1] - half - size[2] / 2.0 - 30.0
        };
        placed[i] = Some(Placed {
            center: [x, z],
            size,
            attached_to: Some(conveyors[c]),
            on_far_side: is_sensor,
        });
    }

    placed
        .into_iter()
        .map(|p| p.expect("すべての種類を並べる"))
        .collect()
}

/// ロボットが `place` (コンベアか作業台) でワークを取る・置く位置 (3D)。
/// コンベアでは、取るのは下流の端、置くのは中ほど。
pub fn work_point(place: &Placed, kind: EquipmentKind, verb: Verb) -> [f64; 3] {
    let [x, z] = place.center;
    match kind {
        EquipmentKind::Conveyor if verb == Verb::Pick => {
            [x + place.size[0] / 2.0 - 250.0, place.top(), z]
        }
        _ => [x, place.top(), z],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line_draft::model::Equipment;

    fn draft(kinds: &[EquipmentKind]) -> LineDraft {
        LineDraft {
            title: "t".into(),
            equipment: kinds
                .iter()
                .enumerate()
                .map(|(i, k)| Equipment::new(format!("e{i}"), *k))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn conveyors_run_in_parallel_rows_with_robot_between() {
        use EquipmentKind::*;
        let d = draft(&[Conveyor, Robot, Conveyor]);
        let p = layout(&d);
        assert_eq!(p[0].center, [0.0, 0.0]);
        assert_eq!(p[2].center, [0.0, ROW_GAP]);
        assert_eq!(p[1].center[1], ROW_GAP / 2.0);
        // 下流の端の近く
        assert!(p[1].center[0] > 0.0 && p[1].center[0] < CONVEYOR_LENGTH / 2.0);
    }

    #[test]
    fn explicit_positions_and_lengths_are_kept() {
        let mut d = draft(&[EquipmentKind::Conveyor]);
        d.equipment[0].position = Some([100.0, 200.0]);
        d.equipment[0].length = Some(3000.0);
        let p = layout(&d);
        assert_eq!(p[0].center, [100.0, 200.0]);
        assert_eq!(p[0].size[0], 3000.0);
    }

    #[test]
    fn sensors_and_cylinders_attach_to_conveyor_ends() {
        use EquipmentKind::*;
        let d = draft(&[Conveyor, Conveyor, Sensor, Cylinder]);
        let p = layout(&d)[1..].to_vec();
        assert_eq!(p[1].attached_to, Some(0));
        assert_eq!(p[2].attached_to, Some(0));
        assert!(p[1].on_far_side);
        assert!(p[1].center[1] > CONVEYOR_WIDTH / 2.0);
        assert!(p[2].center[1] < -CONVEYOR_WIDTH / 2.0);
        assert!(p[1].center[0] > 0.0);
    }

    #[test]
    fn robot_and_table_without_conveyors() {
        use EquipmentKind::*;
        let d = draft(&[Robot, Table, Sensor]);
        let p = layout(&d);
        assert_eq!(p[0].center, [0.0, 0.0]);
        assert_eq!(p[1].center, [0.0, 900.0]);
        assert_eq!(p[2].attached_to, None);
    }

    #[test]
    fn work_points() {
        use EquipmentKind::*;
        let d = draft(&[Conveyor, Table]);
        let p = layout(&d);
        let pick = work_point(&p[0], Conveyor, Verb::Pick);
        assert_eq!(pick, [CONVEYOR_LENGTH / 2.0 - 250.0, CONVEYOR_HEIGHT, 0.0]);
        let place = work_point(&p[0], Conveyor, Verb::Place);
        assert_eq!(place[0], 0.0);
    }
}
