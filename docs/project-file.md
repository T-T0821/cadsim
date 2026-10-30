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
  ]
}
```

| 項目 | 内容 |
| --- | --- |
| `format_version` | ファイル形式のバージョン。アプリより新しいファイルは開かずにエラーを出す |
| `unit` | 長さの単位 |
| `step_files` | 取り込んだ STEP。`File` はプロジェクトファイルと同じフォルダ以下なら相対パス(`/` 区切り)、それ以外は絶対パス。`Sample` は埋め込みのサンプル |
| `nodes` | 最上位のノード。子は `children` に入れ子で書く |
| `position` / `rotation_deg` | 親から見た配置。回転は X → Y → Z の順に回す角度(度) |
| `visible` / `color` | 表示/非表示と、ユーザーが付けた色(sRGB 0〜1、子孫にも効く)。省略すると表示・色なし |
| `kind` | `Assembly`(まとめるだけ)、`Box`(寸法・位置・回転を箱自身が持つ)、`Imported`(`step_files` の番号と製品名) |

実装は `src/project.rs` です。ファイル形式の構造体(`ProjectFile` など)は内部の `Model` と分けているので、内部を変えてもファイルの互換を保てます。

- 開く処理(JSON の解釈と STEP の読み直し)は別スレッドで行い、画面は止まりません。
- 未保存かどうかは、最後に保存した内容と今のモデルから作った内容を比べて判断します。元に戻して保存時と同じ状態になれば、未保存の印も消えます。
- 新規作成やプロジェクトを開いたときは、元に戻す履歴を消します(前のプロジェクトに戻せないように)。「すべて閉じる」は今のプロジェクトのまま中身を空にする操作なので、元に戻せます。
- 新しい項目(タイミングチャートなど)は、`ProjectFile` に `#[serde(default)]` 付きのフィールドとして足します。項目のない古いファイルもそのまま開けます。古いファイルを開けなくなる変更をしたときだけ `FORMAT_VERSION` を上げます。

