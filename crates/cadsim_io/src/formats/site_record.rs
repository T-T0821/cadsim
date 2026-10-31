//! プロジェクトファイル (.cadsim) に書く現場データ (平面図の下敷きと点群)
//!
//! 図面や点の中身は書かず、ファイルの場所と配置・読み込みの設定だけを書く。
//! 開くときに元のファイルから読み直す。

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::pointcloud_read::read_cloud;
use super::read_plan_file;
use cadsim_core::site::model::Site;
use cadsim_core::site::pointcloud::{CloudSettings, PlacedCloud};
use cadsim_core::site::underlay::Underlay;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UnderlayRecord {
    /// プロジェクトファイルと同じフォルダ以下なら相対パス、それ以外は絶対パス
    pub path: String,
    /// 図面の座標 1 がモデルの単位でいくつか
    pub scale: f64,
    /// 図面の原点を置くワールドの (X, Z)
    pub origin: [f64; 2],
    #[serde(default)]
    pub rotation_deg: f64,
    #[serde(default)]
    pub elevation: f64,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    #[serde(default = "visible_by_default")]
    pub visible: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CloudRecord {
    /// プロジェクトファイルと同じフォルダ以下なら相対パス、それ以外は絶対パス
    pub path: String,
    /// 点群の真ん中を置くワールドの位置
    pub offset: [f64; 3],
    #[serde(default)]
    pub rotation_deg: f64,
    #[serde(default = "visible_by_default")]
    pub visible: bool,
    /// 最小の点の間隔 (モデルの単位)
    pub spacing: f64,
    /// 残す点の数の上限
    pub budget: usize,
    /// 点群の座標の単位 1 が何 mm か
    pub unit_mm: f64,
    pub z_up: bool,
}

fn path_record(source: &Path, base_dir: Option<&Path>) -> String {
    let relative = base_dir.and_then(|dir| source.strip_prefix(dir).ok());
    match relative {
        Some(rel) => rel.to_string_lossy().replace('\\', "/"),
        None => source.to_string_lossy().into_owned(),
    }
}

fn file_label(source: &Path, fallback: &str) -> String {
    source
        .file_name()
        .map_or_else(|| fallback.to_owned(), |n| n.to_string_lossy().into_owned())
}

pub fn cloud_records(site: &Site, base_dir: Option<&Path>) -> Vec<CloudRecord> {
    site.clouds
        .iter()
        .map(|c| CloudRecord {
            path: path_record(&c.source, base_dir),
            offset: c.offset,
            rotation_deg: c.rotation_deg,
            visible: c.visible,
            spacing: c.settings.spacing,
            budget: c.settings.budget,
            unit_mm: c.settings.unit_mm,
            z_up: c.settings.z_up,
        })
        .collect()
}

fn default_opacity() -> f32 {
    0.8
}

fn visible_by_default() -> bool {
    true
}

/// 現場データをファイルに書く形にする
pub fn to_records(site: &Site, base_dir: Option<&Path>) -> Vec<UnderlayRecord> {
    site.underlays
        .iter()
        .map(|u| UnderlayRecord {
            path: path_record(&u.source, base_dir),
            scale: u.scale,
            origin: u.origin,
            rotation_deg: u.rotation_deg,
            elevation: u.elevation,
            opacity: u.opacity,
            visible: u.visible,
        })
        .collect()
}

/// 記録から現場データを作り直す。図面と点群は元のファイルから読む (重いので別スレッドで呼ぶ)。
/// 読めなかったものは飛ばし、その注意を `warnings` に足す。
pub fn load_site(
    records: &[UnderlayRecord],
    clouds: &[CloudRecord],
    base_dir: &Path,
    model_unit_mm: f64,
    warnings: &mut Vec<String>,
) -> Site {
    let mut site = Site::default();
    for record in clouds {
        let source = base_dir.join(&record.path);
        let name = file_label(&source, &record.path);
        let settings = CloudSettings {
            spacing: record.spacing,
            budget: record.budget,
            unit_mm: record.unit_mm,
            z_up: record.z_up,
        };
        match read_cloud(&source, &settings, model_unit_mm) {
            Ok(cloud) => {
                let mut placed = PlacedCloud::new(name, source, cloud, settings);
                placed.offset = record.offset;
                placed.rotation_deg = record.rotation_deg;
                placed.visible = record.visible;
                site.add_cloud(placed);
            }
            Err(e) => warnings.push(format!("点群 {name}: {e}")),
        }
    }
    for record in records {
        let source = base_dir.join(&record.path);
        let name = file_label(&source, &record.path);
        match read_plan_file(&source) {
            Ok(content) => {
                site.add_underlay(Underlay {
                    name,
                    source,
                    content: Arc::new(content),
                    scale: record.scale,
                    origin: record.origin,
                    rotation_deg: record.rotation_deg,
                    elevation: record.elevation,
                    opacity: record.opacity,
                    visible: record.visible,
                });
            }
            Err(e) => warnings.push(format!("平面図 {name}: {e}")),
        }
    }
    site
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadsim_core::site::underlay::PlanContent;

    #[test]
    fn records_round_trip_through_files() {
        let dir = std::env::temp_dir().join(format!("cadsim_site_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("plans")).unwrap();
        let dxf =
            "0\nSECTION\n2\nENTITIES\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n1\n0\nENDSEC\n0\nEOF\n";
        std::fs::write(dir.join("plans/floor.dxf"), dxf).unwrap();

        let mut site = Site::default();
        let mut underlay = Underlay::new(
            "floor.dxf".into(),
            dir.join("plans/floor.dxf"),
            PlanContent::default(),
            1.0,
        );
        underlay.scale = 25.0;
        underlay.origin = [100.0, -50.0];
        underlay.visible = false;
        site.add_underlay(underlay);

        let records = to_records(&site, Some(&dir));
        assert_eq!(records[0].path, "plans/floor.dxf");
        let mut warnings = Vec::new();
        let loaded = load_site(&records, &[], &dir, 1.0, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(to_records(&loaded, Some(&dir)), records);
        assert_eq!(loaded.underlays[0].content.polylines.len(), 1);

        // 点群 (ASCII の PLY) も設定と配置ごと読み直せる
        let ply = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nend_header\n0 0 0\n100 0 0\n0 100 50\n";
        std::fs::write(dir.join("scan.ply"), ply).unwrap();
        let settings = CloudSettings {
            spacing: 1.0,
            budget: 1000,
            unit_mm: 1.0,
            z_up: true,
        };
        let cloud = read_cloud(&dir.join("scan.ply"), &settings, 1.0).unwrap();
        let mut placed = PlacedCloud::new("scan.ply".into(), dir.join("scan.ply"), cloud, settings);
        placed.offset = [10.0, 20.0, 30.0];
        site.add_cloud(placed);
        let clouds = cloud_records(&site, Some(&dir));
        assert_eq!(clouds[0].path, "scan.ply");
        let loaded = load_site(&[], &clouds, &dir, 1.0, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cloud_records(&loaded, Some(&dir)), clouds);
        assert_eq!(loaded.clouds[0].cloud.points.len(), 3);

        std::fs::remove_dir_all(&dir).ok();
        let missing = load_site(&records, &[], &dir, 1.0, &mut warnings);
        assert!(missing.is_empty());
        assert_eq!(warnings.len(), 1);
    }
}
