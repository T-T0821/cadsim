# Claude 向けのルール

## プルリクエストとマージ

- `main` へは直接 push しない。変更は必ずブランチを切って PR にする。
- PR は自分でマージしない（`merge_pull_request` も自動マージの有効化もしない）。マージは T-T0821 が行う。
- CI（rustfmt / clippy + test / build）が通ってから T-T0821 にレビューを依頼する。
- マージ方式は squash のみ。
