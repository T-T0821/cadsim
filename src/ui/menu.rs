//! 上部のメニューバーとツールバー

use bevy_egui::egui;

use super::UiState;
use super::modeling::Tab;
use crate::camera::{CameraRequest, StandardView};
use crate::file_io::ExportScope;
use crate::gizmo::Tool;
use crate::history::Request;
use crate::interchange::{FORMATS, LENGTH_UNITS};
use crate::project::ProjectAction;
use crate::result_export::ResultKind;
use cadsim_core::gamify::ThemeKind;

pub fn show(root: &mut egui::Ui, state: &mut UiState) {
    egui::Panel::top("menu_bar").show(root, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            file_menu(ui, state);
            edit_menu(ui, state);
            view_menu(ui, state);
            create_menu(ui, state);
            line_menu(ui, state);
            help_menu(ui, state);
        });
        ui.separator();
        toolbar(ui, state);
        ui.add_space(2.0);
    });
}

fn file_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("ファイル", |ui| {
        if ui.button("新規プロジェクト  (Ctrl+N)").clicked() {
            state.project.request(ProjectAction::New);
        }
        if ui
            .button("開く…  (Ctrl+O)")
            .on_hover_text(
                "プロジェクト (.cadsim) を開くか、STEP / glTF / OBJ / STL を今のプロジェクトに取り込む",
            )
            .clicked()
        {
            state.jobs.request_dialog();
        }
        if ui.button("サンプルのアセンブリを取り込む").clicked() {
            state.jobs.open_sample();
        }
        ui.menu_button("負荷テスト用シーンを開く", |ui| {
            for count in [1000, 5000, 10000] {
                if ui.button(format!("部品 {count} 個")).clicked() {
                    state.jobs.open_stress(count);
                }
            }
        });
        ui.separator();
        if ui.button("保存  (Ctrl+S)").clicked() {
            state.project.request(ProjectAction::Save);
        }
        if ui.button("名前を付けて保存…  (Ctrl+Shift+S)").clicked() {
            state.project.request(ProjectAction::SaveAs);
        }
        ui.separator();
        export_menu(ui, state);
        results_menu(ui, state);
        ui.separator();
        if ui
            .button("すべて閉じる")
            .on_hover_text("今のプロジェクトのまま中身を空にする (元に戻せる)")
            .clicked()
        {
            state.history.clear_all(&mut state.model);
            state.selection.clear();
        }
        ui.separator();
        ui.weak("ウィンドウへのドラッグ&ドロップでも開けます");
        ui.weak(format!("取り込める形式: STEP, {}", format_list()));
        ui.weak("平面図として床に敷く形式: DXF, PDF, PNG, JPG");
        ui.weak("点群: PLY, LAS, E57 (PLY は「OBJ / STL の単位」で読みます)");
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
        undo_redo_buttons(ui, state);
        ui.separator();
        let target = state.selection.node;
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

/// 「元に戻す」「やり直し」のボタン。押せないときは灰色にし、何が戻るかをツールチップに出す。
fn undo_redo_buttons(ui: &mut egui::Ui, state: &mut UiState) {
    for (text, request, label) in [
        (
            "元に戻す (Ctrl+Z)",
            Request::Undo,
            state.history.undo_label(),
        ),
        (
            "やり直し (Ctrl+Y)",
            Request::Redo,
            state.history.redo_label(),
        ),
    ] {
        let button = ui.add_enabled(label.is_some(), egui::Button::new(text));
        if button.on_hover_text(label.unwrap_or_default()).clicked() {
            state.history.request = Some(request);
        }
    }
}

fn view_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("表示", |ui| {
        if ui.button("全体を表示  (Home)").clicked() {
            state.camera.write(CameraRequest::FitAll);
        }
        if ui.button("選択を拡大  (F)").clicked() {
            state.camera.write(CameraRequest::FitSelection);
        }
        projection_buttons(ui, state);
        ui.menu_button("標準ビュー", |ui| {
            for view in StandardView::ALL {
                let label = format!("{}  ({})", view.label(), view.shortcut());
                if ui.button(label).clicked() {
                    state.camera.write(CameraRequest::View(view));
                }
            }
        });
        ui.separator();
        ui.checkbox(&mut state.settings.show_edges, "稜線を表示");
        ui.checkbox(&mut state.settings.show_grid, "グリッドを表示");
        ui.checkbox(&mut state.settings.show_bounds, "選択の外接箱を表示");
        ui.checkbox(
            &mut state.settings.ghost_unselected,
            "選択以外を半透明にする",
        );
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
                egui::Slider::new(&mut state.settings.hide_below_px, 0.0..=20.0)
                    .text("省略 (px 未満)"),
            );
        });
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
        ui.separator();
        if ui.button("すべて表示").clicked() {
            state.model.show_all();
        }
        ui.separator();
        theme_menu(ui, state);
    });
}

/// 見た目 (ゲーム風 / シンプル) とミッションカードの表示
fn theme_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("テーマ", |ui| {
        for (theme, hover) in [
            (
                ThemeKind::Playful,
                "明るい色と丸いボタン。ミッション達成で紙吹雪が舞います",
            ),
            (ThemeKind::Plain, "落ち着いた標準の見た目。演出なし"),
        ] {
            let selected = state.playful.prefs.theme == theme;
            if ui
                .radio(selected, theme.label())
                .on_hover_text(hover)
                .clicked()
            {
                state.playful.set_theme(theme);
            }
        }
    });
    let mut show = state.playful.prefs.show_missions;
    if ui
        .checkbox(&mut show, "ミッションを表示")
        .on_hover_text("3D ビューの左下に、操作を覚えるためのミッションを出す")
        .changed()
    {
        state.playful.set_show_missions(show);
    }
}

fn create_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("作成", |ui| {
        if ui.button("箱を追加").clicked() {
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
                ui.close();
            }
        }
    });
}

fn line_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("ライン", |ui| {
        let label = if state.flow.run.open {
            "工程シミュレーションを閉じる"
        } else {
            "工程シミュレーション…"
        };
        if ui
            .button(label)
            .on_hover_text("工程とバッファを並べてラインを早回しし、稼働率やスループットを集計する")
            .clicked()
        {
            state.flow.toggle();
        }
        if ui.button("サンプル工程を読み込む").clicked() {
            state.flow.load_sample();
        }
    });
}

fn help_menu(ui: &mut egui::Ui, state: &mut UiState) {
    ui.menu_button("ヘルプ", |ui| {
        if ui
            .button("ミッションを最初からやり直す")
            .on_hover_text("チュートリアルのミッションと経験値を最初に戻す")
            .clicked()
        {
            state.playful.reset_missions();
        }
        ui.separator();
        egui::Grid::new("shortcuts").striped(true).show(ui, |ui| {
            for (key, action) in [
                ("左クリック", "部品を選択"),
                ("Alt + 左ドラッグ", "回転 (オービット)"),
                ("中ドラッグ", "平行移動 (パン)"),
                ("ホイール", "ズーム"),
                ("右ドラッグ + WASD/QE", "フライ移動"),
                ("W / E / R", "移動 / 回転 / 拡大縮小ギズモ"),
                ("F / Home", "選択を拡大 / 全体を表示"),
                (
                    "1 / 3 / 7 / 0",
                    "正面 / 右側面 / 上面 / 等角 (Ctrl で反対側)",
                ),
                ("5", "透視投影 / 平行投影を切り替え"),
                ("H / Alt+H", "選択を隠す / すべて表示"),
                ("Ctrl+N / Ctrl+O", "新規プロジェクト / 開く"),
                ("Ctrl+S / Ctrl+Shift+S", "保存 / 名前を付けて保存"),
                ("Esc", "選択を解除"),
                ("Delete", "選択を削除"),
                ("Ctrl+Z", "元に戻す"),
                ("Ctrl+Y / Ctrl+Shift+Z", "やり直し"),
            ] {
                ui.label(key);
                ui.label(action);
                ui.end_row();
            }
        });
    });
}

fn toolbar(ui: &mut egui::Ui, state: &mut UiState) {
    ui.horizontal(|ui| {
        if ui
            .button("📂 開く")
            .on_hover_text("プロジェクトか STEP ファイルを開く (Ctrl+O)")
            .clicked()
        {
            state.jobs.request_dialog();
        }
        if ui
            .button("💾 保存")
            .on_hover_text("プロジェクトを保存 (Ctrl+S)")
            .clicked()
        {
            state.project.request(ProjectAction::Save);
        }
        ui.separator();

        undo_redo_buttons(ui, state);
        ui.separator();

        for tool in [Tool::Translate, Tool::Rotate, Tool::Scale] {
            let selected = state.gizmo.tool == tool;
            if ui.selectable_label(selected, tool.label()).clicked() {
                state.gizmo.tool = tool;
            }
        }
        ui.separator();

        projection_buttons(ui, state);
        ui.separator();

        for view in StandardView::ALL {
            let button = ui.button(view.label()).on_hover_text(format!(
                "{}ビュー ({})",
                view.label(),
                view.shortcut()
            ));
            if button.clicked() {
                state.camera.write(CameraRequest::View(view));
            }
        }
        if ui
            .button("⛶ 全体")
            .on_hover_text("全体を表示 (Home)")
            .clicked()
        {
            state.camera.write(CameraRequest::FitAll);
        }
        ui.separator();

        ui.toggle_value(&mut state.settings.show_edges, "稜線");
        ui.toggle_value(&mut state.settings.ghost_unselected, "半透明")
            .on_hover_text("選択以外を半透明にする");
    });
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
