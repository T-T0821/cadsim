//! パネルで共通に使う小さな部品

use bevy_egui::egui;

/// X/Y/Z の3つの数値を編集する。値が変わったら true を返す。
pub fn xyz_editor(
    ui: &mut egui::Ui,
    label: &str,
    values: &mut [f64; 3],
    speed: f64,
    min: Option<f64>,
) -> bool {
    let mut changed = false;
    ui.label(label);
    ui.horizontal(|ui| {
        for (value, axis) in values.iter_mut().zip(["X", "Y", "Z"]) {
            ui.weak(axis);
            let mut drag = egui::DragValue::new(value).speed(speed).max_decimals(4);
            if let Some(min) = min {
                drag = drag.range(min..=f64::MAX);
            }
            changed |= ui.add(drag).changed();
        }
    });
    changed
}

/// 計算の重い値 (形を作り直す寸法など) の DragValue。ドラッグ中・入力中は手元の値を見せ、
/// ドラッグを離す・入力を終えたときだけ、変わっていれば新しい値を返す
pub fn committed_value(ui: &mut egui::Ui, id: egui::Id, current: f64) -> Option<f64> {
    let mut value = ui.data_mut(|d| *d.get_temp_mut_or(id, current));
    let response = ui.add(egui::DragValue::new(&mut value).speed(0.1).max_decimals(4));
    let editing = response.dragged() || response.has_focus();
    ui.data_mut(|d| d.insert_temp(id, if editing { value } else { current }));
    let done = response.drag_stopped() || response.lost_focus();
    (done && (value - current).abs() > 1e-9).then_some(value)
}

/// [`committed_value`] の X/Y/Z 版。確定したら `values` を書き換えて true を返す
pub fn committed_xyz(ui: &mut egui::Ui, id: egui::Id, values: &mut [f64; 3]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        for (i, (value, axis)) in values.iter_mut().zip(["X", "Y", "Z"]).enumerate() {
            ui.weak(axis);
            if let Some(v) = committed_value(ui, id.with(i), *value) {
                *value = v;
                changed = true;
            }
        }
    });
    changed
}

/// 表の1行 (左に項目名、右に値)
pub fn row(ui: &mut egui::Ui, label: &str, value: impl Into<egui::WidgetText>) {
    ui.weak(label);
    ui.label(value);
    ui.end_row();
}

/// 長さを単位付きで読みやすく表示する (桁数は大きさに合わせる)
pub fn length(value: f64, unit: &str) -> String {
    format!("{} {unit}", round_for_display(value))
}

pub fn round_for_display(value: f64) -> String {
    let digits = if value.abs() >= 100.0 {
        2
    } else if value.abs() >= 1.0 {
        3
    } else {
        5
    };
    let text = format!("{value:.digits$}");
    // 末尾の 0 と小数点を落とす (12.500 → 12.5)
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    };
    if text == "-0" { "0".into() } else { text }
}
