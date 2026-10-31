# リリースの作り方

開発者向けの資料です。使い方は [README](../README.md) を見てください。

Windows / Mac 向けの配布パッケージは、GitHub Actions のワークフロー [`.github/workflows/release.yml`](../.github/workflows/release.yml) で作ります。
Release は**人が手動で実行したときだけ**作られます。PR を main にマージしただけでは作られません。

## 手順

1. GitHub の **Actions** タブ →「Release」→「Run workflow」を押し、ブランチに `main` を選んで実行します。
2. Windows 版と macOS 版がビルドされます(10〜20分ほど)。
3. タグ `v<major>.<minor>.<番号>` を付けた Release が作られ、2つのファイルが添付されます。リリースノートは前回のリリース以降にマージされた PR から自動で作られます。

## できるファイル

| OS | ファイル | 中身 |
| --- | --- | --- |
| Windows (x86_64) | `cadsim-v<版>-windows-x86_64.zip` | `cad_proto.exe` と README |
| macOS (Apple Silicon + Intel) | `cadsim-v<版>-macos-universal.zip` | `CAD Prototype.app`。arm64 と x86_64 を `lipo` で1つにまとめ、アドホック署名だけ付けている |

Windows のリリースビルドでは、起動時に黒いコンソール窓を出さないようにしています(`src/main.rs` の `windows_subsystem`)。
Mac の `.app` に入れる `Info.plist` のひな形は [`packaging/macos/Info.plist`](../packaging/macos/Info.plist) です。

## 版番号

- `<major>.<minor>` は `Cargo.toml` の `version` から取ります(`0.1.0` なら `v0.1.○`)。
- `<番号>` はこのワークフローの実行番号です。実行するたびに増えますが、PR でのビルドでも増えるので連番にはなりません。
- `v0.2.○` に上げたいときは、`Cargo.toml` の `version` を `0.2.0` にしてマージしてから実行してください。

## Release を作らずにビルドだけ試す

- 「Run workflow」で「Release を作成する」のチェックを外すか、`main` 以外のブランチで実行すると、ビルドだけ行います。結果はその実行の **Artifacts** からダウンロードできます。
- このワークフロー・`Cargo.toml`・`Cargo.lock`・`packaging/` を変える PR でも、同じようにビルドだけ行います。
