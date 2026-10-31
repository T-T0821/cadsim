//! プロジェクトファイル (.cadsim) に書く現場データ (平面図の下敷き)
//!
//! 図面の中身は書かず、ファイルの場所と配置だけを書く。開くときに元のファイルから読み直す。

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::read_plan_file;
use crate::site::model::Site;
use crate::site::underlay::Underlay;

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
        .map(|u| {
            let relative = base_dir.and_then(|dir| u.source.strip_prefix(dir).ok());
            let path = match relative {
                Some(rel) => rel.to_string_lossy().replace('\\', "/"),
                None => u.source.to_string_lossy().into_owned(),
            };
            UnderlayRecord {
                path,
                scale: u.scale,
                origin: u.origin,
                rotation_deg: u.rotation_deg,
                elevation: u.elevation,
                opacity: u.opacity,
                visible: u.visible,
            }
        })
        .collect()
}

/// 記録から現場データを作り直す。図面は元のファイルから読む (重いので別スレッドで呼ぶ)。
/// 読めなかった図面は飛ばし、その注意を `warnings` に足す。
pub fn load_site(records: &[UnderlayRecord], base_dir: &Path, warnings: &mut Vec<String>) -> Site {
    let mut site = Site::default();
    for record in records {
        let source = base_dir.join(&record.path);
        let name = source
            .file_name()
            .map_or_else(|| record.path.clone(), |n| n.to_string_lossy().into_owned());
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
    use crate::site::underlay::PlanContent;

    #[test]
    fn records_round_trip_through_files() {
        let dir = std::env::temp_dir().join(format!("cadsim_site_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("plans")).unwrap();
        let dxf = "0\nSECTION\n2\nENTITIES\n0\nLINE\n10\n0\n20\n0\n11\n1\n21\n1\n0\nENDSEC\n0\nEOF\n";
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
        let loaded = load_site(&records, &dir, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(to_records(&loaded, Some(&dir)), records);
        assert_eq!(loaded.underlays[0].content.polylines.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
        let missing = load_site(&records, &dir, &mut warnings);
        assert!(missing.is_empty());
        assert_eq!(warnings.len(), 1);
    }
}
