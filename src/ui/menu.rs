//! 上部のメニューバーとツールバー

use bevy_egui::egui;
use cadsim_core::factory_hall::HallSize;

use super::UiState;
use super::dock;
use super::modeling::Tab;
use super::theme::palette;
use crate::file_io::ExportScope;
use crate::history::Request;
use crate::interchange::{FORMATS, LENGTH_UNITS};
use crate::project::ProjectAction;
use crate::result_export::ResultKind;
use crate::view::camera::{CameraRequest, StandardView};
use crate::view::gizmo::Tool as Tool3d;
use cadsim_core::usecase::help::SHORTCUTS;
use cadsim_core::workspace::{Tool, ToolGroup};

pub fn show(root: &mut egui::Ui, state: &mut UiState) {
    egui::Panel::top("menu_bar")
        .frame(
            egui::Frame::new()
                .fill(palette::INK)
                .inner_margin(egui::Margin::symmetric(8, 4)),
        )
        .show(root, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                file_menu(ui, state);
                edit_menu(ui, state);
                view_menu(ui, state);
                create_menu(ui, state);
                tools_menu(ui, state);
                help_menu(ui, state);
            });
        });
    egui::Panel::top("toolbar")
        .frame(
            egui::Frame::new()
                .fill(palette::NAVY)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .stroke(egui::Stroke::new(1.0, palette::LINE)),
        )
        .show(root, |ui| toolbar(ui, state));
}

fn file_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("ファイル", |ui| {
        if ui
            .add(egui::Button::new("新規プロジェクト").shortcut_text("Ctrl+N"))
            .clicked()
        {
            state.project.request(ProjectAction::New);
        }
        if ui
            .add(egui::Button::new("開く…").shortcut_text("Ctrl+O"))
            .on_hover_text(format!(
                "プロジェクト (.cadsim) を開くか、ファイルを今のプロジェクトに取り込む。\n\
                 ウィンドウへのドラッグ&ドロップでも開けます。\n\n\
                 取り込める形式: STEP, {}\n\
                 平面図として床に敷く形式: DXF, PDF, PNG, JPG\n\
                 点群: PLY, LAS, E57 (PLY は「OBJ / STL の単位」で読みます)",
                format_list()
            ))
            .clicked()
        {
            state.jobs.request_dialog();
        }
        if ui
            .button("ロボット機種 (URDF) を読み込む…")
            .on_hover_text(
                "ROS のロボットの記述 (.urdf) を今のプロジェクトに読み込み、リンクの形状と関節・可動範囲から\n\
                 ジョグ・教示点・逆運動学で動かせるロボットを作る",
            )
            .clicked()
        {
            state.jobs.request_urdf_dialog();
        }
        ui.menu_button("サンプルを開く", |ui| {
            if ui
                .button("デモライン")
                .on_hover_text("起動時に動いているデモのライン (ロボットセル・コンベア・安全機器・AGV) を開いて再生する")
                .clicked()
            {
                state
                    .project
                    .request(ProjectAction::OpenDemoLine { play: true });
                ui.close();
            }
            if ui
                .button("本物の装置 (UR5e + Robotiq 2F-85)")
                .on_hover_text(
                    "メーカー公開の CAD (協働ロボット UR5e と電動グリッパ 2F-85) に関節と動きを付けた\n\
                     ピック & プレースのセルを開いて再生する",
                )
                .clicked()
            {
                state.project.request(ProjectAction::OpenRealRobot);
                ui.close();
            }
            if ui
                .button("ロボット機種 UR5e (URDF)")
                .on_hover_text(
                    "Universal Robots が公開している UR5e の URDF (BSD 3-Clause) を今のプロジェクトに読み込み、\n\
                     6 軸のロボットとして置く",
                )
                .clicked()
            {
                state.jobs.open_urdf_sample();
                ui.close();
            }
            if ui
                .button("組立品 (万力)")
                .on_hover_text("7 部品の小さなアセンブリを取り込む")
                .clicked()
            {
                state.jobs.open_sample();
                ui.close();
            }
            if ui
                .button("工程シミュレーションのサンプル")
                .on_hover_text(
                    "投入・加工・組立・検査の工程を読み込んで、工程シミュレーションを開く",
                )
                .clicked()
            {
                state.flow.load_sample();
                state.dock.set_open(Tool::Flow, true);
                ui.close();
            }
            ui.separator();
            ui.menu_button("負荷テスト用シーン", |ui| {
                for count in [1000, 5000, 10000] {
                    if ui.button(format!("部品 {count} 個")).clicked() {
                        state.jobs.open_stress(count);
                        ui.close();
                    }
                }
            });
        });
        ui.separator();
        if ui
            .add(egui::Button::new("保存").shortcut_text("Ctrl+S"))
            .clicked()
        {
            state.project.request(ProjectAction::Save);
        }
        if ui
            .add(egui::Button::new("名前を付けて保存…").shortcut_text("Ctrl+Shift+S"))
            .clicked()
        {
            state.project.request(ProjectAction::SaveAs);
        }
        ui.separator();
        export_menu(ui, state);
        results_menu(ui, state);
        units_menu(ui, state);
        ui.separator();
        if ui
            .button("すべて閉じる")
            .on_hover_text("今のプロジェクトのまま中身を空にする (元に戻せる)")
            .clicked()
        {
            state.history.clear_all(&mut state.model);
            state.selection.clear();
        }
    });
}

/// 書き出しと、単位を持たない形式 (OBJ / STL) の単位の設定
fn export_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("書き出し", |ui| {
        if ui.button("すべてを書き出し…").clicked() {
            state.jobs.request_export(ExportScope::All);
        }
        let has_selection = state
            .selection
            .node
            .is_some_and(|id| state.model.get(id).is_some());
        if ui
            .add_enabled(has_selection, egui::Button::new("選択を書き出し…"))
            .clicked()
        {
            state.jobs.request_export(ExportScope::Selection);
        }
        if ui
            .button("ブラウザ / VR で見る")
            .on_hover_text("Web ビューアにして既定のブラウザで開きます。WebXR 対応のブラウザなら VR ヘッドセットで実寸で見られます")
            .clicked()
        {
            state.jobs.request_browser_view();
        }
        ui.separator();
        if ui
            .button("図面 (PDF / DXF)…")
            .on_hover_text("表示中の部品の上面図・正面図を、寸法付きで A3 の図面にする")
            .clicked()
        {
            state.documents.open_drawing_dialog();
            ui.close();
        }
        if ui
            .button("部品表 (Excel / CSV)…")
            .on_hover_text("同じ部品をまとめて数えた部品表と、ツリーの構成表")
            .clicked()
        {
            state.documents.request_bom();
            ui.close();
        }
        if ui
            .button("シミュレーションを動画で書き出し…")
            .on_hover_text("タイミングチャートの動きを MP4 / GIF / PNG 連番にする")
            .clicked()
        {
            state.recording.open = true;
            ui.close();
        }
        ui.separator();
        ui.weak("保存ダイアログで選んだ拡張子の形式で書き出します");
        for format in FORMATS.iter().filter(|f| f.export.is_some()) {
            let note = if format.export_note.is_empty() {
                "階層・色・アニメーションを保持"
            } else {
                format.export_note
            };
            ui.weak(format!("・{}: {note}", format.name));
        }
        let unit_formats: Vec<&str> = FORMATS
            .iter()
            .filter(|f| f.needs_units)
            .flat_map(|f| f.extensions.iter().copied())
            .collect();
        ui.weak(format!(
            "{} は「OBJ / STL の単位」の設定で書き出します",
            unit_formats.join(" / ").to_ascii_uppercase()
        ));
    });
}

/// 単位を持たない形式 (OBJ / STL) の単位
fn units_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("OBJ / STL の単位", |ui| {
        ui.weak("OBJ と STL は単位を持たないので、読み書きのときにここで指定します");
        let units = &mut state.jobs.units;
        for &(name, meters) in LENGTH_UNITS {
            if ui.radio(units.meters_per_unit == meters, name).clicked() {
                units.meters_per_unit = meters;
            }
        }
        ui.separator();
        ui.checkbox(&mut units.z_up, "Z 軸が上 (オフにすると Y 軸が上)");
    });
}

/// タイミングチャートと干渉チェックの結果を CSV に書き出す
fn results_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("シミュレーション結果 (CSV)", |ui| {
        let enabled = !state.results.is_busy();
        if ui
            .add_enabled(enabled, egui::Button::new("サイクルタイム…"))
            .on_hover_text("タイミングチャートのステップごとの開始・終了・所要時間")
            .clicked()
        {
            state.results.request(ResultKind::CycleTime);
            ui.close();
        }
        if ui
            .add_enabled(enabled, egui::Button::new("干渉結果 (1サイクル分)…"))
            .on_hover_text(
                "チャートを最初から最後まで動かしながら干渉チェックし、干渉・接触・近接の時間帯を書き出す。\
                 判定の設定はシミュレーションパネルのものを使う",
            )
            .clicked()
        {
            state.results.request(ResultKind::Interference);
            ui.close();
        }
        ui.horizontal(|ui| {
            ui.label("干渉チェックの間隔");
            ui.add(
                egui::DragValue::new(&mut state.results.sample_interval)
                    .range(0.001..=1.0)
                    .speed(0.001)
                    .suffix(" 秒"),
            );
        });
        if !enabled {
            ui.weak("出力中…");
        }
    });
}

fn format_list() -> String {
    FORMATS
        .iter()
        .flat_map(|f| f.extensions.iter().map(|e| e.to_ascii_uppercase()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn edit_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("編集", |ui| {
        for (text, shortcut, request, label) in [
            (
                "元に戻す",
                "Ctrl+Z",
                Request::Undo,
                state.history.undo_label(),
            ),
            (
                "やり直し",
                "Ctrl+Y",
                Request::Redo,
                state.history.redo_label(),
            ),
        ] {
            let button = ui.add_enabled(
                label.is_some(),
                egui::Button::new(text).shortcut_text(shortcut),
            );
            if button.on_hover_text(label.unwrap_or_default()).clicked() {
                state.history.request = Some(request);
            }
        }
        ui.separator();
        let target = state
            .selection
            .node
            .filter(|&id| state.model.get(id).is_some());
        if ui
            .add_enabled(
                target.is_some(),
                egui::Button::new("選択を削除").shortcut_text("Delete"),
            )
            .clicked()
            && let Some(id) = target
        {
            state.history.remove(&mut state.model, id);
            state.selection.clear();
        }
        if ui
            .add_enabled(
                target.is_some(),
                egui::Button::new("選択を隠す").shortcut_text("H"),
            )
            .clicked()
            && let Some(node) = target.and_then(|id| state.model.get_mut(id))
        {
            node.visible = false;
        }
        if ui
            .add(egui::Button::new("すべて表示").shortcut_text("Alt+H"))
            .clicked()
        {
            state.model.show_all();
        }
        if ui
            .add_enabled(
                target.is_some(),
                egui::Button::new("選択を解除").shortcut_text("Esc"),
            )
            .clicked()
        {
            state.selection.clear();
        }
        ui.separator();
        if ui
            .add_enabled(target.is_some(), egui::Button::new("形状を差し替え…"))
            .on_hover_text(
                "選んだ部品・アセンブリの形状を、設計変更後のファイルの形状に入れ替える。\n\
                 名前と構造で部品を対応づけ、動きの設定はそのまま残す",
            )
            .clicked()
            && let Some(target) = target
        {
            state.jobs.request_replace(target);
        }
    });
}

/// ツールバーの「元に戻す」「やり直し」。押せないときは灰色にし、何が戻るかをツールチップに出す。
fn undo_redo_buttons(ui: &mut egui::Ui, state: &mut UiState) {
    for (text, key, request, label) in [
        (
            "⟲ 戻す",
            "Ctrl+Z",
            Request::Undo,
            state.history.undo_label(),
        ),
        (
            "⟳ やり直し",
            "Ctrl+Y",
            Request::Redo,
            state.history.redo_label(),
        ),
    ] {
        let hover = match &label {
            Some(label) => format!("{label} ({key})"),
            None => format!("戻せる操作はありません ({key})"),
        };
        let button = ui.add_enabled(label.is_some(), egui::Button::new(text));
        if button
            .on_hover_text(hover.clone())
            .on_disabled_hover_text(hover)
            .clicked()
        {
            state.history.request = Some(request);
        }
    }
}

fn view_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("表示", |ui| {
        if ui
            .add(egui::Button::new("全体を表示").shortcut_text("Home"))
            .clicked()
        {
            state.camera.write(CameraRequest::FitAll);
        }
        if ui
            .add(egui::Button::new("選択を拡大").shortcut_text("F"))
            .clicked()
        {
            state.camera.write(CameraRequest::FitSelection);
        }
        ui.menu_button("標準ビュー", |ui| {
            for view in StandardView::ALL {
                if ui
                    .add(egui::Button::new(view.label()).shortcut_text(view.shortcut()))
                    .clicked()
                {
                    state.camera.write(CameraRequest::View(view));
                }
            }
        });
        projection_buttons(ui, state);
        ui.separator();
        ui.checkbox(&mut state.settings.show_edges, "稜線を表示");
        ui.checkbox(&mut state.settings.show_grid, "グリッドを表示")
            .on_hover_text("工場の床を出しているときは 1 マス 1 m、太い線は 5 m ごと");
        ui.menu_button("工場の床と壁", |ui| factory_hall_menu(ui, state));
        ui.checkbox(&mut state.settings.show_bounds, "選択の外接箱を表示");
        ui.checkbox(
            &mut state.settings.ghost_unselected,
            "選択以外を半透明にする",
        );
        ui.menu_button("大きなモデルを軽く表示", |ui| {
            lightweight_menu(ui, state)
        });
        ui.separator();
        let mut show = state.playful.prefs.show_missions;
        if ui
            .checkbox(&mut show, "ミッションカードを表示")
            .on_hover_text("3D ビューの左下に、操作を覚えるためのミッションを出す")
            .changed()
        {
            state.playful.set_show_missions(show);
        }
        if ui
            .button("開いている道具をすべて閉じる")
            .on_hover_text("3D ビューに浮かべたパネルとウィンドウを閉じる")
            .clicked()
        {
            state.dock.close_all();
        }
    });
}

/// 背景の工場の建屋: 出すかどうか・壁と柱・広さ
fn factory_hall_menu(ui: &mut egui::Ui, state: &mut UiState) {
    let hall = &mut state.settings.hall;
    ui.checkbox(&mut hall.show, "工場の床を表示")
        .on_hover_text("コンクリートの床・通路の線・メートルのグリッドを出す");
    ui.add_enabled_ui(hall.show, |ui| {
        ui.checkbox(&mut hall.walls, "壁・柱・高窓を表示")
            .on_hover_text("建屋の外から見ているときは、手前の壁と柱を自動で隠す");
        ui.separator();
        ui.label("広さ");
        for size in HallSize::PRESETS {
            ui.radio_value(&mut hall.size, size, size.label());
        }
    });
}

/// 遠くの部品の簡略表示と、小さな穴・フィレットの省略
fn lightweight_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.checkbox(
        &mut state.settings.simplify_small_parts,
        "画面上で小さい部品を簡略表示",
    )
    .on_hover_text("遠くの部品を粗い形にし、ごく小さい部品は描かずに軽くする");
    ui.add_enabled_ui(state.settings.simplify_small_parts, |ui| {
        ui.add(
            egui::Slider::new(&mut state.settings.coarse_below_px, 0.0..=300.0)
                .text("簡略表示 (px 未満)"),
        );
        ui.add(
            egui::Slider::new(&mut state.settings.hide_below_px, 0.0..=20.0).text("省略 (px 未満)"),
        );
    });
    ui.separator();
    ui.checkbox(&mut state.settings.defeature, "小さな穴とフィレットを省略")
        .on_hover_text(
            "取り込んだ部品の小さな穴を埋め、角の丸みを尖らせて三角形を減らす。\n\
             干渉チェックは元の形で行う",
        );
    ui.add_enabled_ui(state.settings.defeature, |ui| {
        let unit = &state.model.unit;
        ui.add(
            egui::Slider::new(&mut state.settings.defeature_hole, 0.0..=100.0)
                .text(format!("穴の直径 ({unit} 以下)")),
        );
        ui.add(
            egui::Slider::new(&mut state.settings.defeature_fillet, 0.0..=50.0)
                .text(format!("フィレットの半径 ({unit} 以下)")),
        );
        if state.settings.defeature {
            let pending = state.defeaturing.pending();
            let removed = state.defeaturing.removed();
            ui.label(if pending > 0 {
                format!("計算中… (残り {pending} 形状)")
            } else {
                format!(
                    "穴 {} 個・フィレット {} 個を省略 (形状ごとに数える)",
                    removed.holes, removed.fillets
                )
            });
        }
    });
}

fn create_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("作成", |ui| {
        if ui.button("📦  箱を追加").clicked() {
            add_box(state);
        }
        ui.separator();
        for tab in [Tab::Cylinder, Tab::Extrude, Tab::Hole, Tab::Boolean] {
            let text = match tab {
                Tab::Cylinder => "円柱を追加…",
                Tab::Extrude => "押し出しで追加…",
                Tab::Hole => "穴をあける…",
                Tab::Boolean => "ブーリアン (和・差・積)…",
            };
            if ui.button(text).clicked() {
                state.modeling.open_tab(tab);
                state.dock.set_open(Tool::Modeling, true);
                ui.close();
            }
        }
        ui.separator();
        // 標準部品は「部品ライブラリ」パネルからビューへドラッグしても置ける
        crate::library::add_menu(ui, state);
        if ui
            .button("部品ライブラリを開く")
            .on_hover_text("標準部品をビューへドラッグして置ける")
            .clicked()
        {
            state.dock.set_open(Tool::Library, true);
            ui.close();
        }
    });
}

/// 道具の一覧 (ツールレールと同じもの)。分類ごとに区切って、開いているものに印を付ける。
fn tools_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("ツール", |ui| {
        for (i, group) in ToolGroup::ALL.into_iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            ui.label(
                egui::RichText::new(group.label())
                    .small()
                    .color(palette::TEXT_FAINT),
            );
            for tool in Tool::ALL.into_iter().filter(|t| t.group() == group) {
                let enabled = tool != Tool::Site || !state.model.site.is_empty();
                let mut open = state.dock.is_open(tool);
                let response = ui
                    .add_enabled(enabled, egui::Checkbox::new(&mut open, dock::title(tool)))
                    .on_hover_text(tool.hint())
                    .on_disabled_hover_text("平面図か点群を開くと使えます");
                if response.changed() {
                    state.dock.set_open(tool, open);
                }
            }
        }
        ui.separator();
        if ui
            .add(egui::Button::new("💬  AI に聞く…").shortcut_text("F1"))
            .on_hover_text("使い方を文で質問する")
            .clicked()
        {
            state.help.open();
            ui.close();
        }
        if ui
            .button("AI の設定…")
            .on_hover_text("生成 AI (Claude API) のキーとモデルを設定する")
            .clicked()
        {
            state.assistant.toggle_settings();
            ui.close();
        }
    });
}

fn help_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("ヘルプ", |ui| {
        if ui
            .add(egui::Button::new("💬  AI に聞く…").shortcut_text("F1"))
            .on_hover_text("したいことを文で聞くと、手順と使う道具を答えます")
            .clicked()
        {
            state.help.open();
            ui.close();
        }
        ui.menu_button("キー操作の一覧", |ui| {
            egui::Grid::new("shortcuts").striped(true).show(ui, |ui| {
                for (key, action) in SHORTCUTS {
                    ui.label(egui::RichText::new(*key).color(palette::TEAL));
                    ui.label(*action);
                    ui.end_row();
                }
            });
        });
        ui.separator();
        if ui
            .button("ようこそ画面")
            .on_hover_text("はじめに出る案内とチュートリアルの入口をもう一度開く")
            .clicked()
        {
            state.tutorial.welcome_requested = true;
            ui.close();
        }
        if ui
            .button("ミッションを最初からやり直す")
            .on_hover_text("チュートリアルのミッションと経験値を最初に戻す")
            .clicked()
        {
            state.playful.reset_missions();
        }
    });
}

/// ツールバーの1グループ (薄い地の上にボタンを並べる)
fn group(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(palette::NAVY_DEEP)
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(4, 3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.horizontal(add);
        });
}

fn toolbar(ui: &mut egui::Ui, state: &mut UiState) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        group(ui, |ui| {
            if ui
                .button("📂 開く")
                .on_hover_text("プロジェクトやファイルを開く (Ctrl+O)")
                .clicked()
            {
                state.jobs.request_dialog();
            }
            let label = if state.project.unsaved() {
                "💾 保存 •"
            } else {
                "💾 保存"
            };
            if ui
                .button(label)
                .on_hover_text("プロジェクトを保存 (Ctrl+S)。• は保存していない変更がある印")
                .clicked()
            {
                state.project.request(ProjectAction::Save);
            }
        });
        group(ui, |ui| undo_redo_buttons(ui, state));
        group(ui, |ui| {
            for tool in [Tool3d::Translate, Tool3d::Rotate, Tool3d::Scale] {
                let (icon, name, key) = match tool {
                    Tool3d::Translate => ("✚", "移動", "W"),
                    Tool3d::Rotate => ("🔃", "回転", "E"),
                    Tool3d::Scale => ("↔", "拡大縮小", "R"),
                };
                let selected = state.gizmo.tool == tool;
                if ui
                    .selectable_label(selected, format!("{icon} {name}"))
                    .on_hover_text(format!("{name}のギズモ ({key})"))
                    .clicked()
                {
                    state.gizmo.tool = tool;
                }
            }
        });
        group(ui, |ui| projection_buttons(ui, state));
        group(ui, |ui| {
            ui.menu_button("🎥 ビュー", |ui| {
                for view in StandardView::ALL {
                    if ui
                        .add(egui::Button::new(view.label()).shortcut_text(view.shortcut()))
                        .clicked()
                    {
                        state.camera.write(CameraRequest::View(view));
                    }
                }
            })
            .response
            .on_hover_text("正面・上面・等角などの標準ビュー (右上のビューキューブでも選べます)");
            if ui
                .button("⛶ 全体")
                .on_hover_text("全体を表示 (Home)")
                .clicked()
            {
                state.camera.write(CameraRequest::FitAll);
            }
            if ui
                .button("🔍 選択")
                .on_hover_text("選んだ部品を拡大 (F)")
                .clicked()
            {
                state.camera.write(CameraRequest::FitSelection);
            }
        });
        group(ui, |ui| {
            ui.toggle_value(&mut state.settings.show_edges, "稜線")
                .on_hover_text("部品の稜線を表示する");
            ui.toggle_value(&mut state.settings.ghost_unselected, "半透明")
                .on_hover_text("選択以外を半透明にする");
            ui.toggle_value(&mut state.settings.hall.show, "工場の床")
                .on_hover_text("背景に工場の床・通路・壁を出す (広さは 表示 → 工場の床と壁)");
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            level_pill(ui, state);
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("💬 AI に聞く").color(egui::Color32::WHITE),
                    )
                    .fill(palette::PURPLE),
                )
                .on_hover_text("使い方を文で質問する (F1)")
                .clicked()
            {
                state.help.toggle();
            }
        });
    });
}

/// 右端のレベル表示。押すとミッションカードを出し入れする。
fn level_pill(ui: &mut egui::Ui, state: &mut UiState) {
    let progress = &state.playful.prefs.progress;
    let level = progress.level();
    let text = egui::RichText::new(format!(
        "★ Lv.{}  {}/{}",
        level.level,
        progress.done_count(),
        cadsim_core::gamify::MISSIONS.len()
    ))
    .color(palette::YELLOW);
    let shown = state.playful.prefs.show_missions;
    if ui
        .add(
            egui::Button::new(text)
                .fill(palette::NAVY_DEEP)
                .selected(shown),
        )
        .on_hover_text(format!(
            "{} ({} / {} XP)\nクリックでミッションカードを{}",
            level.title(),
            level.into_level,
            level.needed,
            if shown { "隠す" } else { "出す" }
        ))
        .clicked()
    {
        state.playful.set_show_missions(!shown);
    }
}

/// 「透視」「平行」の切り替えボタン (メニューとツールバーで共通)
fn projection_buttons(ui: &mut egui::Ui, state: &mut UiState) {
    ui.horizontal(|ui| {
        for (orthographic, label, hover) in [
            (
                false,
                "透視",
                "透視投影: 遠くのものほど小さく見える (5 で切り替え)",
            ),
            (
                true,
                "平行",
                "平行投影: 距離によらず同じ大きさに見える (5 で切り替え)",
            ),
        ] {
            let selected = state.orbit.orthographic == orthographic;
            if ui
                .selectable_label(selected, label)
                .on_hover_text(hover)
                .clicked()
            {
                state
                    .camera
                    .write(CameraRequest::SetOrthographic(orthographic));
            }
        }
    });
}

pub fn add_box(state: &mut UiState) {
    let size = state.model.new_box_size;
    let position = state.model.free_spot(size);
    let id = state.model.add_box(size, position);
    let label = format!("{} を追加", state.model.path_label(id));
    state.history.record_added(&state.model, id, label);
    state.selection.select(id);
    state.selection.reveal_in_tree = true;
}
