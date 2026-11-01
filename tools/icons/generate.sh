#!/usr/bin/env bash
# assets/icon/ の SVG から、アプリが使う PNG と Windows / Mac のアイコンを作り直す。
# ロゴの形や色を変えたら実行してコミットする。
#
# 要るもの: Node.js と Playwright (Chromium で SVG を描く)、Python と Pillow (.ico と .icns を作る)
#   tools/icons/generate.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

dir=assets/icon
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# SVG を指定の大きさの PNG にする ([入力, 出力, 一辺の px] の並び)
jobs="["
for n in 16 24 32 48 64 128 256 512 1024; do
  jobs="$jobs[\"$dir/plantwright-icon.svg\",\"$tmp/icon-$n.png\",$n],"
done
jobs="$jobs[\"$dir/plantwright-mark.svg\",\"$dir/plantwright-mark-128.png\",128]]"

node - "$jobs" <<'JS'
const { chromium } = require('playwright');
const fs = require('fs');
(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  for (const [src, out, n] of JSON.parse(process.argv[2])) {
    const svg = Buffer.from(fs.readFileSync(src)).toString('base64');
    await page.setViewportSize({ width: n, height: n });
    await page.setContent(`<body style="margin:0"><img style="width:${n}px;height:${n}px;display:block" src="data:image/svg+xml;base64,${svg}"></body>`);
    await page.screenshot({ path: out, omitBackground: true, clip: { x: 0, y: 0, width: n, height: n } });
  }
  await browser.close();
})();
JS

cp "$tmp/icon-256.png" "$dir/plantwright-icon-256.png"

python3 - "$tmp" "$dir" <<'PY'
import sys
from PIL import Image
tmp, out = sys.argv[1], sys.argv[2]
icons = {n: Image.open(f"{tmp}/icon-{n}.png").convert("RGBA") for n in (16, 24, 32, 48, 64, 128, 256, 512, 1024)}
win = (16, 24, 32, 48, 64, 128)
icons[256].save(f"{out}/plantwright.ico", sizes=[(n, n) for n in win + (256,)], append_images=[icons[n] for n in win])
icons[1024].save(f"{out}/plantwright.icns", append_images=[icons[n] for n in (16, 32, 64, 128, 256, 512)])
PY

echo "$dir のアイコンを作り直しました"
