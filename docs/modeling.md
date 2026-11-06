# モデリング(円柱・押し出し・穴あけ・ブーリアン・断面・質量)のしくみ

使い方は操作マニュアルの [円柱・押し出し・穴・ブーリアン](src/modeling.md) を見てください。

## 層の分け方

| 層 | 場所 | 役割 |
| --- | --- | --- |
| ドメイン + ユースケース | `crates/plantwright_core/src/modeling/mod.rs` | 形を作る(形状カーネル SMLib の B-rep)、部品として加える・置き換える、元に戻すための `ModelChange` を返す。B-rep の詰め方はポート `SolidStore` で受け取る |
| アダプター | `crates/plantwright_core/src/modeling/record.rs` | `SolidStore` の実装 `JsonSolids`(B-rep ⇔ JSON)と、`ModeledSource` から部品を作り直す `ModeledSourceExt` |
| アプリ | `src/features/modeling/mod.rs` | `JsonSolids` を渡し、返ってきた変更を元に戻す履歴に積むだけ |
| UI | `src/features/modeling/panel.rs` | 「モデリング」ウィンドウ。入力を集めて `modeling` を呼ぶだけ |

## 形の作り方

形状カーネルは NVIDIA の SMLib(`plantwright_core::kernel`。しくみは [internals.md](internals.md) の「形状カーネル」)。

| 操作 | 作り方 |
| --- | --- |
| 円柱 | +Z 向きの円柱(`Brep::cylinder_z`)を作り、X 軸まわりに回して +Y 向きにする |
| 押し出し | (u, v) の多角形を床(Y = 0)に置き、+Y に押し出す(`Brep::extrude`)。図面と同じく v は −Z。頂点の回り方はどちらでもよい(面の向きをそろえる) |
| 穴あけ | 指定した軸の向きの円柱を作り、部品から差し引く。深さ 0 は部品を突き抜ける長さにする。深さを指定するときは中心を入口にして、正なら軸の − 向き、負なら + 向きに掘る。面がぴったり重なると演算できないので、入口側は少し外へはみ出させる |
| ブーリアン | `Brep::boolean` の和・差・積 |

### 押し出しの道具(3D で床に描く)

状態と計算は `plantwright_core::modeling::extrude_tool`(`ExtrudeTool`)、マウスの読み方はアプリの `features/modeling/systems.rs`(`extrude_input`)、線と文字は `view.rs`。

- 頂点はカーソルのレイを床(Y = 0 の平面)に落とした点。閉じる前はカーソルの下の点を次の頂点の候補として線でつなぐ。
- 最初の点をクリック(画面上で近い)か Enter で閉じる。閉じたら高さの初期値を入れる(前回の高さ、初回は形の大きさの半分)。
- 高さの点のドラッグは、形の真ん中を通る縦の線にレイをいちばん近い点で落とす。頂点のドラッグは床に落とす。Ctrl で `GizmoTool` の移動の刻みに丸める。
- 確定(Enter)で `ExtrudeTool::outline()` が上面図の座標(右 +X・上 −Z、頂点の最小の角が原点)と部品の位置を返し、`extrude` → `add_solid` → 履歴に「追加」として積む。
- 道具を使っている間は `PointerLock` を持ち、クリック選択・ギズモ・右クリックメニューを止める。

### 穴あけの道具(3D で面をクリック)

穴の位置・向き・寸法は 3D ビューで決める。状態と計算は `plantwright_core::modeling::hole_tool`(`HoleTool` と置きかけの穴 `HoleDraft`)、マウスの読み方はアプリの `features/modeling/systems.rs`、線と文字は `view.rs`。

- 面をクリックすると、ピッキングが返す面の向き(`PartHit::normal`、面の三角形の法線を面積で重み付けした平均)にいちばん近い軸を穴の軸にし、法線が軸の + 側を向いていれば + 側を入口にする(`HoleDraft::from_face`)。穴あけが軸に平行な穴しか掘れないため。
- 直径の点のドラッグは、カーソルのレイを入口の面(軸に垂直な平面)に落として中心からの距離を直径にする。底の点のドラッグは、レイにいちばん近い軸上の点を深さにする。部品の軸方向の長さ(外接箱)の 99.9% を超えたら貫通(`depth: None`)。
- Ctrl(または常にスナップ)で `GizmoTool` の寸法の刻みに丸める。
- 確定(Enter)で `HoleDraft::spec()` を `drill_hole` に渡す。失敗しても置きかけの穴は残し、エラーをビュー共通の案内バー(`plantwright_core::pointer::ViewHint`、`ui/view_hints.rs`)に赤く出す。
- 道具を使っている間はクリックでの部品の選択とギズモを止める(`hole_tool_off`)。面の強調は `Selection::face` をそのまま使う。

演算の許容誤差は SMLib に任せています(形の大きさから決まる)。

## 形状カーネルならではの操作 (`modeling/ops.rs`)

SMLib の機能を使う操作です。フィレット・面取り・シェルは、下の「丸める・面取り・肉抜きの道具」で画面から使えます。質量特性・断面・修復は下の「調べる機能」で画面に出しています。ほかは core の関数だけがあります。

| 操作 | 関数 | 中身 |
| --- | --- | --- |
| フィレット | `ops::fillet_edges` | 選んだ辺を半径 r で丸める(`Brep::fillet_edges`、円弧の断面) |
| 面取り | `ops::chamfer_edges` | 選んだ辺を、両側の面に沿って同じ長さずつ削る(C 面取り) |
| シェル | `ops::shell` | 内側に厚みを残して中空にする。選んだ面は開ける |
| オフセット | `ops::offset` | 面を外(負なら内)へずらして太らせる・やせさせる |
| フィレットの削除 | `ops::remove_fillets` | 形にある丸みを見つけて消し、角を尖らせる |
| 回転体 | `ops::revolve` | (r, y) の断面を Y 軸まわりに回す。360° 未満なら両端にふたをする |
| パイプ | `ops::pipe` | 通る点を結んだ線に沿った管。4点以上はなめらかな曲線でつなぐ |
| スイープ | `ops::sweep` | 断面を平面上の折れ線に沿って動かす。断面は道筋の始まりの向きに垂直に置く |
| 質量特性 | `ops::mass_properties` | 体積・表面積・質量・重心・重心まわりの慣性モーメントと慣性乗積(ワールド座標)。STEP の部品も対象。複数の本体は平行軸の定理でまとめる |
| 断面 | `ops::section` | 平面で切った断面の線(ワールド座標のポリライン) |

- 作り変える操作は穴あけと同じく、箱とここで作った部品が対象で、元に戻すための `ModelChange` を返す。失敗したら部品は元のまま。どの操作も `modify` → `replace_shape` を通り、そこで形の編集履歴に記録される(次の節)。
- 辺と面の番号は、部品の表示メッシュ(`ImportedShape::edges` / `faces`)の番号。表示では点が1つしかない辺やメッシュにできなかった面を飛ばしているので、カーネルの番号に直してから渡す。
- STEP の読み込みでは、縫い合わせても閉じなかったシェルに形状の修復(`Brep::heal`)をかけ、だめなら縫い合わせの許容誤差を 10 倍ずつ(形の大きさの 1% まで)広げて縫い直す。閉じたときだけその形を使い、結果を `ImportedShape::repair`(`ShapeRepair { healed, open }`)に数えておく。

## 調べる機能(質量・断面・修復の案内)

使い方は操作マニュアルの [質量と重心を見る](src/modeling.md#質量と重心を見る) ほか。どれもパネルを開かずに、部品を選ぶかプロパティの「調べる」から使う。

| 機能 | core | アプリ |
| --- | --- | --- |
| 質量・重心 | `modeling::mass`(`MassCache`・`MATERIALS`) | プロパティの「質量・重心」(`features/modeling/panel.rs` の `mass_section`)、重心の球(`view::draw_centroid`) |
| 断面 | `modeling::section_tool`(`SectionTool`・`section_lines`) | マウス(`systems::section_input`)、平面・切り口・矢印(`view::draw_section`)、プロパティの向きと位置の欄(`panel::inspect_actions`) |
| 修復の案内 | `modeling::repair`(`RepairReport`・`RepairNotice`) | ステータスバーの案内(`ui/status.rs`)、ツリーの印(`ui/tree.rs`)、プロパティの説明(`panel::repair_section`) |

- **質量。** 密度は部品の物理設定(`PartPhysics::density`, g/cm³)をそのまま使い、材質のドロップダウンはその値を書きかえる。モデルの単位から kg/(単位)³ に直して `Brep::mass_properties` に渡し、慣性モーメントは kg·m² にする。プロパティは毎フレーム引くので、部品ごとの結果を「形(箱の寸法か `ImportedShape::id`)・密度・ワールド行列の 3×3 部分」を鍵に覚えておき、平行移動では計算し直さず重心だけずらす。アセンブリは部品ごとの値を平行軸の定理でまとめる(`ops::combine`)。閉じていない部品は計算に入れず、名前を返す。
- **断面。** 平面は「対象の外接箱の中心 `origin`・法線 `normal`・中心からの距離 `offset`」で持ち、`offset` は外接箱の角を法線に投影した範囲に収める。矢印の先のドラッグは、カーソルのレイにいちばん近い法線上の点から矢印の長さを引いた距離にする。面のクリックはピッキングの点と法線(`PartHit`)で平面を合わせる。切り口は平面と部品の形が変わったときだけ(鍵のハッシュ)、外接箱が平面にかかる表示中の部品について `ops::section` で作り直す。道具を使っている間はクリックでの選択とギズモを止める(`hole_tool_off` が断面の道具も見る)。
- **修復の案内。** 読み込みが終わると `ImportJobs::imported` にルートが入り、`systems::repair_notice` が `RepairReport::collect` で直した部品・閉じなかった部品を集めて `RepairNotice` に置く。案内を押すたびに `RepairNotice::next_part` で閉じなかった部品から順に選ぶ。

## 形の編集履歴 (`modeling/features.rs`)

穴あけ・ブーリアン・フィレット・面取り・シェル・オフセット・フィレットの削除は、できた B-rep と一緒に「何をしたか」を `ShapeHistory` として部品に残します。記録する場所は `replace_shape` の 1 か所で、`drill_hole`・`apply_boolean`・`ops::modify` はそれぞれ `Feature` を 1 つ作って履歴の末尾に足し、`Feature::apply` で形を作ります(直接カーネルを呼ばない。あとから作り直すときと同じ道を通すため)。

| 型 | 中身 |
| --- | --- |
| `Base` | 出発点の形。`Box { size }`(原点を中心にした箱。`normalize_box` のあとの箱と同じ)か `Solid { smlib }`(円柱・押し出しなど、最初の編集をかけたときの形をそのまま覚える) |
| `Feature` | 1 つの編集。`Hole`(部品座標の始点・向き・長さ・直径)、`Fillet` / `Chamfer`(辺の印と半径・大きさ)、`Shell`(開ける面の印と厚み)、`Offset`、`RemoveFillets`、`Boolean`(部品座標に置いた相手の SMLib テキストと演算) |
| `EdgeKey` | 辺の印: 部品座標の中点(長さの半分の位置)と長さ |
| `FaceKey` | 面の印: 面積で重み付けした中心と法線 |
| `ShapeHistory` | `base`(無ければ作り直せない)と `features` の列。`rebuild` で出発点から順にやり直す |

- 辺・面を番号で覚えると前の編集を変えたときにずれるので、印で覚えて、やり直すとき(`resolve_edges` / `resolve_faces`)に今の形のテッセレーションから最寄りの辺・面を選ぶ。形の大きさの 5% より遠ければ「形が変わりすぎた」としてエラー。
- あとから直す・消すのは `edit_feature` / `remove_feature` / `edit_base`(core)。履歴を書き換えて `rebuild` し、`replace_shape` で置き換える。失敗したら部品は元のまま。
- 画面はプロパティの「形の編集履歴」(`ui/properties.rs` の `shape_history_section`)。数値はドラッグを離した・入力を終えたときだけ確定する(`widgets::committed_value`。作り直しはブーリアンを含むので毎フレームは重い)。
- 記録を持たない部品(取り込んだ形・この機能より前に保存したファイル)は履歴が空で、画面にも出ない。次に編集をかけたときに、その時の形を `Base::Solid` として覚え始める。

### 丸める・面取り・肉抜きの道具(3D で辺・面をクリック)

状態と計算は `plantwright_core::modeling::shape_tool`(`ShapeTool`・道具の種類 `ShapeOp`・部品の辺と面の形 `PartGeometry`)、マウスの読み方は `features/modeling/systems.rs`(`shape_input`)、線・プレビュー・文字は `view.rs`。

- `PartGeometry::of` は部品の表示メッシュの辺と面(箱は表示と同じ許容誤差でテッセレーションしたもの)をワールド座標にして持つ。辺・面の番号は表示メッシュの番号なので、そのまま `ops` に渡せる。部品が変わったときだけ取り直す。
- 辺のホバー(`PartGeometry::pick_edge`): 辺の線分を画面に投影し、カーソルから 6 px 以内でいちばん近い辺。ほぼ同じ距離なら手前の辺。カーソルの下の面の点(ピッキングの `PartHit::point`)より奥にある辺は隠れているとみなして選ばない。
- 面をクリックしたときの「周りの辺」(`PartGeometry::face_edges`)は、辺の点がすべてその面の三角形の上(部品の大きさの 0.2% 以内)にある辺。面の外接箱で先にふるい落とす。
- 矢印(`ShapeTool::arrow`): 辺の道具は、最初に選んだ辺の長さの真ん中から、部品の中心へ向かう向き(辺に垂直な成分)。肉抜きは、最初に選んだ面の中心から面の法線の逆向き。面がなければ外接箱の上面の中心から下向き。ドラッグはカーソルのレイにいちばん近い矢印上の点までの長さを値にする。
- プレビュー(`shape_tool::preview_solid`): モデルを変えずに部品の B-rep に操作をかけ、テッセレーションして半透明のメッシュで出す。ドラッグ中は 0.12 秒おきに作り直す。出している間は元の部品の面を隠す(`hide_previewed_part`、稜線は残す)。失敗したらその理由を案内の欄に赤く出す。
- 確定(Enter)で `shape_tool::apply` → `ops::fillet_edges` / `chamfer_edges` / `shell` を呼び、変更を履歴に積む(編集履歴にも残る)。プレビューも確定と同じ `ops::*_feature` で編集を作り、`Feature::apply` で形にする。道具を使っている間は `PointerLock` を持ち、クリックでの部品の選択・ギズモ・右クリックメニューを止める。案内は共通の案内バー(`ui::view_hints::gather_hints`、穴あけの次の優先度)。

## 部品としての持ち方

作った形は `NodeKind::Imported` の部品にし、`source` を `StepSource::Modeled(ModeledSource)` にします。`ModeledSource` は B-rep を JSON にした文字列(`{"smlib": SMLib のテキスト形式, "base": 出発点, "features": [編集, …]}`。`base` と `features` は形の編集履歴で、無いこともある)を持ち、`.plantwright` の `step_files` に `Modeled { name, solid }` として埋め込みます(元のファイルがないため)。開くときは JSON から B-rep を戻してテッセレーションし直します。形状カーネルを替える前に保存したファイル(truck の B-rep をそのまま JSON にしたもの)も、面を1つずつ組み立て直して読めます。

箱に穴をあける・演算するときは、箱を「原点を中心にした形 + 配置」に分けてから演算し、結果を `Modeled` の部品に置き換えます。配置はそのまま残るので、ギズモで動かした位置は変わりません。

## 元に戻す

形の追加は `record_added`、置き換え(穴・ブーリアン・フィレット・編集履歴の直し)は変更前後のノードを `ModelChange` として履歴に積みます。ブーリアンで消える2つ目の部品も同じ変更に入れるので、Ctrl+Z 1回で両方戻ります。
