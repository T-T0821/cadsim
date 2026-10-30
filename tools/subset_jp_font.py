"""Noto Sans JP から、UI 表示に必要な文字だけを残した軽いフォントを作る。

元のフォント (約 4.5 MB) をそのまま実行ファイルに埋め込むと重くなるので、
英数字・記号・かな・JIS 第1/第2水準の漢字 (約 7,000 字) に絞る。

使い方:
    pip install fonttools
    curl -LO https://raw.githubusercontent.com/notofonts/noto-cjk/main/Sans/SubsetOTF/JP/NotoSansJP-Regular.otf
    python tools/subset_jp_font.py NotoSansJP-Regular.otf assets/fonts/NotoSansJP-Subset.otf
"""

import sys

from fontTools import subset


def jis_x0208_chars():
    """JIS X 0208 (第1・第2水準漢字と記号) に含まれる文字をすべて返す。"""
    chars = set()
    for row in range(0xA1, 0xFF):
        for cell in range(0xA1, 0xFF):
            try:
                chars.add(bytes([row, cell]).decode("euc_jp"))
            except UnicodeDecodeError:
                pass
    return chars


def unicode_ranges():
    """漢字以外で残す文字の範囲。"""
    ranges = [
        (0x0020, 0x007E),  # ASCII
        (0x00A0, 0x00FF),  # ラテン1補助 (° や × など)
        (0x2000, 0x206F),  # 一般句読点
        (0x2190, 0x21FF),  # 矢印
        (0x2200, 0x22FF),  # 数学記号
        (0x2460, 0x24FF),  # 丸数字
        (0x2500, 0x257F),  # 罫線
        (0x25A0, 0x25FF),  # 図形 (▲ ● など)
        (0x3000, 0x303F),  # CJK の記号と句読点
        (0x3040, 0x309F),  # ひらがな
        (0x30A0, 0x30FF),  # カタカナ
        (0xFF00, 0xFFEF),  # 全角英数・半角カナ
    ]
    return {chr(c) for start, end in ranges for c in range(start, end + 1)}


def main():
    src, dst = sys.argv[1], sys.argv[2]
    text = "".join(sorted(jis_x0208_chars() | unicode_ranges()))
    options = subset.Options()
    options.layout_features = ["*"]
    options.name_IDs = ["*"]
    options.notdef_outline = True
    font = subset.load_font(src, options)
    subsetter = subset.Subsetter(options)
    subsetter.populate(text=text)
    subsetter.subset(font)
    subset.save_font(font, dst, options)


if __name__ == "__main__":
    main()
