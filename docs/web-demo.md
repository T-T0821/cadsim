# ブラウザ版のデモ (WebAssembly)

<https://t-t0821.github.io/cadsim/> のトップでは、デスクトップ版と同じアプリ (plantwright) を WebAssembly にしたものが動く。起動すると本物のロボット (UR5e + Robotiq 2F-85) のピック & プレースを開いて再生する。ブラウザでは使えない機能を外した「機能限定版」で、外したものはページの上に一覧で出している。

サンプル装置を .html 1 つに書き出した軽いビューア ([web-viewer.md](web-viewer.md)) は `/viewer/` に移した。

## 作り方

```sh
rustup target add wasm32-unknown-unknown
# wasi-sdk を展開して場所を渡す (https://github.com/WebAssembly/wasi-sdk/releases の wasi-sdk-25)
export WASI_SDK_PATH=/path/to/wasi-sdk-25.0-x86_64-linux
# Cargo.lock の wasm-bindgen と同じ版の wasm-bindgen CLI を PATH に置く (または WASM_BINDGEN で場所を渡す)
tools/web_demo.sh                    # target/web_demo にできる
python3 -m http.server -d target/web_demo 8000   # http://localhost:8000/ で開く
```

- `cargo build --profile web --target wasm32-unknown-unknown` でビルドする。`web` プロファイル (Cargo.toml) は release を元に大きさ優先 (`opt-level = "s"`) で最適化し、デバッグ情報と関数名を捨てる。できる .wasm は約 190 MB、gzip で約 34 MB。
- 乱数 (getrandom) をブラウザの `crypto.getRandomValues` から取るため、`RUSTFLAGS` に `--cfg getrandom_backend="wasm_js"` を付ける (スクリプトが付ける)。
- GitHub Pages では `.github/workflows/pages.yml` の「ブラウザ版を作る」ジョブが同じスクリプトで作り、軽いビューアと合わせて公開する。

## 形状カーネル (C++) を WebAssembly にする

形状の「正」を持つ SMLib は C++ なので、Rust の `wasm32-unknown-unknown` には C/C++ の標準ライブラリが無いのが問題になる。そこで次のようにしている。

1. `crates/plantwright_core/build.rs` は、ターゲットが wasm32 のとき wasi-sdk の clang (`--target=wasm32-wasip1`) と、wasi-sdk に入っている C/C++ の標準ライブラリ (wasi-libc・libc++) で SMLib と `cpp/*.cpp` をコンパイルし、`libc++`・`libc++abi`・`libc`・`clang_rt.builtins` を静的にリンクする。
2. wasi-sdk の libc++ は C++ の例外を使えない。SMLib はメモリ不足を例外で知らせるだけなので、`SM_NO_EXCEPTIONS` を定義して例外を使わせない (`vendor/smlib/SMLib/inc/SmConfig.h` に、これを見る1行を足してある)。
3. できた .wasm は wasi-libc が呼ぶ `wasi_snapshot_preview1` の関数 (`fd_write`・`path_open` など) を外から受け取る形になる。wasm-bindgen はこれをそのまま `import ... from "wasi_snapshot_preview1"` にするので、ページの import map でこの名前を `wasi_shim.js` に向け、そこで足りる分だけを用意する (`assets/web_demo/wasi_shim.js`)。
   - 標準出力・標準エラー出力はブラウザのコンソールに出す。
   - ファイルはメモリの中に持つ。SMLib との形の受け渡し (`Brep::to_text` / `from_text`) は一時ファイルを通すので、ブラウザ版ではこれを Rust の `std::fs` ではなく C の `fopen` などで読み書きし (`kernel/brep.rs` の `libc_file`)、中身は `wasi_shim.js` のメモリに置く。
   - wasm-bindgen の JS はインスタンスのメモリを外に出さないので、`WebAssembly.instantiate` を包んで作られたときに拾う。

## ブラウザで外したもの

ブラウザ (wasm32) には、ファイルシステム・スレッド・ソケット・`std::time` の時計が無い。呼ぶと panic してアプリごと止まるので、次のように外している。

| 使えないもの | どうしたか |
| --- | --- |
| OS のファイル選択ダイアログ (rfd) | rfd を wasm32 では依存に入れず、`document/dialog.rs` の `FileDialog` (いつも「選ばなかった」を返す) を使う。開く・保存・取り込み・書き出し (図面・Excel・動画・CSV など) はダイアログから先に進まない |
| サンプルのファイルの書き出し | 埋め込みのサンプル (UR5e の glTF、URDF) は書き出さず `plantwright_core::formats::memory_files` に置き、プロジェクト・glTF・URDF の読み込みはそこを先に見る |
| 別スレッドの読み込み | `plantwright_core::background::spawn` が、ブラウザ版ではその場で動かす (読み込み中は画面が止まる) |
| PLC との通信 (Modbus TCP・SLMP・OPC UA)、実機ロボット (RTDE) | 通信のスレッドを立てず、状態に「ブラウザ版では使えない」を出す。tokio-modbus・async-opcua・tokio は wasm32 では依存に入れない |
| 生成 AI (Claude API) | ureq を依存に入れず、問い合わせると「ブラウザ版では使えない」を返す (キーワードから作る簡易版は動く) |
| MP4 の書き出し (OpenH264) | 依存に入れない (保存先を選べないので、そもそも書き出しは始まらない) |
| `std::time::Instant` / `SystemTime` | 毎フレーム使うところは `bevy::platform::time::Instant` (core は `bevy_platform`)、図面の日付は `js_sys::Date` |
| `std::env::temp_dir` / `std::process::id` | ブラウザ版では使わない道に分ける |

外部の PLC・レビューの共有・ビューアの書き出しなど、ボタン自体は残っているものもある。押すと理由を出して何もしない。

## 気をつけること

- 新しく `std::thread::spawn`・`std::time::Instant::now`・`std::fs`・`std::env::temp_dir` を足すときは、ブラウザ版で通る道かを考える。通るなら `background::spawn`・`bevy::platform::time::Instant`・`memory_files` を使うか、`#[cfg(target_arch = "wasm32")]` で分ける。
- ブラウザの WebGL2 では、平行光源は1つ、SSAO・被写界深度などは使えない (Bevy がそう知らせて外す)。見た目はデスクトップ版より少し簡素になる。
- ブラウザ版の画質は最初「軽い」(`RenderQuality::default` が wasm では `Low`)。スマホで「きれい」の大きな影の地図や後処理を使うと GPU のメモリが尽き、WebGL の文脈が失われて描けなくなる (Android の Chrome では小さな顔の絵だけが出る)。文脈が失われたときは `index.html` が `webglcontextlost` を受けて、理由と GPU の名前をページに出す。
- `cargo build --target wasm32-unknown-unknown` が通るかは、`tools/web_demo.sh` か Pages のワークフローで確かめる。
