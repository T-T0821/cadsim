# インストールと起動

## ダウンロード版(Windows / Mac)

> **準備中**: ビルド済みアプリの自動配布は現在作っているところです。公開されたらこの節の手順で使えます。それまでは下の「ソースから実行」を使ってください。

1. [Releases](https://github.com/T-T0821/cadsim/releases) を開き、最新版の「Assets」から自分の OS 用のファイルをダウンロードします。

   | OS | ファイル |
   | --- | --- |
   | Windows 10 / 11 (64bit) | `cadsim-<版>-windows-x86_64.zip` |
   | macOS 11 以降 (Apple Silicon / Intel) | `cadsim-<版>-macos-universal.zip` |

2. zip を展開して起動します。
   - **Windows**: 中の `.exe` をダブルクリック。
   - **Mac**: `.app` を「アプリケーション」フォルダへ移してから開く。
3. 初回は署名がないため警告が出ます。
   - **Windows**: 「Windows によって PC が保護されました」→「詳細情報」→「実行」。
   - **Mac**: 「開発元を検証できない」→ アプリを右クリック →「開く」→「開く」。

## ソースから実行

[Rust](https://rustup.rs/)(1.97.1 以上)を入れてから、次を実行します。

```sh
git clone https://github.com/T-T0821/cadsim.git
cd cadsim
cargo run --release                      # サンプルのアセンブリを開いて起動
cargo run --release -- path/to/part.step # 指定した STEP を開いて起動
cargo run --release -- path/to/line.cadsim # 保存したプロジェクトを開いて起動
```

初回はビルドに数分かかります。Linux では先に `libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev` を入れてください。

起動すると、サンプルのアセンブリ(万力風の小さな組立品)が開いた画面になります。
