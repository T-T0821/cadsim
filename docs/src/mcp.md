# Claude に画面を操作させる(MCP)

Claude Code や Claude デスクトップアプリから、Claude に plantwright の画面を操作してもらえます。
「デモラインを開いて再生し、止まった所の写真を見せて」のように頼むと、Claude が画面写真を見ながらボタンを押したり 3D ビューを回したりします。
ボタンは表示名で押すので、画面の座標を当て推量するより確実です。

## 設定する

plantwright を `--mcp` を付けて起動する設定を、Claude 側に一度だけ書きます。plantwright は Claude が起動し、Claude を終えると閉じます。

- **Claude Code**: 端末で次を実行します(パスは plantwright.exe・plantwright.app を置いた場所に合わせてください)。

  ```sh
  claude mcp add plantwright -- C:\tools\plantwright.exe --mcp                          # Windows
  claude mcp add plantwright -- /Applications/plantwright.app/Contents/MacOS/plantwright --mcp   # Mac
  ```

- **Claude デスクトップアプリ**: 設定 → 開発者 → 設定を編集 で開く `claude_desktop_config.json` に足して、アプリを開き直します。

  ```json
  {
    "mcpServers": {
      "plantwright": { "command": "C:\\tools\\plantwright.exe", "args": ["--mcp"] }
    }
  }
  ```

開くファイルを続けて書くと(`--mcp line.plantwright`)、そのファイルを開いた状態で始まります。

## できること・気をつけること

- Claude が使える操作は、画面写真・ボタンなどの一覧・表示名でクリック・座標でクリック・ドラッグ・ホイール・キー・文字入力・ファイルを開く・待つ、と、UI チェックの手順書をまとめて流すことです。
- 保存ダイアログのような OS の窓は操作できないので、保存や書き出しの場所を選ぶところは手で操作してください。
- Claude が操作している間は、マウスやキーボードで同じ窓を触らないでください。

しくみは開発者向けの [docs/mcp.md](https://github.com/T-T0821/cadsim/blob/main/docs/mcp.md) にあります。
