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
