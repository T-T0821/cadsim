//! 点群ファイル (PLY / LAS / E57) を読む
//!
//! どの形式も1点ずつ [`Decimator`] に渡して、読みながら間引く (全点をメモリに載せない)。
//! 座標はモデルの単位・Y 軸が上に直してから渡す。
//! - PLY: ASCII とバイナリ (リトル / ビッグエンディアン)。頂点の x / y / z と色 (red / green / blue)
//! - LAS / LAZ: `las` クレートで読む (LAZ は圧縮した LAS)。1.0〜1.4、点の形式 0〜10
//! - E57: `e57` クレートで読む。スキャンが複数あればすべて、各スキャンの姿勢を当てはめて読む

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek};
use std::path::Path;

use glam::DVec3;

use cadsim_core::site::pointcloud::{CloudSettings, Decimator, PointCloud};

/// 点群として読む拡張子
pub const CLOUD_EXTENSIONS: &[&str] = &["ply", "las", "laz", "e57"];

pub fn is_cloud_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| CLOUD_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// 形式ごとの読み込みの既定値 (LAS / E57 はメートル・Z 上)
pub fn default_settings(path: &Path, model_unit_mm: f64) -> CloudSettings {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    CloudSettings {
        // 5 mm 間隔より細かい点は描いても見分けられない
        spacing: 5.0 / model_unit_mm,
        budget: 3_000_000,
        unit_mm: if ext == "ply" { 1.0 } else { 1000.0 },
        z_up: true,
    }
}

/// 点群ファイルを読み、間引いた点群にする
pub fn read_cloud(
    path: &Path,
    settings: &CloudSettings,
    model_unit_mm: f64,
) -> Result<PointCloud, String> {
    let scale = settings.unit_mm / model_unit_mm;
    let z_up = settings.z_up;
    let mut decimator = Decimator::new(settings.spacing, settings.budget);
    let mut push = |x: f64, y: f64, z: f64, color: Option<[u8; 3]>| {
        let p = if z_up {
            DVec3::new(x, z, -y)
        } else {
            DVec3::new(x, y, z)
        };
        decimator.push(p * scale, color);
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let open =
        || File::open(path).map_err(|e| format!("{} を開けませんでした: {e}", path.display()));
    match ext.as_str() {
        "ply" => read_ply(BufReader::new(open()?), &mut push)?,
        "las" | "laz" => read_las(BufReader::new(open()?), &mut push)?,
        "e57" => read_e57(path, &mut push)?,
        _ => return Err("点群として読めない形式です".into()),
    }
    decimator.finish()
}

type Sink<'a> = dyn FnMut(f64, f64, f64, Option<[u8; 3]>) + 'a;

// ---------------------------------------------------------------------------
// PLY
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum PlyFormat {
    Ascii,
    Little,
    Big,
}

#[derive(Clone, Copy)]
enum PlyType {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl PlyType {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "char" | "int8" => PlyType::I8,
            "uchar" | "uint8" => PlyType::U8,
            "short" | "int16" => PlyType::I16,
            "ushort" | "uint16" => PlyType::U16,
            "int" | "int32" => PlyType::I32,
            "uint" | "uint32" => PlyType::U32,
            "float" | "float32" => PlyType::F32,
            "double" | "float64" => PlyType::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            PlyType::I8 | PlyType::U8 => 1,
            PlyType::I16 | PlyType::U16 => 2,
            PlyType::I32 | PlyType::U32 | PlyType::F32 => 4,
            PlyType::F64 => 8,
        }
    }

    fn read(self, b: &[u8], big: bool) -> f64 {
        macro_rules! num {
            ($t:ty, $n:expr) => {{
                let bytes: [u8; $n] = b[..$n].try_into().unwrap_or([0; $n]);
                (if big {
                    <$t>::from_be_bytes(bytes)
                } else {
                    <$t>::from_le_bytes(bytes)
                }) as f64
            }};
        }
        match self {
            PlyType::I8 => b[0] as i8 as f64,
            PlyType::U8 => b[0] as f64,
            PlyType::I16 => num!(i16, 2),
            PlyType::U16 => num!(u16, 2),
            PlyType::I32 => num!(i32, 4),
            PlyType::U32 => num!(u32, 4),
            PlyType::F32 => num!(f32, 4),
            PlyType::F64 => num!(f64, 8),
        }
    }
}

struct PlyVertex {
    count: u64,
    /// (名前, 型)
    properties: Vec<(String, PlyType)>,
}

fn read_ply(mut reader: impl BufRead, push: &mut Sink) -> Result<(), String> {
    let mut line = String::new();
    let read_line = |reader: &mut dyn BufRead, line: &mut String| -> Result<(), String> {
        line.clear();
        let n = reader.read_line(line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("PLY のヘッダーが途中で終わっています".into());
        }
        Ok(())
    };
    read_line(&mut reader, &mut line)?;
    if line.trim() != "ply" {
        return Err("PLY ファイルではありません".into());
    }
    let mut format = PlyFormat::Ascii;
    let mut vertex: Option<PlyVertex> = None;
    let mut vertex_first = true;
    let mut in_vertex = false;
    loop {
        read_line(&mut reader, &mut line)?;
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["format", f, ..] => {
                format = match *f {
                    "ascii" => PlyFormat::Ascii,
                    "binary_little_endian" => PlyFormat::Little,
                    "binary_big_endian" => PlyFormat::Big,
                    _ => return Err(format!("PLY の形式 {f} には対応していません")),
                }
            }
            ["element", name, count] => {
                in_vertex = *name == "vertex";
                if in_vertex {
                    vertex = Some(PlyVertex {
                        count: count.parse().map_err(|_| "PLY の頂点数を読めません")?,
                        properties: Vec::new(),
                    });
                } else if vertex.is_none() {
                    vertex_first = false;
                }
            }
            ["property", "list", ..] if in_vertex => {
                return Err("頂点にリストを持つ PLY には対応していません".into());
            }
            ["property", ty, name] if in_vertex => {
                let ty = PlyType::parse(ty).ok_or_else(|| format!("PLY の型 {ty} を読めません"))?;
                if let Some(v) = &mut vertex {
                    v.properties.push((name.to_string(), ty));
                }
            }
            ["end_header"] => break,
            _ => {}
        }
    }
    let vertex = vertex.ok_or("PLY に頂点がありません")?;
    if !vertex_first {
        return Err("頂点が先頭の要素でない PLY には対応していません".into());
    }
    let index = |names: &[&str]| {
        vertex
            .properties
            .iter()
            .position(|(n, _)| names.contains(&n.as_str()))
    };
    let (Some(xi), Some(yi), Some(zi)) = (index(&["x"]), index(&["y"]), index(&["z"])) else {
        return Err("PLY の頂点に x / y / z がありません".into());
    };
    let color = match (
        index(&["red", "r", "diffuse_red"]),
        index(&["green", "g", "diffuse_green"]),
        index(&["blue", "b", "diffuse_blue"]),
    ) {
        (Some(r), Some(g), Some(b)) => Some([r, g, b]),
        _ => None,
    };
    // 色が 16bit や 0〜1 の小数のこともあるので、型に合わせて 0〜255 にする
    let to_byte = |ty: PlyType, v: f64| -> u8 {
        match ty {
            PlyType::F32 | PlyType::F64 => (v * 255.0).clamp(0.0, 255.0) as u8,
            PlyType::U16 | PlyType::I16 => (v / 257.0).clamp(0.0, 255.0) as u8,
            _ => v.clamp(0.0, 255.0) as u8,
        }
    };
    let mut values = vec![0.0f64; vertex.properties.len()];
    let emit = |values: &[f64], push: &mut Sink| {
        let rgb = color.map(|c| c.map(|i| to_byte(vertex.properties[i].1, values[i])));
        push(values[xi], values[yi], values[zi], rgb);
    };
    match format {
        PlyFormat::Ascii => {
            for _ in 0..vertex.count {
                line.clear();
                if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                    break;
                }
                for (slot, word) in values.iter_mut().zip(line.split_whitespace()) {
                    *slot = word.parse().unwrap_or(0.0);
                }
                emit(&values, push);
            }
        }
        PlyFormat::Little | PlyFormat::Big => {
            let big = format == PlyFormat::Big;
            let stride: usize = vertex.properties.iter().map(|(_, t)| t.size()).sum();
            let mut record = vec![0u8; stride];
            for _ in 0..vertex.count {
                if reader.read_exact(&mut record).is_err() {
                    break;
                }
                let mut at = 0;
                for (slot, (_, ty)) in values.iter_mut().zip(&vertex.properties) {
                    *slot = ty.read(&record[at..], big);
                    at += ty.size();
                }
                emit(&values, push);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// LAS
// ---------------------------------------------------------------------------

/// 一度に読む点の数 (全点をメモリに載せないよう、区切って読む)
const LAS_CHUNK: u64 = 65_536;

fn read_las(
    reader: impl Read + Seek + Send + Sync + 'static,
    push: &mut Sink,
) -> Result<(), String> {
    let mut reader =
        las::Reader::new(reader).map_err(|e| format!("LAS / LAZ を読めませんでした: {e}"))?;
    // 色が 8bit で入っているファイルもあるので、最初の色付きの点で見分ける
    let mut sixteen_bit: Option<bool> = None;
    loop {
        let chunk = reader
            .read_points(LAS_CHUNK)
            .map_err(|e| format!("LAS / LAZ の点を読めませんでした: {e}"))?;
        if chunk.is_empty() {
            break;
        }
        for point in chunk.points() {
            let point = point.map_err(|e| format!("LAS / LAZ の点を読めませんでした: {e}"))?;
            let color = point.color.map(|c| {
                let rgb = [c.red, c.green, c.blue];
                let wide = *sixteen_bit.get_or_insert(rgb.iter().any(|&v| v > 255));
                rgb.map(|v| if wide { (v >> 8) as u8 } else { v as u8 })
            });
            push(point.x, point.y, point.z, color);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// E57
// ---------------------------------------------------------------------------

fn read_e57(path: &Path, push: &mut Sink) -> Result<(), String> {
    let mut reader =
        e57::E57Reader::from_file(path).map_err(|e| format!("E57 を読めませんでした: {e}"))?;
    let clouds = reader.pointclouds();
    if clouds.is_empty() {
        return Err("E57 に点群がありません".into());
    }
    for pc in &clouds {
        let points = reader
            .pointcloud_simple(pc)
            .map_err(|e| format!("E57 の点群を読めませんでした: {e}"))?;
        for point in points {
            let Ok(point) = point else { continue };
            if let e57::CartesianCoordinate::Valid { x, y, z } = point.cartesian {
                let color = point
                    .color
                    .map(|c| [c.red, c.green, c.blue].map(|v| (v * 255.0).clamp(0.0, 255.0) as u8));
                push(x, y, z, color);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn collect(
        f: impl FnOnce(&mut Sink) -> Result<(), String>,
    ) -> Vec<(f64, f64, f64, Option<[u8; 3]>)> {
        let mut out = Vec::new();
        let mut sink = |x, y, z, c| out.push((x, y, z, c));
        f(&mut sink).unwrap();
        out
    }

    #[test]
    fn ascii_ply() {
        let text = "ply\nformat ascii 1.0\nelement vertex 2\nproperty float x\nproperty float y\nproperty float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nelement face 0\nproperty list uchar int vertex_indices\nend_header\n1 2 3 255 0 0\n4 5 6 0 255 0\n";
        let points = collect(|s| read_ply(Cursor::new(text), s));
        assert_eq!(
            points,
            vec![
                (1.0, 2.0, 3.0, Some([255, 0, 0])),
                (4.0, 5.0, 6.0, Some([0, 255, 0]))
            ]
        );
    }

    #[test]
    fn binary_ply() {
        let mut bytes = b"ply\nformat binary_little_endian 1.0\nelement vertex 2\nproperty double x\nproperty double y\nproperty double z\nend_header\n".to_vec();
        for v in [1.5f64, -2.0, 3.0, 10.0, 20.0, 30.0] {
            bytes.extend(v.to_le_bytes());
        }
        let points = collect(|s| read_ply(Cursor::new(bytes), s));
        assert_eq!(
            points,
            vec![(1.5, -2.0, 3.0, None), (10.0, 20.0, 30.0, None)]
        );
    }

    /// 点の形式 2 (色付き) の LAS 1.2 を作る。`compressed` なら LAZ
    fn las_bytes(points: &[(f64, f64, f64, [u16; 3])], compressed: bool) -> Vec<u8> {
        let mut builder = las::Builder::from((1, 2));
        builder.point_format = las::point::Format::new(2).unwrap();
        builder.point_format.is_compressed = compressed;
        let header = builder.into_header().unwrap();
        let mut writer = las::Writer::new(Cursor::new(Vec::new()), header).unwrap();
        for &(x, y, z, [red, green, blue]) in points {
            writer
                .write_point(las::Point {
                    x,
                    y,
                    z,
                    color: Some(las::Color { red, green, blue }),
                    ..Default::default()
                })
                .unwrap();
        }
        writer.into_inner().unwrap().into_inner()
    }

    #[test]
    fn las_points_and_colors() {
        let bytes = las_bytes(
            &[
                (1001.0, 2.0, 3.0, [65535, 0, 32768]),
                (0.0, 0.0, 0.0, [0; 3]),
            ],
            false,
        );
        let points = collect(|s| read_las(Cursor::new(bytes), s));
        assert_eq!(points.len(), 2);
        let (x, y, z, c) = points[0];
        assert!((x - 1001.0).abs() < 1e-9 && (y - 2.0).abs() < 1e-9 && (z - 3.0).abs() < 1e-9);
        assert_eq!(c, Some([255, 0, 128]));
        assert!(
            read_las(
                Cursor::new(b"not a las file".to_vec()),
                &mut |_, _, _, _| {}
            )
            .is_err()
        );
    }

    #[test]
    fn laz_is_read_like_las() {
        let points = [(1.0, 2.0, 3.0, [10, 20, 30]), (4.0, 5.0, 6.0, [40, 50, 60])];
        let laz = collect(|s| read_las(Cursor::new(las_bytes(&points, true)), s));
        let las = collect(|s| read_las(Cursor::new(las_bytes(&points, false)), s));
        assert_eq!(laz, las);
        assert_eq!(laz[1].3, Some([40, 50, 60]));
    }

    #[test]
    fn files_are_decimated_into_model_units() {
        let dir = std::env::temp_dir().join(format!("plantwright_pc_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // LAS (m, Z 上): 1 m 上の点は mm のモデルで Y = 1000
        let path = dir.join("scan.las");
        std::fs::write(
            &path,
            las_bytes(&[(0.0, 0.0, 1.0, [0; 3]), (1.0, 0.0, 0.0, [0; 3])], false),
        )
        .unwrap();
        let settings = default_settings(&path, 1.0);
        assert_eq!(settings.unit_mm, 1000.0);
        let cloud = read_cloud(&path, &settings, 1.0).unwrap();
        assert_eq!(cloud.points.len(), 2);
        let (min, max) = cloud.bounds();
        assert!(((max - min).y - 1000.0).abs() < 1e-3, "{min} {max}");
        assert!(((max - min).x - 1000.0).abs() < 1e-3);
        assert!(is_cloud_file(&path));
        std::fs::remove_dir_all(&dir).ok();
    }
}
