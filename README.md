# CAD最小プロトタイプ(箱を作る・選ぶ・回す)

Bevy + bevy_egui + truck で作った最小構成のCADビューアです。
形状の「正」は truck の B-rep(f64)が持ち、Bevy は表示用の f32 メッシュと操作だけを担当します。

## 動作確認したバージョン

| crate | バージョン |
| --- | --- |
| Rust | 1.97.1 以上(依存crateの要件) |
| bevy | 0.20 |
| bevy_egui | 0.43 |
| truck-modeling | 0.6 |
| truck-meshalgo | 0.4 |
| transform-gizmo | 0.11 |

## ダウンロード(Windows / Mac)

ビルド済みのアプリを [Releases](https://github.com/T-T0821/cadsim/releases) に置いています。最新版の「Assets」から自分のOS用のファイルを取ってください。

| OS | ファイル | 起動方法 |
| --- | --- | --- |
| Windows 10 / 11 (64bit) | `cadsim-<版>-windows-x86_64.zip` | 展開して `cad_proto.exe` をダブルクリック |
| macOS 11 以降 (Apple Silicon / Intel 両対応) | `cadsim-<版>-macos-universal.zip` | 展開して `CAD Prototype.app` を「アプリケーション」へ移し、開く |

どちらもコード署名(公証)をしていないため、初回は警告が出ます。

- **Windows**: 「WindowsによってPCが保護されました」と出たら「詳細情報」→「実行」。
- **Mac**: 「開発元を検証できない」と出たら、アプリを右クリック →「開く」→「開く」。それでも開けない場合はターミナルで `xattr -dr com.apple.quarantine "/Applications/CAD Prototype.app"` を実行してから開いてください。

## ソースから実行

```sh
cargo run
```

初回は Bevy のビルドに数分かかります。Linux では `libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev` が必要です。

## テストとCI

PR を作ると GitHub Actions(`.github/workflows/ci.yml`)が次を自動で確認します。main への push でも同じものが走ります。

| ジョブ | 内容 |
| --- | --- |
| rustfmt | `cargo fmt --all --check` |
| clippy + test (Linux) | `cargo clippy --all-targets --locked -- -D warnings` と `cargo test --locked` |
| build (Windows / macOS) | `cargo check --all-targets --locked` |

どれかが失敗するとPRに赤い✕が付きます。push 前に手元で同じ確認をするときは次を実行します。

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

依存のビルド結果は `Swatinem/rust-cache` で OS ごとにキャッシュし(共有キー `cadsim`)、main での実行時だけ保存します。`Cargo.lock` と食い違うと `--locked` で失敗するので、依存を変えたら `Cargo.lock` もコミットしてください。

## 操作

Unity / Unreal Engine のシーンビューと同じ割り当てにしています。

| 操作 | 内容 |
| --- | --- |
| 左クリック | 面を選択(面はオレンジ、その箱は青になる) |
| Alt + 左ドラッグ | 注視点のまわりを回転(オービット) |
| 中ドラッグ | 平行移動(パン) |
| ホイール / Alt + 右ドラッグ | ズーム |
| 右ドラッグ | その場で見回す |
| 右ボタンを押しながら W / A / S / D | 前後左右に移動(Shiftで加速) |
| 右ボタンを押しながら Q / E | 下降 / 上昇 |
| F | 選択中の箱にフォーカス |
| Delete | 選択中の箱を削除 |
| W / E / R | 移動 / 回転 / 拡大縮小のギズモに切り替え(パネル上部のボタンでも可)。ギズモの軸や輪を左ドラッグ |
| Ctrl + ギズモを左ドラッグ | スナップ(一定量ずつ動かす)。刻み幅はパネルの「スナップ」欄で設定 |
| 左パネル | 箱の追加、寸法・位置・回転の編集、+15°回転ボタン |

### スナップ

Ctrl を押している間(またはパネルの「常にスナップ」をオンにしている間)、ギズモの操作が一定量ずつになります。

| ギズモ | 刻み(初期値) | 動き方 |
| --- | --- | --- |
| 移動 | Move step(0.1) | ドラッグを始めた位置から 0.1 ずつ動く |
| 回転 | Rotate step(15°) | ドラッグを始めた向きから 15° ずつ回る。輪に目盛りが出る |
| 拡大縮小 | Size step(0.1) | 動かした軸の寸法が 0.1 の倍数になる(触っていない軸の寸法はそのまま) |

移動と回転は Unity と同じく「ドラッグ開始からの変化量」を丸めます。拡大縮小だけは倍率ではなく寸法を丸めるので、寸法をきりのいい値にそろえられます。

## リリースの作り方(開発者向け)

`.github/workflows/release.yml` が GitHub Actions でパッケージを作ります。**PR が main にマージされるたびに自動でリリースされる**ので、手作業はいりません。

1. Actions が Windows 版(`.zip`)と macOS 版(arm64 と x86_64 をまとめたユニバーサルの `.app` を `.zip` に固めたもの)をビルドします。
2. タグ `v<major>.<minor>.<番号>` を付けて Release を作り、2つのファイルを添付します。リリースノートはマージされたPRから自動生成されます。
   - `<major>.<minor>` は `Cargo.toml` の `version` から取ります(`0.1.0` なら `v0.1.○`)。
   - `<番号>` はこのワークフローの実行番号で、マージのたびに増えます。
   - 大きな変更で `v0.2.○` に上げたいときは、`Cargo.toml` の `version` を `0.2.0` にしてマージしてください。

リリースせずに試したいときは、Actions タブの「Release」→「Run workflow」で実行すると、ビルド結果がその実行の Artifacts に置かれます(Release は作られません)。このワークフローや `Cargo.toml` を変える PR でも、同じようにビルドだけ行います。

## しくみ

1. `BoxPart` が寸法・位置・回転を f64 で保持する。
2. 編集されると `dirty` に入り、`rebuild_dirty_parts` が truck で B-rep ソリッドを作り直す(`tsweep` を3回 → `rotated` → `translated`)。
3. ソリッドを面ごとにテッセレーションし、面1枚につき Bevy のメッシュエンティティを1つ作る。各エンティティには `FaceTag { part, face }` を付ける。
4. 左クリックで `MeshRayCast` を撃ち、当たったエンティティの `FaceTag` から部品と面を特定する。
5. truck の稜線(ポリライン化済み)は Gizmos で黒い線として描く。
6. ギズモ(`src/gizmo.rs`)は transform-gizmo で計算し、egui の背景レイヤーに描く。ドラッグ結果は箱の位置・回転・寸法(f64)に書き戻し、2. の流れで形状を作り直す。スナップは移動・回転を transform-gizmo の機能に任せ、拡大縮小の寸法だけ `snap_size` で丸める。

回転はメッシュの Transform ではなく truck 側の形状に適用しているので、寸法計算や今後の干渉チェックは常に f64 の形状で行えます。

## 日本語フォント

egui の標準フォントには日本語がないため、[Noto Sans JP](https://github.com/notofonts/noto-cjk)(SIL Open Font License 1.1)を埋め込んでいます。

- `assets/fonts/NotoSansJP-Subset.otf` … 英数字・記号・かな・JIS 第1/第2水準漢字だけに絞ったもの(約 2.1 MB)。`include_bytes!` で実行ファイルに入るので、配布時にフォントを別に置く必要はありません。
- `assets/fonts/OFL.txt` … フォントのライセンス。配布物にも同梱してください。
- `src/japanese_font.rs` … 起動直後に egui の予備フォントとして登録する。英数字は egui の標準フォント、日本語だけ Noto Sans JP で描かれます。
- `tools/subset_jp_font.py` … 元フォントから絞り込み版を作り直すスクリプト(使い方はファイル先頭)。

## 次に手を入れるところ

- 平行投影への切り替え
- 干渉チェック(truck のソリッド同士、または Avian のコライダー)
