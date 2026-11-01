//! 部品スクリプト
//!
//! 部品 (ツリーのノード) に Rhai のスクリプトを付け、PLC 連携の「部品スクリプト」モードで
//! PLC の代わりに設備を動かす。スクリプトは信号の読み書き、関節 (アクチュエータ) の移動、
//! 待ちができる。
//!
//! - `runner`: 1つのスクリプトをシミュレーションの時刻に合わせて動かす (Bevy なし)
//! - `rhai_engine`: スクリプトから呼べる関数の登録 (Rhai)
//! - `controller`: スクリプトを PLC 連携の I/O・センサーにつなぐ (Bevy なし)
//! - このファイルと `ui`: 実行状態の Resource と、プロパティ欄のエディタ・PLC 連携ウィンドウの操作
//!
//! スクリプトのソースは [`crate::model::Node::script`] に持つので、保存・元に戻す・
//! 形状の差し替えはノードの他の設定と同じように扱われる。

mod controller;
mod rhai_engine;
mod runner;
mod ui;

use bevy::prelude::*;

pub use controller::{ScriptController, ScriptSource};
pub use rhai_engine::API_HELP;
pub use runner::ScriptState;
pub use ui::{script_controls, script_section};

use plantwright_core::model::Model;

/// 実行中の部品スクリプト。止めている間は None。
#[derive(Resource, Default)]
pub struct ScriptRun {
    controller: Option<ScriptController>,
}

impl ScriptRun {
    pub fn running_mut(&mut self) -> Option<&mut ScriptController> {
        self.controller.as_mut()
    }

    pub fn running(&self) -> Option<&ScriptController> {
        self.controller.as_ref()
    }

    /// モデルのスクリプトをすべて最初から動かす
    pub fn start(&mut self, model: &Model) {
        self.controller = Some(ScriptController::start(scripts_of(model)));
    }

    pub fn stop(&mut self) {
        self.controller = None;
    }
}

/// スクリプトの付いたノード (部品・アセンブリ) を、ツリーの並び順に返す。
/// 自作部品の中のスクリプトには、その部品の名前の頭 (例: "プッシャー 1/") を付けて返す。
pub fn scripts_of(model: &Model) -> Vec<ScriptSource> {
    model
        .roots
        .iter()
        .flat_map(|&root| model.subtree(root))
        .filter_map(|id| {
            let node = model.get(id)?;
            let source = node.script.as_ref().filter(|s| !s.trim().is_empty())?;
            let scope = model
                .ancestors(id)
                .into_iter()
                .rev()
                .find_map(|a| model.get(a)?.custom_part.as_ref())
                .map(|c| c.prefix.clone())
                .unwrap_or_default();
            Some(ScriptSource {
                node: id,
                name: node.name.clone(),
                scope,
                source: source.clone(),
            })
        })
        .collect()
}

pub struct ScriptPlugin;

impl Plugin for ScriptPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScriptRun>();
    }
}

/// 文法だけ確かめる (プロパティ欄で、書いている途中に誤りを出す)
pub fn check(source: &str) -> Result<(), String> {
    rhai_engine::check_syntax(source)
}
