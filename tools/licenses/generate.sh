#!/usr/bin/env bash
# 配布物とアプリ (ヘルプ → ライセンス) に入れる第三者ライセンス一覧を作り直す。
# 依存を変えたら (Cargo.lock が変わったら) 実行してコミットする。
#
#   cargo install --locked cargo-about --features cli
#   tools/licenses/generate.sh          # 作り直す
#   tools/licenses/generate.sh --check  # 今のファイルが最新かを確かめる (CI 用)
set -euo pipefail
cd "$(dirname "$0")/../.."

out=assets/licenses/THIRD_PARTY_LICENSES.txt
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

cargo about generate --offline --fail tools/licenses/about.hbs -o "$tmp"

append() { # 見出し, ファイル
  {
    printf '\n================================================================\n%s\n----------------------------------------------------------------\n\n' "$1"
    cat "$2"
  } >> "$tmp"
}
append "同梱フォント Noto Sans JP (assets/fonts/NotoSansJP-Subset.otf) - SIL Open Font License 1.1" assets/fonts/OFL.txt
append "サンプル形状 UR5e (samples/real_robot/ur5e.glb, ur5e_urdf) - BSD 3-Clause" samples/real_robot/LICENSE-UR5e.txt
append "サンプル形状 Robotiq 2F-85 (samples/real_robot/robotiq_2f85.glb) - BSD 3-Clause" samples/real_robot/LICENSE-Robotiq-2F-85.txt

if [ "${1:-}" = "--check" ]; then
  if ! cmp -s "$tmp" "$out"; then
    echo "$out が古くなっています。tools/licenses/generate.sh を実行してコミットしてください。" >&2
    diff -u "$out" "$tmp" | head -40 >&2 || true
    exit 1
  fi
  echo "$out は最新です"
else
  cp "$tmp" "$out"
  chmod 644 "$out"
  echo "$out を書き出しました"
fi
