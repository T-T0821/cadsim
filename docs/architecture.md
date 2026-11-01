# コードの構成

plantwright のコードは **機能ごとのフォルダ** に分けます。1つの機能は、同じ名前のフォルダ2つにまとまります。

| 場所 | 置くもの |
| --- | --- |
| `crates/plantwright_core/src/<機能>/` | Bevy・egui を使わないもの全部。データの型、計算、プロジェクトファイルの JSON の形、ファイル形式、通信。単体テストもここ |
| `src/features/<機能>/` | Bevy のプラグイン・システム・3D 表示と、egui の画面 |

たとえば工程シミュレーションを読むなら `crates/plantwright_core/src/flow/` と `src/features/flow/` の2か所だけを開けば足ります。

## 全体の地図

```text
Cargo.toml                 ワークスペース + アプリ (package 名 plantwright)
src/                       アプリ (Bevy・egui)。assets/ の場所を変えないようにルートに置く
├── main.rs                App を作り、土台と機能のプラグインを足すだけ
├── document/              土台: モデルの Resource・選択・元に戻す・開く/保存・取り込み・ファイルのダイアログ
├── viewport/              土台: 3D 表示・カメラ・ギズモ・クリックでの選択・画質・工場の建屋・歩行・ショートカット
├── ui/                    土台: 画面の枠 (メニュー・ツールレール・部品の木・プロパティ・状態表示・テーマ・共通部品・フォント)
├── features/              機能ごと。1 フォルダ = 1 プラグイン
│   ├── mod.rs             FeaturePlugins (機能のプラグインを並べるだけ)
│   └── motion/ plc/ flow/ conveyor/ line/ floor/ safety/ physics/ annotation/ drawing/ site/ ...
└── dev/                   開発用: 負荷テストのシーン、FPS の計測
crates/
└── plantwright_core/      Bevy・egui を使わないもの全部
    └── src/
        ├── document/      Model・Node・形状・BVH・表示用メッシュ・元に戻す履歴
        ├── formats/       機能をまたぐ 3D の形式 (STEP・glTF・OBJ・STL・USD・Web ビューア)
        └── motion/ plc/ flow/ conveyor/ line/ ...   アプリの features/ と同じ名前
```

## 決まり

1. **crate は「Bevy・egui を使うかどうか」だけで分ける。** 計算も JSON・Excel・Modbus も、Bevy を使わなければ `plantwright_core`。この線のおかげで、計算のテストは Bevy をビルドせずに速く回り、アプリを直したときの再コンパイルも小さく済む。
2. **再公開で中継しない。** アプリは `use plantwright_core::flow::sim;` のように実体の場所をそのまま書く。
3. **`Resource` は core の型に直接付ける。** `plantwright_core` は `bevy_ecs`(描画を含まない ECS だけ)に依存してよい。`Resource` を付けるためだけの包み型は作らない。
4. **トレイトは本当に入れ替えるものだけ。** PLC との接続 (Modbus・SLMP・OPC UA)、生成 AI、テストで偽物を差すものなど。実装が1つなら普通の関数・メソッドにする。
5. **機能同士の受け渡しは Bevy の Event か core の型で。** ほかの機能の `systems.rs` を直接 `use` しない。
6. **プラグインは自分の Resource を自分で登録する。**

## 機能フォルダの中のファイル名

| 場所 | ファイル | 中身 |
| --- | --- | --- |
| core | `mod.rs` | 機能の説明 (`//!`) と、外へ出す型の `pub use` だけ |
| | `model.rs` | データの型 |
| | 中身を表す名前 (`sim.rs`・`kinematics.rs` など) | 計算。テストは同じファイルの `#[cfg(test)]` |
| | `record.rs` | プロジェクトファイルの JSON の形 (serde) |
| | 形式の名前 (`xlsx.rs`・`slmp.rs` など) | その機能だけが使うファイル形式・通信 |
| アプリ | `mod.rs` | `XxxPlugin` と、その機能の Resource・Event の登録。ここを読めば何がいつ動くか分かる |
| | `systems.rs` | 毎フレームの Bevy システム |
| | `view.rs` | 3D に置くもの (メッシュ・線・色) |
| | `panel.rs` または `panel/` | egui のウィンドウ。複数あれば `panel/staff.rs` のように分ける |

- 合わないものは無理に作らず、要るファイルだけ置く。
- `ui.rs`・`*_ui.rs`・`usecase.rs`・`domain/`・`adapter/` という名前は使わない。
- 1ファイルはおよそ 800 行まで。超えたら画面の区切りや計算の段階で分ける。

## 移行の状況

前の構成 (Clean Architecture の4層、`cadsim_core` / `cadsim_io` の2つの crate) から、動作を変えずに次の順で移します。

1. ✅ この文書と CLAUDE.md を新しい決まりに書き換える。
2. `cadsim_io` を `cadsim_core` に統合し、`plantwright_core` に改名する。ファイル形式は機能の隣に置く。
3. アプリ側の再公開だけのモジュールと、`Model`・`TimingChart` の包み型をなくす。
4. アプリ側を `document/`・`viewport/`・`ui/`・`features/`・`dev/` に移し、ファイル名をそろえる。
5. 大きすぎるファイルを分け、この文書の地図を実際の形に合わせる。
