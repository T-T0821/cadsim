#!/usr/bin/env bash
# ブラウザ版のデモ (機能限定版の plantwright を WebAssembly にしたもの) を作る。
#
#   tools/web_demo.sh [出力先]     既定の出力先は target/web_demo
#
# 要るもの (しくみは docs/web-demo.md):
#   - rustup target add wasm32-unknown-unknown
#   - wasi-sdk (形状カーネルの C++ をビルドする clang と C/C++ の標準ライブラリ)。
#     展開した場所を WASI_SDK_PATH に入れる
#   - wasm-bindgen (Cargo.lock の wasm-bindgen と同じ版)。PATH に置くか WASM_BINDGEN で場所を渡す
#
# 出来た出力先を http で配れば動く (例: python3 -m http.server -d target/web_demo)。
set -euo pipefail

cd "$(dirname "$0")/.."
OUT="${1:-target/web_demo}"
WASM_BINDGEN="${WASM_BINDGEN:-wasm-bindgen}"

if [[ -z "${WASI_SDK_PATH:-}" ]]; then
  echo "WASI_SDK_PATH に wasi-sdk を展開した場所を入れてください" >&2
  exit 1
fi

# getrandom (乱数) をブラウザの crypto.getRandomValues から取る
export RUSTFLAGS="${RUSTFLAGS:-} --cfg getrandom_backend=\"wasm_js\""
cargo build --locked --profile web --target wasm32-unknown-unknown

rm -rf "$OUT"
mkdir -p "$OUT"
"$WASM_BINDGEN" --target web --no-typescript --out-dir "$OUT" \
  target/wasm32-unknown-unknown/web/plantwright.wasm
cp assets/web_demo/index.html assets/web_demo/wasi_shim.js "$OUT/"
cp assets/icon/plantwright-icon-256.png "$OUT/icon.png"
ls -la "$OUT"
