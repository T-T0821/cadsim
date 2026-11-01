# 開発コンテナと Makefile

Rust や Linux のライブラリを手元に入れなくても、Docker さえあればビルド・テスト・画面での動作確認・
Windows 向けのクロスビルドができるように、開発コンテナ ([`.devcontainer/`](../.devcontainer/)) と
[`Makefile`](../Makefile) を用意しています。

## 使い方は 2 通り

### VS Code の Dev Containers で開く

1. Docker と VS Code の拡張「Dev Containers」を入れます。
2. このフォルダを VS Code で開き、コマンドパレットから「Dev Containers: Reopen in Container」を選びます。
3. 初回はコンテナの像を作るので数分かかります。開いたら、VS Code の端末で `make build` や `make test` を実行します。

rust-analyzer などの拡張はコンテナ側に自動で入ります。GitHub Codespaces でも同じ設定で開けます。

### ホストから `make docker-<目標>` で動かす

コンテナに入らずに、ホストの端末から目標の頭に `docker-` を付けて実行すると、コンテナの中で同じ目標を動かします。
像がなければ最初に作ります (2 回目からはキャッシュが効くので待ちません)。

```sh
make docker-build             # コンテナの中で make build
make docker-test              # コンテナの中で make test
make docker-check             # fmt・clippy・test をまとめて
make docker-package-windows   # Windows 向けの exe と zip を dist/ に作る
make docker-ui-check          # 仮想画面で画面での動作確認
make docker-shell             # コンテナの中のシェルに入る
```

## Makefile の目標

`make help` で一覧が出ます。

| 目標 | 内容 |
| --- | --- |
| `make build` | 開発ビルド (`cargo build --locked`) |
| `make release` | リリースビルド |
| `make run ARGS="..."` | 開発ビルドで起動。`ARGS` はアプリへの引数 (例: `ARGS="samples/demo_line.cadsim"`) |
| `make test` | テスト |
| `make fmt` / `make fmt-check` | 書式を整える / 整っているかを確かめる |
| `make clippy` | `cargo clippy --all-targets --locked -- -D warnings` |
| `make check` | PR を出す前の確認 (`fmt-check`・`clippy`・`test`) |
| `make ui-check ARGS="..."` | `tools/ui_check.sh` を流す。画面がなければ Xvfb の仮想画面で |
| `make windows` | `x86_64-pc-windows-gnu` 向けにクロスビルド |
| `make package-linux` | `dist/cadsim-v<版>-linux-<CPU>.tar.gz` (実行ファイルと README) |
| `make package-windows` | `dist/cadsim-v<版>-windows-x86_64.exe` と `.zip` (Release と同じ名前と中身) |
| `make package` | 上の 2 つ |
| `make clean` | `cargo clean` と `dist/` の削除 |
| `make docker-image` | 開発コンテナの像 (`cadsim-dev`) を作る |
| `make docker-shell` | コンテナの中でシェルを開く |
| `make docker-<目標>` | 上の目標をコンテナの中で動かす |

配布物の `<版>` は既定で `Cargo.toml` の `version` です。`make docker-package-windows VERSION=0.1.5` のように変えられます。

## コンテナの中身

[`.devcontainer/Dockerfile`](../.devcontainer/Dockerfile) は Ubuntu 24.04 に次を入れています。

- Rust (rustup で版を固定。`ARG RUST_VERSION`)、rustfmt、clippy、`x86_64-pc-windows-gnu` ターゲット
- C / C++ のビルド道具 (openh264 と lzma-sys をソースからビルドするため)、nasm
- Bevy が Linux で要るライブラリ: ALSA (`libasound2-dev`)、udev、X11 (`libx11-dev` ほか)、Wayland、xkbcommon
- 描画: Vulkan のローダーと Mesa (`mesa-vulkan-drivers`)。GPU がなくても lavapipe (ソフトウェア描画) で描けます
- `cargo ui-check` 用の Xvfb と xauth
- Windows 向けのクロスビルド用の MinGW-w64 と zip

x86_64 と aarch64 (Apple Silicon の Mac の Docker など) のどちらでも像を作れます。

### ビルド結果の置き場所

コンテナの中では `CARGO_TARGET_DIR=/workspace/target/container` にしているので、ビルド結果は
`target/container/` に入ります。ホスト (Windows / Mac) で `cargo build` した結果 (`target/debug` など) と混ざって
互いに作り直しになることはありません。Makefile と `tools/ui_check.sh` はこの変数に合わせて実行ファイルを探します。

### 依存クレートのキャッシュ

取ってきた依存クレートは Docker のボリューム `cadsim-cargo-registry` と `cadsim-cargo-git` に置き、
Dev Containers と `make docker-<目標>` で共有します。コンテナを作り直しても取り直しません。
消したいときは `docker volume rm cadsim-cargo-registry cadsim-cargo-git` です。

### 利用者の uid

`make docker-<目標>` はホストの利用者の uid・gid でコンテナを動かすので、`target/` や `dist/` にできるファイルの持ち主はホストの自分になります。
どの uid でも依存クレートを書き込めるよう、像の中の cargo のホームは誰でも書けるようにしています。
Dev Containers では `dev` ユーザーで動き、uid はホストに合わせて自動で付け替えられます。

別の uid (たとえば root) で一度動かしたあとに `Permission denied` が出たら、キャッシュのボリュームを消して取り直してください
(`docker volume rm cadsim-cargo-registry cadsim-cargo-git`)。`target/container/` の持ち主が違うときはそのフォルダも消します。

## Windows 向けのクロスビルドと GitHub の Release の違い

`make windows` は MinGW (`x86_64-pc-windows-gnu`) でビルドします。GitHub の Release ワークフロー
([docs/release.md](release.md)) は Windows のランナーで MSVC (`x86_64-pc-windows-msvc`) を使っています。
どちらも C ランタイムを静的リンクして exe 1 つで動くようにしています ([`.cargo/config.toml`](../.cargo/config.toml))。
手元で試しに配る分には `make docker-package-windows` で足りますが、正式な配布物は Release ワークフローで作ってください。

Mac 向けはクロスビルドできません (Apple の SDK が要るため)。Mac の上で `make release` するか、Release ワークフローを使ってください。

## プロキシの内側で像を作る

社内プロキシなどで TLS を検査している環境では、証明書を入れた像を土台にできます。

```sh
export DOCKER_BUILD_ARGS="--build-arg BASE_IMAGE=my-ubuntu-with-ca:24.04 --network host"
export DOCKER_RUN_ARGS="--network host -e HTTPS_PROXY=http://proxy:8080"
make docker-build
```

`DOCKER_BUILD_ARGS` は `docker build` に、`DOCKER_RUN_ARGS` は `docker run` にそのまま足されます。
`make docker-<目標>` は毎回 `docker build` も流す (変わっていなければキャッシュで一瞬) ので、
`export` しておくとどの目標でも同じ像が使われます。

## 画面のあるアプリをコンテナから開く

コンテナには画面がないので、`make ui-check` は Xvfb の仮想画面で動きます。アプリを実際の窓で触りたいときは、
ホストで `cargo run` するのが手軽です。Linux のホストなら、X11 を渡して `make docker-run` もできます。

```sh
xhost +local:
make docker-run DOCKER_RUN_ARGS="-e DISPLAY=$DISPLAY -v /tmp/.X11-unix:/tmp/.X11-unix --device /dev/dri"
```
