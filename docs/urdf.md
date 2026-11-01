# ロボット機種を URDF で読み込む

ROS のロボットの記述ファイル **URDF**(`.urdf`)を読み込み、リンクの形状と関節・可動範囲から、cadsim のロボット(タイミングチャートの関節 + ロボット)を作る機能の中身です。
使い方は README の「ロボット機種を URDF で読み込む」を見てください。

## 層の分け方

| 層 | 場所 | 内容 |
| --- | --- | --- |
| ドメイン | `cadsim_core::motion::robot_import` | 形式によらないロボットの機種 `RobotDescription`(リンクの名前と関節の木)と、それをチャートの関節とロボットにする `add_robot` |
| アダプター | `cadsim_io::urdf`(`mod.rs`・`collada.rs`) | URDF の XML を `RobotDescription` と 3D の中間表現 `Scene` にする。見た目のメッシュ(COLLADA・STL・OBJ・glTF)と箱・円柱・球を読む |
| UI | `src/file_io.rs`(読み込み)、`src/motion/urdf_robot.rs`(チャートへのつなぎ)、`src/ui/menu.rs`(ファイルメニューの入口) | 別スレッドで読み、形状をモデルに加えたあと、リンクの位置を集めて `add_robot` を呼ぶ |

## 読み込み(`cadsim_io::urdf`)

- 読むもの: `<link>` の `<visual>`(`origin`・`geometry`・`material`)、`<joint>` の `type`・`origin`・`parent`・`child`・`axis`・`limit`(`lower` / `upper` / `velocity`)・`mimic`、`<robot>` 直下の名前付きの `<material>`。
- 読まないもの: `<collision>`・`<inertial>`・`<transmission>`・`<gazebo>`・ros2_control。**xacro** も読まない(`${…}` や `<xacro:…>` が残っていたら、xacro で URDF に直すよう伝えるエラーにする)。
- シーンの形は「ロボット名のノード → リンクごとのノード → 見た目のメッシュ」。リンクのノードは木にせず、全関節 0 の位置(関節の `origin` を根元から掛けたもの)で横に並べる。動きはチャートの関節が付けるため。
- URDF は m・Z が上なので、いちばん上のノードを X 軸まわりに −90° 回して、アプリの座標(Y が上)にする。m からモデルの単位への換算は、ほかの 3D ファイルと同じく `scene_to_import` が行う。
- メッシュのノード名は URDF の中で重ならないようにする(`shoulder_link ur5e_shoulder_2` など)。プロジェクトを開き直すとき、形状を部品の名前(製品名)で引くため。
- メッシュの場所: `package://名前/パス` は URDF のあるフォルダから上へたどり、`名前` のフォルダ(またはその下の `share/名前`)を探す。見つからなければ URDF のフォルダとその親からの相対パスとしても探す。`file://` と相対パスもそのまま読む。
- `<mesh scale>` はメッシュの頂点に焼き込む(鏡映なら三角形の向きを入れ替える)。
- COLLADA(`collada.rs`): `triangles` / `polylist` / `polygons`、ノードの `matrix` / `translate` / `rotate` / `scale`、`instance_node`、材質の拡散色、`<unit meter>` を読む。ROS の RViz と同じく `<up_axis>` は見ない。色ごとに 1 つのメッシュにまとめる。
- STL・OBJ・glTF は 3D ファイルの読み込み(`interchange::import_file`)を「m・座標はそのまま」で呼び、ノードの配置を焼き込む。

## 関節にする(`cadsim_core::motion::robot_import::add_robot`)

- `revolute` / `continuous` は回転テーブル、`prismatic` は直動軸。`fixed` でつながったリンクは根元側の関節の部品に足す。`floating` / `planar` は動かせないので固定にして注意を出す。`mimic` は独立した関節にして注意を出す。
- 関節の位置と向きは、モデルに置いたリンクのノードのワールド行列から求める。URDF では子リンクの座標系が関節の座標系と同じなので、子リンクの原点が回転の中心(`pivot`)、子リンクの座標で書いた `axis` をワールドに回したものが軸になる。ユーザーがロボットを動かしてから関節を付けても合うように、モデルの行列を使う。
- 軸が X / Y / Z(と逆向き)にそろえば `axis` と `reversed` に、斜めなら `Actuator::custom_axis` に入れる。`custom_axis` は `.cadsim` の `custom_axis` に保存し、`pose::direction` が優先して使う(順運動学・ヤコビアン・MELFA の書き出しが同じ軸で動く)。
- 可動範囲はラジアン → 度、直動は m → モデルの単位。`continuous` は範囲なし。
- ロボットの先端: 根元から、動く関節のある枝が 1 本だけのあいだたどった最後の動く関節。2 本指のグリッパのような枝分かれの手前で止まるので、指は逆運動学に入らない(関節としては残り、ジョグやステップで動かせる)。
- TCP: 先端の関節の子リンクから固定の関節だけでつながったリンクのうち、`tool0` → 名前に `tcp` → `tool` の順に探し、なければいちばん先のリンク。工具の向きはそのリンクの Z 軸(ROS-Industrial の決まり)。
- 関節の最高速度は、回転関節の `velocity` のうちいちばん遅いものを度/秒にしてロボットの速度に入れる。
- 読み込んだロボットのいちばん上のノードには `skip_inner_interference`(「本物の装置」のサンプルで足したもの)を付け、リンクの継ぎ目で重なるメッシュどうしを干渉にしない(`file_io::receive_imports`)。
- 関節の値 0 は URDF の全関節 0 の姿勢。実機 (UR) とつなぐ機能は、反転・ずれなしでそのまま合う。

## 同梱の UR5e

`samples/real_robot/ur5e_urdf/` に、UR5e の URDF(`ur5e.urdf`)と見た目のメッシュ(リンクごとの GLB 7 つ、合わせて約 2.2 MB)を置き、アプリにも埋め込んでいる(`src/file_io.rs` の `URDF_SAMPLE_FILES`)。
「ファイル → サンプルを開く → ロボット機種 UR5e (URDF)」で、設定ファイルと同じ場所の `samples/ur5e_urdf/` に書き出して読み込む。

- 出典は「本物の装置」のサンプル([real-robot.md](real-robot.md))と同じ [Universal_Robots_ROS2_Description](https://github.com/UniversalRobots/Universal_Robots_ROS2_Description)(BSD 3-Clause)。ライセンス文も同じ `samples/real_robot/LICENSE-UR5e.txt` を一緒に書き出す。
- URDF は `urdf/ur_macro.xacro` を `ur_type:=ur5e` で展開したものと同じリンク・関節・寸法(`config/ur5e/*.yaml`)を写し、慣性・衝突・ros2_control を省いたもの。
- 「本物の装置」の `ur5e.glb` はホーム姿勢に組んだ 1 ファイルで、リンクの座標系で置いたメッシュがないため、URDF 用には元の COLLADA をリンクごとに GLB にした。変換は cadsim の COLLADA の読み込みを使う `cargo run -p cadsim_io --example dae_to_glb -- base.dae base.glb ur5e_base`(形は変えない)。

テスト: `src/motion/urdf_robot.rs` で、同梱の URDF が注意なしで 6 軸のロボットになること、全関節 0 の TCP が UR の寸法どおりの位置に来ること、逆運動学で教示点に届き、届かない点は届かないと分かることを確かめる。

## ほかの公開 URDF(同梱していない)

メーカーや ROS-Industrial が公開している記述パッケージの多くは xacro なので、ROS の入った環境で URDF に直してから読み込む。

```sh
# 例: UR (ほかの機種は ur_type を変える)
ros2 run xacro xacro ur.urdf.xacro ur_type:=ur10e name:=ur10e > ur10e.urdf
# 例: ROS-Industrial の support パッケージ
ros2 run xacro xacro urdf/lrmate200id.xacro > lrmate200id.urdf
```

URDF をパッケージのフォルダ(`urdf/` など)に置いたまま読み込めば、`package://` のメッシュはフォルダをたどって見つかる。

| 機種 | リポジトリ | ライセンス(2026-11 に確認) |
| --- | --- | --- |
| UR3e / UR5e / UR10e / UR16e など | [UniversalRobots/Universal_Robots_ROS2_Description](https://github.com/UniversalRobots/Universal_Robots_ROS2_Description) | BSD 3-Clause。ただし UR8 Long・UR15・UR18・UR20・UR30・UR10g/17g/18g のメッシュは UR 独自の「Graphical Documentation」の条件 |
| Franka (FR3 / Panda) | [frankaemika/franka_description](https://github.com/frankaemika/franka_description) | Apache-2.0 |
| ファナック LR Mate など | [ros-industrial/fanuc](https://github.com/ros-industrial/fanuc) | BSD(パッケージの記載) |
| ABB IRB 2400 など | [ros-industrial/abb](https://github.com/ros-industrial/abb) | Apache-2.0 / BSD 3-Clause(パッケージごと) |
| 安川 Motoman GP7 など | [ros-industrial/motoman](https://github.com/ros-industrial/motoman) | BSD 3-Clause(パッケージの記載) |
| KUKA KR 6 など | [ros-industrial/kuka_experimental](https://github.com/ros-industrial/kuka_experimental) | Apache-2.0(リポジトリ)。パッケージの記載は BSD |
| Kinova Gen3 | [Kinovarobotics/ros2_kortex](https://github.com/Kinovarobotics/ros2_kortex) | BSD 3-Clause |
| Robotiq 2F-85 グリッパ | [PickNikRobotics/ros2_robotiq_gripper](https://github.com/PickNikRobotics/ros2_robotiq_gripper) | BSD 3-Clause |

再配布するときは、各リポジトリのライセンス文と著作権表示を一緒に置くこと。
