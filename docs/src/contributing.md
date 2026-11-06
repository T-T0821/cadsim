# このマニュアルの更新方法

このマニュアルは [mdBook](https://rust-lang.github.io/mdBook/) で書いています。原稿は `docs/src/` の Markdown で、GitHub 上でもそのまま読めます。

## 機能を追加したとき

1. 該当する章を書き換えます。「開発中の機能」の章が完成したら、`docs/src/upcoming/` から `docs/src/` に移し(`git mv`)、`docs/src/SUMMARY.md` の目次を「基本操作」側に動かします。新しい機能なら章を作って目次に足します。
2. 画面の画像を `docs/src/images/` に置き、`![説明](images/ファイル名.png)` で貼ります。画像は PNG で、画面全体なら 1440×900 程度で撮ってください。README と同じ写真を使うときは、`docs/screenshots/scenarios/` の手順書で撮ったものを `tools/copy_manual_images.sh` で写します(写す一覧はこのスクリプトの中にあります)。
3. [はじめに](introduction.md) の「できること」の表の状況を更新します。
4. キー操作を足したときは [ショートカット一覧](shortcuts.md) にも追加します。

## 手元で見る

```sh
cargo install mdbook
mdbook serve docs --open
```
