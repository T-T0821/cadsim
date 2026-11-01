# Web ビューアと VR / AR 表示

plantwright が無い人にモデルと動きを見てもらうための書き出しと、VR ヘッドセットで実寸で見るためのしくみです。

## 全体の流れ

```text
Model + 取り込んだアニメーション + タイミングチャート
  │ model_io::model_to_scene / Animations::to_scene / motion::chart_animation
  ▼
Scene (メートル・Y 軸が上)
  │ gltf::to_glb_bytes
  ▼
GLB ─ Base64 ─▶ assets/web_viewer/viewer.html の __PLANTWRIGHT_MODEL__ に差し込む ─▶ 1つの .html
```

- 書き出しは `src/interchange/web_viewer.rs`。`FORMATS` に「Web ビューア (.html)」として書き出し専用で登録しているので、書き出しダイアログで拡張子 `.html` を選べば出る。
- 「ファイル → 書き出し → ブラウザ / VR で見る」は、一時フォルダ(`<temp>/plantwright_viewer/plantwright.html`)に同じものを書き、OS の既定のアプリ(Windows は `start`、Mac は `open`、Linux は `xdg-open`)で開く(`file_io.rs` の `open_in_browser_if_requested`)。
- タイミングチャートの動きは、glTF / USD の書き出しと同じく `motion/export.rs` が 1/30 秒ごとに1サイクル分を標本化してキーフレームにする。

## ビューア(`assets/web_viewer/viewer.html`)

外部のライブラリや CDN を使わない、素の WebGL(WebGL2、無ければ WebGL1)のビューアです。HTML・CSS・JavaScript と GLB がすべて1ファイルに入るので、オフラインでもメール添付でも動きます。

- **読み込み**: 埋め込んだ Base64 を GLB に戻し、JSON チャンクと BIN チャンクを読む。対応するのは plantwright が書く範囲(ノードの階層と TRS、三角形の `POSITION` と `indices`、`baseColorFactor`、移動 / 回転 / 拡大の `LINEAR` / `STEP` アニメーション)。
- **陰影と稜線**: 三角形を角ごとの頂点に展開し、折れ角 30° 以内の面だけで法線を平均する(平らな面はくっきり、円筒はなめらか)。同じ位置の頂点をまとめてから、面が 30° より折れている辺と縁の辺を稜線として線で描く。plantwright 本体の `model_io::feature_edges` と同じ考え方。
- **床の格子**: モデルの下面の高さに、大きさに合わせた 10 のべき乗の間隔で引く。
- **再生**: クリップごとにチャンネルを二分探索で補間(回転は球面線形補間)し、親から順にワールド行列を掛ける。
- **視点の操作**: Pointer Events でマウスとタッチをまとめて扱う。1本指 / 左ドラッグで回転、2本指でピンチ拡大と平行移動、右・中ドラッグか Shift で平行移動。動かさずに離したら、画面の点からのレイと三角形の交差(Möller–Trumbore)で部品を選び、名前とツリーの経路を出す。

## VR / AR(WebXR)

ネイティブの OpenXR は `bevy_mod_openxr` がまだ Bevy 0.20 に対応していないため、plantwright 本体ではなくビューアの WebXR で実現しています。Meta Quest のブラウザや、PC VR で使う Chrome / Edge の WebXR は OpenXR ランタイムの上で動くので、同じヘッドセットで見られます。

- `navigator.xr.isSessionSupported` が `immersive-vr` / `immersive-ar` を返したときだけボタンを出す。
- 参照空間は `local-floor`(床が原点)。使えないときは `local` にして床を 1.6 m 下に置く。
- **実寸**: glTF はメートルなので拡大縮小しない。モデルの下面を床(y = 0)に合わせ、X は中央、手前の端(+Z 側)を利用者の 1 m 前に置く。この置き方は「リグ」行列としてシーン全体に掛ける。
- **移動**: 左スティックで頭の向き(水平成分)を前として 1.5 m/s で歩く。右スティックを倒すと頭の位置を中心に 30° ずつ回る(戻すまで次の回転はしない)。どちらもリグ行列を書き換えるだけで、参照空間は変えない。
- AR では背景を透明にし、格子を描かない。
- XR 中もアニメーションは再生し続ける。

WebXR は安全なコンテキストでしか使えません。`file://` で開いたファイルと `https://` のサイトは使えますが、`http://` のサイトに置くと VR ボタンが出ません。

## 確かめ方

- `cargo test write_samples -- --ignored` で、サンプル装置(タイミングチャート付き)を `target/samples/line_demo.html` に書き出せる。README のスクリーンショットはこれを開いて撮った。
- `cargo test web_viewer` で、HTML に埋め込んだ GLB を取り出して読み直せることと、タイトルのエスケープを確かめる。
- ビューアの見た目と操作は、書き出した .html を Chromium(Playwright)で開いて確かめた。VR は `navigator.xr` と `XRWebGLLayer` を差し替えた疑似セッションで、左右の目の描画とスティックでの移動を確かめた(実機のヘッドセットでは未確認)。
