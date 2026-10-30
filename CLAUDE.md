# Claude 向けのルール

## プルリクエストとマージ

- `main` へは直接 push しない。変更は必ずブランチを切って PR にする。
- マージ前に手元で次を実行し、すべて通ることを確認する。
  - `cargo fmt --all --check`
  - `cargo clippy --all-targets --locked -- -D warnings`
  - `cargo test --locked`
- 上が通ったら、自分が作った PR は CI を待たずに squash マージしてよい。
- 他の人の PR はマージしない。
- マージ方式は squash のみ。
