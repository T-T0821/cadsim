# リリースの作り方

開発者向けの資料です。使い方は [README](../README.md) を見てください。

Windows / Mac 向けの配布パッケージは、GitHub Actions のワークフロー [`.github/workflows/release.yml`](../.github/workflows/release.yml) で作ります。
Release は**人が手動で実行したときだけ**作られます。PR を main にマージしただけでは作られません。

## 手順

1. GitHub の **Actions** タブ →「Release」→「Run workflow」を押し、ブランチに `main` を選んで実行します。
2. Windows 版と macOS 版がビルドされます(10〜20分ほど)。
3. タグ `v<major>.<minor>.<番号>` を付けた Release が作られ、3つのファイルが添付されます。リリースノートは前回のリリース以降にマージされた PR から自動で作られます。

## できるファイル

| OS | ファイル | 中身 |
| --- | --- | --- |
| Windows (x86_64) | `plantwright-v<版>-windows-x86_64.exe` | 単体の実行ファイル。これだけで起動する |
| Windows (x86_64) | `plantwright-v<版>-windows-x86_64.zip` | 上と同じ exe(`plantwright.exe`)、README、ライセンス文(`LICENSE-MIT`・`LICENSE-APACHE`・`THIRD_PARTY_LICENSES.txt`) |
| macOS (Apple Silicon + Intel) | `plantwright-v<版>-macos-universal.zip` | `plantwright.app`。arm64 と x86_64 を `lipo` で1つにまとめ、アドホック署名だけ付けている。ライセンス文は `Contents/Resources` に入れる |

## インストール不要にするための工夫

- フォント・サンプルの STEP・Web ビューアのひな形などの素材は `include_bytes!` / `include_str!` で実行ファイルに埋め込んでいるので、別ファイルを同梱しなくてよい。素材を増やすときも実行時にファイルを読まず、同じように埋め込むこと。
- Windows では C ランタイムを静的リンクしている([`.cargo/config.toml`](../.cargo/config.toml) の `+crt-static`)。VC++ 再頒布可能パッケージ(`VCRUNTIME140.dll` など)が入っていない PC でも exe だけで動く。
- xz (liblzma) は同じ設定ファイルの `LZMA_API_STATIC` でソースからビルドして静的リンクしている。これがないと Mac の CI で Homebrew の `liblzma.dylib` に依存してしまい、Homebrew のない Mac で起動できなくなる。
- ワークフローで依存ライブラリを確かめている。Windows は `dumpbin /dependents` で C ランタイムの DLL に依存していないこと、Mac は `otool -L` で `/System/Library/` と `/usr/lib/` 以外の dylib に依存していないことを見て、違えば失敗させる。
- Mac は `.app` 1つにまとまっているので、どこに置いても動く。
- 第三者ライセンスの一覧(`assets/licenses/THIRD_PARTY_LICENSES.txt`)も実行ファイルに埋め込み、ヘルプ → ライセンスで見せている。依存を変えたら作り直す([licenses.md](licenses.md))。
- 見た目などの設定だけは OS の設定フォルダ(`src/preferences_file.rs`)に保存する。なくても標準の設定で起動する。

Windows のリリースビルドでは、起動時に黒いコンソール窓を出さないようにしています(`src/main.rs` の `windows_subsystem`)。
Mac の `.app` に入れる `Info.plist` のひな形は [`packaging/macos/Info.plist`](../packaging/macos/Info.plist) です。

## 版番号

- `<major>.<minor>` は `Cargo.toml` の `version` から取ります(`0.1.0` なら `v0.1.○`)。
- `<番号>` はこのワークフローの実行番号です。実行するたびに増えますが、PR でのビルドでも増えるので連番にはなりません。
- `v0.2.○` に上げたいときは、`Cargo.toml` の `version` を `0.2.0` にしてマージしてから実行してください。

## Release を作らずにビルドだけ試す

- 「Run workflow」で「Release を作成する」のチェックを外すか、`main` 以外のブランチで実行すると、ビルドだけ行います。結果はその実行の **Artifacts** からダウンロードできます。
- このワークフロー・`Cargo.toml`・`Cargo.lock`・`packaging/`・`.cargo/` を変える PR でも、同じようにビルドだけ行います。

## 手元で配布物を作る

`make docker-package-windows` で、開発コンテナの中で MinGW を使って Windows 向けの `.exe` と `.zip` を `dist/` に作れます(名前と中身は Release と同じ)。MSVC ではないので正式な配布には Release ワークフローを使ってください。詳しくは [dev-environment.md](dev-environment.md)。
