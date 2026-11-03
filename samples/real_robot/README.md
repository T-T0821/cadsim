# 本物の装置の CAD で組んだサンプル

`ur5e_pick_place.plantwright` を plantwright で開くと、協働ロボット **UR5e**(Universal Robots)に電動グリッパ **2F-85**(Robotiq)を付けたピック & プレースのセルが出てきます。
形状は同じフォルダの `ur5e.glb` と `robotiq_2f85.glb` から読み込みます。`real_line.plantwright` は同じセルのまわりに安全柵・作業者・AGV などを置いたラインです。**プロジェクトと glb は同じフォルダに置いたまま** 使ってください。

| ファイル | 中身 | 出典 | ライセンス |
| --- | --- | --- | --- |
| `ur5e.glb` | UR5e の 7 リンク(ベース〜手首3) | [UniversalRobots/Universal_Robots_ROS2_Description](https://github.com/UniversalRobots/Universal_Robots_ROS2_Description) の `meshes/ur5e/visual/*.dae`(コミット `81f7c9f`) | BSD 3-Clause([LICENSE-UR5e.txt](LICENSE-UR5e.txt)) |
| `robotiq_2f85.glb` | 2F-85 の本体・指のリンク 8 つと UR 用アダプタ | [PickNikRobotics/ros2_robotiq_gripper](https://github.com/PickNikRobotics/ros2_robotiq_gripper) の `robotiq_description/meshes/visual/2f_85/*.dae`(コミット `a74d007`) | BSD 3-Clause, Copyright (c) 2022, PickNik Robotics([LICENSE-Robotiq-2F-85.txt](LICENSE-Robotiq-2F-85.txt)) |
| `ur5e_pick_place.plantwright` | 関節・教示点・タイミングチャート・コンベアの設定 | このリポジトリで作成 | リポジトリと同じ(MIT / Apache-2.0) |
| `real_line.plantwright` | 上のセルに安全柵・制御盤・作業者・AGV・工程モデルを足したライン | このリポジトリで作成 | リポジトリと同じ(MIT / Apache-2.0) |

どちらのメッシュも、メーカーや開発元が ROS 向けに公開しているロボットの記述パッケージに入っているもので、BSD 3-Clause の条件(著作権表示とライセンス文を残す)で再配布できます。
glTF への変換(色ごとにまとめ直し、ホーム姿勢に組んで 1 ファイルにする)は `tools/real_robot/convert_meshes.py` で行いました。形は変えていません。

なお、UR の記述パッケージのうち UR8 Long・UR15・UR18・UR20・UR30 などの新しい機種のメッシュは、UR 独自の「Graphical Documentation」の条件になっていて BSD ではありません。ここでは BSD の UR5e だけを使っています。

作り直し方は [docs/real-robot.md](../../docs/real-robot.md) を見てください。
