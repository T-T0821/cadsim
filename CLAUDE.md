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

## コードの構成

- [docs/architecture.md](docs/architecture.md) の Clean Architecture に従う。依存は外側(Bevy・egui の UI)から内側(ユースケース → ドメイン)へだけ向ける。
- ドメインとユースケースには Bevy・egui・ファイル形式のライブラリを持ち込まない。Bevy の `Resource` / `Component` は UI 層の包み型に付ける。
- 判断や計算は内側の層に書いて単体テストを付け、Bevy のシステムと egui の画面は「入力を集める → ユースケースを呼ぶ → 結果を表示に写す」だけにする。
- 移行の途中で今のコードが目標の構成と違う場合も、新しく書くコードは目標の層に置く(層の場所がまだなければ、Bevy に依存しないモジュールとして分ける)。
- 構成を変えたら docs/architecture.md も更新する。

## README の書き方

- README は使う人向け。「何ができて、どう操作するか」を中心に書く。
- 新しい機能は README の「できること」に、画面のどこで何をすればよいかを短く書く。キー操作が増えたら操作の表にも足す。スクリーンショットがあれば載せる。
- 使っているライブラリ、アルゴリズム、データ構造、ファイル形式の中身などの技術的な説明は README に書かず、docs/ 以下(docs/internals.md や機能ごとの設計文書)に書いて README の「開発する人へ」からリンクする。
- 予定の機能が入ったら「開発中・予定」から外す。
