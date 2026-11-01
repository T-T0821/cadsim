//! 図面 (DXF / PDF) と部品表 (CSV / Excel) の書き出し (UI 層)
//!
//! メニューから頼まれたら、図面は設定のウィンドウを出してから、保存先を選ばせて書く。
//! 図面と部品表の中身は `drawing` / `bom`、ファイル形式は `formats` が作る。

use std::path::Path;

use bevy::{ecs::system::NonSendMarker, prelude::*};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::document::import::ImportJobs;
use crate::document::project::ProjectState;
use plantwright_core::bom::make_bom;
use plantwright_core::drawing::DrawingOptions;
use plantwright_core::drawing::collect::drawing_input;
use plantwright_core::drawing::make_sheet;
use plantwright_core::model::Model;
use plantwright_core::model::file_name;

pub struct DocumentsPlugin;

impl Plugin for DocumentsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DocumentExport>()
            .add_systems(Update, save_if_requested)
            .add_systems(EguiPrimaryContextPass, drawing_dialog);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Request {
    Drawing,
    Bom,
}

#[derive(Resource, Default)]
pub struct DocumentExport {
    /// 図面の設定ウィンドウを出している
    dialog_open: bool,
    options: DrawingOptions,
    request: Option<Request>,
}

impl DocumentExport {
    pub fn open_drawing_dialog(&mut self) {
        self.dialog_open = true;
    }

    pub fn request_bom(&mut self) {
        self.request = Some(Request::Bom);
    }
}

fn drawing_dialog(mut contexts: EguiContexts, mut export: ResMut<DocumentExport>) -> Result {
    if !export.dialog_open {
        return Ok(());
    }
    let export = &mut *export;
    let mut open = true;
    egui::Window::new("図面を書き出し")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(contexts.ctx_mut()?, |ui| {
            let o = &mut export.options;
            ui.label("表示中の部品を、A3 横の図面にします (第三角法)");
            ui.horizontal(|ui| {
                ui.checkbox(&mut o.top, "上面図");
                ui.checkbox(&mut o.front, "正面図");
            });
            ui.checkbox(&mut o.hidden_lines, "かくれ線 (破線) を描く");
            ui.checkbox(&mut o.overall_dims, "全体寸法");
            ui.checkbox(&mut o.chain_dims, "設備の間隔寸法 (直列寸法)");
            ui.checkbox(&mut o.labels, "上面図に設備名を書く");
            ui.separator();
            ui.weak("尺度は図が用紙に収まるよう自動で選びます");
            ui.horizontal(|ui| {
                let any_view = o.top || o.front;
                if ui
                    .add_enabled(any_view, egui::Button::new("保存先を選んで書き出し…"))
                    .clicked()
                {
                    export.request = Some(Request::Drawing);
                    export.dialog_open = false;
                }
                if ui.button("やめる").clicked() {
                    export.dialog_open = false;
                }
            });
        });
    if !open {
        export.dialog_open = false;
    }
    Ok(())
}

/// 保存ダイアログはメインスレッドで出す必要があるので NonSendMarker を付ける
fn save_if_requested(
    mut export: ResMut<DocumentExport>,
    mut jobs: ResMut<ImportJobs>,
    model: Res<Model>,
    project: Res<ProjectState>,
    _main_thread: NonSendMarker,
) {
    let Some(request) = export.request.take() else {
        return;
    };
    let title = Path::new(&project.file_name())
        .file_stem()
        .map_or_else(|| "無題".to_owned(), |s| s.to_string_lossy().into_owned());
    let result = match request {
        Request::Drawing => {
            let Some(path) = rfd::FileDialog::new()
                .set_title("図面を書き出し")
                .set_file_name(format!("{title}_図面.pdf"))
                .add_filter("PDF", &["pdf"])
                .add_filter("DXF", &["dxf"])
                .save_file()
            else {
                return;
            };
            save_drawing(&model, &title, &export.options, &path).map(|()| path)
        }
        Request::Bom => {
            let Some(path) = rfd::FileDialog::new()
                .set_title("部品表を書き出し")
                .set_file_name(format!("{title}_部品表.xlsx"))
                .add_filter("Excel", &["xlsx"])
                .add_filter("CSV", &["csv"])
                .save_file()
            else {
                return;
            };
            save_bom(&model, &path).map(|()| path)
        }
    };
    let message = match result {
        Ok(path) => format!("{} に書き出しました", file_name(&path)),
        Err(e) => {
            error!("{e}");
            format!("書き出しに失敗しました: {e}")
        }
    };
    jobs.messages.push(message);
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn today() -> String {
    // 外部のクレートを足さずに、UNIX 時刻から UTC の日付を求める
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// 1970-01-01 からの日数 → (年, 月, 日)。Howard Hinnant の方法
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

pub fn save_drawing(
    model: &Model,
    title: &str,
    options: &DrawingOptions,
    path: &Path,
) -> Result<(), String> {
    let input =
        drawing_input(model, title.to_owned(), today()).ok_or("表示中の部品がありません")?;
    let sheet = make_sheet(&input, options);
    let bytes = match extension(path).as_str() {
        "dxf" => plantwright_core::drawing::out::write_dxf(&sheet, &model.unit).into_bytes(),
        _ => plantwright_core::drawing::out::write_pdf(&sheet)?,
    };
    let path = if path.extension().is_none() {
        path.with_extension("pdf")
    } else {
        path.to_owned()
    };
    std::fs::write(&path, bytes).map_err(|e| e.to_string())
}

pub fn save_bom(model: &Model, path: &Path) -> Result<(), String> {
    let bom = make_bom(model);
    if bom.rows.is_empty() {
        return Err("部品がありません".into());
    }
    let bytes = match extension(path).as_str() {
        "csv" => plantwright_core::bom::out::write_csv(&bom).into_bytes(),
        _ => plantwright_core::bom::out::write_xlsx(&bom)?,
    };
    let path = if path.extension().is_none() {
        path.with_extension("xlsx")
    } else {
        path.to_owned()
    };
    std::fs::write(&path, bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_from_unix_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_392), (2025, 10, 31));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn files_are_written() {
        let dir = std::env::temp_dir().join(format!("plantwright_docs_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut model = Model::default();
        model.add_box([1000.0, 500.0, 600.0], [0.0, 250.0, 0.0]);
        for name in ["a.pdf", "a.dxf"] {
            save_drawing(
                &model,
                "テスト",
                &DrawingOptions::default(),
                &dir.join(name),
            )
            .unwrap();
        }
        for name in ["b.xlsx", "b.csv"] {
            save_bom(&model, &dir.join(name)).unwrap();
        }
        assert!(
            std::fs::read(dir.join("a.pdf"))
                .unwrap()
                .starts_with(b"%PDF")
        );
        assert!(
            std::fs::read_to_string(dir.join("b.csv"))
                .unwrap()
                .contains("箱")
        );
        std::fs::remove_dir_all(&dir).ok();
        assert!(save_bom(&Model::default(), &dir.join("c.csv")).is_err());
    }
}
