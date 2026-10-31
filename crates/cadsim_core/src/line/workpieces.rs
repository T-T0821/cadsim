//! 3D に描くワークの位置
//!
//! - 設備の中のワークは、設備の上に格子状に積む ([`stack`])。
//! - 設備から設備へ渡るワークは、2つの設備の上を弧を描いて移る ([`arc`])。
//!
//! 長さの単位は呼び出し側に合わせる (cadsim では mm)。

use std::collections::HashMap;

use glam::DVec3;

use crate::flow::model::{ElementId, ElementKind, FlowModel};

/// AGV のステーションに置く台の高さ (mm)。3D 部品を対応付けていない要素のワークはここに積む。
pub const STATION_TABLE_HEIGHT: f64 = 700.0;

/// 1段に並べる数 (一辺)
const PER_ROW: usize = 3;

/// `top` (設備の上面の中心) の上に `count` 個のワーク (一辺 `size`) を積んだときの各ワークの中心。
/// 1段に 3×3 個まで並べ、あふれたら上の段へ。
pub fn stack(top: DVec3, count: usize, size: f64) -> Vec<DVec3> {
    let pitch = size * 1.2;
    let per_layer = PER_ROW * PER_ROW;
    (0..count)
        .map(|i| {
            let layer = i / per_layer;
            let k = i % per_layer;
            let in_layer = (count - layer * per_layer).min(per_layer);
            // その段の並びの数に合わせて中央に寄せる
            let cols = in_layer.min(PER_ROW);
            let rows = in_layer.div_ceil(PER_ROW);
            let (r, c) = (k / PER_ROW, k % PER_ROW);
            let x = (c as f64 - (cols as f64 - 1.0) / 2.0) * pitch;
            let z = (r as f64 - (rows as f64 - 1.0) / 2.0) * pitch;
            let y = size / 2.0 + layer as f64 * pitch;
            top + DVec3::new(x, y, z)
        })
        .collect()
}

/// 要素ごとのワークを積む場所。
/// 1. 3D の部品を対応付けていれば、その上面の中心 (`part_top`)。
/// 2. なければ、AGV で送り出す要素は積み込みのステーション、AGV で届く先の要素は
///    荷降ろしのステーション (`station` は床の位置を返す。台の高さを足す)。
/// 3. どちらもない要素は描かない (結果に入らない)。
pub fn element_anchors(
    model: &FlowModel,
    part_top: impl Fn(u32) -> Option<DVec3>,
    station: impl Fn(u32) -> Option<DVec3>,
) -> HashMap<ElementId, DVec3> {
    let on_table = |id: u32| station(id).map(|p| p + DVec3::Y * STATION_TABLE_HEIGHT);
    let mut out = HashMap::new();
    for e in &model.elements {
        if let Some(top) = e.part.and_then(&part_top) {
            out.insert(e.id, top);
        }
    }
    for e in &model.elements {
        let Some(call) = e.carrier else {
            continue;
        };
        if !out.contains_key(&e.id)
            && let Some(p) = on_table(call.from)
        {
            out.insert(e.id, p);
        }
        for &n in &e.next {
            let sink = model
                .get(n)
                .is_some_and(|t| matches!(t.kind, ElementKind::Sink));
            if !out.contains_key(&n)
                && !sink
                && let Some(p) = on_table(call.to)
            {
                out.insert(n, p);
            }
        }
    }
    out
}

/// `from` から `to` へ渡る途中の位置。`t` は 0〜1、`lift` は弧の高さ。
pub fn arc(from: DVec3, to: DVec3, t: f64, lift: f64) -> DVec3 {
    let t = t.clamp(0.0, 1.0);
    // なめらかに動き出して止まる
    let s = t * t * (3.0 - 2.0 * t);
    from.lerp(to, s) + DVec3::Y * (lift * 4.0 * s * (1.0 - s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_prefer_parts_then_agv_stations() {
        use crate::flow::model::{AgvCall, sample_line};
        let mut m = sample_line();
        // 加工 (1) は部品あり、バッファ (2) は AGV で組立 (3) へ送る
        m.elements[1].part = Some(9);
        m.elements[2].carrier = Some(AgvCall { from: 1, to: 2 });
        let ids: Vec<_> = m.elements.iter().map(|e| e.id).collect();
        let anchors = element_anchors(
            &m,
            |p| (p == 9).then_some(DVec3::new(0.0, 500.0, 0.0)),
            |s| Some(DVec3::new(s as f64 * 1000.0, 0.0, 0.0)),
        );
        assert_eq!(anchors[&ids[1]], DVec3::new(0.0, 500.0, 0.0));
        assert_eq!(
            anchors[&ids[2]],
            DVec3::new(1000.0, STATION_TABLE_HEIGHT, 0.0)
        );
        assert_eq!(
            anchors[&ids[3]],
            DVec3::new(2000.0, STATION_TABLE_HEIGHT, 0.0)
        );
        assert!(!anchors.contains_key(&ids[4]));
    }

    #[test]
    fn single_work_sits_centered_on_top() {
        let p = stack(DVec3::new(10.0, 100.0, 0.0), 1, 50.0);
        assert_eq!(p, vec![DVec3::new(10.0, 125.0, 0.0)]);
    }

    #[test]
    fn stack_fills_a_layer_then_goes_up() {
        let p = stack(DVec3::ZERO, 10, 10.0);
        assert_eq!(p.len(), 10);
        assert!(p[..9].iter().all(|v| (v.y - 5.0).abs() < 1e-9));
        assert!(p[9].y > 5.0);
        // 1段目は中央に寄っている
        let mean: DVec3 = p[..9].iter().copied().sum::<DVec3>() / 9.0;
        assert!(mean.x.abs() < 1e-9 && mean.z.abs() < 1e-9);
    }

    #[test]
    fn arc_starts_and_ends_at_the_stations_and_lifts_in_between() {
        let (a, b) = (DVec3::ZERO, DVec3::new(100.0, 0.0, 0.0));
        assert_eq!(arc(a, b, 0.0, 30.0), a);
        assert_eq!(arc(a, b, 1.0, 30.0), b);
        let mid = arc(a, b, 0.5, 30.0);
        assert!((mid.x - 50.0).abs() < 1e-9);
        assert!((mid.y - 30.0).abs() < 1e-9);
    }
}
