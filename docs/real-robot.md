# 本物の装置のサンプル(UR5e + Robotiq 2F-85)

「ファイル → サンプルを開く → 本物の装置 (UR5e + Robotiq 2F-85)」で開くサンプルの中身と作り方です。
メーカーが公開している本物の装置の形状を読み込み、plantwright の関節・ロボット・タイミングチャート・ワーク搬送の機能だけで動きを付けています。

## 使っている CAD と再配布の条件

| 装置 | 形状の出どころ | ライセンス | リポジトリに入れたか |
| --- | --- | --- | --- |
| 協働ロボット UR5e(Universal Robots) | [Universal_Robots_ROS2_Description](https://github.com/UniversalRobots/Universal_Robots_ROS2_Description) の `meshes/ur5e/visual/*.dae` | BSD 3-Clause | 入れた(`samples/real_robot/ur5e.glb`) |
| 電動グリッパ 2F-85(Robotiq)と UR 用アダプタ | [ros2_robotiq_gripper](https://github.com/PickNikRobotics/ros2_robotiq_gripper) の `robotiq_description/meshes/visual/2f_85/*.dae` | BSD 3-Clause | 入れた(`samples/real_robot/robotiq_2f85.glb`) |

どちらも ROS(ロボット用のミドルウェア)向けにメーカーや開発元が公開しているロボットの記述パッケージで、関節の寸法(URDF / xacro と YAML)も一緒に公開されています。
BSD 3-Clause は、著作権表示とライセンス文を残せば改変・再配布してよい条件なので、`samples/real_robot/` にライセンス文(`LICENSE-*.txt`)と出典(`README.md`)を置き、アプリに埋め込むときも一緒に書き出しています。

探したけれど使わなかったもの:

- **UR の新しい機種(UR8 Long・UR15・UR18・UR20・UR30 など)**: 同じリポジトリにあるが、UR 独自の「Terms and Conditions for use of Graphical Documentation」の条件で BSD ではない。
- **メーカーの CAD ダウンロード(SMC・CKD・Festo・ミスミ・THK・IAI など)の STEP**: 無償でダウンロードできるが、利用規約で第三者への再配布が禁じられているか、条件がはっきりしない。リポジトリには入れず、README に手元でダウンロードして同じ手順で動きを付ける方法だけを書いた。

## ファイルの作り方

```sh
git clone https://github.com/UniversalRobots/Universal_Robots_ROS2_Description.git ur
git clone https://github.com/PickNikRobotics/ros2_robotiq_gripper.git robotiq
pip install trimesh pycollada
python3 tools/real_robot/convert_meshes.py ur robotiq   # ur5e.glb と robotiq_2f85.glb
python3 tools/real_robot/make_demo.py                   # ur5e_pick_place.plantwright
```

| スクリプト | 内容 |
| --- | --- |
| `tools/real_robot/kinematics.py` | UR5e と 2F-85 の関節の位置(ROS の記述の寸法を写したもの)と順運動学。標準ライブラリだけで動く |
| `tools/real_robot/convert_meshes.py` | COLLADA を読んで、リンクごとに置き、色ごとにまとめて GLB に書く。`trimesh` と `pycollada` が要る |
| `tools/real_robot/make_demo.py` | 関節・ロボット・教示点・ステップ・コンベアを書いたプロジェクトを作る。標準ライブラリだけで動く |

### GLB の形

- 単位は m、Y が上(glTF の決まり)。ROS の座標(Z が上)からは (x, y, z) → (x, z, −y) で写す。
- `ur5e.glb` は **ホーム姿勢**(関節角 [0, −90, 90, −90, −90, 0]°、腕を立てて工具を真下に向けた姿勢)に組んだ形。原点はベースの底面の中心。
  全関節 0° の姿勢は腕が水平に伸びきった特異姿勢なので、設計位置(plantwright の関節の値が 0 の姿勢)には使わない。
- `robotiq_2f85.glb` は全開の形。原点はアダプタの取り付け面(UR の `tool0`)で、指は +Y を向く。
- ノードは「ファイル名のルート → リンク → 色ごとのメッシュ」。plantwright は glTF のノード名を製品名として使い、
  プロジェクトの `Imported` ノードは製品名で形状を引くので、ノード名はファイルの中で重ならないようにしている(`upper_arm_blue` など)。

### 関節の付け方

plantwright の関節(回転テーブル)は「設計位置でのワールドの軸と回転中心」で決まり、親の関節の動きで運ばれる(指数積の形。[robot.md](robot.md))。
URDF の関節は子リンクの座標の Z 軸まわりに回るので、ホーム姿勢での子リンクの座標の Z 軸を `axis`(X / Y / Z と `reversed`)に、原点を `pivot` にすれば、
同じ関節角で URDF と同じ姿勢になる。ホーム姿勢ではどの関節の軸も座標軸に沿うので、plantwright の X / Y / Z だけで表せる。
関節の値はホーム姿勢からの角度になる(UR のペンダントの角度 = ホームの角度 + plantwright の値)。

| アクチュエータ | 動かすノード | 載せ先 |
| --- | --- | --- |
| J1 ベース 〜 J6 手首3 | ショルダー 〜 手首3。J6 にはグリッパの本体とアダプタも載せる | 1 つ根元側の関節 |
| 左指 外側 / 右指 外側 | 外側ナックルとフィンガー | J6 |
| 左指 内側 / 右指 内側 | 内側ナックル | J6 |
| 左指 指先 / 右指 指先 | 指先 | 同じ側の外側 |
| 把持 (ワークを掴む合図) | なし(シリンダ) | J6 |

2F-85 の指は平行リンクで、ROS の記述では 1 つの角度に「外側・内側は同じ向き、指先は逆向き」の係数を掛けて動かしている(mimic)。
plantwright では 6 つの回転テーブルに同じ目標角を置き、係数が負のものは `reversed` で逆に回す。指先は外側に載っているので、外側と逆に同じだけ回ると向きが保たれ、平行に閉じる。
ワークの幅 60 mm に 2 mm 食い込む角度(約 16°)は、指先の関節の位置から二分法で求める(`kinematics.gripper_angle_for`)。

ワークを掴む・離すのはワーク搬送のグリッパの機能で、チャックにはシリンダが要るので、動かす部品のないシリンダ「把持」を指と同じステップで出し入れしている。

### 動き

| ステップ | 内容 |
| --- | --- |
| ピック上へ (ワーク待ち) | 関節動作。角度はスクリプトの逆運動学(数値微分の減衰最小二乗法)で求める。ワークがストッパに着くまでタイマーで待つ(ワーク搬送のワークはセンサーの検知対象にならないため) |
| 下降 | 教示点「ピック」への直線補間(アプリの逆運動学) |
| 把持 | 指を閉じる + 把持シリンダを出す |
| 上昇 / プレース上へ / 下降 | 直線補間 / 関節動作 / 直線補間 |
| 解放 | 指を開く + 把持シリンダを戻す |
| 上昇 / ホームへ | 直線補間 / 関節動作(全関節 0 = ホーム) |

ワークは投入シュートからサイクルタイム(8.83 秒)ごとに供給コンベアへ出て、終端のストッパで止まる。ロボットが排出コンベアに置いたワークは回収箱で数えられる。
掴む高さはワークの上から 22 mm。2F-85 は閉じると指先が手前(下)へ回り込むので、ワークの中央を掴むとベルトと駆動プーリーに当たる。

### 干渉の扱い

メーカーのメッシュはリンクの継ぎ目(カバーと軸受け)や色ごとに分けた部品どうしで重なっていて、そのままでは干渉が 58 件出る。
UR5e とグリッパを「ロボット (UR5e + 2F-85)」のアセンブリにまとめ、ノードの `skip_inner_interference` を付けて、中の部品どうしは調べないようにしている
(画面ではプロパティの「干渉: 中の部品どうしは調べない」)。干渉チェックでは、印の付いたいちばん上の祖先を部品のまとまり(`SimBody::group`)にする(`Model::interference_group`)。
ロボットとコンベア・架台など、外の部品との干渉はこれまでどおり調べる。

## アプリへの埋め込み

`src/project/document.rs` の `REAL_ROBOT_FILES` に、プロジェクト・GLB 2 つ・README・ライセンス文を `include_bytes!` で埋め込んでいる(合わせて約 3 MB)。
メニューから開くと、設定ファイルと同じ場所の `samples/real_robot/`(なければ一時フォルダ)に書き出し、そこからプロジェクトとして読む。
プロジェクトは GLB を相対パスで参照するので、同じフォルダに並べる必要があるため。中身が同じファイルは書き直さない。
開いたプロジェクトはファイルに結びつけない(デモラインと同じく、保存すると名前を聞く)。

テストは `src/project/tests.rs` の `real_robot_sample_opens_and_reaches_every_point`。
注意なしで開けること、関節がどれも読み込んだリンクを動かすこと、4 つの教示点に届くこと、
スクリプトの逆運動学で求めた関節角でアプリの TCP が教示点に来る(関節の位置と向きが合っている)ことを確かめる。
