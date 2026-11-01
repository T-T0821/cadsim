# 標準部品ライブラリ

シリンダ・コンベア・センサー・シーケンサーなどの標準部品を、寸法パラメータから作って置く機能です。使い方は README の「標準部品を置く」を見てください。

## コードの構成(`src/library/`)

| ファイル | 層 | 内容 |
| --- | --- | --- |
| `catalog.rs` | ドメイン(Bevy なし) | 部品の種類 (`PartKind`)、寸法パラメータ (`ParamSpec` / `Spec`)、I/O 点 (`IoPoint`)、アクチュエータ動作 (`ActuatorSpec` / `Motion`)、センサーの検出範囲 (`SensingSpec`)、コンベアの搬送面 (`ConveyingSurface`) |
| `geometry.rs` | ドメイン(Bevy なし) | パラメータから truck の B-rep を本体ごとに作る。直方体と円柱の組み合わせで、座標は部品ローカルの mm(取付け面の中心が原点、Y が上) |
| `mod.rs` | モデル操作 | モデルの木への追加 (`add_standard`)、本体の作り直し (`rebuild_bodies`)、動作テストの表示位置 (`display_matrix`)、I/O 一覧 (`io_table` / `io_capacity`) |
| `ui.rs` | UI | 部品パレットとドラッグ & ドロップ、「作成 → 標準部品」メニュー、プロパティの「標準部品」欄、I/O 一覧、光軸と搬送方向の表示 |

## モデルでの持ち方

- 部品は `NodeKind::Standard(Spec)` のノード。子に本体 `NodeKind::StandardBody { body, shape }` を持つ。`body` は `geometry::bodies` の並びでの番号。
- 本体の形状は `ImportedShape` と同じ型なので、表示・外接箱・計測・干渉チェック・書き出しは取り込んだ部品と同じ道を通る。
- パラメータを変えると子を作り直す(子のノード id は変わる)。同じ種類・パラメータ・単位の形状はキャッシュして共有するので、同じ部品を何個置いてもメッシュは1つ。
- 寸法パラメータは mm。モデルの単位 (`Model::unit`) が mm 以外なら `units_per_mm` で換算してから形状を作る。
- ビューでクリックした本体は `Model::pick_target` で親の部品に置き換えて選ぶ。
- 干渉チェックでは `SimBody::group` に親の部品の id を入れ、同じ部品の本体どうしは調べない(ロッドとチューブのように組み付いた状態で重なっているため)。
- `.plantwright` には `{"type": "Standard", "part": "air_cylinder", "params": {"bore": 32.0, "stroke": 100.0}}` のように種類とパラメータだけを書き、本体は書かない(開くときに作り直す)。種類のキーは `PartKind::key`。

## ほかの機能からの使い方

| 使う側 | 使うもの |
| --- | --- |
| タイミングチャート | `Spec::actuators()`。`bodies` が動かす本体の番号で、`library::body_node` で本体のノード id に直せる。`drive` がその動作を起こす出力、`end_inputs` が原位置 / 動作端の入力。動きは `library::motion_matrix`(部品ローカルの剛体変換)で求められる |
| センサー | `Spec::sensing()` の光軸(部品ローカル mm)を `library::local_to_world` / `dir_to_world` でワールド座標にする |
| コンベア・搬送 | `Spec::conveying_surface()` が搬送面の中心・向き・長さ・幅・速度を返す |
| 部品表 | 標準部品は部品全体で1行。種類と寸法パラメータが同じものを同じ品物として数え、本体は数えない (`bom::item_key`) |
| PLC の I/O 割付け | `library::io_table` が現場機器の I/O 点、`io_capacity` がシーケンサーとリモート I/O の点数を返す。シーケンサーの端子名は三菱 Q / iQ-R 流の 16 進 (X0..XF, X10..) |

## 部品を増やすには

1. `catalog.rs` の `PartKind` に種類を足し、`ALL`・`label`・`key`・`category`・`param_specs` を埋める。
2. 必要なら `io_points`・`actuators`・`sensing` を足す。アクチュエータが動かす本体の番号は `catalog::body` に定数で置く。
3. `geometry.rs` の `bodies` に形状を足す。`catalog::body` の番号と本体の並びを合わせる(テスト `named_body_indices_match` で確かめる)。

すべての種類について「既定値が範囲内」「閉じたソリッドで床より下に出ない」「アクチュエータの I/O が宣言済み」をテストで確かめているので、足したらそのまま `cargo test` で検査される。
