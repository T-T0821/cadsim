//! 取り込むファイルを「アセットの鍵」(相対パスの形の文字列) に直す、とその逆
//!
//! アプリはファイルの読み込みを Bevy のアセットの仕組みに任せる。アセットは決まったフォルダの中の
//! 相対パスでしか呼べないので、どこにあるファイルでも呼べるよう、絶対パスと単位を相対パスの形に詰める。
//!
//! 形: `<単位>/<根>/<残りのパス>`
//! - 単位: ファイルに単位の情報がない形式 (OBJ / STL) は `<1単位のメートルの f64 のビット (16進)><y か z>`、
//!   それ以外は `-`。単位を変えて読み直すと別のアセットになる
//! - 根: Unix の `/` は `r`、Windows のドライブ `C:\` は `d-C`、ネットワークの `\\server\share` は `unc/server/share`
//!
//! Bevy には依存しない。

use std::path::{Component, Path, PathBuf, Prefix};

use crate::interchange::{UnitOptions, format_for_path};

/// 単位を使わない形式の印
const NO_UNITS: &str = "-";

fn units_tag(path: &Path, units: &UnitOptions) -> String {
    if format_for_path(path).is_some_and(|f| f.needs_units) {
        format!(
            "{:016x}{}",
            units.meters_per_unit.to_bits(),
            if units.z_up { "z" } else { "y" }
        )
    } else {
        NO_UNITS.to_owned()
    }
}

fn parse_units(tag: &str) -> Option<Option<UnitOptions>> {
    if tag == NO_UNITS {
        return Some(None);
    }
    let (bits, up) = tag.split_at_checked(16)?;
    let z_up = match up {
        "z" => true,
        "y" => false,
        _ => return None,
    };
    let meters_per_unit = f64::from_bits(u64::from_str_radix(bits, 16).ok()?);
    Some(Some(UnitOptions {
        meters_per_unit,
        z_up,
    }))
}

/// 絶対パスのファイルを鍵にする。相対パスや、根の形が分からないパスは None
pub fn encode(path: &Path, units: &UnitOptions) -> Option<PathBuf> {
    let mut key = PathBuf::from(units_tag(path, units));
    let mut components = path.components();
    match components.next()? {
        Component::RootDir => key.push("r"),
        Component::Prefix(prefix) => {
            match prefix.kind() {
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                    key.push(format!("d-{}", char::from(letter).to_ascii_uppercase()))
                }
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                    key.push("unc");
                    key.push(server);
                    key.push(share);
                }
                _ => return None,
            }
            // ドライブの後ろの `\`
            if components.next()? != Component::RootDir {
                return None;
            }
        }
        _ => return None,
    }
    for c in components {
        match c {
            Component::Normal(name) => key.push(name),
            // `..` や `.` が残っていると、ほかのファイルと同じ鍵にならないことがある
            _ => return None,
        }
    }
    Some(key)
}

/// 鍵から、ファイルの絶対パスと単位 (単位を使わない形式は None) に戻す
pub fn decode(key: &Path) -> Option<(PathBuf, Option<UnitOptions>)> {
    let mut parts = key.components().map(|c| match c {
        Component::Normal(s) => s.to_str(),
        _ => None,
    });
    let units = parse_units(parts.next()??)?;
    let root = parts.next()??;
    let mut path = if root == "r" {
        PathBuf::from("/")
    } else if let Some(letter) = root.strip_prefix("d-") {
        PathBuf::from(format!("{letter}:\\"))
    } else if root == "unc" {
        let server = parts.next()??;
        let share = parts.next()??;
        PathBuf::from(format!("\\\\{server}\\{share}\\"))
    } else {
        return None;
    };
    // 残りは OS の文字列のまま (UTF-8 でない名前も通す)
    let skip = match root {
        "unc" => 4,
        _ => 2,
    };
    for c in key.components().skip(skip) {
        path.push(c.as_os_str());
    }
    Some((path, units))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mm() -> UnitOptions {
        UnitOptions::default()
    }

    #[test]
    fn round_trips_an_absolute_path_on_this_os() {
        let dir = std::env::temp_dir();
        let path = std::path::absolute(dir.join("工場 #1").join("robot.glb")).unwrap();
        let key = encode(&path, &mm()).unwrap();
        assert!(key.is_relative());
        assert!(key.starts_with("-"), "glTF は単位を使わない: {key:?}");
        let (back, units) = decode(&key).unwrap();
        assert_eq!(back, path);
        assert_eq!(units, None);
    }

    #[cfg(unix)]
    #[test]
    fn unix_paths_start_from_the_root() {
        let key = encode(Path::new("/home/user/part.stl"), &mm()).unwrap();
        let parts: Vec<_> = key.iter().map(|s| s.to_str().unwrap()).collect();
        assert_eq!(parts[1..], ["r", "home", "user", "part.stl"]);
        let (back, units) = decode(&key).unwrap();
        assert_eq!(back, Path::new("/home/user/part.stl"));
        assert_eq!(units, Some(mm()));
    }

    #[test]
    fn units_are_part_of_the_key_only_for_formats_that_need_them() {
        let dir = std::path::absolute(std::env::temp_dir()).unwrap();
        let inch = UnitOptions {
            meters_per_unit: 0.0254,
            z_up: true,
        };
        let stl = dir.join("part.stl");
        assert_ne!(encode(&stl, &mm()), encode(&stl, &inch));
        assert_eq!(decode(&encode(&stl, &inch).unwrap()).unwrap().1, Some(inch));
        let glb = dir.join("part.glb");
        assert_eq!(encode(&glb, &mm()), encode(&glb, &inch));
    }

    #[test]
    fn relative_or_unnormalized_paths_have_no_key() {
        assert_eq!(encode(Path::new("part.glb"), &mm()), None);
        let dir = std::path::absolute(std::env::temp_dir()).unwrap();
        assert_eq!(encode(&dir.join("..").join("part.glb"), &mm()), None);
        assert_eq!(decode(Path::new("zz/r/a.glb")), None);
        assert_eq!(decode(Path::new("-/x/a.glb")), None);
    }
}
