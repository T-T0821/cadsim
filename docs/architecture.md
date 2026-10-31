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
│  │    .cadsim の JSON、Modbus TCP、動画のエンコード          │  │
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
| 1. ドメイン | 設計データの型とその規則。純粋な計算 | `glam`(f64 のベクトル・行列)、`truck`(形状カーネル)、標準ライブラリ | Bevy、egui、ファイル形式・通信のライブラリ、ファイルシステム |
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
│       ├── domain/       model, math, selection, remap (形状の差し替えの対応づけ), simulation (幾何・干渉・計測),
│       │                 motion (チャート・姿勢・運動学), plc (シーケンス・I/O 表), flow, conveyor,
│       │                 staffing (作業者の名簿・スキル・シフト・人数の比較),
│       │                 safety (安全機器・検知・安全距離), floor (作業者・AGV・床の幾何),
│       │                 line (工程と AGV の受け渡し・PLC の信号表・ワークの位置)
│       └── usecase/      document (開く・保存・取り込み・書き出し), edit, history,
│                         report (シミュレーション結果の CSV),
│                         ports (SceneReader / SceneWriter / ProjectStore などのトレイト)
└── cadsim_io/            3. アダプター (Bevy なし)
    └── src/              step, interchange (gltf / obj / stl / usd), project_file (JSON),
                          modbus, video, xlsx (工程シミュレーションの結果)
```

ルートの `Cargo.toml` の `default-members` に全 crate を並べているので、引数なしの `cargo test` / `cargo clippy` でワークスペース全体を確かめられます。

## いまのコードからの移し方

機能追加の PR(タイミングチャート、ロボット、PLC、センサー、動画書き出し、搬送など)が落ち着いてから、動作を変えずに次の順で少しずつ移します。
1つの PR は1段だけにし、毎回 `cargo fmt` / `clippy` / `test` が通る状態でマージします。

1. **ワークスペース化と純粋な計算を core へ。** ✅ ルートをワークスペースにして `cadsim_core` を作り、`math` と `simulation` の幾何・干渉の計算を移した。`bevy::math` は同じ版の `glam` に置き換え、アプリ側の `math` / `simulation` から再公開しているので呼び出し側は変わらない。
   - ゲーム風 UI の判定(`gamify`)は最初から `cadsim_core` に置いた。設定ファイルの読み書き(`src/preferences_file.rs`)はアダプター層の候補で、`cadsim_io` ができたら移す。
2. **残りの純粋な計算を core へ。** `motion` のチャート・姿勢・運動学、PLC のシーケンスなど。進行中の機能ブランチが触っているので、それらが入ってから移す。
3. **ファイル形式を io へ。** 🔶 `cadsim_io` を作り、3D ファイル形式の読み書き (`interchange` の Scene・glTF・OBJ・STL・USD・Web ビューア) を移した。アプリ側の `interchange` から再公開しているので呼び出し側は変わらない。`Scene` ⇔ `Model` の変換 (`model_io`) は手順 4 で `Model` が core に移ってから io へ移す。残りは `step_import`、`.cadsim` の JSON の構造体、`formats/`、`flow` の `record.rs`・`xlsx.rs`。
4. **設計データを core へ。** `Model`・`Selection` などから `#[derive(Resource)]` を外し、UI 層の包み型に置き換える。
5. **ユースケースを切り出す。** 開く・保存・取り込み・書き出し・削除・元に戻すを `usecase` の関数にし、Bevy のシステムからはそれを呼ぶだけにする。元に戻す履歴は `World` ではなくドキュメント(モデル + チャートなど)を書き換える形にする。
6. **UI 層を整理する。** 残った Bevy / egui のコードを `view` / `input` / `ui` に分ける。

いまの `src/safety/` と `src/floor/` は、`device.rs`・`check.rs`・`geom.rs`・`operator_plan.rs`・`agv_sim.rs` が Bevy に依存しない層 (2. で core へ移すもの)、`mod.rs`・`view.rs`・`panel.rs` が UI 層です。

新しく足した機能のうち、すでに層を分けてあるもの:

- `src/result_export/report/` … ユースケース層 (CSV の作成)。Bevy の ECS・egui・ファイルシステムに依存しない。移行時は `usecase/report` へ移す。`src/result_export/plugin.rs` は UI 層。
- PLC 連携(`src/plc/`)は先に `domain` / `usecase` / `adapter` / UI に分けてあるので、移行時はディレクトリごと core / io へ移せます。

各段の終わりに、この文書の「目標のディレクトリ構成」と実際の構成の差を更新します。

### すでに層を分けて書いてあるもの

- 工程シミュレーション: ドメイン(工程モデル・離散事象シミュレーション・色)は `cadsim_core::flow` に置いた。アプリの `src/flow/` のうち `record.rs`(.cadsim の形)と `xlsx.rs`(Excel 書き出し)は Bevy に依存しないアダプターで、`cadsim_io` を作る段でそのまま移す。`mod.rs`・`ui.rs` が UI 層。

### 移行の途中で足した機能の置き場所

| 機能 | ドメイン(`cadsim_core`) | アダプター(いまはアプリ側、手順 3 で `cadsim_io` へ。3D ファイル形式は移動済み) | UI(アプリ側) |
| --- | --- | --- | --- |
| 平面図の下敷き・点群 | `site::{underlay, pointcloud, model}` | `formats::{dxf, pdf_plan, pointcloud_read, site_record}` | `site/`(表示・パネル・縮尺合わせ) |
| 2D 図面 | `drawing` | `formats::drawing_out` | `documents.rs` |
| 部品表 | —(`Model` を読むので `bom.rs` はアプリ側。手順 4 で core へ) | `formats::bom_out` | `documents.rs` |
| モデリング | —(`Model` と履歴を書き換えるので `modeling.rs` はアプリ側。手順 4 で core へ) | — | `ui/modeling.rs` |
| 物理演算 | `physics::{settings, cable}`(ドメイン)、`physics::world`(ユースケース。物理エンジンのポート `PhysicsEngine`) | `physics::{rapier, records}`(Rapier と .cadsim の形) | `physics/{mod, panel, sample}` |
| 動画書き出し | `video`(撮る時刻・切り抜き・書き込み先のポート `VideoSink`) | `video::{encode, mp4}`(MP4 / GIF / PNG) | `recording.rs`(画面のキャプチャ・設定ウィンドウ) |
| タイミングチャート・アニメーションの保存 | —(`motion::chart` は手順 2 で core へ) | `formats::chart_record` | `project.rs` |
| 文書の変更を元に戻す履歴にまとめる | `usecase::edit_tracker`(前に確定した状態との比較) | — | `history.rs` の `TrackedDoc`(各プラグインで登録) |
| ライン連携 | `line::{coupler, signals, workpieces}`、`flow::sim` の連携の口 | — | `line/`(`stepper.rs` は Bevy 非依存。AGV のシミュレーションと一緒に手順 2 で core へ) |

`formats/` と `bom.rs`・`modeling.rs`・`drawing/collect.rs` は Bevy に依存しない書き方にしてあり、`Model` が core に移ればそのまま移せます。
