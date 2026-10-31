//! 動作の下書き ([`SequenceSpec`]): 生成 AI とタイミングチャートのあいだで受け渡す形
//!
//! 装置やセンサーは id ではなく名前で指す。生成 AI は名前で書くほうが間違えにくく、
//! 今のチャートの装置と名前で突き合わせれば、部品の割り当てを残したままステップだけ作り直せる。
//!
//! Bevy・egui・ファイル形式に依存しない。

use crate::motion::chart::{Interlock, SensorKind};
use crate::motion::{Action, ActuatorKind, Step, TimingChart};

#[derive(Clone, Debug, PartialEq)]
pub struct ActuatorSpec {
    pub name: String,
    pub kind: ActuatorKind,
    /// シリンダのストローク (モデル単位)。0 以下なら既定値。
    pub stroke: f64,
    /// 標準の動作時間 (秒)。0 以下なら既定値。
    pub default_duration: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SensorSpec {
    pub name: String,
    pub kind: SensorKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionSpec {
    pub actuator: String,
    /// シリンダは 1 = 出 / 0 = 戻、直動は移動量、回転は角度 (度)
    pub target: f64,
    /// 動作時間 (秒)。0 以下なら装置の標準の動作時間。
    pub duration: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InterlockSpec {
    pub sensor: String,
    pub on: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct StepSpec {
    pub comment: String,
    pub actions: Vec<ActionSpec>,
    pub interlocks: Vec<InterlockSpec>,
    /// 動作完了後に待つ時間 (秒)
    pub dwell: f64,
}

/// 動作の下書き一式
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SequenceSpec {
    pub actuators: Vec<ActuatorSpec>,
    pub sensors: Vec<SensorSpec>,
    pub steps: Vec<StepSpec>,
    /// 生成 AI が置いた前提や、説明文から分からなかった点
    pub notes: String,
}

/// チャートへの反映のしかた
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ApplyMode {
    /// ステップをすべて置き換える (装置とセンサーは残す)
    #[default]
    Replace,
    /// 今のステップの後ろに足す
    Append,
}

/// 反映した結果
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ApplyReport {
    pub added_actuators: Vec<String>,
    pub added_sensors: Vec<String>,
    pub steps: usize,
    /// 名前の見つからない装置を指した動作など、読み飛ばしたもの
    pub warnings: Vec<String>,
}

impl ApplyReport {
    pub fn summary(&self) -> String {
        let mut text = format!("{} ステップを反映しました。", self.steps);
        if !self.added_actuators.is_empty() {
            text += &format!(
                " 装置を追加: {}(動かす部品はプロパティで割り当ててください)。",
                self.added_actuators.join("、")
            );
        }
        if !self.added_sensors.is_empty() {
            text += &format!(
                " センサーを追加: {}(位置は 3D ビューで合わせてください)。",
                self.added_sensors.join("、")
            );
        }
        text
    }
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// 今のチャートを下書きの形にする (生成 AI に今の装置を伝える・説明させるのに使う)
pub fn from_chart(chart: &TimingChart) -> SequenceSpec {
    let actuator_name = |id: u32| chart.actuator(id).map(|a| a.name.clone());
    let sensor_name = |id: u32| chart.sensor(id).map(|s| s.name.clone());
    SequenceSpec {
        actuators: chart
            .actuators
            .iter()
            .map(|a| ActuatorSpec {
                name: a.name.clone(),
                kind: a.kind,
                stroke: a.stroke,
                default_duration: a.default_duration,
            })
            .collect(),
        sensors: chart
            .sensors
            .iter()
            .map(|s| SensorSpec {
                name: s.name.clone(),
                kind: s.kind,
            })
            .collect(),
        steps: chart
            .steps
            .iter()
            .map(|step| StepSpec {
                comment: step.comment.clone(),
                actions: step
                    .actions
                    .iter()
                    .filter_map(|a| {
                        Some(ActionSpec {
                            actuator: actuator_name(a.actuator)?,
                            target: a.target,
                            duration: a.duration,
                        })
                    })
                    .collect(),
                interlocks: step
                    .interlocks
                    .iter()
                    .filter_map(|i| {
                        Some(InterlockSpec {
                            sensor: sensor_name(i.sensor)?,
                            on: i.on,
                        })
                    })
                    .collect(),
                dwell: step.dwell,
            })
            .collect(),
        notes: String::new(),
    }
}

/// 下書きをチャートに反映する。装置・センサーは名前で突き合わせ、ないものは足す。
pub fn apply(spec: &SequenceSpec, chart: &mut TimingChart, mode: ApplyMode) -> ApplyReport {
    let mut report = ApplyReport::default();

    for a in &spec.actuators {
        if a.name.trim().is_empty() || chart.actuators.iter().any(|x| same_name(&x.name, &a.name)) {
            continue;
        }
        let id = chart.add_actuator(a.kind);
        if let Some(actuator) = chart.actuator_mut(id) {
            actuator.name = a.name.trim().to_string();
            if a.kind == ActuatorKind::Cylinder && a.stroke > 0.0 {
                actuator.stroke = a.stroke;
            }
            if a.default_duration > 0.0 {
                actuator.default_duration = a.default_duration;
            }
        }
        report.added_actuators.push(a.name.trim().to_string());
    }
    for s in &spec.sensors {
        if s.name.trim().is_empty() || chart.sensors.iter().any(|x| same_name(&x.name, &s.name)) {
            continue;
        }
        let id = chart.add_sensor(s.kind);
        if let Some(sensor) = chart.sensor_mut(id) {
            sensor.name = s.name.trim().to_string();
        }
        report.added_sensors.push(s.name.trim().to_string());
    }

    let mut steps = Vec::with_capacity(spec.steps.len());
    for (index, s) in spec.steps.iter().enumerate() {
        let label = format!("S{}", index + 1);
        let mut step = Step {
            comment: s.comment.trim().to_string(),
            dwell: if s.dwell.is_finite() {
                s.dwell.max(0.0)
            } else {
                0.0
            },
            ..Default::default()
        };
        for a in &s.actions {
            let Some(actuator) = chart
                .actuators
                .iter()
                .find(|x| same_name(&x.name, &a.actuator))
            else {
                report.warnings.push(format!(
                    "{label}: 装置「{}」がないので動作を飛ばしました",
                    a.actuator
                ));
                continue;
            };
            if step.action(actuator.id).is_some() {
                report.warnings.push(format!(
                    "{label}: 「{}」の動作が2つあったので最初のものを使いました",
                    actuator.name
                ));
                continue;
            }
            let target = if !a.target.is_finite() {
                0.0
            } else if actuator.kind == ActuatorKind::Cylinder {
                if a.target >= 0.5 { 1.0 } else { 0.0 }
            } else {
                actuator.clamp(a.target)
            };
            let duration = if a.duration.is_finite() && a.duration > 0.0 {
                a.duration
            } else {
                actuator.default_duration
            };
            step.actions.push(Action {
                actuator: actuator.id,
                target,
                duration,
            });
        }
        for i in &s.interlocks {
            match chart.sensors.iter().find(|x| same_name(&x.name, &i.sensor)) {
                Some(sensor) => {
                    if step.interlock(sensor.id).is_none() {
                        step.interlocks.push(Interlock {
                            sensor: sensor.id,
                            on: i.on,
                        });
                    }
                }
                None => report.warnings.push(format!(
                    "{label}: センサー「{}」がないので条件を飛ばしました",
                    i.sensor
                )),
            }
        }
        steps.push(step);
    }
    report.steps = steps.len();
    match mode {
        ApplyMode::Replace => chart.steps = steps,
        ApplyMode::Append => chart.steps.extend(steps),
    }
    report
}

/// 下書きを人が読める文章にする (画面のプレビューと、説明させるときの材料)
pub fn describe(spec: &SequenceSpec) -> String {
    let mut text = String::new();
    if !spec.actuators.is_empty() {
        text += "装置:\n";
        for a in &spec.actuators {
            let extra = match a.kind {
                ActuatorKind::Cylinder => format!("ストローク {}", a.stroke),
                _ => String::new(),
            };
            text += &format!("- {} ({}) {extra}\n", a.name, a.kind.label());
        }
    }
    if !spec.sensors.is_empty() {
        text += "センサー:\n";
        for s in &spec.sensors {
            text += &format!("- {} ({})\n", s.name, s.kind.label());
        }
    }
    text += "ステップ:\n";
    for (i, s) in spec.steps.iter().enumerate() {
        text += &format!("S{} {}\n", i + 1, s.comment);
        for c in &s.interlocks {
            text += &format!(
                "    条件: {} が {}\n",
                c.sensor,
                if c.on { "ON" } else { "OFF" }
            );
        }
        for a in &s.actions {
            let kind = spec
                .actuators
                .iter()
                .find(|x| same_name(&x.name, &a.actuator))
                .map(|x| x.kind);
            let value = match kind {
                Some(ActuatorKind::Cylinder) if a.target >= 0.5 => "出".to_string(),
                Some(ActuatorKind::Cylinder) => "戻".to_string(),
                Some(ActuatorKind::RotaryTable) => format!("{}°", a.target),
                _ => format!("{}", a.target),
            };
            text += &format!("    {} → {value} ({} 秒)\n", a.actuator, a.duration);
        }
        if s.dwell > 0.0 {
            text += &format!("    タイマー {} 秒\n", s.dwell);
        }
    }
    if !spec.notes.trim().is_empty() {
        text += &format!("メモ: {}\n", spec.notes.trim());
    }
    text
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample_spec() -> SequenceSpec {
        SequenceSpec {
            actuators: vec![
                ActuatorSpec {
                    name: "リフタ".into(),
                    kind: ActuatorKind::Cylinder,
                    stroke: 50.0,
                    default_duration: 0.5,
                },
                ActuatorSpec {
                    name: "X軸".into(),
                    kind: ActuatorKind::LinearAxis,
                    stroke: 0.0,
                    default_duration: 0.0,
                },
            ],
            sensors: vec![SensorSpec {
                name: "ワーク有無".into(),
                kind: SensorKind::Photoelectric,
            }],
            steps: vec![
                StepSpec {
                    comment: "ワーク到着待ちで上昇".into(),
                    actions: vec![ActionSpec {
                        actuator: "リフタ".into(),
                        target: 0.9,
                        duration: 0.0,
                    }],
                    interlocks: vec![InterlockSpec {
                        sensor: "ワーク有無".into(),
                        on: true,
                    }],
                    dwell: 0.2,
                },
                StepSpec {
                    comment: "搬送".into(),
                    actions: vec![
                        ActionSpec {
                            actuator: "x軸".into(),
                            target: 300.0,
                            duration: 1.5,
                        },
                        ActionSpec {
                            actuator: "グリッパ".into(),
                            target: 1.0,
                            duration: 0.3,
                        },
                    ],
                    interlocks: vec![InterlockSpec {
                        sensor: "扉".into(),
                        on: false,
                    }],
                    dwell: -1.0,
                },
            ],
            notes: "ストロークは仮の値".into(),
        }
    }

    #[test]
    fn apply_adds_devices_and_steps_with_warnings() {
        let mut chart = TimingChart::default();
        let report = apply(&sample_spec(), &mut chart, ApplyMode::Replace);
        assert_eq!(report.added_actuators, vec!["リフタ", "X軸"]);
        assert_eq!(report.added_sensors, vec!["ワーク有無"]);
        assert_eq!(report.steps, 2);
        assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);

        let lifter = &chart.actuators[0];
        assert_eq!(lifter.stroke, 50.0);
        let s1 = &chart.steps[0];
        assert_eq!(s1.actions[0].target, 1.0, "シリンダは 0 / 1 に丸める");
        assert_eq!(s1.actions[0].duration, 0.5, "0 秒は装置の標準時間");
        assert_eq!(s1.interlocks[0].sensor, chart.sensors[0].id);
        assert_eq!(s1.dwell, 0.2);
        let s2 = &chart.steps[1];
        assert_eq!(s2.actions.len(), 1, "大文字・小文字を区別せず X軸 に当てる");
        assert_eq!(s2.actions[0].target, 300.0);
        assert_eq!(s2.dwell, 0.0);
        assert!(report.summary().contains("2 ステップ"));
    }

    #[test]
    fn apply_reuses_existing_devices_and_appends() {
        let mut chart = TimingChart::default();
        apply(&sample_spec(), &mut chart, ApplyMode::Replace);
        chart.actuators[0].parts = vec![7];
        let report = apply(&sample_spec(), &mut chart, ApplyMode::Append);
        assert!(report.added_actuators.is_empty());
        assert_eq!(chart.actuators.len(), 2);
        assert_eq!(chart.actuators[0].parts, vec![7], "部品の割り当てを残す");
        assert_eq!(chart.steps.len(), 4);
        apply(&sample_spec(), &mut chart, ApplyMode::Replace);
        assert_eq!(chart.steps.len(), 2);
    }

    #[test]
    fn from_chart_round_trips_through_apply() {
        let mut chart = TimingChart::default();
        apply(&sample_spec(), &mut chart, ApplyMode::Replace);
        let spec = from_chart(&chart);
        assert_eq!(spec.steps.len(), 2);
        assert_eq!(spec.steps[1].actions[0].actuator, "X軸");
        let mut copy = TimingChart::default();
        apply(&spec, &mut copy, ApplyMode::Replace);
        assert_eq!(copy.steps, chart.steps);
        let text = describe(&spec);
        assert!(text.contains("S1 ワーク到着待ちで上昇"));
        assert!(text.contains("条件: ワーク有無 が ON"));
        assert!(text.contains("リフタ → 出"));
    }
}
