# Claude に画面を操作させる (MCP の操作口)

`plantwright --mcp` で起動すると、標準入力・標準出力で [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) の
サーバーとして動きます。Claude Code や Claude デスクトップアプリがこれをツールとして読み込み、
画面写真を見ながらボタンを押したり 3D ビューを回したりできます。使う人向けの設定の仕方は README の
「Claude に画面を操作させる(MCP)」にあります。

画面写真とマウス・キーボードだけで操作する汎用の Computer Use と比べて、次の点で確実です。

- ボタン・メニュー・タブは **表示名でクリック** できます。AI が座標を読み違えても外しません。
- `list_widgets` で、いま画面にある部品の表示名・種類・位置を文字で渡せます。
- 入力はアプリの中で本物と同じメッセージとして流すので、OS の権限 (画面収録・アクセシビリティ) が要りません。
  窓が後ろに隠れていても動きます。
- 読み込みや計算の途中は手順を進めず、終わるまで待ってから次の操作をします。

## ツール

| ツール | 引数 | すること |
| --- | --- | --- |
| `screenshot` | なし | 画面写真 (PNG) を返す |
| `list_widgets` | なし | 画面の部品の一覧 (種類・x・y・幅・高さ・表示名) を返す |
| `click` / `double_click` / `hover` | `label`、`nth` | 表示名の部品をクリック・ダブルクリック・マウスを乗せる。同じ名前が複数あれば上から `nth` 番目 |
| `click_at` | `x`、`y` | 座標をクリック (3D ビューの部品を選ぶとき) |
| `drag` | `from_x`、`from_y`、`to_x`、`to_y`、`button` | 押したまま動かして離す (3D ビューの回転・平行移動、ギズモ) |
| `scroll` | `x`、`y`、`lines` | ホイールを回す (拡大・縮小) |
| `key` | `keys` | キーを押して離す。`ctrl+z`・`shift+tab`・`f5`・`delete` など |
| `hold` | `keys`、`frames` | キーを押したままにする (歩行モードで歩く) |
| `type` | `text` | 選んでいる入力欄に文字を打つ |
| `open` | `path` | ファイルを開き、読み込みが終わるまで待つ |
| `wait` | `frames` | 待つ (60 フレームでおよそ 1 秒) |
| `run_script` | `script` | [UI チェックの手順書](ui-check.md) を何行かまとめて流す。`screenshot` の行ごとに写真を、`dump_widgets` の行ごとに一覧を返す |

`screenshot` と `list_widgets` と `run_script` 以外には `screenshot: true` を付けられます。付けると、操作のあとの画面写真も返します。

失敗したとき (表示名の部品が 3 秒待っても出てこない、など) は `isError: true` で理由を返し、そのときの画面写真も付けます。
表示名が見つからないときは、似た表示名の候補も理由に入ります。

### 座標

写真はウィンドウの **論理ピクセル** の大きさに縮めて返します (高解像度の画面では、撮った写真は 2 倍の大きさで届くため)。
写真の上の位置をそのまま `click_at`・`drag`・`scroll` に渡せます。`list_widgets` の位置も同じ座標です。

## しくみ

- 標準入力を読むスレッドが、1行ずつ `plantwright_core::ui_check::mcp::handle_line` に渡します。
  `initialize`・`tools/list`・`ping` と引数の誤りは、そのスレッドがすぐ返事をします。
- ツールの呼び出しは `tool_steps` で UI チェックの手順書の行 (`Step`) に直してから、毎フレームのシステムに渡します。
  システムは UI チェックと同じ `Runner` で手順を1つずつ進め、入力を流し、写真を撮ります。呼び出しは届いた順に1つずつ流します。
- 部品は egui のアクセシビリティ情報 (AccessKit) から集めます。UI チェックと同じく、`--mcp` のときだけ有効にします。
- 時計は止めず、ふつうに動かします (UI チェックのように 1 フレーム = 1/60 秒には固定しません)。
- 標準出力は MCP の返事だけに使います。Bevy のログは標準エラーに出ます。標準入力が閉じたらアプリを終えます。
- 保存ダイアログのような OS の窓は操作できません。

| 場所 | 中身 |
| --- | --- |
| `crates/plantwright_core/src/ui_check/mcp.rs` | JSON-RPC の読み取りと返事、ツールの一覧、引数から手順への直し方、写真の縮小と PNG・Base64 |
| `src/features/ui_check/mcp.rs` | 標準入力を読むスレッド、手順を流すシステム、写真の受け取り |

## 手で試す

MCP Inspector で、ツールを1つずつ呼んで結果を見られます。

```sh
cargo build
npx @modelcontextprotocol/inspector target/debug/plantwright --mcp
```

端末から直接 JSON を流しても試せます。

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_widgets","arguments":{}}}' \
  | (cat; sleep 20) | cargo run -- --mcp
```
