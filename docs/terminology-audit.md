# Zeal 用語監査レポート (2026-11-02)

> 追記: 本レポートの A 項リネーム(run→invocation)は適用済み。
> 以下の「現状」列とファイル名・行番号の言及はリネーム前の状態の記録である。

対象: `src/` (49 ファイル), `src-tauri/` (31 ファイル + `proto/bes.proto`),
`tests/`, `docs/`, README, 設定ファイル — 全行精査。

## 結論

「1回の Bazel コマンド実行とその観測記録」を指す名詞は **run ではなく invocation**
に改めるべき。Bazel 公式・BuildBuddy・bb-portal の用語体系ではこの概念は
invocation で統一されており、Zeal 自身のコード・設計書・テストも一貫して
「実体は invocation」と認めている。

### 根拠

- Bazel 公式: BEP は "a Bazel invocation" の観測手段。実行単位は
  `invocation_id` (UUID) で識別され、`--invocation_id` フラグと BES
  `StreamId.invocation_id` に現れる。
- BuildBuddy: `/invocation/` URL、"Invocations" 一覧、"Invocation details" タブ。
- bb-portal (buildbarn): "Invocations are the primary unit of interest"、
  `/bazel-invocations` エンドポイント、invocation を build で束ねるモデル。
- Zeal 自身が invocation と呼んでいる箇所:
  - `types.ts:88` — `/** An invocation and the facts observed in its build events. */ interface RunRecord` (型名とコメントの矛盾)
  - `store/database.rs:202` — `reserve_invocation(data, run_id)` (関数名は invocation、引数は run、挿入先は `run` テーブル)
  - `runargs.rs:44` — run.id を `--invocation_id=` として Bazel に渡す
  - `state.rs:148` — "At most one bazel invocation is active"
  - `query.rs:56` — "one ad-hoc bazel query invocation"
  - `store.rs:1`, `store/events.rs:1`, `store/analytics.rs:1` — "Bazel invocation facts"
  - テスト名: `real_bazel_records_invocation_*`, `unobserved_invocation_*`,
    `run.test.ts:83` "an unobserved invocation"
- 設計書の自己矛盾:
  - `overview.md:32` が `run: 一回のBazel invocation` と定義する一方、
    `m4-run-history.md:12,25,74` は invocation / invocation_uuid を使用。
    `overview.md:125` の `runs/<run_id>/` と `m4:74` の `runs/<invocation_uuid>/`
    は**同じパスを別名で記述**している。
  - `m2-run-detail.md` はタイトル「一回の run の説明」でありながら
    Non Goals (L19) に「`bazel run`」を掲げており、タイトルと非目標が衝突。
  - `query-editor.md:157` は「この Run は composer の run(build・test)ではなく…」
    と語義衝突を弁明する注記を必要としている。
- UI 上の衝突: 履歴ペイン見出し "Runs" (App.svelte:217) の直上に
  `bazel run` を意味する "Run" ボタン (App.svelte:70) がある。
- "terminal runs" (README:37) は `bazel run` と PTY の `terminal.log`
  両方と語義が衝突する。

---

## 情報源

| 参照                                                                                | 用語の裏付け                                                                                                                                   |
| :---------------------------------------------------------------------------------- | :--------------------------------------------------------------------------------------------------------------------------------------------- |
| Bazel 公式ドキュメント — Build Event Protocol (bazel.build/remote/bep)              | "a Bazel invocation"; イベントは `BuildStarted`→`BuildFinished` の lifecycle; "The payload… a build event identifier is unique within a build" |
| BEP Glossary (bazel.build/remote/bep-glossary)                                      | `BuildStarted`=command の metadata、`BuildFinished`=exit code、`TestSummary`/`TestResult` の区別 (TestResult は shard×run×attempt 単位)        |
| `build_event_stream.proto` (本リポジトリ `src-tauri/proto/bazel/` 収録の上流 proto) | `BuildStarted.command`/`uuid`、`PatternExpandedId`/`PatternSkippedId`、`TestSummary.overall_status`、test status enum                          |
| googleapis `publish_build_event.proto` (BES)                                        | `StreamId{build_id, component, invocation_id}`、`invocation_attempt_started/finished`                                                          |
| Bazel 評価モデル (bazel.build/extending/concepts)                                   | loading → analysis → execution phase、Skymeld                                                                                                  |
| BuildBuddy (app.buildbuddy.io / docs / GitHub README)                               | "Invocations"、タブ名 Targets/Details/Artifacts/Timing/Cache/Raw、フィールド名 Command/Pattern/Duration/Status、"Trends"                       |
| bb-portal (github.com/buildbarn/bb-portal, wiki)                                    | "Invocations are the primary unit of interest"、`/bazel-invocations`、Profile ボタン、"Invocations can be grouped into builds" (build_id)      |
| Bazel `--profile` 出力                                                              | chrome-trace フィールド ("build phase marker", counter 名)                                                                                     |
| Bazel exit code 表                                                                  | exit code 8 = interrupted                                                                                                                      |

---

## 修正部分

### A. run → invocation (コアリネーム)

#### A-1. 型・識別子

| 場所                                  | 現状                                                            | 改定案                                                                                             |
| :------------------------------------ | :-------------------------------------------------------------- | :------------------------------------------------------------------------------------------------- |
| `src/lib/types.ts:89`                 | `interface RunRecord`                                           | `InvocationRecord`                                                                                 |
| `src/lib/types.ts:131`                | `interface RunDetail`                                           | `InvocationDetail`                                                                                 |
| `src/lib/types.ts:86`                 | `type RunCmd = "build" \| "test" \| "run"`                      | `type BazelCommand` (値は変更なし)                                                                 |
| `src/lib/types.ts:75`                 | `consoleRun?: string`                                           | `consoleInvocation`                                                                                |
| `src/lib/types.ts:78`                 | `runNote: string \| null`                                       | `invocationNote`                                                                                   |
| `src/lib/run.ts:4,10`                 | `RunStatus`, `runStatus()`                                      | `InvocationStatus`, `invocationStatus()`                                                           |
| `src/lib/session.ts:37-39,129,314`    | `runs`, `runsError`, `selectedRun`, `loadRuns()`, `selectRun()` | `invocations`, `invocationsError`, `selectedInvocation`, `loadInvocations()`, `selectInvocation()` |
| `src/lib/analytics.ts:30,126-135`     | `targets[].runs`, `slot.runs`                                   | `invocations`                                                                                      |
| `src-tauri/src/state.rs:60`           | `struct RunId`                                                  | `InvocationId`                                                                                     |
| `src-tauri/src/state.rs:99`           | `struct ActiveRun`                                              | `ActiveInvocation`                                                                                 |
| `src-tauri/src/state.rs:33,35`        | `running: Option<RunId>`, `console_run`                         | `active_invocation: Option<InvocationId>`, `console_invocation`                                    |
| `src-tauri/src/store/database.rs:100` | `struct RunRecord`                                              | `InvocationRecord`                                                                                 |
| `src-tauri/src/detail.rs:16`          | `struct RunDetail`                                              | `InvocationDetail`                                                                                 |
| `src-tauri/src/detail.rs:37,94`       | `run_detail`, `run_profile`                                     | `invocation_detail`, `invocation_profile`                                                          |
| `src-tauri/src/lib.rs:24-27`          | `list_run_records`, `read_run_record`, `open_run_artifact`      | `list_invocation_records`, `read_invocation_record`, `open_invocation_artifact`                    |
| `src-tauri/src/store/database.rs:180` | `run_dir()`, パス `runs/<id>`                                   | `invocation_dir()`, パス `invocations/<id>`                                                        |
| `src-tauri/src/store/database.rs:202` | `reserve_invocation(data, run_id)`                              | 引数を `invocation_id` に統一                                                                      |
| `src-tauri/src/store/events.rs:52`    | `let run_id = id(&stream.invocation_id)`                        | `let invocation_id = ...`                                                                          |
| `src-tauri/src/store/analytics.rs:41` | `TargetDuration { runs }`                                       | `{ invocations }`                                                                                  |
| `src-tauri/src/runner.rs`             | `st.run`, `clear_run`, `signal_run`, `kill_run`, `"run slot"`   | `ActiveInvocation` 系に追従                                                                        |
| `src-tauri/src/state.rs`              | `st.run`, `run_finished` Condvar                                | `st.active_invocation`, `invocation_finished`                                                      |

#### A-2. Tauri IPC コマンド (frontend/backend 同時改名)

| `src-tauri/src/lib.rs`   | `src/lib/commands.ts`              | 改定案                   |
| :----------------------- | :--------------------------------- | :----------------------- |
| `start_run` (L140)       | `startRun: "start_run"`            | `start_invocation`       |
| `stop_run` (L162)        | `stopRun: "stop_run"`              | `stop_invocation`        |
| `list_runs` (L226)       | `listRuns: "list_runs"`            | `list_invocations`       |
| `get_run` (L289)         | `getRun: "get_run"`                | `get_invocation`         |
| `get_run_profile` (L320) | `getRunProfile: "get_run_profile"` | `get_invocation_profile` |

`preview.ts` のモックハンドラも同時に改名。

#### A-3. DB スキーマ

| `src-tauri/src/store/database.rs` | 現状                          | 改定案                         |
| :-------------------------------- | :---------------------------- | :----------------------------- |
| L161                              | `CREATE TABLE run`            | `CREATE TABLE invocation`      |
| L165                              | `INDEX run_workspace_started` | `invocation_workspace_started` |
| L162                              | カラム `target_raw`           | `target_pattern` (後述 B-1)    |
| L183, 209, 319                    | ディスク上 `runs/<id>/`       | `invocations/<id>/`            |

#### A-4. UI 表示文字列

| 場所                               | 現状                                                     | 改定案                                                       |
| :--------------------------------- | :------------------------------------------------------- | :----------------------------------------------------------- |
| `App.svelte:217`                   | ペイン見出し `Runs`                                      | `Invocations`                                                |
| `App.svelte:488`                   | `Resize target and run panes`                            | `...invocation panes`                                        |
| `RunView.svelte:108`               | `Run details`                                            | `Invocation details` (または BuildBuddy 準拠の `Details`)    |
| `RunView.svelte:115`               | `Run ID`                                                 | `Invocation ID`                                              |
| `RunView.svelte:84,189`            | `Could not load run details`, `after the run finishes`   | invocation に                                                |
| `AnalyticsView.svelte:126,133,157` | `Runs`, `Runs per day`, `N run(s)`                       | `Invocations`, `Invocations per day`, `N invocation(s)`      |
| `AnalyticsView.svelte:204-212`     | `Total run time per day`                                 | `Total invocation time per day`                              |
| `AnalyticsView.svelte:241-249`     | `Per-day run duration percentiles`                       | `Per-day invocation duration percentiles`                    |
| `AnalyticsView.svelte:309`         | 列見出し `Runs`                                          | `Invocations`                                                |
| `QueryView.svelte:175,639`         | `a build/test run is in progress`, `The last run failed` | `a Bazel invocation is in progress`, `The last query failed` |
| `session.ts:522`                   | `The run started, but getting its state failed…`         | `The invocation started,…`                                   |
| `preview.ts:269,280,287,394`       | `Preview run not found`, `Run ended with exit N`         | invocation に                                                |

#### A-5. ファイル名・CSS・span・fixture

| 現状                                                                                   | 改定案                                                                          |
| :------------------------------------------------------------------------------------- | :------------------------------------------------------------------------------ |
| `src/lib/run.ts`, `src/lib/run/`                                                       | `src/lib/invocation.ts`, `src/lib/invocation/`                                  |
| `RunView.svelte`, `RunHeader.svelte`, `RunStatus.svelte`                               | `InvocationView.svelte` 等                                                      |
| `src/styles/run.css`, `.run-*` クラス群一式                                            | `invocation.css`, `.invocation-*` (または短く `.inv-*`)                         |
| `run-arguments-popover`, `run-arguments` id                                            | `command-arguments-*` (build/test でも使う引数欄のため)                         |
| trace span `run.start`, `run.stop`, `runs.list`, `run.detail.*`, `run.profile.request` | `invocation.*` 系                                                               |
| `src-tauri/tests/fixtures/run/`                                                        | `fixtures/invocation/` (bep.rs:425, profile.rs:334 が参照)                      |
| `docs/design/m2-run-detail.md`, `m3-find-and-run.md`, `m4-run-history.md`              | `m2-invocation-detail.md`, `m3-find-and-execute.md`, `m4-invocation-history.md` |

#### A-6. ドキュメント・README

| 場所                                                                                           | 現状                                                                                                       | 改定案                                                                                                                     |
| :--------------------------------------------------------------------------------------------- | :--------------------------------------------------------------------------------------------------------- | :------------------------------------------------------------------------------------------------------------------------- |
| `docs/design/overview.md:32`                                                                   | `run: 一回のBazel invocationと、その観測記録全体`                                                          | **用語定義の基点。ここを直す**: `invocation: 一回のBazelコマンド呼び出しと、その観測記録全体`                              |
| `overview.md:37,60-61,125` 他                                                                  | run record、runごとのBEP、`runs/<run_id>/`                                                                 | invocation record、`invocations/<invocation_id>/` (m4:74 の `invocation_uuid` と統一)                                      |
| `README.md:5,25,29,31,37,39`                                                                   | run history / Target and Run / Run Page / a run's / terminal runs・run counts・run time / daily run counts | invocation history / Invocation Page / an invocation's / terminal invocations・invocation counts / daily invocation counts |
| `README.md:56`                                                                                 | `## Record terminal builds`                                                                                | `## Record terminal invocations` (test/run も記録対象のため)                                                               |
| `docs/prd.md:56,105,110,113`                                                                   | 名詞としての run                                                                                           | invocation (日本語なら「呼び出し」と既存表現を統一)                                                                        |
| `m4-run-history.md`, `m5-analytics.md`                                                         | `run` テーブル記述、グラフ見出し `Runs`/`Wait time`                                                        | `invocation` テーブル、`Invocations`/`Total duration`                                                                      |
| テストタイトル (session.test.ts 約17件, query.test.ts:565,625, preview_analytics.test.ts:9 他) | 名詞 run ("terminal run", "a new run", "untimed runs")                                                     | invocation                                                                                                                 |

### B. run 以外の用語修正

| #    | 場所                                                                     | 現状                                                   | 問題                                                                                                                                                             | 改定案                                                            |
| :--- | :----------------------------------------------------------------------- | :----------------------------------------------------- | :--------------------------------------------------------------------------------------------------------------------------------------------------------------- | :---------------------------------------------------------------- |
| B-1  | `database.rs:107,162`, `types.ts:93`, `analytics.rs:18`, `Filter.target` | `target`, `target_raw`                                 | 格納するのは `//...`/`:all` 等を含む空白区切りの **target pattern**。events.rs:302 の `target_patterns()` 側は正しく pattern と呼ぶ。BuildBuddy 表記も "Pattern" | `pattern` / `target_pattern`                                      |
| B-2  | `bep.rs:117-122`                                                         | `struct TestResult { label, status }`                  | BEP の `TestResult` は shard×run×attempt 単位のメッセージ。この型は `TestSummary.overall_status` を写すもので名前が衝突                                          | `TestSummary`                                                     |
| B-3  | `model.ts:35,80`                                                         | `DirNode.rules`, `Row.kind: "rule"`                    | ツリーの子要素は rule ではなく target                                                                                                                            | `targets`, `"target"`                                             |
| B-4  | `RunView.svelte:111-112`                                                 | `<dt>Command</dt>` が `bazel {cmd} {target}` を表示    | command (動詞) と command line の混同                                                                                                                            | 動詞のみを "Command" として表示し、行全体は "Command line" に分離 |
| B-5  | `WorkspacePrompt.svelte`                                                 | `//...` の探索範囲                                     | main repository の pattern であり、外部 repository を含む workspace 全体ではない                                                                                 | `main repository`                                                 |
| B-6  | `AnalyticsView.svelte:203`, `m5:23-25`                                   | `Wait time`                                            | "Wait time" はキュー待ち時間 (queue time) の意味。実際は日別の合計 duration                                                                                      | `Total duration`                                                  |
| B-7  | `LogPanel.svelte`                                                        | BEP の Progress 出力                                   | build log は Bazel の build/test/run の文脈でも使える。出力元を明確にする                                                                                        | `Bazel output` など                                               |
| B-8  | `QueryView.svelte:452-457`                                               | `Copy command`                                         | コピーするのは動詞ではなく command line 全体                                                                                                                     | `Copy command line`                                               |
| B-9  | `RunView.svelte:149`                                                     | `Bazel arguments`                                      | options のみを表示 (target は含まない)                                                                                                                           | `Bazel options` / `Command options`                               |
| B-10 | `bes.rs:255`, `README:37`                                                | `terminal builds`, `terminal runs`                     | `bazel run` とも PTY (`terminal.log`) とも衝突                                                                                                                   | `terminal invocations` / `invocations recorded from the terminal` |
| B-11 | `QueryView.svelte:483`, `query-editor.md`                                | Query Editor の `Run` ボタン                           | 動詞用法としては許容範囲だが、設計書が「composer の run ではない」と弁明を要するほど衝突している                                                                 | `Execute` / `Evaluate` (推奨: Execute)                            |
| B-12 | `target.ts`, `target.test.ts`                                            | `Enter a target`, `A target cannot contain whitespace` | 入力は label または pattern を受理する                                                                                                                           | `Enter a target or pattern` (軽微)                                |
| B-13 | `query-editor.md:17`                                                     | `Sky Query`                                            | 公式表記は `SkyQuery`                                                                                                                                            | `SkyQuery` (軽微)                                                 |

### C. 任意の改善 (優先度低)

| #   | 場所                           | 現状                                                                                                      | 提案                                                                                                                                                                              |
| :-- | :----------------------------- | :-------------------------------------------------------------------------------------------------------- | :-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| C-1 | `store/events.rs:45-51`        | BES `StreamId.build_id` を検証のみで破棄                                                                  | bb-portal は invocation を build で束ねる。CI 連携の拡張性のため `build_id` カラム追加を検討                                                                                      |
| C-2 | `lib.rs:170` vs `query.rs:132` | span 名 `query.run` / `query.exec` が同一操作で分裂、`run.detail` / `detail.bep_parse` も命名体系が不統一 | `query.exec` 系に統一 (run が多義なので)                                                                                                                                          |
| C-3 | `bes.rs:18,48`                 | `Session`/`MAX_SESSIONS`                                                                                  | BES の語彙では `Stream`/`MAX_STREAMS`                                                                                                                                             |
| C-4 | `lib.rs:390`                   | `history-changed` イベント                                                                                | アプリ内語彙としては一貫。BuildBuddy 準拠なら `invocations-changed`                                                                                                               |
| C-5 | `runner.rs`, `runargs.rs`      | モジュール名                                                                                              | subprocess 実行役として `runner` は許容 (`runner.rs:1` の doc 内名詞 run は要修正)。改名するなら `executor.rs`/`invocation_args.rs`。CI Runner と混同する懸念はあるが必須ではない |
| C-6 | `types.ts:104`                 | `Section` status `"incomplete"`                                                                           | Bazel test status `INCOMPLETE` と同形。リスク低                                                                                                                                   |
| C-7 | `build.rs`                     | `compile BES protocol`                                                                                    | BEP proto + BES service の両方をコンパイル。表記を `BES/BEP` に                                                                                                                   |

---

## 保つ部分 (rename 対象外・改名禁止)

以下は Bazel 用語として正しい、または Bazel とは無関係の「run」のため、
機械的一括置換で巻き込んではいけない。

| 区分               | 対象                                                                                                                                                                                                                                                                                                                                                                                                                |
| :----------------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `bazel run` 動詞   | `RunCmd` の**値** `"run"`、`<option>run`、`cmd === "run"` の分岐、actionLabel `Run` (cmd が run の時のみ)、`COMMANDS = ["build","test","run"]` (runargs.rs:11)、`--` 後のプログラム引数、`real_bazel.rs:114` の `command("run", ...)`、docs 内の `` `bazel run` `` 表記                                                                                                                                             |
| プロセス状態       | `running`/`stopping` の状態値と `setRunning`、"● running"/"■ stopping" ラベル、`ingest_running`/`query_running`/`detail_query_running`/`bazel_busy`、"a query is already running"、"re-run the query"                                                                                                                                                                                                               |
| 手続き実行の動詞   | `run_query`/`runQuery`、QueryView の `run()`、`runner.rs`/`terminal.rs` の subprocess spawn 系、`zeal_lib::run()`、Tauri `RunEvent`、`mise run`/`pnpm run`/`cargo run`、CI `run:`/`runs-on`、テストの `run_timezone_case`、`check_gui_run`                                                                                                                                                                          |
| 非 Bazel の "run"  | `search.ts` の "runs of text" (run-length の run)、`analytics.ts:127 let run` (折れ線の連続区間)                                                                                                                                                                                                                                                                                                                    |
| BEP/BES 分離       | `bep.rs` (build_event_stream parse) / `bes.rs` (gRPC `PublishBuildEvent` サービス) / `proto/bes.proto` / `besError`/`besReady`/"BES receiver" — 模範的な分離。プロトコル名とサービス名の混同なし                                                                                                                                                                                                                    |
| `attempt`          | BES 再送試行、`attempt/<n>/bep.binpb` ディスクレイアウト、DB カラム `attempt`、`received_complete` — 全て正しい BES semantics                                                                                                                                                                                                                                                                                       |
| phase/profile      | "build phase marker"、"Launch Blaze"/"Initialize command"/"Evaluate target patterns" 等の verbatim phase 名、chrome-trace フィールド、counter 名 ("action count", "CPU usage (Bazel)" 等 — BuildBuddy 準拠)、"Timing" タブ名 (BuildBuddy の tab と一致)、"Profile timeline"                                                                                                                                         |
| test 語彙          | `TestStatus` マッピング (NO_STATUS..TOOL_HALTED_BEFORE_TESTING)、`overall_status`、outcome `ok`/`failed`/`interrupted` (exit 8)                                                                                                                                                                                                                                                                                     |
| label/pattern 語彙 | canonical label、canonical name (`@@`) と apparent name (`@`) の区別、`is_external` (@repo//)、`is_select_key` (`//conditions:default`)、`$`/`:` 始まりの implicit attr、`kind()` の `<rule_class> rule` 形式、"target pattern"、"label or pattern"、`PATTERN_HINTS`                                                                                                                                                |
| workspace 語彙     | `is_workspace` (MODULE.bazel/WORKSPACE 判定)、`bazelisk`→`bazel` 解決、`--output_user_root`、"Workspace" 表示                                                                                                                                                                                                                                                                                                       |
| command 語彙       | `cmd` フィールド (BEP `BuildStarted.command` の動詞)、`validate_cmd`/`unsupported command`、"Command" フィルタ、`aria-label="Bazel command"`                                                                                                                                                                                                                                                                        |
| その他正しい表現   | "Original/Canonical command line"、"Explicit/Effective command options"、"startup options" (OptionsParsed 準拠)、"artifact" (bep.binpb/profile.gz/terminal.log)、"Started"/"Finished"/"Exit code"/"Wall time"、"Targets"/"Details"/"Timing" タブ、"Build phases"/"Skymeld"、Bazel フラグ列 (`--bes_*`, `--tool_tag`, `--invocation_id` 他全て正確)、query 関数カタログ (deps/rdeps/somepath/kind/attr/... 全て実在) |

---

## 実施上の注意

1. **起点は `docs/design/overview.md:32`** — `run` の定義を `invocation` に
   改めることが前提。用語定義を先に確定させる。
2. **IPC 契約**: Tauri コマンド名・DTO フィールド名 (`consoleRun`,
   `runNote`, `running`)・イベント名は frontend/backend で同時に変える。
   `preview.ts` のモック層も同期が必要。
3. **一括置換の危険**: 「保つ部分」に挙げた `bazel run` 動詞・プロセス状態・
   非 Bazel の run (run-length、連続区間) を巻き込まないよう、
   語義ごとに分けてレビューする。
4. **UI 表記**: "Invocations" は初学者にやや難しい語だが、PRD の目的
   「Bazel に詳しい人の見方を GUI で学ぶ」と一致し、BuildBuddy/bb-portal と
   同じ語でドキュメント・エコシステムとの往還がしやすい。

## 改定後の用語標準 (案)

| 用語                          | 定義                                                           | 用法                                                                                              |
| :---------------------------- | :------------------------------------------------------------- | :------------------------------------------------------------------------------------------------ |
| **Invocation**                | `bazel <command>` 1 回の実行とその観測記録 (BEP, profile, log) | 型: `InvocationRecord`/`InvocationDetail`/`InvocationId`。UI: "Invocations" 一覧、"Invocation ID" |
| **Command**                   | サブコマンド動詞 (`build`/`test`/`run`/`query`)                | 型: `BazelCommand`、フィールド `cmd`。UI: "Command"                                               |
| **Run**                       | 実行。Bazel では `bazel run` や test の run も指す             | コマンドの記録は invocation と呼び、文脈に応じて使う                                              |
| **Attempt**                   | BES ストリームの再送番号、flaky test の再試行                  | `attempt/` ディレクトリ、DB カラム                                                                |
| **Pattern**                   | `//...`, `:all` や単一 label などの target pattern             | 複数 pattern を連結した記録は pattern 列として区別する                                            |
| **Target**                    | label で識別される rule、file、package group など              | ツリーは rule target に限定。詳細と結果一覧では対象を明示                                         |
| **Build**                     | ビルド処理、またはその結果。`bazel build` はコマンド名         | `bazel test` や `bazel run` に含まれるビルドにも使える                                            |
| **Workspace**                 | main repository と定義された外部 repository を含む環境         | workspace root は main repository のルート                                                        |
| **Duration / Total duration** | invocation の所要時間                                          | "Wait time" はキュー待ちを示唆するため使わない                                                    |
