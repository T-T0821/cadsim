//! 動作確認用のサンプル: 部品棚とラインを往復する作業者と、十字の通路を走る AGV

use super::agv_sim::{AgvParams, Flow};
use super::operator_plan::OperatorTask;
use super::{AgvLayoutRes, FloorState, OperatorsRes};

pub fn load(operators: &mut OperatorsRes, layout: &mut AgvLayoutRes, state: &mut FloorState) {
    *operators = OperatorsRes::default();
    let id = operators.add([-2000.0, 3000.0]);
    if let Some(op) = operators.operators.iter_mut().find(|o| o.id == id) {
        op.tasks = vec![
            OperatorTask::Walk {
                to: [-4000.0, 3000.0],
            },
            OperatorTask::Pick {
                label: "部品棚".into(),
                duration: 4.0,
            },
            OperatorTask::Walk {
                to: [-1000.0, 1500.0],
            },
            OperatorTask::Pick {
                label: "ラインに供給".into(),
                duration: 3.0,
            },
            OperatorTask::Walk { to: [0.0, 1500.0] },
            OperatorTask::Button {
                label: "起動".into(),
                duration: 1.0,
            },
        ];
    }

    // 十字の通路: 中央が交差点、端の 4か所がステーション
    *layout = AgvLayoutRes::default();
    let g = &mut layout.graph;
    let center = g.add_node([4000.0, 4000.0], false);
    let mut ends = Vec::new();
    for (dx, dz) in [(4000.0, 0.0), (-4000.0, 0.0), (0.0, 4000.0), (0.0, -4000.0)] {
        let id = g.add_node([4000.0 + dx, 4000.0 + dz], true);
        g.toggle_edge(center, id);
        ends.push(id);
    }
    layout.flows = vec![
        Flow {
            from: ends[0],
            to: ends[1],
            interval: 30.0,
        },
        Flow {
            from: ends[2],
            to: ends[3],
            interval: 30.0,
        },
    ];
    layout.params = AgvParams {
        count: 2,
        ..Default::default()
    };
    state.selected_operator = Some(id);
    state.selected_node = None;
    state.reset_sim();
}
