# ゲーム風の画面(テーマ・ミッション・経験値)

ツールに慣れていない人の心理的なハードルを下げるため、画面をゲームのように「次にやること」「できたらほめる」形にしています。
CAD の作業を邪魔しないよう、演出はすべて「表示 → テーマ → シンプル」で消せます。

## 層の分け方

| 層 | 場所 | 役目 |
| --- | --- | --- |
| ドメイン | `crates/cadsim_core/src/gamify.rs` | ミッションの一覧、達成の判定、経験値とレベル、紙吹雪の軌道。Bevy・egui なしで単体テスト |
| アダプター | `src/preferences_file.rs` | 設定 (`Preferences`) と JSON の変換、OS ごとの設定フォルダへの読み書き |
| UI | `src/ui/theme.rs` | egui の `Style` (配色・角丸・余白) と 3D ビューの背景色 |
| UI | `src/ui/playful.rs` | 画面の状態を集めて `Progress::observe` に渡し、カード・マスコット・お祝い・紙吹雪を描く |

## ミッションの判定

UI 層は毎フレーム `Observation`(選択の有無、カメラの角度、最新の履歴の名前、計測・寸法の数、干渉している組の数、再生中か、保存済みか など)を作って渡すだけです。
`Progress::observe` は直前の `Observation` と比べて「変化したとき」だけ達成にします。

- 最初の観測は基準にするだけで、何も達成しない(起動直後の状態で勝手にクリアしない)。
- 見まわし: 3D ビューの上でマウスのボタンを押している間のヨー・ピッチの変化を足し、0.6 rad を超えたら達成。全体表示などの自動のカメラの動きは数えない。
- 編集: 履歴の名前が「◯◯ を編集」「◯◯ を追加」に変わったとき(ファイルの読み込みは数えない)。名前は `history.rs` が付ける。
- 干渉: 一度編集したあとで、干渉している組の数が増えたとき(サンプルに最初からある干渉は数えない)。
- 保存: ファイルに結びつき、未保存の変更がない状態になったとき。未保存かどうかは、ウィンドウのタイトルを更新するときに `ProjectState` が覚えておいた値を使う(モデルを比べ直さない)。

経験値は 150 XP ごとに 1 レベル上がります。称号は `level_title` にあります。

## 設定ファイル

テーマ・ミッションカードの表示・達成したミッション・経験値を `preferences.json` に保存します。変更があったフレームだけ書き出します。

| OS | 場所 |
| --- | --- |
| Windows | `%APPDATA%\cadsim\preferences.json` |
| macOS | `~/Library/Application Support/cadsim/preferences.json` |
| その他 | `$XDG_CONFIG_HOME/cadsim/preferences.json`(なければ `~/.config/cadsim/…`) |

```json
{
  "theme": "playful",
  "show_missions": true,
  "completed_missions": ["select_part", "orbit_view"],
  "xp": 100
}
```

ミッションは `key`(例: `select_part`)で保存するので、並び順を変えたり新しいミッションを足したりしても前の進み具合は残ります。知らない key と壊れた JSON は無視して標準の設定で始めます。

## 演出

- お祝い: egui の前面の `Area` に「ミッション達成!」を出す。出るときは ease-out-back で少し大きくなり、最後の 0.5 秒でふわっと消える(3.2 秒)。
- 紙吹雪: `confetti(seed, count, colors)` が初速と回転をもつ破片を作り、`ConfettiPiece::at(t)` が重力つきの位置を返す。描画は前面のレイヤーに回転した四角形を描くだけ。レベルアップと全クリアでは2倍の量にする。
- マスコット: 立方体のキャラクターを egui の `Painter` で描く。ふだんはゆっくり上下し、4 秒ごとにまばたき、お祝い中は跳ねて目が「^ ^」になる。

## ミッションを足すには

1. `gamify.rs` の `MissionId` と `MISSIONS` に足す(`key` は保存に使うので後から変えない)。
2. 判定に要る値を `Observation` に足し、`Progress::observe` の `reached` に条件を書く。単体テストを足す。
3. `ui/playful.rs` の `track_missions` でその値を集める。
4. README の「ミッションで操作を覚える」を更新する。
