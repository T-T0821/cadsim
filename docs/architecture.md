# アーキテクチャ

cadsim は Clean Architecture をもとに、コードを4つの層に分けます。
**依存は外側から内側へだけ向けます。** 内側の層は外側の層(Bevy、egui、ファイル形式のライブラリなど)を知りません。

```text
┌──────────────────────────────────────────────────────────┐
│ 4. UI・フレームワーク (app)                                 │
│    Bevy のプラグインとシステム、egui のパネル、3D 表示、       │
│    カメラ、ギズモ、ショートカット、ファイルダイアログ          │
│  ┌────────────────────────────────────────────────────┐  │
│  │ 3. アダプター (io)                                   │  │
│  │    STEP / glTF / OBJ / STL / USD の読み書き、          │  │
│  │    .cadsim の JSON、Modbus TCP、SLMP、動画のエンコード    │  │
│  │  ┌──────────────────────────────────────────────┐  │  │
│  │  │ 2. ユースケース (core::usecase)                 │  │  │
│  │  │    開く・保存・取り込み・書き出し・編集・            │  │  │
│  │  │    元に戻す・干渉チェックの実行・チャートの再生       │  │  │
│  │  │  ┌────────────────────────────────────────┐  │  │  │
│  │  │  │ 1. ドメイン (core::domain)                │  │  │  │
│  │  │  │    アセンブリの木・部品・箱・選択、          │  │  │  │
│  │  │  │    幾何計算、干渉判定、タイミングチャート、    │  │  │  │
│  │  │  │    運動学、PLC のシーケンス                  │  │  │  │
│  │  │  └────────────────────────────────────────┘  │  │  │
│  │  └──────────────────────────────────────────────┘  │  │
│  └────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────┘
```

## 各層の決まり

| 層 | 置くもの | 依存してよいもの | 依存してはいけないもの |
| --- | --- | --- | --- |
| 1. ドメイン | 設計データの型とその規則。純粋な計算 | `glam`(f64 のベクトル・行列)、`truck`(形状カーネル。B-rep の曲線・曲面の型だけ `truck-stepio` から使う)、標準ライブラリ | Bevy、egui、ファイル形式・通信のライブラリ、ファイルシステム |
| 2. ユースケース | 「ユーザーがしたいこと」1つ = 関数 1つ。必要な外部 I/O はトレイト(ポート)で受け取る | ドメイン | Bevy、egui、具体的なファイル形式 |
| 3. アダプター | ポートの実装。ファイル形式・通信・エンコードの変換 | ドメイン、ユースケース(ポート)、`ruststep`・`gltf`・`openusd`・`serde_json` など | Bevy、egui |
| 4. UI・フレームワーク | Bevy の `Resource` / `Component` / システム、egui の画面。入力をユースケースの呼び出しに変え、結果を画面に写す | すべての内側の層 | — |

- Bevy の `Resource` や `Component` はドメインの型に直接付けず、UI 層で包みます(例: `struct ModelRes(Model)` に `Deref` を付ける)。
- UI 層のシステムは薄く保ちます。判断や計算は内側に置き、システムは「入力を集める → ユースケースを呼ぶ → 結果を表示に写す」だけにします。
- 内側の層は Bevy なしで `cargo test` できるようにします。新しい計算処理には内側の層で単体テストを付けます。
- 新しいファイル形式は、アダプター層に読み書きを足し、ユースケースのポートにつなぎます。UI からは形式の一覧を見るだけにします。

## 目標のディレクトリ構成

Cargo のワークスペースに分け、依存の向きをコンパイラに守らせます(内側の crate の `Cargo.toml` に Bevy を書かなければ、使おうとした時点でビルドが通りません)。

```text
Cargo.toml                ワークスペース兼アプリ本体 (4. UI・フレームワーク)
src/                      アプリ本体。assets/ の場所を変えないようにルートに置いたままにする
├── main.rs               プラグインを並べるだけ
├── resources.rs          ドメインの型を包む Resource
├── view/                 viewport, camera, gizmo, 干渉の表示, 動画の撮影
├── input/                shortcuts, ファイルダイアログ, ドラッグ&ドロップ
└── ui/                   menu, tree, properties, status, 各パネル
crates/
├── cadsim_core/          1. ドメイン + 2. ユースケース (Bevy なし)
│   └── src/
│       ├── domain/       model, math, melsec (三菱 PLC のデバイス), selection, remap (形状の差し替えの対応づけ), simulation (幾何・干渉・計測),
│       │                 motion (チャート・姿勢・運動学), plc (シーケンス・I/O 表), flow, conveyor,
│       │                 staffing (作業者の名簿・スキル・シフト・人数の比較),
│       │                 safety (安全機器・検知・安全距離), floor (作業者・AGV・床の幾何・作業者と AGV の 3D の形),
│       │                 line (工程と AGV・コンベアの受け渡しと時間の進め方・PLC の信号表・ワークの位置),
│       │                 line_draft (言葉で組むラインの下書き・整え方・並べ方),
│       │                 ui_check (画面の自動確認の手順書・進め方・部品の探し方・画像の比べ方)
│       └── usecase/      document (開く・保存・取り込み・書き出し), edit, history, compose_line (言葉でラインを組む),
│                         report (シミュレーション結果の CSV), llm (生成 AI のポート LlmClient), help (使い方を AI に聞く), script (部品スクリプトの実行と ScriptHost ポート),
│                         ports (SceneReader / SceneWriter / ProjectStore などのトレイト)
└── cadsim_io/            3. アダプター (Bevy なし)
    └── src/              step, interchange (gltf / obj / stl / usd), project_file (JSON),
                          modbus, slmp (三菱 PLC との SLMP 通信・テスト用の PLC), rtde (UR の関節角度の受信), video, xlsx (工程シミュレーションの結果), rhai (部品スクリプトのエンジン),
                          claude (生成 AI の Claude API), line_draft_json (生成 AI とやり取りするラインの下書きの JSON),
                          ui_check (画面の自動確認の PNG・結果の JSON・報告書の HTML)
```

ルートの `Cargo.toml` の `default-members` に全 crate を並べているので、引数なしの `cargo test` / `cargo clippy` でワークスペース全体を確かめられます。

## いまのコードからの移し方

機能追加の PR(タイミングチャート、ロボット、PLC、センサー、動画書き出し、搬送など)が落ち着いてから、動作を変えずに次の順で少しずつ移します。
1つの PR は1段だけにし、毎回 `cargo fmt` / `clippy` / `test` が通る状態でマージします。

1. **ワークスペース化と純粋な計算を core へ。** ✅ ルートをワークスペースにして `cadsim_core` を作り、`math` と `simulation` の幾何・干渉の計算を移した。`bevy::math` は同じ版の `glam` に置き換え、アプリ側の `math` / `simulation` から再公開しているので呼び出し側は変わらない。
   - ゲーム風 UI の判定(`gamify`)は最初から `cadsim_core` に置いた。道具の一覧と開け閉め(ツールレール)の `workspace` も `cadsim_core` に置き、UI 層の `ui::dock` が Resource に包む。設定ファイルの読み書きは `cadsim_io::preferences_file` に移した。
2. **残りの純粋な計算を core へ。** 標準部品の `library/catalog.rs`(種類・パラメータ・I/O 点・動作)と `library/geometry.rs`(形状)はすでに Bevy に依存しないので、そのまま移せる。そのほか `motion` のチャート・姿勢・運動学、PLC のシーケンスなど。進行中の機能ブランチが触っているので、それらが入ってから移す。 🔶 タイミングチャートと動きの計算 (`motion::{chart, profile, pose, kinematics, robot_path, robot_time, path_interference, sensor}`) を `cadsim_core::motion` に移した。アプリ側の `motion::chart::TimingChart` は `#[derive(Resource, Deref, DerefMut)] struct TimingChart(cadsim_core::motion::TimingChart)` という包み型。
3. **ファイル形式を io へ。** 🔶 `cadsim_io` を作り、3D ファイル形式の読み書き (`interchange` の Scene・glTF・OBJ・STL・USD・Web ビューア) を移した。アプリ側の `interchange` から再公開しているので呼び出し側は変わらない。`Scene` ⇔ `Model` の変換 (`model_io`) も、手順 4 で `Model` が core に移ったあと io へ移した。続いて STEP の読み込みを `cadsim_io::step` に移し、取り込んだ形状とアセンブリの木の型 (`ImportedShape` など) は `cadsim_core::shape`、表示用メッシュ (`part_mesh`)・BVH・アニメーションの型 (`Animation` など) は `cadsim_core` に移した (io → core の向き)。さらに平面図・点群・図面の形式 (`formats::{dxf, pdf_plan, pointcloud_read, site_record, drawing_out, conveyor_record}`)、工程シミュレーションの記録と Excel 書き出し (`flow::{record, xlsx}`)、設定ファイル (`preferences_file`) を `cadsim_io` に移した。アプリ側の `formats` / `flow` / `preferences_file` から再公開している。部品表の書き出し (`formats::bom_out`) も io。タイミングチャートとアニメーションの保存形式 (`formats::chart_record`) も io。`.cadsim` の JSON の構造体はまだアプリ側。
4. **設計データを core へ。** 🔶 `Model` と `Node`・`NodeKind`・`BoxPart` などを `cadsim_core::model` に移した。アプリ側の `model::Model` は `#[derive(Resource, Deref, DerefMut)] struct Model(cadsim_core::model::Model)` という包み型で、`Res<Model>` やメソッド呼び出しはそのまま使える。標準部品の種類・パラメータ・形状 (`library::{catalog, geometry}`) と、モデリングで作った形状の出どころ `ModeledSource` も core に置き、JSON との変換は `cadsim_io::modeled` の `ModeledSourceExt` に置いた。`Selection` は画面の状態なので UI 層 (アプリ側) のまま。`Model` のテストはサンプルの STEP (io) を読むのでアプリ側に残した。部品表の計算 (`bom`) も core に移した (テストはサンプルの STEP を読むのでアプリ側)。読み込み元の表示名は `StepSource::display_name`。モデリング (円柱・押し出し・穴あけ・ブーリアン) も `cadsim_core::modeling` に移し、B-rep を JSON に詰める部分はポート `SolidStore` にして io (`JsonSolids`) が実装する。
5. **ユースケースを切り出す。** 開く・保存・取り込み・書き出し・削除・元に戻すを `usecase` の関数にし、Bevy のシステムからはそれを呼ぶだけにする。元に戻す履歴は `World` ではなくドキュメント(モデル + チャートなど)を書き換える形にする。 🔶 まず履歴の山 (`UndoStack`)・モデルの変更 (`ModelChange`)・ドラッグ中の編集のまとめ方 (`ModelEdits`) を `cadsim_core::history` に移した。アプリ側の `History` (Resource) はそれを持ち、`Command` で `World` を書き換える部分と見張る文書 (`TrackedDoc`) だけを持つ。
6. **UI 層を整理する。** 残った Bevy / egui のコードを `view` / `input` / `ui` に分ける。 🔶 3D 表示の `viewport`・`camera`・`gizmo`・`recording` を `src/view/` に、キーのショートカット `shortcuts` を `src/input/` に移した。`main.rs` はそれらを `use` してプラグインを並べる。ファイルのダイアログ・ドラッグ&ドロップ・起動時の引数も `input::{file_dialog, command_line}` に分け、`file_io` には別スレッドでの読み込みとモデルへの追加・書き出しだけを残した。プロジェクトの開く・保存も `src/project/` で層に分けた。`file.rs` が .cadsim の形 (アダプター層)、`document.rs` が開く・保存の手順 (ユースケース層。ファイルを読んで STEP の形状を読み直す `LoadedProject::load`、モデルと寸法・工程・チャート・床などの文書を作り直す `LoadedProject::open`、書く `save_project`)、`mod.rs` が UI 層 (頼まれた操作を受け、結果を Resource に写す)。どちらも Bevy に依存しないが、寸法 (`annotation`)・アニメーション・床の配置などの型がまだアプリ側にあるので、それらが core / io に移ったら `cadsim_core::usecase::document` と `cadsim_io::project_file` に移す。

安全機器と作業者・AGV は、`device.rs`・`geom.rs`・`operator_plan.rs`・`agv_sim.rs` を `cadsim_core::{safety, floor}` に移した。動作範囲と距離の確認 (`check.rs`) も、チャートが core に移ったあと `cadsim_core::safety` に移した。`src/safety/`・`src/floor/` の `mod.rs`・`view.rs`・`panel.rs` が UI 層です。

新しく足した機能のうち、すでに層を分けてあるもの:

- `src/result_export/report/` … ユースケース層 (CSV の作成)。Bevy の ECS・egui・ファイルシステムに依存しない。移行時は `usecase/report` へ移す。`src/result_export/plugin.rs` は UI 層。
- PLC 連携(`src/plc/`)は先に `domain` / `usecase` / `adapter` / UI に分けてあるので、移行時はディレクトリごと core / io へ移せます。三菱 PLC との SLMP 通信は、デバイスの型を `cadsim_core::melsec`、電文とクライアントを `cadsim_io::slmp` に置き、`src/plc/domain/device_map.rs`(信号 → デバイス)と `src/plc/adapter/slmp.rs`(`PlcLink` の実装)がそれを使う。
- 部品スクリプト(`src/scripting/`)は、`runner.rs`(実行とポート `ScriptHost`)と `controller.rs`(PLC 連携の I/O へのつなぎ)がユースケース層、`rhai_engine.rs` がアダプター層、`mod.rs`・`ui.rs` が UI 層。`controller.rs` は PLC の I/O の型を使うので、PLC 連携と一緒に core へ移す。

- 自作部品は、ノードに付けるしるしと公開パラメータの型(`CustomInstance`・`ParamTarget`)を `cadsim_core::custom_part` に置いた。アプリの `src/custom_part/` のうち `param.rs`(公開パラメータのチャートへの読み書き)と `place.rs`(置くときの名前の頭と id の付け替え)がユースケース層、`file.rs`(`part.cadpart` の JSON とフォルダへの保存)がアダプター層、`mod.rs`・`ui.rs` が UI 層。`param.rs`・`place.rs` はタイミングチャートの型を使うので、チャートを core へ移すときに一緒に移す。

各段の終わりに、この文書の「目標のディレクトリ構成」と実際の構成の差を更新します。

### すでに層を分けて書いてあるもの

- 工程シミュレーション: ドメイン(工程モデル・離散事象シミュレーション・色・工場のライン・品種と生産計画・案の比較)は `cadsim_core::flow` に置いた。`.cadsim` の形 (`record`)、Excel 書き出し (`xlsx`)、生産計画の表の読み込み (`plan_table`。Excel・ODS は `calamine`、CSV は Shift_JIS も `encoding_rs` で読む) は `cadsim_io::flow`。アプリの `src/flow/` の `mod.rs`・`ui.rs`・`staff_ui.rs`・`factory_ui.rs`・`plan_ui.rs`・`scenario_ui.rs` が UI 層。

### 移行の途中で足した機能の置き場所

| 機能 | ドメイン(`cadsim_core`) | アダプター(いまはアプリ側、手順 3 で `cadsim_io` へ。3D ファイル形式は移動済み) | UI(アプリ側) |
| --- | --- | --- | --- |
| 平面図の下敷き・点群 | `site::{underlay, pointcloud, model}` | `formats::{dxf, pdf_plan, pointcloud_read, site_record}` | `site/`(表示・パネル・縮尺合わせ) |
| 2D 図面 | `drawing` | `formats::drawing_out` | `documents.rs` |
| 部品表 | `bom` | `formats::bom_out` | `documents.rs` |
| モデリング | `modeling`(B-rep の詰め方のポート `SolidStore`) | `modeled`(B-rep ⇔ JSON) | `ui/modeling.rs` |
| 物理演算 | `physics::{settings, cable}`(ドメイン)、`physics::world`(ユースケース。物理エンジンのポート `PhysicsEngine`) | `physics::{rapier, records}`(Rapier と .cadsim の形) | `physics/{mod, panel, sample}` |
| 動画書き出し | `video`(撮る時刻・切り抜き・書き込み先のポート `VideoSink`) | `video::{encode, mp4}`(MP4 / GIF / PNG) | `view/recording.rs`(画面のキャプチャ・設定ウィンドウ) |
| タイミングチャート・アニメーションの保存 | `motion::chart` | `formats::chart_record` | `project/` |
| 文書の変更を元に戻す履歴にまとめる | `usecase::edit_tracker`(前に確定した状態との比較) | — | `history.rs` の `TrackedDoc`(各プラグインで登録) |
| 実機ロボット (UR) の関節角度の受信 | —(角度から関節の値への直し方 `motion::live_joints` は Bevy 非依存。`motion::chart` と一緒に手順 2 で core へ) | `cadsim_io::rtde`(RTDE の読み書きと通信スレッド) | `motion/ur_ui.rs` |
| 生成 AI の呼び出し | `usecase::llm`(ポート `LlmClient`) | `cadsim_io::claude`(Claude API) | `assistant/`(`settings_file.rs`・`job.rs` は Bevy 非依存) |
| チャート・ラダーの自動生成 | —(`TimingChart` がアプリ側なので `sequence_gen/{spec, ladder, log_events, usecase}.rs` はアプリ側。Bevy 非依存で、チャートと一緒に手順 2 で core へ) | `sequence_gen/json.rs`(下書き ⇔ JSON) | `sequence_gen/{mod, ui}.rs` |
| ライン連携 | `line::{coupler, signals, stepper, workpieces}`、`flow::sim` と `conveyor::sim` の連携の口 | — | `line/`(毎フレームの進め方・3D のワーク・ウィンドウ) |
| 画面での動作確認の自動化 | `ui_check::{scenario, runner, widget, compare, report}` | `cadsim_io::ui_check`(PNG・JSON・HTML) | `ui_check/mod.rs`(入力を流して撮る Bevy のシステム)、`ui_check/suite.rs`(手順書ごとにアプリを起動し直す。Bevy なし) |
| 言葉でラインを組む | `line_draft::{model, check, layout, rules}`(ドメイン)、`usecase::compose_line`(生成 AI のポート `LineDraftAi`) | `cadsim_io::line_draft_json`(AI への指示と返答の JSON) | `line_compose/`(`apply.rs` は Bevy 非依存。`TimingChart` が core に移ったら一緒に移す) |
| 最初の5分 (ようこそ画面・章立てのチュートリアル) | `gamify`(章・ミッションの手順・使う道具 `workspace::Tool`・とばす・判定) | `cadsim_io::preferences_file`(とばしたミッション・ようこそ画面を閉じたか) | `ui/tutorial.rs`(ようこそ画面・カードの手順・矢印)、`ui/playful.rs`、起動時のデモラインは `project/mod.rs` の `ProjectAction::OpenDemoLine` |

`drawing/collect.rs` と `motion::live_joints` は Bevy に依存しない書き方にしてあり、次の手順で core に移します。
