# 生成 AI (Claude) を使う機能の設計

タイミングチャート・ラダーの自動生成など、生成 AI を使う機能に共通する部分のしくみ。

## 層の分け方

[architecture.md](architecture.md) に合わせ、問い合わせ口をポートにして、Claude API の呼び出しはアダプターに置く。

| 層 | 場所 | 内容 |
| --- | --- | --- |
| ユースケース (ポート) | `cadsim_core::usecase::llm` | `LlmClient` トレイト、問い合わせ (`LlmRequest`)・失敗 (`LlmError`) の型、返事から JSON を取り出す `extract_json`、テスト用の `ScriptedLlm` |
| アダプター | `cadsim_io::claude` | `ClaudeClient`: Messages API への HTTPS の問い合わせ ([ureq](https://crates.io/crates/ureq) + rustls)。送る JSON の組み立て (`request_body`) と返事の読み取り (`parse_response`) は通信と切り離してテストする |
| UI と設定 | `src/assistant/` | `settings_file.rs`(API キーとモデルの保存、Bevy なし)、`job.rs`(問い合わせを別スレッドで待つ `LlmJob`、Bevy なし)、`mod.rs`(`AiAssistant` リソース)、`ui.rs`(「AI の設定」ウィンドウ) |

新しく生成 AI を使う機能を足すときは、

1. 問い合わせの組み立てと返事の読み取りを Bevy に依存しないユースケースの関数にし、`&dyn LlmClient` を受け取る。テストは `ScriptedLlm` で決まった返事を返して書く。
2. 画面からは `AiAssistant::client()` で呼び出し口を受け取り、`LlmJob::start` で別スレッドに投げ、毎フレーム `poll` で結果を見る。
3. 新しい道具なら `cadsim_core::workspace::Tool` に足す (ツールレールとメニューの「ツール」に並び、「AI に聞く」の説明にも入る)。

## Claude API への問い合わせ

- 送り先は `POST https://api.anthropic.com/v1/messages`、ヘッダーは `x-api-key`・`anthropic-version: 2026-10-21`。
- 既定のモデルは `claude-opus-5-5`。「AI の設定」で Sonnet 5.5・Fable 5.1 にも切り替えられる。
- 考える過程は `thinking: {type: "adaptive"}` でモデルに任せ、深さは `output_config.effort`(`Effort::Low` / `Medium` / `High`)で変える。
- JSON で返してほしいときは `LlmRequest::json_schema` に JSON Schema を入れる。`output_config.format` の構造化出力になり、返事の形が保証される。
- 安全上の理由で断られたときは、`fallbacks: "default"`(beta ヘッダー `server-side-fallback-2026-07-01`)で別のモデルが答え直す。それでも断られたら `LlmError::Refused`。
- `stop_reason` が `max_tokens` なら途中で切れているので `LlmError::Truncated` にする(切れた JSON を読もうとしない)。
- 1回の問い合わせは 10 分まで待つ。プロキシは環境変数 (`HTTPS_PROXY` など) に従う。

## API キーの保存

- 置き場所は見た目の設定と同じフォルダの `ai.json`(Windows: `%APPDATA%\cadsim\ai.json`、macOS: `~/Library/Application Support/cadsim/ai.json`、その他: `~/.config/cadsim/ai.json`)。Unix では自分だけが読める権限 (600) にする。
- プロジェクトファイル (.cadsim) には書かない。
- 保存したキーがなければ環境変数 `ANTHROPIC_API_KEY` を使う。
- 画面にはキーの末尾 4 文字だけを出す。

## チャート・ラダーの自動生成 (`src/sequence_gen/`)

| 層 | ファイル | 内容 |
| --- | --- | --- |
| ドメイン | `spec.rs` | 動作の下書き `SequenceSpec`(装置・センサー・ステップを名前で指す)。チャートからの変換 `from_chart`、チャートへの反映 `apply`、読める文章にする `describe` |
| ドメイン | `ladder.rs` | チャート → 三菱向けステップラダー (`Ladder`) と I/O 表。命令リストのテキスト、GX Works 用 CSV、I/O 表の CSV / TSV |
| ドメイン | `log_events.rs` | PLC のログ (CSV) → 信号が変わった所だけの一覧 (`LogSummary`) |
| ユースケース | `usecase.rs` | `generate`(説明文 → 下書き)、`explain`(チャート・ラダー・ログ → 説明文)。`&dyn LlmClient` を受け取る |
| アダプター | `json.rs` | 下書きと JSON の変換、生成 AI に渡す JSON Schema |
| UI | `mod.rs`, `ui.rs` | `SequenceGen` リソースと「チャート・ラダーの自動生成」ウィンドウ |

`TimingChart` がまだアプリ側にあるので、ここもアプリ側に置いている。チャートが `cadsim_core` に移ったら、Bevy に依存しない 4 つのファイルも移す。

### 説明文 → 下書き

1. 今のチャートを `from_chart` で下書きの形にし、JSON にして説明文と一緒に送る。生成 AI は今の装置の名前を使い回せる。
2. 返事は構造化出力 (`json.rs` の `SCHEMA`) で JSON に決め、`json::parse` で下書きに戻す。
3. 画面ではチャートの写しに `apply` してみて、足す装置・読み飛ばした動作 (名前の見つからない装置など)・反映後の I/O 表を先に見せる。
4. 「チャートに反映」で本物のチャートに `apply` する。チャートは元に戻す履歴に載っている (`ChartDoc`) ので Ctrl+Z で戻る。

`apply` の決まり:

- 装置・センサーは名前で突き合わせる (前後の空白と大文字・小文字は無視)。ないものは足し、今あるものは部品の割り当てごと残す。
- シリンダの目標は 0 / 1 に丸め、直動・回転は可動範囲に収める。動作時間が 0 以下なら装置の標準の動作時間。
- ロボットの移動は扱わない (「置き換える」とロボットの移動のあるステップも消える)。

### チャート → ラダー

生成 AI は使わず、ステップを決まった型の回路に置き換える。

| デバイス | 内容 |
| --- | --- |
| X0 / X1 / X2 | 起動 / 停止 / 異常リセット |
| X3〜 | シリンダの出端・戻端、軸の位置決め完了、ロボットの完了、センサー (16 進で詰めて振る) |
| Y20〜 | シリンダの電磁弁 (シングルソレノイド)、軸・ロボットの起動、異常ランプ、運転中ランプ。入力が 32 点を超えたら、入力と重ならない 16 の倍数から |
| D100〜 / D200〜 | 軸の目標位置 (0.1 単位、PLC 連携のワードと同じ) / ロボットの教示点番号 (1 始まり) |
| M0 / M1 / M2 | 運転中 / サイクル中 / 異常 |
| M100〜 / M200〜 / M300〜 | ステップ中 / 条件成立 / 完了。ステップが 100 を超えたら 200・400・600 番台のように幅を広げる |
| T0〜 / T100〜 | ステップのタイマー (dwell) / 条件待ちの異常検出。100 ms タイマーで K は 0.1 秒単位 |

ステップ i の回路:

1. 開始条件: `LD ステップ中 → AND/ANI センサー → OUT 条件成立`
2. 動作: `LD 条件成立 → SET/RST 電磁弁、MOV 目標位置 + SET 起動`
3. 完了: `LD 条件成立 → AND 出端/戻端/位置決め完了 → OUT タイマー (なければ OUT 完了)`、`LD タイマー → OUT 完了`
4. 異常: `LD ステップ中 → ANI 条件成立 → OUT T K50`、`LD T → SET M2` (チャート再生の `INTERLOCK_TIMEOUT` と同じ 5 秒)
5. 次へ: `LD 完了 → RST ステップ中 → RST 起動 → SET 次のステップ中`。最後のステップは運転中 (M0) なら最初に戻る

軸の「位置決め完了」は、起動で OFF になり目標位置に着いたら ON になる前提にしている。

GX Works 用 CSV は、命令リストの CSV の列 (ステップ番号・行間ステートメント・命令・I/O(デバイス)・ブランク・PIステートメント・ノート) に合わせ、2 つ目からのオペランドは次の行に書く。文字コードは Shift_JIS。実際の GX Works2 / GX Works3 で読み込めることはまだ確かめていない。読み込めないときは命令リストのテキストを見ながら入力する。

### 動作の説明

- チャートは `describe` の文章、ラダーは貼られたテキストのまま、ログは `log_events::summarize` で変化だけにして送る。
- ログの変化は 3000 件まで送り、超えた分は送らないことを画面と問い合わせの両方に書く (黙って切らない)。
- 送る材料は 40 万文字まで。超えたら分けてもらう。

## 言葉でラインを組む

文から設備の配置とタイミングチャートの下書きを作る。しくみは [line-compose.md](line-compose.md)。生成 AI の口はポート `LineDraftAi` (`cadsim_core::usecase::compose_line`) で、`cadsim_io::line_draft_json::LlmLineDrafter` が `LlmClient` で実装する。API キーがない・失敗したときは文のキーワードから作る。
