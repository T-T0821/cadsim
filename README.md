# CAD最小プロトタイプ(箱を作る・選ぶ・回す)

Bevy + bevy_egui + truck で作った最小構成のCADビューアです。
形状の「正」は truck の B-rep(f64)が持ち、Bevy は表示用の f32 メッシュと操作だけを担当します。

## 動作確認したバージョン

| crate | バージョン |
| --- | --- |
| Rust | 1.97.1 以上(依存crateの要件) |
| bevy | 0.20 |
| bevy_egui | 0.43 |
| truck-modeling | 0.6 |
| truck-meshalgo | 0.4 |

## 実行

```sh
cargo run
```

初回は Bevy のビルドに数分かかります。Linux では `libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev` が必要です。

## 操作

Unity / Unreal Engine のシーンビューと同じ割り当てにしています。

| 操作 | 内容 |
| --- | --- |
| 左クリック | 面を選択(面はオレンジ、その箱は青になる) |
| Alt + 左ドラッグ | 注視点のまわりを回転(オービット) |
| 中ドラッグ | 平行移動(パン) |
| ホイール / Alt + 右ドラッグ | ズーム |
| 右ドラッグ | その場で見回す |
| 右ボタンを押しながら W / A / S / D | 前後左右に移動(Shiftで加速) |
| 右ボタンを押しながら Q / E | 下降 / 上昇 |
| F | 選択中の箱にフォーカス |
| Delete | 選択中の箱を削除 |
| 左パネル | 箱の追加、寸法・位置・回転の編集、+15°回転ボタン |

## しくみ

1. `BoxPart` が寸法・位置・回転を f64 で保持する。
2. 編集されると `dirty` に入り、`rebuild_dirty_parts` が truck で B-rep ソリッドを作り直す(`tsweep` を3回 → `rotated` → `translated`)。
3. ソリッドを面ごとにテッセレーションし、面1枚につき Bevy のメッシュエンティティを1つ作る。各エンティティには `FaceTag { part, face }` を付ける。
4. 左クリックで `MeshRayCast` を撃ち、当たったエンティティの `FaceTag` から部品と面を特定する。
5. truck の稜線(ポリライン化済み)は Gizmos で黒い線として描く。

回転はメッシュの Transform ではなく truck 側の形状に適用しているので、寸法計算や今後の干渉チェックは常に f64 の形状で行えます。

## 次に手を入れるところ

- UI の日本語表示(egui に日本語フォントを読み込む)
- W / E / R で移動・回転・拡大縮小のギズモを切り替える(Unity / Unreal と同じ)
- 平行投影への切り替え
- 干渉チェック(truck のソリッド同士、または Avian のコライダー)
