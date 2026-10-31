# モデリング(円柱・押し出し・穴あけ・ブーリアン)のしくみ

使い方は操作マニュアルの [円柱・押し出し・穴・ブーリアン](src/modeling.md) を見てください。

## 層の分け方

| 層 | 場所 | 役割 |
| --- | --- | --- |
| ドメイン + ユースケース | `crates/cadsim_core/src/modeling.rs` | 形を作る(truck の B-rep)、部品として加える・置き換える、元に戻すための `ModelChange` を返す。B-rep の詰め方はポート `SolidStore` で受け取る |
| アダプター | `crates/cadsim_io/src/modeled.rs` | `SolidStore` の実装 `JsonSolids`(B-rep ⇔ JSON)と、`ModeledSource` から部品を作り直す `ModeledSourceExt` |
| アプリ | `src/modeling.rs` | `JsonSolids` を渡し、返ってきた変更を元に戻す履歴に積むだけ |
| UI | `src/ui/modeling.rs` | 「モデリング」ウィンドウ。入力を集めて `modeling` を呼ぶだけ |

## 形の作り方

形状カーネルは truck(`truck-modeling` と、ブーリアン用の `truck-shapeops`)。

| 操作 | 作り方 |
| --- | --- |
| 円柱 | 1/4 円の円弧4本で底面の円を作り、面を張って +Y に掃引(`tsweep`)。円弧の端が X・Z の端に来るので、メッシュの外接箱が実寸どおりになる |
| 押し出し | (u, v) の多角形を床(Y = 0)に置いて面を張り、+Y に掃引。図面と同じく v は −Z。頂点の回り方はどちらでもよい(面の向きをそろえる) |
| 穴あけ | 指定した軸の向きの円柱を作り、部品から差し引く。深さ 0 は部品を突き抜ける長さにする。深さを指定するときは中心を入口にして、正なら軸の − 向き、負なら + 向きに掘る。面がぴったり重なると演算できないので、入口側は少し外へはみ出させる |
| ブーリアン | `truck_shapeops::or`(和)、`and`(積)、`and` と `not`(差) |

演算の許容誤差は形の大きさの 1/2000 程度にしています(`tolerance_for`)。小さすぎると truck の交線計算が失敗しやすく、大きすぎると形がずれます。

## 部品としての持ち方

作った形は `NodeKind::Imported` の部品にし、`source` を `StepSource::Modeled(ModeledSource)` にします。`ModeledSource` は B-rep を JSON にした文字列を持ち、`.cadsim` の `step_files` に `Modeled { name, solid }` として埋め込みます(元のファイルがないため)。開くときは JSON から B-rep を戻してテッセレーションし直します。

箱に穴をあける・演算するときは、箱を「原点を中心にした形 + 配置」に分けてから演算し、結果を `Modeled` の部品に置き換えます。配置はそのまま残るので、ギズモで動かした位置は変わりません。

## 元に戻す

形の追加は `record_added`、置き換え(穴・ブーリアン)は変更前後のノードを `ModelChange` として履歴に積みます。ブーリアンで消える2つ目の部品も同じ変更に入れるので、Ctrl+Z 1回で両方戻ります。
