# cadsim

工場のライン設備向けの 3D CAD / 動作シミュレーターです。Rust + Bevy + truck で作っています。
形状の「正」は truck の B-rep(f64)が持ち、Bevy は表示用の f32 メッシュと操作だけを担当します。

![サンプルのアセンブリを開いたところ](docs/screenshots/01_overview.png)

## 機能と進み具合

| 機能 | 状況 |
| --- | --- |
| 箱の作成・選択(面単位)・寸法と位置と回転の編集 | 実装済み |
| Unity / Unreal 準拠のカメラ操作、W / E / R ギズモ、Ctrl スナップ | 実装済み |
| 日本語 UI | 実装済み |
| PR ごとの CI(fmt・clippy・テスト・ビルド) | 実装済み |
| STEP 読み込みとアセンブリツリー、選択の強調表示、プロパティ・ビューキューブなど | 実装済み |
| Windows / Mac 向けリリースの自動作成 | 開発中 |
| 干渉チェック | 予定 |
| タイミングチャートによる動作シミュレーション | 予定 |
| 保存 / 読み込み、Undo / Redo | 予定 |
| CSV 出力 | 予定 |

## できること

- **STEP の取り込み**(AP203 / AP214 / AP242)。メニューの「ファイル → STEP を開く」、ウィンドウへのドラッグ&ドロップ、`cargo run -- 部品.step` のどれでも開けます。読み込みは別スレッドなので、大きなファイルでも画面は止まりません。
  - 製品構成(アセンブリ → サブアセンブリ → 部品)と各部品の配置
  - 部品の色(`STYLED_ITEM`)と長さの単位(mm / cm / m / inch)
  - 同じ部品を何個使っていても形状とメッシュは1回だけ作る
- **アセンブリツリー**(左)。行をクリックで選択、ダブルクリックで拡大表示、目のボタンで表示/非表示、右クリックで「これだけ表示」「削除」など。
- **選択の強調表示**。アセンブリを選ぶとその中の部品がすべて青くなり、黄色い外接箱が出ます。ビューで部品をクリックするとツリーがその行まで開きます。ツリーの行にカーソルを乗せるとビューで薄く光ります。
- **プロパティ**(右)。名前・種類・製品名・場所・表示・色、親から見た位置と回転、外接箱の寸法、部品数・面数・稜線数・体積・表面積。
- **ビュー操作**。ビューキューブ(右上)、正面/背面/左右/上下/等角の標準ビュー、全体表示、選択の拡大、座標軸(左下)。
- **表示の切り替え**。稜線、グリッド、選択の外接箱、選択以外を半透明。
- **箱の作成と編集**。寸法・位置・回転の数値入力と、W / E / R のギズモ(Ctrl でスナップ)。取り込んだ部品やアセンブリもギズモで移動・回転できます。

| アセンブリを選択 | 部品をクリック |
| --- | --- |
| ![アセンブリを選択](docs/screenshots/02_assembly_selected.png) | ![部品を選択](docs/screenshots/03_part_picked.png) |

| 上面ビュー + 選択以外を半透明 | 公開サンプル as1-oc-214.stp |
| --- | --- |
| ![上面ビューと半透明](docs/screenshots/04_top_view_ghost.png) | ![as1](docs/screenshots/05_as1_assembly.png) |

## 動作確認したバージョン

| crate | バージョン |
| --- | --- |
| Rust | 1.97.1 以上(依存crateの要件) |
| bevy | 0.20 |
| bevy_egui | 0.43 |
| truck-modeling | 0.6 |
| truck-meshalgo | 0.4 |
| truck-stepio | 0.3 |
| transform-gizmo | 0.11 |
| rfd(ファイル選択ダイアログ) | 0.17 |

## 実行

```sh
cargo run                      # サンプルのアセンブリ (assets/sample_assembly.step) を開いて起動
cargo run -- path/to/part.step # 指定した STEP を開いて起動
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

テストでは、サンプルの STEP を読み込んで構成・配置・寸法・体積・色が正しいかを確かめています(`src/step_import.rs` と `src/model.rs` の `tests`)。

依存のビルド結果は `Swatinem/rust-cache` で OS ごとにキャッシュし(共有キー `cadsim`)、main での実行時だけ保存します。`Cargo.lock` と食い違うと `--locked` で失敗するので、依存を変えたら `Cargo.lock` もコミットしてください。

## 操作

マウス操作は Unity / Unreal Engine のシーンビューと同じ割り当てです。

| 操作 | 内容 |
| --- | --- |
| 左クリック | 部品を選択(クリックした面はオレンジ、部品は青になる)。何もない所で解除 |
| Alt + 左ドラッグ | 注視点のまわりを回転(オービット) |
| 中ドラッグ | 平行移動(パン) |
| ホイール / Alt + 右ドラッグ | ズーム |
| 右ドラッグ | その場で見回す |
| 右ボタンを押しながら W / A / S / D | 前後左右に移動(Shiftで加速) |
| 右ボタンを押しながら Q / E | 下降 / 上昇 |
| W / E / R | 移動 / 回転 / 拡大縮小のギズモに切り替え。ギズモの軸や輪を左ドラッグ |
| Ctrl + ギズモを左ドラッグ | スナップ(一定量ずつ動かす)。刻み幅はプロパティの「ギズモのスナップ」欄で設定 |
| F / Home | 選択を拡大 / 全体を表示 |
| 1 / 3 / 7 / 0 | 正面 / 右側面 / 上面 / 等角ビュー(Ctrl を押すと背面 / 左側面 / 下面) |
| H / Alt + H | 選択を隠す / すべて表示 |
| Esc | 選択を解除 |
| Delete | 選択を削除 |
| Ctrl + O | STEP を開く |

ビューキューブの面をクリックするとその方向から見たビューになります。拡大縮小のギズモは箱だけで使えます(取り込んだ部品の形状は変えません)。

### スナップ

Ctrl を押している間(または「常にスナップ」をオンにしている間)、ギズモの操作が一定量ずつになります。

| ギズモ | 刻み(初期値) | 動き方 |
| --- | --- | --- |
| 移動 | 移動の刻み(0.1) | ドラッグを始めた位置から 0.1 ずつ動く |
| 回転 | 回転の刻み(15°) | ドラッグを始めた向きから 15° ずつ回る。輪に目盛りが出る |
| 拡大縮小 | 寸法の刻み(0.1) | 動かした軸の寸法が 0.1 の倍数になる(触っていない軸の寸法はそのまま) |

移動と回転は Unity と同じく「ドラッグ開始からの変化量」を丸めます。拡大縮小だけは倍率ではなく寸法を丸めるので、寸法をきりのいい値にそろえられます。

## コードの構成

```text
src/
├── main.rs          アプリの組み立て (プラグインを並べるだけ)
├── model.rs         設計データ: アセンブリの木 (f64) と選択状態
├── step_import.rs   STEP → アセンブリの木と B-rep (Bevy に依存しない)
├── file_io.rs       ダイアログ・ドラッグ&ドロップ・別スレッドでの読み込み
├── viewport.rs      3D 表示: メッシュ作成、選択の強調、稜線・グリッド、クリック選択
├── camera.rs        オービットカメラ、標準ビュー、全体表示
├── gizmo.rs         移動・回転・拡大縮小のギズモとスナップ
├── shortcuts.rs     キーボードショートカット
├── math.rs          回転・行列・外接箱の計算
├── japanese_font.rs egui に日本語フォントを登録
└── ui/
    ├── mod.rs        画面の割り付け (上 → 下 → 左 → 右 → 残りが 3D ビュー)
    ├── menu.rs       メニューバーとツールバー
    ├── tree.rs       アセンブリツリー
    ├── properties.rs プロパティパネル
    ├── status.rs     ステータスバーと読み込み中の表示
    ├── view_cube.rs  ビューキューブと座標軸
    └── widgets.rs    数値入力などの小さな部品
```

データは一方向に流れます: **UI・ギズモ・読み込み → `Model`(f64)→ `viewport` が Bevy のエンティティに写す**。
`Model` は Bevy のエンティティを知らないので、表示を変えても設計データには影響しません。

## しくみ

### STEP の読み込み(`step_import.rs`)

1. `ruststep` で STEP をパースし、`#id` からレコードを引ける表を作る。
2. `PRODUCT_DEFINITION` を製品、`NEXT_ASSEMBLY_USAGE_OCCURRENCE` を親子関係として木を組む。配置は `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION` → `ITEM_DEFINED_TRANSFORMATION` の2つの `AXIS2_PLACEMENT_3D` から「子 → 親」の行列を求める。
3. 部品の形状(`MANIFOLD_SOLID_BREP` などのシェル)は `truck-stepio` で truck の B-rep に変換し、`robust_triangulation` で面ごとのメッシュと稜線にする。許容誤差は部品の大きさ(と一番薄い方向の寸法)に合わせる。
4. 同じ製品の形状は `Arc` で共有し、Bevy 側のメッシュも形状ごとに1回だけ作る。
5. STEP は Z 軸が上なので、ルートの配置に「Z 上 → Y 上」の回転を入れて Bevy の座標に合わせる。

構成が書かれていない STEP(シェルだけのもの)は、シェルを1つずつ部品として並べます。読めなかった形状はスキップしてステータスバーに件数を出します。

### 表示と選択(`viewport.rs`)

1. 部品1つにつき Bevy のエンティティ(`PartView`)を作り、子に面ごとのメッシュ(`FaceTag { node, face }`)を付ける。
2. 配置と表示/非表示は毎フレーム `Model` から写す。形状が変わったとき(`dirty`)だけメッシュを作り直す。
3. 左クリックで `MeshRayCast` を撃ち、当たった `FaceTag` から部品と面を特定する。
4. マテリアルは「選択(アセンブリなら子孫すべて)→ 青」「クリックした面 → オレンジ」「ツリーでホバー → 水色」「それ以外 → 部品の色」で決める。
5. 稜線・グリッド・選択の外接箱は Gizmos で描く。グリッドの間隔はモデルの大きさに合わせて 1, 10, 100… と変わる。

### 画面の割り付け(`ui/mod.rs`)

egui のパネルを上・下・左・右の順に置き、残った領域を 3D ビューにします。3D カメラはその領域だけに描き(`Camera::viewport`)、egui はウィンドウ全体を覆う別のカメラに描きます。カーソルがパネルやビューキューブの上にあるときは、3D ビューのクリックやカメラ操作を止めます。

### 箱とギズモ

1. 箱(`BoxPart`)は寸法・位置・回転を f64 で持ち、編集されると truck で B-rep ソリッドを作り直す(`tsweep` を3回 → `rotated` → `translated`)。
2. ギズモ(`src/gizmo.rs`)は transform-gizmo で計算し、egui の背景レイヤーに描く。箱ならパラメータに、取り込んだ部品やアセンブリなら親から見た配置(f64 の行列)に書き戻す。スナップは移動・回転を transform-gizmo の機能に任せ、拡大縮小の寸法だけ `snap_size` で丸める。

## サンプルの STEP

`assets/sample_assembly.step` は `tools/make_sample_step.py` で作った、箱と円柱だけの小さなアセンブリ(万力風)です。外部ライブラリを使わずに AP214 の書式で書き出しているので、STEP の構成を調べる手がかりにもなります。

```text
SampleVise
├── BasePlate_1
├── Bracket_L ─┐  同じサブアセンブリ Bracket を2回使っている
├── Bracket_R ─┴─ Bracket ── Upright_1, Bushing_1
├── Shaft_1
└── Knob_1
```

作り直すときは `python3 tools/make_sample_step.py` を実行します。

## 日本語フォント

egui の標準フォントには日本語がないため、[Noto Sans JP](https://github.com/notofonts/noto-cjk)(SIL Open Font License 1.1)を埋め込んでいます。

- `assets/fonts/NotoSansJP-Subset.otf` … 英数字・記号・かな・JIS 第1/第2水準漢字だけに絞ったもの(約 2.1 MB)。`include_bytes!` で実行ファイルに入るので、配布時にフォントを別に置く必要はありません。
- `assets/fonts/OFL.txt` … フォントのライセンス。配布物にも同梱してください。
- `src/japanese_font.rs` … 起動直後に egui の予備フォントとして登録する。英数字は egui の標準フォント、日本語だけ Noto Sans JP で描かれます。
- `tools/subset_jp_font.py` … 元フォントから絞り込み版を作り直すスクリプト(使い方はファイル先頭)。

## 次に手を入れるところ

- 面ごとの色(STEP で面に付いた色)の読み込み
- 平行投影への切り替え
- 計測ツール(2点間の距離、面と面の角度)
- 干渉チェック(取り込んだ B-rep は `ImportedShape::breps` に f64 のまま残してある)
- STEP への書き出し
