# plantwright の内部のしくみ

開発者向けの資料です。使い方は [README](../README.md) を見てください。
コードの構成 (機能ごとのフォルダ) は [architecture.md](architecture.md) にまとめています。

## 動作確認したバージョン

| crate | バージョン |
| --- | --- |
| Rust | 1.97.1 以上(依存crateの要件) |
| bevy | 0.20 |
| bevy_egui | 0.43 |
| SMLib(NVIDIA の形状カーネル usd-brep。同梱の C++ ソースをビルド) | 1.0.1 |
| ruststep(STEP の構文の読み込み) | 0.4 |
| cc(SMLib のビルド) | 1 |
| transform-gizmo | 0.11 |
| rfd(ファイル選択ダイアログ) | 0.17 |
| serde / serde_json(プロジェクトファイル、glTF の書き出し) | 1 |
| gltf(glTF / GLB の読み込み) | 1.4 |
| tobj(OBJ の読み込み) | 4 |
| stl_io(STL の読み書き) | 0.11 |
| openusd(OpenUSD の読み書き。純 Rust) | 0.8 |
| rhai(部品スクリプト。純 Rust) | 1 |
| glam(f64 のベクトル・行列。bevy_math と同じ版) | 0.33 |
| lopdf(PDF の平面図の読み込み) | 0.46 |
| image(PNG / JPEG の平面図) | 0.25 |
| e57(E57 の点群の読み込み) | 0.11 |
| las(LAS / LAZ の点群の読み込み。`laz` 機能) | 0.11 |
| csv(CSV / TSV の読み書き) | 1 |
| parry3d-f64(干渉チェックとクリック選択の検索木 `Bvh`) | 0.31 |
| krilla(図面の PDF 書き出し) | 0.8 |
| rust_xlsxwriter(部品表・工程シミュレーションの Excel 書き出し) | 0.102 |
| bevy_rapier3d(物理演算。Bevy の物理プラグイン、中身は Rapier 0.36) | 0.37 |
| glam(Bevy に依存しない層のベクトル・行列。Bevy と同じ版) | 0.33 |
| openh264(動画書き出しの H.264。同梱の C++ ソースをビルド) | 0.9 |
| gif / image(動画書き出しの GIF・PNG) | 0.14 / 0.25 |

## コードの構成

機能ごとのフォルダの分け方とファイル名の決まりは [architecture.md](architecture.md) を見てください。

```text
crates/plantwright_core/src/   Bevy の描画・egui を使わないもの全部 (cargo test -p plantwright_core で単体テスト)
├── model.rs / shape.rs / math.rs   設計データ (アセンブリの木, f64)・取り込んだ形状・回転と行列の計算
├── bvh.rs / part_mesh.rs           外接箱の木 (parry3d-f64 の Bvh)・部品1つ分の表示用メッシュ
├── history/                        元に戻す履歴の山・モデルの変更・文書の変更の見張り (edit_tracker)
├── formats/                        機能をまたぐ 3D の形式: STEP・glTF・OBJ・STL・USD・Web ビューア・アセットの鍵・CSV の文字
├── preferences.rs / render_settings.rs  設定ファイル (preferences.json / render.json) の読み書き
└── motion/ plc/ flow/ conveyor/ line/ floor/ safety/ physics/ site/ drawing/ bom/ modeling/ ...
                                    機能ごと。アプリの src/features/ と同じ名前
src/                                アプリ (Bevy + egui)
├── main.rs                         App を作り、プラグインを足すだけ
├── document/                       設計データの Resource・選択・元に戻す・開く/保存 (project/)・取り込みと書き出し (import.rs)・ダイアログ
├── viewport/                       3D 表示 (parts.rs: メッシュ・選択の強調・LOD)、クリック選択 (picking.rs)、カメラ、ギズモ、画質、工場の建屋、歩行、ショートカット
├── ui/                             画面の枠: 割り付け (mod.rs)・メニュー・部品の木・プロパティ・状態表示・テーマ・ゲーム風 UI・チュートリアル・フォント
├── features/<機能>/                 機能ごとのプラグイン (mod.rs)・毎フレームの処理・3D 表示 (view.rs)・画面 (panel.rs / panel/)
└── dev/                            負荷テストのシーン (--stress)・FPS の計測 (--bench)・サンプルファイルの作り直し
```

データは一方向に流れます: **UI・ギズモ・読み込み → `Model`(f64)→ `viewport` が Bevy のエンティティに写す**。
動作シミュレーションは `Model` を書き換えず、`viewport` が配置を写すときに `SimulationPose` の動きを重ねます。物理演算で動いている部品は `PhysicsPose` の位置に置きます(`physics::displayed_matrix`)。
`Model` は Bevy のエンティティを知らないので、表示を変えても設計データには影響しません。

## 各機能のしくみ

### 形状カーネル(`plantwright_core/src/kernel/`)

形状の B-rep・ブーリアン・テッセレーションは NVIDIA の SMLib(usd-brep、Apache-2.0)を使う。

1. C++ のソースは `crates/plantwright_core/vendor/smlib/` に手を加えずに置き、`build.rs` が `cc` で静的ライブラリにしてリンクする(C++17 のコンパイラが要る。TBB は使わない)。
2. Rust から呼ぶための薄い C の層が `cpp/kernel.cpp`、その宣言が `kernel/ffi.rs`。Rust 側は `Brep`(`kernel/brep.rs`)のメソッドだけを使う: 直方体・円柱・押し出し、移動・回転・拡大、和・差・積、テッセレーション、テキスト形式での保存と読み込み、面からの組み立て(`Brep::from_faces`)。
3. SMLib は1つの共有の状態で動き、同時に複数のスレッドから呼べないので、呼び出しはすべて1つのロックで順番にする。
4. テッセレーションは面ごとのメッシュ(頂点・曲面の正確な法線・三角形)と、辺ごとのポリラインを返す(`MeshedBrep`)。面の縁の頂点と辺のポリラインの点は同じ座標になる。
5. 形を作り変える操作(フィレット・面取り・シェル・オフセット・回転・パイプ・スイープ・修復)と、質量特性・断面は `kernel/ops.rs` と `cpp/ops.cpp`。どれも元の形は変えずに複製に対して行う。辺と面の番号は `SmApiGetEdges` / `SmApiGetFaces` の並びで、テッセレーションの番号と同じ。モデリング操作としての使い方は [modeling.md](modeling.md)。
6. 面からの組み立ては、面ごとに曲面を輪郭の曲線で切り取り(`TrimSurfaceWithModelSpaceCurves`)、縫い合わせて閉じていればソリッドにする。STEP の読み込みと、以前の truck の形式で保存したモデリングの形状の読み込みに使う。

### STEP の読み込み(`plantwright_core/src/formats/step/`)

1. `ruststep` で STEP をパースし、`#id` からレコードを引ける表を作る。
2. `PRODUCT_DEFINITION` を製品、`NEXT_ASSEMBLY_USAGE_OCCURRENCE` を親子関係として木を組む。配置は `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION` → `ITEM_DEFINED_TRANSFORMATION` の2つの `AXIS2_PLACEMENT_3D` から「子 → 親」の行列を求める。
3. 部品の形状(`MANIFOLD_SOLID_BREP` などのシェル)は面ごとに曲面(平面・円柱・円錐・球・トーラス・B スプライン・回転面・押し出し面)と輪郭の辺(直線・円・楕円・B スプライン・折れ線)を読み(`step/brep.rs`)、形状カーネルで B-rep に組み立てて、面ごとのメッシュと稜線にする。輪郭は FACE_BOUND と ORIENTED_EDGE の向きを反映し、円柱などの継ぎ目の辺は外して別々の輪郭にする。許容誤差は部品の大きさ(と一番薄い方向の寸法)に合わせる。形状カーネルは同時に1つしか動かないので、部品は順に処理する。
4. 同じ製品の形状は `Arc` で共有し、Bevy 側のメッシュも形状ごとに1回だけ作る。
5. STEP は Z 軸が上なので、ルートの配置に「Z 上 → Y 上」の回転を入れて Bevy の座標に合わせる。

構成が書かれていない STEP(シェルだけのもの)は、シェルを1つずつ部品として並べます。読めなかった形状はスキップしてステータスバーに件数を出します。

### 3D ファイルの読み書き(`interchange/`)

各形式はアプリの `Model` を直接触らず、形式に依存しない中間表現 `Scene` とだけやり取りします。

```text
読み込み: ファイル ─(gltf.rs など)→ Scene ─(model_io.rs)→ Model
書き出し: Model ─(model_io.rs)→ Scene ─(gltf.rs など)→ ファイル
```

1. `Scene` はノードの並び(名前・親・親から見た移動 / 回転 / 拡大・三角形メッシュ・色)とアニメーション(ノードごとの「時刻 → 姿勢」のキー)。親は必ず子より前に並ぶ。
2. `Scene` は常にメートル・Y 軸が上で持つ。glTF はそのまま、OBJ / STL は「OBJ / STL の単位」の設定で換算する。`Model` はファイルの単位(STEP なら多くは mm)で持つので、`model_io.rs` が位置とメッシュを換算する。
3. 取り込んだメッシュは STEP の部品と同じ `ImportedShape`(B-rep なし)になる。稜線は B-rep がないので、隣の面と 30° 以上折れている辺と穴の縁を描く。
4. 書き出しでは、箱は形状カーネルでテッセレーションし、STEP の部品は表示用のメッシュをそのまま使う。glTF には形状ごとのメッシュ・色ごとのマテリアル・ノードの階層・アニメーション(線形補間の移動 / 回転 / 拡大)を書く。OBJ は部品ごとにワールド座標へ焼き込んだ `o`、STL は全体を1つの形状にまとめる。
5. glTF のアニメーションは、移動・回転・拡大の別々のチャンネルをノードごとの姿勢のキーにまとめて取り込む(Step はそのまま、CubicSpline は値だけを使って線形で近似)。再生中は `animation.rs` が毎フレームそのノードの配置を書き換える。
6. プロジェクト(`.plantwright`)には取り込んだファイルの場所を保存し、開くときにこの経路で読み直す。
7. 「開く」やドラッグ&ドロップで取り込むファイルのうち、ファイル 1 つで読める形式 (STEP / glTF / OBJ / STL / USD) は Bevy のアセットとして読む (`scene_asset.rs`)。下の「アセットとして読む・更新で読み直す」を参照。

書き出した glb は Khronos の glTF Validator でエラー・警告なしを確認しています。

**新しい形式の足し方**: `crates/plantwright_core/src/interchange/xxx.rs` に `import(&Path, &UnitOptions) -> Result<Scene, String>` と `export(&Scene, &Path, &UnitOptions) -> Result<(), String>` を書き、`interchange/mod.rs` の `FORMATS` に1行足します。開くダイアログ・ドラッグ&ドロップ・書き出しメニューに自動で出ます。

| glTF のアニメーションを再生(干渉チェックも追従) | OBJ に書き出したサンプルを取り込み直したところ |
| --- | --- |
| ![glTF アニメーション](screenshots/10_gltf_animation.png) | ![OBJ の取り込み](screenshots/11_obj_import.png) |

![書き出しメニュー](screenshots/12_export_menu.png)

動作確認用のサンプル `assets/sample_robot.glb`(階層とアニメーション付きのロボット)は `cargo test write_samples -- --ignored` で作り直せます。同時に STEP のサンプルを glb / gltf / obj / stl / usda / usdz に、ロボットを usda / usdz に書き出したものが `target/samples/` にできます。

いまの制限: テクスチャ・スキン・モーフターゲットは読み飛ばします。プロジェクトに保存する配置は移動と回転だけなので、glTF ノードの拡大縮小は保存すると失われます。プロジェクトを開き直すとき OBJ / STL は既定の単位(mm・Y 軸が上)で読み直します。

### アセットとして読む・更新で読み直す(`scene_asset.rs`)

取り込むファイルの読み込みは Bevy のアセットサーバーに任せます。モデルの形にする中身 (`file_io::load_source`) は Bevy に依存しないので、ここは「どのファイルを、いつ読むか」だけです。

1. Bevy のアセットは決まったフォルダの中の相対パスでしか呼べないので、専用の読み元 `plantwright-file://` を `AssetPlugin` より前に登録し (`SceneSourcePlugin`)、ファイルの絶対パスと単位を `plantwright_core::formats::asset_key` で相対パスの形 (`<単位>/<根>/<残りのパス>`) に詰めて呼ぶ。OBJ / STL は単位を鍵に入れるので、単位を変えて読むと別のアセットになる。`..` を含むなど鍵にできないパスは、これまでどおり別スレッドで読む。
2. 読み元はファイルがあるかだけを確かめ、読み込み (`SceneFileLoader`) が鍵からパスに戻して `load_source` を呼ぶ (glTF の .bin や OBJ の .mtl も一緒に読むため)。重い変換は `AsyncComputeTaskPool` で行う。読み込みは拡張子を名乗らず型で選ぶので、Bevy の glTF の読み込みとは取り合わない。
3. 読み終わったアセット (`SceneFile`) から中身を 1 回だけ取り出し (形状は複製できない)、別スレッドの結果と同じ `ImportJobs` の受け口に渡す。モデルへの追加・形状の差し替え・メッセージは `file_io::receive_imports` がする。同じファイルをもう一度開くと、取り出し済みなので `AssetServer::reload` で読み直す。
4. 0.7 秒ごとに、モデルの部品の読み込み元 (`Model::file_sources`) の更新時刻と大きさを調べる。判断は `plantwright_core::file_watch::FileWatch` で、印が変わったあと次に調べたときも同じ (書き込みが落ち着いた) なら読み直す。プロジェクトから開いたファイルも見張る。
5. 読み直したら、そのファイルから取り込んだ木 (`Model::roots_from_file`: 中の部品がすべてそのファイルのもので、親がそうでないノード) を `Model::replace_geometry` で差し替える。「設計変更後の形状に差し替える」と同じ対応づけなので、動きの設定は残る。差し替えは 1 回の「元に戻す」で戻せる。取り込んだ木に別の部品を足したものは、木の一部だけを差し替えることになるので読み直さない。読めなかったら前の形状のまま、メッセージを出す。

いまの制限: 見張るのは取り込んだファイルそのものだけで、glTF の .bin や OBJ の .mtl だけを書き換えても読み直さない。URDF は複数のファイルを読むので別スレッドのまま。

### OpenUSD の読み書き(`plantwright_core/src/formats/usd.rs`)

純 Rust の [`openusd`](https://crates.io/crates/openusd) crate を使い、C++ の USD ライブラリには依存しません。

**書き出し**は USDA のテキストを組み立て、`.usdc` / `.usd` はそれを `openusd` でバイナリ(Crate)形式に、`.usdz` はさらにパッケージにします。

```text
/World (Xform, kind = assembly)          defaultPrim。metersPerUnit = 1、upAxis = "Y"
├── Node_0 (Xform, displayName = "ライン 1")
│   ├── xformOp:translate / orient / scale    動く部品は同じ属性の timeSamples (60 タイムコード / 秒)
│   ├── Geom (Mesh)                           三角形、faceVarying 法線 (30° で角を立てる)、extent
│   └── …子のノード
└── Looks (Scope)                         色ごとの Material + UsdPreviewSurface
```

- プリム名は ASCII の識別子にし(日本語名は `Node_番号`)、元の名前は `displayName` に入れるので Omniverse の画面には日本語名が出ます。
- アニメーションは1本のタイムラインになるので、複数のクリップがあるときはノードごとに最初に見つかったものを書きます。タイミングチャートの動きは先頭に入れるので、取り込んだアニメーションより優先されます。

**読み込み**は `openusd` の Stage で合成(サブレイヤー・リファレンス・ペイロード・バリアント・インスタンス)した結果をたどります。

- `xformOpOrder` の translate / scale / orient / rotateX・Y・Z / rotateXYZ など6通り / transform(行列)/ `!invert!` を評価し、時刻サンプルのある時刻ごとの姿勢をアニメーションのキーにします(時刻は `startTimeCode` からの秒)。
- 形状は Mesh(多角形は扇形に三角形分割、leftHanded は向きを反転)と Cube / Sphere / Cylinder / Cone / Capsule(テッセレーションして取り込み)。
- 色はバインドされたマテリアルの `inputs:diffuseColor`(祖先へのバインドもたどる)、なければ `primvars:displayColor`。
- 変換も子もない形状が Xform の唯一の中身なら(plantwright の `Geom` も)、その Xform の部品として1つにまとめます。形状のない枝(マテリアルの Scope など)は取り込みません。
- 非アクティブなプリム、`purpose` が guide / proxy のもの、カメラ・ライト・マテリアルは読み飛ばします。

確認: 書き出した .usda / .usdz を Pixar の USD(usd-core 26)で開き、全バリデーターでエラーがないこと、マテリアルのバインドと時刻ごとの姿勢が正しいことを確かめています。逆に Pixar の USD で作った .usdc(Z 上・mm・アニメーション・Cylinder・マテリアル付き)も読み込めることを確認しています。

いまの制限(読み込み時に無視するもの): NURBS 曲面・曲線・点群・PointInstancer、スキニング・ブレンドシェイプ・頂点アニメーション(points の timeSamples)、テクスチャと UsdPreviewSurface 以外のシェーダー、GeomSubset ごとのマテリアル。シアーを含む変換は移動・回転・拡大に近似します。

### 表示と選択(`view/viewport.rs`)

1. 部品1つにつき Bevy のエンティティを2つだけ作る。面をすべてまとめた1つのメッシュ(`PartView`)と、子の稜線のメッシュ(`EdgeLines`)。どの三角形がどの面かは `PartMesh::triangle_face` が持つ。
2. 配置・表示/非表示・色・強調は、`Model`・選択・表示設定・干渉の結果・動作シミュレーションのどれかが変わったフレームだけ、木を1回たどって(`Model::visit_parts`)写す。形状が変わったとき(`dirty`)だけメッシュを作り直す。
3. カーソルの下の部品と面は `PartPicker` で求める。部品の外接箱の BVH → 部品内の三角形の BVH の順に調べる(`plantwright_core::bvh`)。外接箱の BVH は部品の配置・表示・形が変わったフレームだけ作り直す。これを Bevy の picking (`bevy_picking`) の backend にしていて(`view/picking.rs`)、当たった部品と面は `HitData` の `extra` に `PartHit` として載せる。ホバーと押したイベント、egui の画面の下では当たりを止めるのは Bevy と bevy_egui に任せる。クリック選択は `PointerPress` を、計測・寸法・教示点・レビューの指さしは `PartUnderPointer`(Bevy の `HoverMap`)を読む。Bevy 標準の `MeshPickingPlugin` は部品を1つずつ調べて数千部品では重く、簡略表示の粗いメッシュに当たるので使わない。
   ダブルクリック(0.4 秒以内の 2 回目の押下)でその部品を画面いっぱいに、右ボタンを押して動かさずに離すと部品のメニュー(`ui/context_menu.rs`)、カーソルが部品の上で 0.35 秒止まると部品名を出す。判定は `plantwright_core::pointer`(`ClickWatch`・`ViewOverlay`)。3D でクリックを使う道具は `PointerLock` を `hold` して、クリック選択・ギズモ・右クリックメニューを止める。道具の「次にすること」の案内は `ViewHint` に書き、`ui/view_hints.rs` がビューの下に同じ見た目で描く。
4. マテリアルは「選択(アセンブリなら子孫すべて)→ 青」「ツリーでホバー → 水色」「それ以外 → 部品の色」で決める。クリックした面は、その面だけのメッシュを重ねてオレンジにする。
5. 稜線は線のメッシュで GPU に描かせる。グリッドと選択の外接箱は Gizmos で描く。グリッドの間隔はモデルの大きさに合わせて 1, 10, 100… と変わる。
6. 部品のエンティティには干渉チェック用の `SimBody`(部品座標の f64 三角形メッシュ + 姿勢 + モデル上で表示中か)も付ける。姿勢と表示/非表示は 2. と一緒に写し、`src/features/simulation/` がこれを拾って判定する。干渉中の部品は半透明の赤にする。
7. 画面上で小さい部品は粗いメッシュに替え、ごく小さい部品は描かない(`update_level_of_detail`)。大規模アセンブリ向けの工夫は [performance.md](performance.md) にまとめている。
8. 「小さな穴とフィレットを省略」がオンなら、取り込んだ形状ごとに裏のスレッドで簡略化した形を作り、できたものから表示を差し替える(`defeature_shapes`)。しくみは [defeature.md](defeature.md)。

### 背景の工場の建屋(`plantwright_core::factory_hall`、`view/factory_hall.rs`)

装置の配置をイメージしやすいよう、3D ビューの背景に工場の建屋を置く。設計データではないので `Model` には入れず、保存・干渉チェック・書き出しの対象にもならない。

1. 形はドメインの `factory_hall::layout(外接箱 (mm), 設定)` が決める。床の長方形・床の高さ・グリッド・床の線(壁際から 歩行者通路 0.6〜1.6 m、黄線、幅 3 m のフォークリフト通路、黄線)・壁の板(腰壁・上の壁・高窓)・柱を返す。長さは mm で、モデルの単位が m・cm・inch のときは UI 層が建屋の親の `Transform` の縮尺で合わせる。
2. 自動の広さは、モデルの床への投影に 6 m の余裕を足し、5 m の倍数に丸める(最小 30 m × 20 m、一辺 300 m まで)。部品を少し動かしたくらいでは形が変わらないので、作り直しは区切りをまたいだときだけ。決まった広さはモデルの中心に置き、1 m に揃える。
3. 柱は壁ぞいに 10 m を超えない等間隔で立て、角の柱は2つの壁に属させる。柱と柱の間に高窓を1つずつ付ける。
4. UI 層の `sync_hall` は毎フレーム形を計算し、前と違うときだけエンティティを作り直す。床の線・壁の板は種類と壁ごとに1つのメッシュにまとめ、エンティティは柱を入れても数十個に収める。
5. `hide_near_walls` は、カメラが壁の外にいる側(`hidden_sides`)の壁と柱を隠す。壁の板は内向きの片面だけなので、外からは裏面カリングでも見えない。
6. 床のコンクリートの模様は `concrete_texture` が作る端のつながる値ノイズの画像(5 m 角に 512 px)。2×2 平均の縮小(ミップマップ)も作って渡し、遠くでちらつかないようにしている。壁と窓は光の向きで明るさが変わらない `unlit` にして、背景として落ち着かせている。
7. グリッドは Gizmos で床いっぱいに 1 m ごと(太い線は 5 m、床の目地と同じ)。一辺 120 m を超える建屋では 5 m / 25 m にする。グリッドが重ならないよう床は 1 mm、床の線は 0.4〜0.6 mm 下げて描く。モデルが小さく従来のグリッドの間隔が 1 m 未満のときは、モデルの周りに従来の細かいグリッドも重ねる。
8. ライブラリの部品をドラッグで置くときの当たり判定(`MeshRayCast`)からは、建屋のメッシュ(`HallMesh`)を外している。

### 画面の割り付け(`ui/mod.rs`)

egui のパネルを上・下・左・右の順に置き、残った領域を 3D ビューにします。3D カメラはその領域だけに描き(`Camera::viewport`)、egui はウィンドウ全体を覆う別のカメラに描きます。カーソルがパネルやビューキューブの上にあるときは、3D ビューのクリックやカメラ操作を止めます。

### カメラと投影(`view/camera.rs`)

1. カメラの状態は `OrbitCamera`(注視点・向き・距離・平行投影かどうか)だけ。毎フレーム Bevy の `Transform` と `Projection` に写す。
2. 平行投影の画面の高さは `2 × 距離 × tan(画角/2)`、つまり透視投影で注視点の位置に見える高さと同じにしている。切り替えても注視点付近の大きさが変わらず、距離を変えるズーム(ホイール・Alt + 右ドラッグ)や全体表示がそのまま平行投影でも効く。
3. 平行投影ではカメラの後ろ(注視点までの距離の 20 倍まで)も描く。ズームやフライ移動でカメラが物体を通り越しても物体が消えないようにするため。
4. ギズモには、透視投影なら画角から、平行投影なら Bevy が計算した表示範囲から作った投影行列を渡す。

### 描画の質(`plantwright_core::render_quality`、`view/render_quality.rs`)

1. 画質の段階(軽い・標準・きれい)ごとに使う Bevy の機能を `RenderQuality::profile` が決める。

   | 段階 | 影 | 環境光 | SSAO | ブルーム | アンチエイリアス |
   | --- | --- | --- | --- | --- | --- |
   | 軽い | なし | なし(一様な `GlobalAmbientLight`) | なし | なし | MSAA 4x |
   | 標準 | 2048 px・2 段、Hardware2x2 | あり | なし | なし | MSAA 4x |
   | きれい | 4096 px・3 段、Gaussian | あり | Medium | あり(HDR) | SMAA |

2. 影を落とすのは主光源(`KeyLight` の平行光)だけ。補助光は影を落とさない。
3. 環境光は `EnvironmentMapLight::hemispherical_gradient` の1画素の立方体マップ(上: 天井と高窓の白、横: 壁の灰色、下: コンクリートの色)。拡散光と映り込みの両方に使う。HDR 画像を読まないので軽い。
4. SSAO は MSAA と両立しないので、「きれい」では MSAA を切って SMAA にする。TAA は使わない(画面の自動確認で毎回同じ画になるように、時間で揺れる処理を避ける)。
5. モデルの単位は mm のことが多いので、Bevy の既定値(m 前提)のままでは影の範囲や SSAO の半径が合わない。`shadow_range` と `occlusion_scale` が、カメラの距離・シーンの外接箱・1 m あたりの単位数から影の段の境目、影のずらし(depth bias)、SSAO の半径を決める。10% を超えて変わったときだけ書き換える。
6. Bevy の接地の影(`ContactShadows`)は、小さな部品に寄ったときに床の奥へ縞が出たので使っていない。接地感は SSAO となめらかな影が受け持つ。
7. 画面に映る範囲だけで計算する反射(SSR)は deferred 描画が要り、MSAA や今のマテリアルの作りと合わないので使っていない。光沢のある面の映り込みは環境光が受け持つ。
8. 選んだ段階は `plantwright_core::render_settings` が設定フォルダの `render.json` に保存する。

### 箱とギズモ

1. 箱(`BoxPart`)は寸法・位置・回転を f64 で持ち、編集されると形状カーネルで B-rep ソリッドを作り直す(原点の角から直方体を作り、中心へ移してから回転・移動する)。
2. ギズモ(`src/viewport/gizmo.rs`)は transform-gizmo で計算し、egui の背景レイヤーに描く。箱ならパラメータに、取り込んだ部品やアセンブリなら親から見た配置(f64 の行列)に書き戻す。スナップは移動・回転を transform-gizmo の機能に任せ、拡大縮小の寸法だけ `snap_size` で丸める。

### タイミングチャートと干渉チェックのつながり(`motion/`)

- チャートは「ステップ × アクチュエータ」の表。ステップの動作は同時に始まり、すべて完了 + タイマーで次へ進む。サイクル開始時はすべてのアクチュエータが原点(設計位置)にある。
- 再生中の動きは `motion::SimulationPose` に入る(アクチュエータに割り当てたノード id → 設計位置からのワールド座標での剛体変換, f64)。
- `SimulationPose::pose_of(&model, node)` は、部品自身か、いちばん近い祖先のアセンブリの動きを返す。
- `view/viewport.rs` の `sync_views` が `.after(MotionSet::Pose)` で動きを部品の配置に重ね、表示用の `Transform` と干渉チェック用の `SimBody.pose` の両方に写すので、再生・スクラブ中も各時刻の位置で干渉チェックが走る。設計データ(`Model`)は書き換えない。
- 書き出し(`motion/export.rs`)では、1サイクルを 1/30 秒ごとに標本化し、親と違う動きをするノードだけに「親から見た配置」のキーを作る(`(動き × 設計のワールド配置)` を親の同じ時刻のワールド配置で割る)。アセンブリに割り当てた動きは、シーンの親子関係で子孫に伝わる。

### 加減速と電子カム(`plantwright_core/src/motion/profile.rs`)

- 加減速は「時間の進み具合 x (0〜1) → 位置の進み具合 s (0〜1)」の関数(`Easing::apply_with`)。`Schedule::value_at` は動作ごとに、直前の値から目標値までをこの s で補間する。
- 台形加減速は加速の割合 r で、速度 v = 1/(1-r) まで一定の加速度で加速し、同じ時間で減速する。サイクロイドは s = x - sin(2πx)/2π。
- 電子カム表は (時間, 位置) の点に始点 (0, 0) と終点 (1, 1) を足し、PCHIP(Fritsch–Carlson の傾きで決める区分 3 次エルミート補間)でつなぐ。単調な区間は単調のままで、点の間で行き過ぎない。表は `TimingChart::cams` にあり、`Schedule` を作るときに写して値を読むのに使う。
- 速度のグラフは `Schedule::velocity_at`(位置の中心差分)。シリンダは 0〜1 の値なのでストロークを掛けて表示する。

### センサーとインターロック(`plantwright_core/src/motion/sensor.rs`, `motion/targets.rs`)

- `sensor.rs` と `targets.rs` は Bevy に依存しない計算だけのモジュールで、単体テストを持つ。Bevy のシステム(`mod.rs`)と描画(`sensor_view.rs`)・パネル(`ui.rs`)はそれを呼ぶだけ。
- 検知対象は向きつきの直方体(`TargetBox`)。`targets.rs` が部品を直方体にする(箱はそのまま、取り込んだ部品は部品座標系の外接箱)。アクチュエータに割り当てた部品(またはその祖先のアセンブリ)には、そのアクチュエータの動きを付ける。
- 光電センサーは光軸(線分)と直方体の交差をスラブ法で、近接センサーは検出面の中心から直方体までの距離で判定する。センサーは取り付け先のアクチュエータと一緒に動き、そのアクチュエータが動かす部品は検知しない。`force` があればその値を返す。
- インターロックはステップの開始条件(`Step::interlocks`)。`TimingChart::schedule_with` が時刻表を作るとき、前のステップの終了時刻から 0.02 秒ずつ時刻を進めてセンサーを調べ、成り立った時刻に動作を始める(`StepSpan::action_start`)。5 秒(`INTERLOCK_TIMEOUT`)で成り立たなければ `Schedule::stop` に止まったステップを記録し、以降のステップは長さ 0 で並べる。時刻表は再生前に作るので、スクラブや繰り返しでも結果は毎回同じ。
- サイクル全体の ON / OFF は `SensorTimeline`(チャートの波形用)、再生位置の ON / OFF は `SensorStates` に入る。`SensorStates` は `MotionSet::Sensors`(`MotionSet::Pose` の後)で更新するので、外部へ渡す処理は `.after(MotionSet::Sensors)` で読む。

### 安全機器・作業者・AGV(`safety/`, `floor/`)

- 検知・ラッチ・安全距離・動作範囲・作業者の計画・AGV の配車はすべて Bevy に依存しない関数で、単体テストを付けている。
- 侵入物は `safety::SafetyIntruders` に毎フレーム集める (可動部・作業者・AGV)。停止信号が出ると、タイミングチャートの再生と AGV が止まる。
- 作業者と AGV はチャートとは別の時計 `floor::FloorClock` で動く。詳しくは [safety-and-floor.md](safety-and-floor.md)。

### 寸法と注記(`annotation.rs`)

- 値は f64 の形状から計算し、単位はモデルの単位(mm など)で出す。吸着は頂点・辺の中点・面の中心の 12 px 以内。
- 辺の長さは直線なら両矢印、円弧などは線に沿った長さ。面間距離は平行な平面なら1つ目の面の中心から2つ目の面への垂線、平行でなければ面どうしの角度、曲面を含むと中心どうしの距離。
- 寸法は「どの部品のどこか」で覚えるので、部品やアセンブリを動かすと付いて行く。箱の点は箱の半寸法で割った値で覚えるので、箱の寸法を変えても同じ頂点・面に付いたまま。
- 寸法線は部品に隠れないよう手前に描く。部品を隠すとその寸法も隠れる。
- 追加・削除・すべて削除・注記の文字の編集は元に戻せる。部品を削除すると寸法は隠れ、部品の削除を元に戻すと寸法も戻る。
- 距離計測(シミュレーション)と寸法の道具は、片方を始めるともう片方は止まる。

### 形状の差し替え(`plantwright_core::remap`、`Model::replace_geometry`)

設計変更後のファイルを読み、選んだノード以下の木と新しい木のノードを対応づけてから入れ替えます。
対応づけ(`remap::plan_remap`)は Bevy にもファイル形式にも依存しない純粋な関数で、次の順に試します。

1. 一番上のノード同士は必ず対応させる(ファイル名が変わっても差し替えられるように)
2. 一番上からの名前の並び(パス)が同じもの。同じパスが複数あれば出てきた順に組にする
3. まだ決まっていないノードの中で、インスタンス名が1つずつしかないもの(階層を移った部品)
4. 同じく、製品名が1つずつしかないもの(インスタンス名を付け直した部品)

2〜4 はアセンブリと部品を別々に数えます。どれにも当てはまらないものは対応させません(取り違えるよりは一覧に出して人が確かめる方がよいため)。

`Model::replace_geometry` は対応したノードの ID を残したまま名前・配置・形状を書き換えるので、
ID で部品を指している設定(アクチュエータなど)はそのまま引き継がれます。一番上のノードの配置はユーザーが置いた位置なので変えません。
新しい側にだけあるノードは対応した親の下に新しい ID で加え、古い側にだけあるノードは古い形状のまま、親の子の並びの最後に残します。
元に戻すために、差し替え前後の写し(`Snapshot`)を `ModelChange` として履歴に積みます。

### 工程シミュレーション(`flow/`)

工程モデルの離散事象シミュレーションと集計、3D の色分け、Excel 書き出しのしくみは [flow.md](flow.md) にまとめています。工程モデルの編集は、下の「見張る文書」のしくみで元に戻す履歴に入ります。

### 元に戻す・やり直し(`history.rs`)

変更を済ませてから記録する方式です。モデルを書き換えたあと、その変更を戻すための `Command` を `History` に積みます。履歴の山 (`UndoStack`)・`ModelChange`・ドラッグ中の編集のまとめ方 (`ModelEdits`) は Bevy に依存しない `plantwright_core::history` にあり、アプリ側の `history.rs` は `World` を書き換える部分とシステムだけを持ちます。

- モデルの変更は `ModelChange` で表します。中身は変更前と変更後のノードの写し(`Model::snapshot`、子孫ごと)です。取り込んだ形状は `Arc` で共有しているので、大きなアセンブリでも写すのは軽く済みます。
- 履歴に載るもの: 箱の追加、円柱・押し出しの追加、穴あけ・ブーリアン、STEP の読み込み、タイミングチャートのサンプル装置の読み込み(`History::replace_all`)、削除(Delete キー・ツリー・プロパティ)、すべて閉じる、ギズモでの移動・回転・拡大縮小、プロパティでの寸法・位置・回転・名前・色・部品スクリプトの変更。
- 表示/非表示、選択、カメラは見た目の操作なので履歴に載せません。
- ギズモや数値欄のドラッグは毎フレーム値が変わります。`History::begin_edit` が最初の状態だけを覚え、マウスを離した時点(数値を打ち込んだときは入力欄から抜けた時点)で1件にまとめます。
- 新しく編集するとやり直しの履歴は消えます。履歴は最大 200 件です。
- タイミングチャート・工程モデル・床の配置(安全機器・作業者・AGV)・平面図と点群は「見張る文書」として登録しています(下)。

| ギズモで移動 → Delete で削除 | Ctrl+Z で削除を戻す → もう一度 Ctrl+Z で移動を戻す |
| --- | --- |
| ![移動](screenshots/06_undo_1_moved.png) | ![削除を戻す](screenshots/08_undo_3_ctrl_z_delete.png) |
| ![削除](screenshots/07_undo_2_deleted.png) | ![移動を戻す](screenshots/09_undo_4_ctrl_z_move.png) |

新しい操作を履歴に載せるには次のようにします。

- ノードを書き換えるなら、書き換える直前に `history.begin_edit(&model, id)` を呼ぶ。追加なら `record_added`、削除なら `history.remove(&mut model, id)` を使う。
- 画面のあちこちで編集されるモデル以外のデータ(文書)は、`TrackedDoc` を実装してプラグインで `app.track_doc::<D>()` と登録する。編集する場所に手を入れなくても、マウスを離して入力欄からも抜けた時点で「前に確定した状態」と比べ、違えば1件の履歴(`D::LABEL`)にする。比べる・戻すための写し(`Value`)は毎フレーム作るので、大きな中身は `Arc` で共有する。元に戻した直後は戻した状態を確定し直すので、戻したこと自体は履歴に載らない。新規作成・開くでは `History::clear` が見張りもやり直す。比べて確定するところは `plantwright_core::history::edit_tracker` にある。
- 1回の操作をはっきり区切りたいときは、`Command` トレイトを実装した型を作り、変更を済ませたあとで `history.push(...)` に渡す。

| 見張る文書 | 登録している場所 | 比べるもの |
| --- | --- | --- |
| タイミングチャート | `motion/mod.rs` | `TimingChart`(センサーの強制 ON / OFF は比べず、戻しても今の状態を残す) |
| コンベア・グリッパ | `conveyor/mod.rs` | `ConveyorConfig` |
| 工程モデル | `flow/mod.rs` | `FlowModel`(戻すと回した結果は捨てる) |
| 安全機器・作業者・AGV | `floor/mod.rs` | 保存と同じ `FloorRecord`(動かしている間の状態は含まない) |
| 平面図・点群 | `site/mod.rs` | `Site` の配置。図面・点の中身は `Arc` が同じものかだけを見る |
| ケーブル・ホース | `physics/mod.rs` | `Model::cables` |
| PLC の番号の割り付け | `plc/mod.rs` | `AddressPlan`(外部 PLC の接続先は履歴に載せない) |

```rust
struct MoveStep { before: f64, after: f64 }

impl Command for MoveStep {
    fn label(&self) -> String { "ステップを移動".into() }
    fn undo(&self, world: &mut World) { /* before に戻す */ }
    fn redo(&self, world: &mut World) { /* after にする */ }
}

history.push(MoveStep { before, after });
```


### ギズモのスナップ

Ctrl を押している間(または「常にスナップ」をオンにしている間)、ギズモの操作が一定量ずつになります。

| ギズモ | 刻み(初期値) | 動き方 |
| --- | --- | --- |
| 移動 | 移動の刻み(0.1) | ドラッグを始めた位置から 0.1 ずつ動く |
| 回転 | 回転の刻み(15°) | ドラッグを始めた向きから 15° ずつ回る。輪に目盛りが出る |
| 拡大縮小 | 寸法の刻み(0.1) | 動かした軸の寸法が 0.1 の倍数になる(触っていない軸の寸法はそのまま) |

移動と回転は Unity と同じく「ドラッグ開始からの変化量」を丸めます。拡大縮小だけは倍率ではなく寸法を丸めるので、寸法をきりのいい値にそろえられます。

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

## 本物の装置のサンプル

`samples/real_robot/` の UR5e と Robotiq 2F-85 の GLB は、メーカーが ROS 向けに公開しているメッシュ(BSD 3-Clause)を `tools/real_robot/convert_meshes.py` で変換したもので、
プロジェクト `ur5e_pick_place.plantwright` は `tools/real_robot/make_demo.py` で生成しています。どちらもアプリに埋め込んでいます。出典・関節の付け方・作り直し方は [real-robot.md](real-robot.md) を見てください。

同じ UR5e を URDF で読み込むサンプル(`samples/real_robot/ur5e_urdf/`)と、URDF の読み込みの中身は [urdf.md](urdf.md) にあります。

## 日本語フォント

egui の標準フォントには日本語がないため、[Noto Sans JP](https://github.com/notofonts/noto-cjk)(SIL Open Font License 1.1)を埋め込んでいます。

- `assets/fonts/NotoSansJP-Subset.otf` … 英数字・記号・かな・JIS 第1/第2水準漢字だけに絞ったもの(約 2.1 MB)。`include_bytes!` で実行ファイルに入るので、配布時にフォントを別に置く必要はありません。
- `assets/fonts/OFL.txt` … フォントのライセンス。配布物にも同梱してください。
- `src/ui/font.rs` … 起動直後に egui の予備フォントとして登録する。英数字は egui の標準フォント、日本語だけ Noto Sans JP で描かれます。
- `tools/subset_jp_font.py` … 元フォントから絞り込み版を作り直すスクリプト(使い方はファイル先頭)。

## ロゴとアイコン

ロゴは頭文字の p をベルトコンベヤのループにし、上にワーク(黄色い箱)を載せた形です。色は画面の配色(`ui/theme.rs` の紫・ティール・黄・紺)に合わせています。

- `assets/icon/plantwright-mark.svg` … 背景なしのマーク。元の絵はこれ。
- `assets/icon/plantwright-icon.svg` … 紺の角丸にマークを載せたアプリのアイコン。README の先頭にも出している。
- `assets/icon/plantwright-icon-256.png`、`plantwright-mark-128.png` … アプリに埋め込む PNG。`ui/logo.rs` がウィンドウのアイコン(Windows・Linux のタスクバーや Alt+Tab)と、ようこそ画面のロゴに使う。ウィンドウのアイコンは Bevy に設定がないため、`bevy::winit::WINIT_WINDOWS` から winit のウィンドウを取り出してメインスレッドで付けている。
- `assets/icon/plantwright.ico` … Windows の .exe に埋め込むアイコン。`build.rs` が Windows 向けのビルドのときだけ `winresource` で埋め込む(MSVC は rc.exe、クロスビルドは mingw-w64 の windres を使う)。
- `assets/icon/plantwright.icns` … Mac の .app のアイコン。リリースのワークフローが `Contents/Resources/` に置き、`packaging/macos/Info.plist` の `CFBundleIconFile` で指す。
- `tools/icons/generate.sh` … SVG から PNG・.ico・.icns を作り直すスクリプト。形や色を変えたら実行してコミットする。

## 次に手を入れるところ

- 面ごとの色(STEP で面に付いた色)の読み込み
- 計測ツールの拡張(面と面の角度、面どうしの距離)
- 部品数が数百を超えるときの干渉チェックの高速化(空間分割)
- ワークの受け渡し(把持中だけ一緒に動く)。ワークを `motion/targets.rs` の `extra_targets_at` で返せば、センサーの検知とインターロックの待ちに使える
- STEP への書き出し
