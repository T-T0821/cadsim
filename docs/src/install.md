# インストールと起動

## ダウンロード版(Windows / Mac)

インストールは要りません。ダウンロードしたものを開くだけで起動し、フォントやサンプルも中に入っています。

1. [Releases](https://github.com/T-T0821/cadsim/releases) を開き、最新版の「Assets」から自分の OS 用のファイルをダウンロードします。

   | OS | ファイル | 起動のしかた |
   | --- | --- | --- |
   | Windows 10 / 11 (64bit) | `cadsim-<版>-windows-x86_64.exe` | ダウンロードした exe をそのままダブルクリック。好きな場所に置いて使えます |
   | 〃(zip 版) | `cadsim-<版>-windows-x86_64.zip` | 展開して `cad_proto.exe` をダブルクリック。中身は上の exe と README です |
   | macOS 11 以降 (Apple Silicon / Intel) | `cadsim-<版>-macos-universal.zip` | ダブルクリックで展開して `CAD Prototype.app` を開く。「アプリケーション」に移すと、ほかのアプリと同じように使えます |

2. 初回は署名がないため警告が出ます。
   - **Windows**: 「Windows によって PC が保護されました」→「詳細情報」→「実行」。
   - **Mac**: 「開発元を検証できない」「開けません」→ アプリを右クリック →「開く」→「開く」。macOS 15 以降でこの方法が出ないときは、一度開こうとしたあと「システム設定」→「プライバシーとセキュリティ」の下のほうにある「このまま開く」を押します。それでも開けないときは、ターミナルで `xattr -dr com.apple.quarantine "/Applications/CAD Prototype.app"`(置いた場所に合わせて変えてください)を実行してから開いてください。

やめるときは、ダウンロードしたファイル(Mac は `.app`)を削除するだけです。見た目などの設定は小さなファイルとして残ります。

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

Rust を入れずに Docker だけでビルドすることもできます。`make docker-build` で開発コンテナの中でビルドし、`make help` でほかの目標が見られます(詳しくは `docs/dev-environment.md`)。

起動すると、サンプルのアセンブリ(万力風の小さな組立品)が開いた画面になります。
