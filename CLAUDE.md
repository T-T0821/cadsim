# Claude 向けのルール

## プルリクエストとマージ

- `main` へは直接 push しない。変更は必ずブランチを切って PR にする。
- 自分が作った PR は、CI（rustfmt / clippy + test / build）がすべて通ったら squash マージしてよい。
- CI が赤いままマージしない。他の人の PR はマージしない。
- マージ方式は squash のみ。
