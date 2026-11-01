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

- [docs/architecture.md](docs/architecture.md) に従う。1つの機能は、同じ名前のフォルダ2つにまとめる。
  - `crates/plantwright_core/src/<機能>/` … Bevy・egui を使わないもの全部(データの型・計算・ファイル形式・通信)。単体テストもここ。
  - `src/features/<機能>/` … Bevy のプラグイン・システム・3D 表示と egui の画面。
- crate を分ける線は「Bevy・egui を使うかどうか」の1本だけ。層 (ドメイン・ユースケース・アダプター) では分けない。
- 判断や計算は `plantwright_core` に書いて単体テストを付ける。Bevy のシステムと egui の画面は、状態を集めて core を呼び、結果を画面に写すだけにする。
- 再公開 (`pub use`) だけのモジュールや、`Resource` を付けるためだけの包み型は作らない。アプリからは `plantwright_core::<機能>::...` を直接 `use` し、`Resource` は core の型に直接付ける。
- トレイトは、実装を本当に入れ替えるもの(PLC との通信、生成 AI、物理エンジン、テストで偽物を差すもの)だけに使う。実装が1つなら普通の関数・メソッドにする。
- 機能フォルダの中のファイル名をそろえる。
  - core 側: `mod.rs`(説明と `pub use`)、`model.rs`(データの型)、中身を表す名前(`sim.rs` など)、`record.rs`(プロジェクトファイルの JSON の形)、形式の名前(`xlsx.rs`・`slmp.rs` など)。
  - アプリ側: `mod.rs`(`XxxPlugin` と Resource・Event の登録)、`systems.rs`(毎フレームのシステム)、`view.rs`(3D に置くもの)、`panel.rs` または `panel/`(egui のウィンドウ)。
  - `ui.rs`・`*_ui.rs`・`usecase.rs`・`domain/`・`adapter/` という名前は使わない。
- 1ファイルはおよそ 800 行まで。超えたら画面の区切りや計算の段階で分ける。
- 構成を変えたら docs/architecture.md も更新する。

## README の書き方

- README は使う人向け。「何ができて、どう操作するか」を中心に書く。
- 新しい機能は README の「できること」に、画面のどこで何をすればよいかを短く書く。キー操作が増えたら操作の表にも足す。スクリーンショットがあれば載せる。
- 使っているライブラリ、アルゴリズム、データ構造、ファイル形式の中身などの技術的な説明は README に書かず、docs/ 以下(docs/internals.md や機能ごとの設計文書)に書いて README の「開発する人へ」からリンクする。
- 予定の機能が入ったら「開発中・予定」から外す。
