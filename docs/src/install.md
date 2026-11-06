# インストールと起動

## ダウンロード版(Windows / Mac)

インストールは要りません。ダウンロードしたものを開くだけで起動し、フォントやサンプルも中に入っています。

1. [Releases](https://github.com/T-T0821/cadsim/releases) を開き、最新版の「Assets」から自分の OS 用のファイルをダウンロードします。

   | OS | ファイル | 起動のしかた |
   | --- | --- | --- |
   | Windows 10 / 11 (64bit) | `plantwright-<版>-windows-x86_64.exe` | ダウンロードした exe をそのままダブルクリック。好きな場所に置いて使えます |
   | 〃(zip 版) | `plantwright-<版>-windows-x86_64.zip` | 展開して `plantwright.exe` をダブルクリック。中身は上の exe と README です |
   | macOS 11 以降 (Apple Silicon / Intel) | `plantwright-<版>-macos-universal.zip` | ダブルクリックで展開して `plantwright.app` を開く。「アプリケーション」に移すと、ほかのアプリと同じように使えます |

2. 初回は署名がないため警告が出ます。
   - **Windows**: 「Windows によって PC が保護されました」→「詳細情報」→「実行」。
   - **Mac**: 「開発元を検証できない」「開けません」→ アプリを右クリック →「開く」→「開く」。macOS 15 以降でこの方法が出ないときは、一度開こうとしたあと「システム設定」→「プライバシーとセキュリティ」の下のほうにある「このまま開く」を押します。それでも開けないときは、ターミナルで `xattr -dr com.apple.quarantine "/Applications/plantwright.app"`(置いた場所に合わせて変えてください)を実行してから開いてください。

やめるときは、ダウンロードしたファイル(Mac は `.app`)を削除するだけです。見た目などの設定は小さなファイルとして残ります。

以前の名前 (cadsim) の版を使っていた場合も、`.cadsim` のプロジェクトファイルはそのまま開けます。ミッションの進み具合・AI のキー・自作部品などの設定も、初めて起動したときに自動で引き継ぎます。

## ソースから実行

[Rust](https://rustup.rs/)(1.97.1 以上)を入れてから、次を実行します。

```sh
git clone https://github.com/T-T0821/cadsim.git
cd cadsim
cargo run --release                      # デモラインを開き、動かした状態で起動
cargo run --release -- path/to/part.step # 指定した STEP を開いて起動
cargo run --release -- path/to/line.plantwright # 保存したプロジェクトを開いて起動
```

初回はビルドに数分かかります。形状カーネルなどの C++ のソースも一緒にビルドするので、C++ のコンパイラ(Windows は Visual Studio の C++ ビルドツール、Mac は Xcode のコマンドラインツール、Linux は g++)が要ります。Linux では先に `g++ libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev` を入れてください。

ほかに、`--stress 5000`(部品 5000 個ほどの負荷テスト用シーン)、`--physics`(物理演算のサンプルライン)、`--mcp`([Claude に画面を操作させる](mcp.md))を付けて起動できます。

Rust を入れずに Docker だけでビルドすることもできます。`make docker-build` で開発コンテナの中でビルドし、`make help` でほかの目標が見られます(詳しくは `docs/dev-environment.md`)。

起動すると、ロボットセル・コンベア・安全柵・AGV・作業者を並べたデモラインが動いた状態で開き、はじめての起動ではようこそ画面が出ます。続きは [ミッション](missions.md) を見てください。
