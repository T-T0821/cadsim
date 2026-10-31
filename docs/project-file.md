# プロジェクトファイル(.cadsim)の形式

ファイルメニューの「保存」「名前を付けて保存」で `.cadsim` ファイルに保存し、「開く」で開き直せます。未保存の変更があるとウィンドウのタイトルに `*` が付き、新規作成や別のプロジェクトを開く前に破棄してよいか確認します。起動時に開くサンプルは未保存の変更として扱いません。

中身は人が読める JSON です。保存するのは設計データだけで、STEP の形状(B-rep やメッシュ)は書きません。開くときに `step_files` の STEP を読み直し、部品の `product`(製品名)で形状を引きます。STEP が見つからない・製品がないときは、名前と配置だけのノードとしてツリーに残し、ステータスバーに注意を出します。

```json
{
  "format_version": 1,
  "unit": "mm",
  "step_files": [
    { "type": "File", "path": "parts/vise.step" }
  ],
  "nodes": [
    {
      "name": "SampleVise",
      "position": [0.0, 0.0, 0.0],
      "rotation_deg": [-90.0, 0.0, 0.0],
      "visible": true,
      "kind": { "type": "Assembly", "product": "SampleVise" },
      "children": [
        {
          "name": "BasePlate_1",
          "position": [0.0, 0.0, 0.0],
          "rotation_deg": [0.0, 0.0, 0.0],
          "visible": true,
          "color": [1.0, 0.5, 0.25],
          "kind": { "type": "Imported", "step_file": 0, "product": "BasePlate" }
        }
      ]
    },
    {
      "name": "箱 9",
      "position": [0.0, 0.0, 0.0],
      "rotation_deg": [0.0, 0.0, 0.0],
      "visible": true,
      "kind": { "type": "Box", "size": [1.0, 1.0, 1.0], "position": [150.0, 0.5, 0.0], "rotation_deg": [0.0, 15.0, 0.0] }
    }
  ],
  "annotations": [
    { "type": "EdgeLength", "node": [0, 0], "edge": 3 },
    {
      "type": "FaceDistance",
      "a": { "node": [0, 0], "face": 1 },
      "b": { "node": [0, 1, 0], "face": 4 }
    },
    {
      "type": "Note",
      "at": { "node": [1], "local": [1.0, 1.0, 0.0] },
      "text": "ストッパー位置\n要確認"
    }
  ]
}
```

| 項目 | 内容 |
| --- | --- |
| `format_version` | ファイル形式のバージョン。アプリより新しいファイルは開かずにエラーを出す |
| `unit` | 長さの単位 |
| `step_files` | 取り込んだ STEP。`File` はプロジェクトファイルと同じフォルダ以下なら相対パス(`/` 区切り)、それ以外は絶対パス。`Sample` は埋め込みのサンプル。`Stress { parts }` は負荷テスト用のシーン。`Modeled { name, solid }` はモデリングの画面で作った形で、`solid` に truck の B-rep を JSON 文字列で埋め込む(元のファイルがないため) |
| `nodes` | 最上位のノード。子は `children` に入れ子で書く |
| `position` / `rotation_deg` | 親から見た配置。回転は X → Y → Z の順に回す角度(度) |
| `visible` / `color` | 表示/非表示と、ユーザーが付けた色(sRGB 0〜1、子孫にも効く)。省略すると表示・色なし |
| `kind` | `Assembly`(まとめるだけ)、`Box`(寸法・位置・回転を箱自身が持つ)、`Imported`(`step_files` の番号と製品名)、`Standard`(標準部品。`part` が種類、`params` が寸法パラメータ mm。本体は書かず、開くときにパラメータから作り直す) |
| `physics`(ノード) | 部品の物理の設定。`mode`(`Fixed` / `Mechanism` / `Dynamic` / `Ignored`)、`shape`(`Auto` / `Box` / `ConvexHull` / `ConvexDecomposition` / `Mesh`)、`friction`、`restitution`、`density`(g/cm³)、`belt`(`axis`・`reversed`・`speed`)。既定(機構で動く)なら省く |
| `script`(ノード) | 部品スクリプト(Rhai のソース)。付けていなければ省く |
| `custom_part`(ノード) | 自作部品を置いた一番上のノードに付くしるし。`part`(ライブラリでの部品名)、`prefix`(この部品のアクチュエータ・センサーの名前の頭。例 `"プッシャー 1/"`)、`params`(公開パラメータ。`label`・`target`(`{"type": "Stroke", "actuator": id}` / `Duration` / `LimitMin` / `LimitMax` / `{"type": "SensorRange", "sensor": id}`)・`value`)。自作部品のファイル(`part.cadpart`)については docs/custom-parts.md |
| `cables` | ケーブル・ホース。`name`、`hose`、`start` / `end`(`node` は寸法と同じ番号の道筋、省くとワールド。`local` は部品の座標系かワールドの位置)、`length`、`diameter`、`min_bend_radius`。端の部品が見つからないものは読み飛ばす |
| `annotations` | 寸法と注記。`PointDistance` / `EdgeLength` / `FaceDistance` / `Note`。部品は `node` に「最上位から順にたどる子の番号」で書く(例 `[0, 1, 0]` は1つ目の最上位ノードの2番目の子の1番目の子)。`local` は箱なら半寸法で割った値(-1〜1)、取り込んだ部品なら部品の座標系の位置。`face` / `edge` は部品の面・稜線の番号。寸法がなければ項目ごと省く |
| `flow` | 工程モデル。`duration` / `warmup`(秒)と `seed`、`elements` に要素を並べる。要素は `id`・`name`・`type`(`Source` / `Process` / `Buffer` / `Sink`)と種類ごとの値(`interval` / `cycle_time` / `spread` / `limit` / `mtbf` / `mttr` / `capacity`、時間は秒、`spread` は ± の割合)、送り先の `next`(要素の id)、対応する 3D の部品 `part`(`annotations` と同じ子の番号の並び)、`rated_kw` / `standby_kw` / `air_l_per_min`、ライン連携のつなぎ方 `agv`(送り出しを運ぶ AGV の `from` / `to`。`floor.agv.nodes` の点の id)と `starts_chart`(加工開始でタイミングチャートを頭から再生)、人が要るか `labor`(`{"type": "Full"}` か `{"type": "Load", "seconds": 8}`、要らなければ省く)。`staffing` に作業者の名簿(`start_clock` 始める時刻・`walk_seconds` 歩行時間・`rule` 割り当てのルール(`LongestIdle` / `Primary`、既定の `Fastest` なら省く)、`shifts` に `id`・`name`・`start` / `end`・`breaks`(`[開始, 終了]` の並び)、`workers` に `id`・`name`・`shift`・`absent`・`skills`(`[工程の id, 速さ]` の並び、速さ 1.0 が標準)・`primary` 持ち場の工程の id)。時刻は 0:00 からの秒。名簿がなければ省く。`scenarios` に比べるために取っておいた案(`name` と、`flow` と同じ形の `flow`。案の中に `scenarios` は書かない)、なければ省く。工程も案もなければ項目ごと省く |
| `floor` | 床の配置。`safety` に安全機器(`id`・`name`・`kind`(`LightCurtain` / `AreaSensor` / `Fence` / `EmergencyStop`)・`position` [x, 下端の高さ, z]・`yaw_deg`・`size`・`stop_time`(秒)・`resolution`・`required_clearance`)、`operators` に作業者(`start` [x, z]・`walk_speed` mm/s・`tasks`。手順は `type` が `Walk`(`to`)/ `Pick` / `Button`(`label`・`duration`)/ `Wait`(`duration`))、`agv` に AGV の経路(`nodes`・`edges`(点の id の組)・`flows`(`from` / `to` / `interval`))と性能(`count`・`speed`・`load_time`・`unload_time`・`length`・`width`・`gap`)。長さは mm。何も置いていなければ項目ごと省く |
| `chart` | タイミングチャート。`actuators` にアクチュエータ(`id`・`name`・`kind`(`Cylinder` / `LinearAxis` / `RotaryTable`)・`axis`(`X` / `Y` / `Z`)・`reversed`・`stroke`・`pivot`・載っている親の `mounted_on`(アクチュエータの id)・動かす部品 `parts`(`annotations` と同じ子の番号の並びのリスト)・`easing`(`Linear` / `Smooth` / `Cycloid` / `{"Trapezoid": {"accel": 割合}}` / `{"Cam": カム表の id}`)・`default_duration`・関節の可動範囲 `limits`(`[下限, 上限]`、なければ省く))、`sensors` にセンサー(`id`・`name`・`kind`(`Photoelectric` / `Proximity`)・`position`・`axis`・`reversed`・`range`・載せ先の `mounted_on`。強制 ON / OFF は書かない)、`steps` にステップ(`comment`・`dwell`(秒)・`actions`(`actuator` の id・`target`・`duration`)・開始条件 `interlocks`(`sensor` の id・`on`)・ロボットの移動 `moves`(`robot` の id・`point` の id・`duration`・補間のしかた `kind`(`Linear` のときだけ書く。ないときは関節補間 `Joint`)))、`robots` にロボット(`id`・`name`・先端の関節 `tip`(アクチュエータの id)・設計位置での `tcp` と `tool_axis`・教示点 `points`(`id`・`name`・`position`・工具の向き `approach`)・速度の上限 `joint_speed`(度/秒)・`linear_speed`(モデル単位/秒、自動なら省く)・`accel_time`(秒))、`cams` に電子カム表(`id`・`name`・`points`(`[時間の割合, 位置の割合]` の並び、0〜1))。すべて空なら項目ごと省く |
| `conveyors` | ワーク搬送。`kinds` にワーク種類(`id`・`name`・`size`・`color`)、`sources` に投入口(`id`・`name`・部品 `part`・出す先 `output`(`{ "type": "Conveyor", "conveyor": id }` か `{ "type": "OnPart" }`)・`interval`(秒)・出す種類 `kinds`・`limit`)、`sinks` に排出口(`id`・`name`・`part`)、`conveyors` にコンベア(`id`・`name`・ベルトにする部品 `part`・`axis`(`X` / `Z`)・`reversed`・`speed`・`end`(`Stopper` / `Discharge`))、`grippers` にグリッパ(`id`・`name`・`chuck`(アクチュエータの id)・`tcp`・`reach`)。部品は子の番号の並びで書く。なければ項目ごと省く |
| `animations` | glTF などから取り込んだアニメーション。クリップごとに `name` と `tracks`。トラックは動かすノード `node`(子の番号の並び)と `keys`(`time` 秒・親から見た `position`(モデルの単位)・`rotation`(四元数 [x, y, z, w])・`scale`)。元のファイルを読み直さなくても、ツリーを組み替えたあとの対応のまま戻せるように中身ごと書く。なければ項目ごと省く |
| `plc` | PLC 連携の設定。番号の割り付けの先頭 `output_bits` / `output_words` / `input_bits` / `input_words`、CSV で個別に決めた信号 `custom`(`actuator` の id・`signal`(`CylinderExtend` / `CylinderExtended` / `CylinderRetracted` / `AxisTarget` / `AxisPosition` / `AxisInPosition`)・`address`・`scale`・`node`(OPC UA)・`log_column`(ログ CSV の列))、外部 PLC の通信方式 `use_opcua`(true なら OPC UA、省くと Modbus TCP)と接続先 `modbus`(`host`・`port`・`unit_id`・`poll_ms`)・`opcua`(`endpoint`・`interval_ms`)。接続先は初期値のままなら省く。どれも初期値なら項目ごと省く |
| `underlays` | 床に敷いた平面図。`path`(`step_files` と同じ書き方)、`scale`(図面の 1 がモデルの単位でいくつか)、`origin`(図面の原点を置くワールドの [X, Z])、`rotation_deg`、`elevation`(高さ Y)、`opacity`、`visible`。図面の中身は書かず、開くときに読み直す。なければ項目ごと省く |
| `point_clouds` | 読み込んだ点群。`path`、`offset`(点群の中心を置くワールドの位置)、`rotation_deg`(Y 軸まわり)、`visible` と、読み直すときの設定 `spacing`(最小の点の間隔、モデルの単位)・`budget`(残す点の数の上限)・`unit_mm`(点群の座標 1 が何 mm か)・`z_up`。なければ項目ごと省く |

```json
{
  "underlays": [
    { "path": "plans/floor.dxf", "scale": 1.0, "origin": [0.0, 0.0], "rotation_deg": 0.0, "elevation": 0.0, "opacity": 0.8, "visible": true }
  ],
  "point_clouds": [
    { "path": "scan/building.e57", "offset": [12000.0, 3500.0, -8000.0], "rotation_deg": 0.0, "visible": true, "spacing": 5.0, "budget": 3000000, "unit_mm": 1000.0, "z_up": true }
  ]
}
```

実装は `src/project/`(形は `file.rs`、開く・保存の手順は `document.rs`)、平面図と点群の記録は `crates/cadsim_io/src/formats/site_record.rs`、タイミングチャートとアニメーションの記録は `crates/cadsim_io/src/formats/chart_record.rs` です。ファイル形式の構造体(`ProjectFile` など)は内部の `Model` と分けているので、内部を変えてもファイルの互換を保てます。

- 開く処理(JSON の解釈と STEP の読み直し)は別スレッドで行い、画面は止まりません。
- 工程モデルやタイミングチャートの 3D の部品が見つからないときは、対応付けだけ外して注意を出します。見つからないノードのアニメーションのトラックは読み飛ばします。
- 部品が見つからない寸法・注記は読み飛ばし、件数をステータスバーに出します。部品を削除して隠れている寸法は保存しません。
- 未保存かどうかは、最後に保存した内容と今のモデルから作った内容を比べて判断します。元に戻して保存時と同じ状態になれば、未保存の印も消えます。
- 新規作成やプロジェクトを開いたときは、元に戻す履歴を消します(前のプロジェクトに戻せないように)。「すべて閉じる」は今のプロジェクトのまま中身を空にする操作なので、元に戻せます。
- 新しい項目(タイミングチャートなど)は、`ProjectFile` に `#[serde(default)]` 付きのフィールドとして足します。項目のない古いファイルもそのまま開けます。古いファイルを開けなくなる変更をしたときだけ `FORMAT_VERSION` を上げます。

