# ライセンスと第三者の権利

開発者向けの資料です。使う人は README の「ライセンス」と、アプリの **ヘルプ → ライセンス** を見てください。

ここに書いているのは、配布するときに守ることと、気をつけておく点のメモです。法的な助言ではありません。
有料で売る・企業に納める・ロゴや認証を名乗るといった段階に進むときは、弁理士や弁護士に確かめてください。

## plantwright 自身

- 本体と `crates/` 以下のクレートは MIT License と Apache License 2.0 のデュアルライセンス(`LICENSE-MIT` / `LICENSE-APACHE`)。新しいクレートを足すときも `Cargo.toml` に `license = "MIT OR Apache-2.0"` を書く。
- 同梱フォント `assets/fonts/NotoSansJP-Subset.otf` は SIL Open Font License 1.1(`assets/fonts/OFL.txt`)。
- サンプルの形状 `samples/real_robot/` は BSD 3-Clause(出典は [real-robot.md](real-robot.md))。

## 依存クレートのライセンス

### 許可しているライセンス

[`deny.toml`](../deny.toml) の `licenses.allow` に並べたライセンスだけを許す。MIT・Apache-2.0・BSD・ISC・Zlib などの寛容なライセンスと、次の例外。

| ライセンス | 使っているクレート | 守ること |
| --- | --- | --- |
| MPL-2.0 | `async-opcua` 一式(OPC UA)、`smartstring`(Rhai 経由) | ファイル単位のコピーレフト。改変せずに使う限りは表示だけでよい。これらのクレートのソースを改変して配るときは、改変したファイルのソースを公開する |
| OFL-1.1・Ubuntu-font-1.0 | `epaint_default_fonts`(egui の既定フォント) | フォント単体で売らない。表示を付ける |
| CDLA-Permissive-2.0 | `webpki-roots`(証明書のデータ) | 表示を付ける |

GPL・LGPL・AGPL だけのクレートは入れない。`r-efi`(`MIT OR Apache-2.0 OR LGPL-2.1-or-later`)や `self_cell`(`Apache-2.0 OR GPL-2.0-only`)のように選べるものは、寛容な側を選んで使っている扱いになる。

### 確かめ方

依存を足したり上げたりしたら、次を実行する。PR では [`.github/workflows/licenses.yml`](../.github/workflows/licenses.yml) が同じことを確かめる。

```sh
cargo install --locked cargo-deny
cargo install --locked cargo-about --features cli

cargo deny check licenses sources bans   # 許可していないライセンス・取得元がないか
tools/licenses/generate.sh               # ライセンス一覧を作り直す (変わったらコミット)
```

`cargo deny` が許可していないライセンスで止まったら、そのクレートを使わない方法をまず探す。どうしても使うなら、守ることをこの表に書いてから `deny.toml` と `about.toml` の両方に足す。

## ライセンス一覧(著作権表示)

MIT・Apache-2.0・BSD などは、配るときに著作権表示とライセンス文を付けることが条件になっている。
そこで [`tools/licenses/generate.sh`](../tools/licenses/generate.sh) が `cargo-about` で Cargo.lock から一覧を作り、[`assets/licenses/THIRD_PARTY_LICENSES.txt`](../assets/licenses/THIRD_PARTY_LICENSES.txt) に書き出す。

- 中身: 依存クレートのライセンス文と、それを使っているクレートの一覧(テスト用の依存は除く)。最後に同梱フォントとサンプル形状のライセンス文を足す。
- 書式は [`tools/licenses/about.hbs`](../tools/licenses/about.hbs)、対象のライセンスは [`about.toml`](../about.toml)。
- アプリは `include_str!` で埋め込み、**ヘルプ → ライセンス** で見せる(`src/ui/licenses.rs`)。exe 1 つで配っても表示が付いて回る。
- Release の Windows の zip と Mac の `.app`(`Contents/Resources`)にも、`LICENSE-MIT`・`LICENSE-APACHE` と一緒に入れる([release.md](release.md))。
- Cargo.lock と合っていないと、PR の「ライセンス一覧が最新か」が失敗する。`tools/licenses/generate.sh` を実行してコミットすれば直る。

## 特許・商標などで気をつける点

2026-11 にリポジトリの機能と依存を見て整理したもの。

### H.264(MP4 書き出し)

- MP4 は `openh264` クレートが同梱の OpenH264 のソースをビルドして H.264 にしている([video-export.md](video-export.md))。OpenH264 のソースは BSD 2-Clause だが、**H.264 には特許があり、ソースのライセンスとは別の話になる**。
- Cisco が特許料を負担するのは、Cisco が配っている OpenH264 のバイナリをダウンロードして使う場合だけ。ソースからビルドして同梱している plantwright はその対象に入らない。
- H.264 の特許プール(Via LA の AVC)には、少ない台数なら無料の枠がある。無料で少し配るぶんには現実的な問題になりにくいが、**有料で売る・大量に配る前には確認する**。
- 避けたくなったときの手: (1) 実行時に Cisco のバイナリをダウンロードして使う、(2) OS のエンコーダ(Windows の Media Foundation、macOS の AVFoundation)を使う、(3) MP4 をやめて VP9 / AV1(WebM)にする。GIF と PNG 連番は問題ない。

### 機能そのものの特許

- 工場・ロボットのシミュレーション、仮想立ち上げ、オフラインティーチングの分野では、Siemens・Rockwell・FANUC・三菱電機などが多くの特許を持っている。
- 干渉チェック、順・逆運動学、離散事象シミュレーション、タイミングチャートなど、昔から知られた手法そのものは特許になりにくい。気にするなら、独自色の強い機能(言葉からのライン・ラダーの生成、巻き戻し、人員配置の自動化など)。
- 無料の個人配布なら狙われる理由は小さいが、特許の侵害は無料かどうかに関係なく成り立つ。**有料化・企業への納入の前に、特徴的な機能に絞って弁理士に侵害予防調査(FTO)を頼む**。自分でざっと見るなら J-PlatPat や Google Patents で、似た機能のキーワードと競合会社の名前で探す。

### ファイル形式・通信方式

- STEP(ISO 10303)、glTF、USD、OBJ、STL、URDF、LAS / LAZ、E57、DXF の読み書きは自由に実装できる。仕様書の文章や図をそのまま写して配らない。
- Modbus は公開されていて自由に使える。三菱の SLMP も仕様が公開されている。
- OPC UA は実装してよいが、**「OPC UA 認証済み」と名乗ったりロゴを使ったりするには OPC Foundation の認証が要る**。README などでは「OPC UA で接続できる」とだけ書く。

### 商標

- 競合比較などで他社の製品名(Process Simulate、ROBOGUIDE、MELSOFT など)を事実として書くのはふつう問題ない。**ロゴは使わない**、提携や推奨と取られる書き方をしない、各社の登録商標であることを書き添えると丁寧。
- MELSEC・MELFA・Universal Robots・Robotiq などの機器名も同じ。「○○ に対応」とは書いてよいが、メーカー公認と取られる書き方はしない。
- アプリの名前(plantwright、plantwright)は、公開・販売で名前を前に出す前に、J-PlatPat で同じ区分(ソフトウェアは第 9 類・第 42 類)に同じか似た登録がないか確かめる。

### 生成 AI

- Claude API は使う人が自分のキーを入れて使う。API の利用規約は使う人と Anthropic の間の話で、plantwright はキーを預からない。
