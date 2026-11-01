//! 三菱電機の PLC (MELSEC) のデバイス
//!
//! PLC のメモリは「デバイス」(X・Y・M・D など) の名前と番号で指す。
//! X・Y・B・W は 16 進数、それ以外は 10 進数で番号を書く (X1F、M100、D200 など)。
//! 通信の形式 (SLMP のデバイスコード) は`crate::plc::slmp` が受け持つ。

use std::fmt;

/// デバイスの種類
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Device {
    /// 入力
    X,
    /// 出力
    Y,
    /// 内部リレー
    M,
    /// ラッチリレー
    L,
    /// リンクリレー
    B,
    /// データレジスタ
    D,
    /// リンクレジスタ
    W,
    /// ファイルレジスタ
    R,
}

impl Device {
    pub const ALL: [Device; 8] = [
        Device::X,
        Device::Y,
        Device::M,
        Device::L,
        Device::B,
        Device::D,
        Device::W,
        Device::R,
    ];

    /// ビットデバイス (ON / OFF) の一覧
    pub const BITS: [Device; 5] = [Device::X, Device::Y, Device::M, Device::L, Device::B];

    /// ワードデバイス (16 ビットの数値) の一覧
    pub const WORDS: [Device; 3] = [Device::D, Device::W, Device::R];

    /// "X" "D" などの名前
    pub fn name(self) -> &'static str {
        match self {
            Device::X => "X",
            Device::Y => "Y",
            Device::M => "M",
            Device::L => "L",
            Device::B => "B",
            Device::D => "D",
            Device::W => "W",
            Device::R => "R",
        }
    }

    /// 日本語の呼び名
    pub fn description(self) -> &'static str {
        match self {
            Device::X => "入力",
            Device::Y => "出力",
            Device::M => "内部リレー",
            Device::L => "ラッチリレー",
            Device::B => "リンクリレー",
            Device::D => "データレジスタ",
            Device::W => "リンクレジスタ",
            Device::R => "ファイルレジスタ",
        }
    }

    /// ビットデバイスなら true。ワードデバイスなら false。
    pub fn is_bit(self) -> bool {
        Self::BITS.contains(&self)
    }

    /// 番号を 16 進数で書くデバイスなら true
    pub fn is_hex(self) -> bool {
        matches!(self, Device::X | Device::Y | Device::B | Device::W)
    }

    /// 名前から探す (大文字・小文字は問わない)
    pub fn from_name(name: &str) -> Option<Device> {
        Self::ALL
            .into_iter()
            .find(|d| d.name().eq_ignore_ascii_case(name.trim()))
    }
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// デバイスの1点 ("M100" "X1F" など)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DeviceRef {
    pub device: Device,
    pub number: u32,
}

impl DeviceRef {
    pub fn new(device: Device, number: u32) -> Self {
        Self { device, number }
    }

    /// "M100" "X1F" のような文字列を読む
    pub fn parse(text: &str) -> Option<DeviceRef> {
        let text = text.trim();
        // 16 進数の番号は A〜F で始まることがある (XA など) ので、名前は先頭1文字で決める
        if !text.is_char_boundary(1) || text.len() < 2 {
            return None;
        }
        let (name, digits) = text.split_at(1);
        let device = Device::from_name(name)?;
        let radix = if device.is_hex() { 16 } else { 10 };
        let number = u32::from_str_radix(digits, radix).ok()?;
        Some(DeviceRef { device, number })
    }
}

impl fmt::Display for DeviceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.device.is_hex() {
            write!(f, "{}{:X}", self.device, self.number)
        } else {
            write!(f, "{}{}", self.device, self.number)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_devices_are_written_in_hex() {
        assert_eq!(DeviceRef::new(Device::X, 31).to_string(), "X1F");
        assert_eq!(DeviceRef::new(Device::W, 256).to_string(), "W100");
        assert_eq!(DeviceRef::new(Device::M, 100).to_string(), "M100");
        assert_eq!(DeviceRef::new(Device::D, 0).to_string(), "D0");
    }

    #[test]
    fn parse_reads_what_display_writes() {
        for text in ["X1F", "YA", "M100", "D0", "B3FF", "R12"] {
            assert_eq!(DeviceRef::parse(text).unwrap().to_string(), text);
        }
        assert_eq!(DeviceRef::parse("xa"), Some(DeviceRef::new(Device::X, 10)));
        assert_eq!(DeviceRef::parse("D1A"), None);
        assert_eq!(DeviceRef::parse("Q10"), None);
        assert_eq!(DeviceRef::parse("100"), None);
    }

    #[test]
    fn bit_and_word_devices_are_separate() {
        assert!(Device::BITS.iter().all(|d| d.is_bit()));
        assert!(Device::WORDS.iter().all(|d| !d.is_bit()));
        assert_eq!(Device::BITS.len() + Device::WORDS.len(), Device::ALL.len());
    }
}
