#!/usr/bin/env bash
# 画面での動作確認 (UI チェック) を流す。引数はそのまま --ui-check に渡す。
#   tools/ui_check.sh                 # tests/ui の手順書をすべて
#   tools/ui_check.sh --only chart    # 名前に chart を含むものだけ
#   tools/ui_check.sh --update        # 基準画像を置き換える
# 画面 (DISPLAY / WAYLAND_DISPLAY) がなければ仮想画面 (Xvfb) で動かす。
# Linux の基準画像はソフトウェア描画 (Mesa の lavapipe) で撮っているので、あればそれを使う。
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --locked
app="${CARGO_TARGET_DIR:-target}/debug/plantwright"

lvp=/usr/share/vulkan/icd.d/lvp_icd.json
if [[ "$(uname)" == Linux && -f "$lvp" && -z "${PLANTWRIGHT_UI_CHECK_GPU:-}" ]]; then
    export VK_ICD_FILENAMES="$lvp"
fi

if [[ "$(uname)" == Linux && -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
    exec xvfb-run -a -s "-screen 0 1600x1000x24" "$app" --ui-check "$@"
else
    exec "$app" --ui-check "$@"
fi
