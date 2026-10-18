# Zeal 設計 M3: target を見つけて実行する

_OhataKenji_
_Last update: 2026/10/18_
_Status: Approved_

## Goal

target の label を正確に覚えていなくても、検索、tree、pattern から対象を見つけ、`build` / `test` / `run` を実行できるようにする(PRD シナリオ3と M3)。

### Non Goals

- bazel flag、プログラム引数、複数 label/pattern の指定
- bazel query 言語全般のサポート
- 複数 run の並列実行と実行キュー
- stdin を必要とする対話的な run
- Windows

## Background

共通用語と構成は [overview](./overview.md) に従う。
CLI では label の記憶と補完に頼るため、mock の composer と dropdown を使い、検索から実行までをつなぐ。
索引は [M1](./m1-targets.md) の snapshot、実行と保存は [M2](./m2-run-detail.md) の runner を使う。

## Overview

実行の入口は composer(cmd 選択、target 欄、Send)のみとする。
tree と dropdown の選択は target 欄への文字列挿入であり、別の実行モードを持たない。
cmd は利用者が `build` / `test` / `run` から選ぶ。
入力を label 列へ展開せず Bazel に渡し、対象と操作の最終判断を委ねる。

```mermaid
flowchart LR
    pick["検索 / tree / pattern"] --> input["target 欄"]
    input --> send["Send: 入力制約を確認"]
    send --> record["開始 record を保存"]
    record -->|"保存成功後に対象を1引数で渡す"| bazel["Bazel を実行"]
    bazel --> finish["終了 record を保存"]
```

## Detailed Design

### 索引と検索

検索には [Fuse.js](https://github.com/krisk/Fuse)(npm: `fuse.js`)の安定版を使い、UI 側で snapshot の label と file 名を候補として渡す。
照合、スコアリング、一致文字位置の取得はライブラリに任せ、自作しない。
`ignoreLocation: true` で長い label の末尾も検索し、`includeMatches: true` の一致位置から highlight を描画する。
入力に応じて候補を絞り、rule class を添える。
↑↓で移動し、Enter で target 欄全体を置き換え、Esc で候補を閉じる。
file は owner への導線とし、`Parser.java → //app/lib:parser` の形で表示する。
owner が複数なら、M1 の tree と同じく label 昇順の先頭を挿入する。

### 入力

target 欄で送れるのは label または pattern を表す1つの文字列だけとする。
空文字、空白文字(スペース、タブ、改行を含む)、制御文字(NUL を含む)、先頭が `-` の入力は拒否する。
分割、quote、escape の解釈は行わず、複数の対象指定や追加引数を受け付けない。
実行時は cwd=workspace root とし、対象文字列全体を shell を介さず `Command.arg` の1引数として渡す。

`//...` / `:all` / `:*` を含む label/pattern の解釈と構文エラーは Bazel に委ねる。

### composer と実行

Send の条件は、workspace と bazel が利用可能、取り込みや起動処理中ではない、対象が上の入力条件を満たすこととする。
Rust core は cmd を `build` / `test` / `run` に限定し、観測 flag は Zeal が組み立てる。
利用者から受け取る実行パラメータは cmd と対象文字列だけとし、任意の引数配列や自由形式のコマンド行は受け取らない。
条件は UI と Rust core の両方で検証し、stdin は閉じる。

| 状態     | 操作                                                                      |
| -------- | ------------------------------------------------------------------------- |
| idle     | 上の条件を満たせば Send。snapshot の有無には依存しない                    |
| running  | Send は stop になる。次の run と workspace 切替は受けず、query は保留する |
| stopping | 終了確認まで running と同じ抑止を続け、未確認のまま idle へ戻さない       |

取り込み中は「target データの取得待ち」と示し、完了後に自動送信しない。
取得失敗後も取り込みが終われば手入力を送れ、古い snapshot や部分結果には時刻と状態を付ける。
spawn 後、最初の出力が一定時間なければ「出力待ち」と示す。
原因、異常、対象プログラムの開始前後は判定せず、最初の出力後は timer を再開しない。
cmd と入力文字列を run record へ保存し、組み立てた argv は永続化しない。
開始 record の保存に失敗した場合は spawn しない。

### 中断

stop は独立したプロセスグループへ SIGINT を送り、数秒の猶予後も応答がなければ SIGKILL を送る。
対象 binary も同じグループで回収する。
SIGKILL 後も終了未確認なら「停止を確認できません」と示し、stopping のまま終了確認と回収を続ける。
終了確認後に record を更新して idle へ戻り、保留中の取り込みを許可する。
wait がエラーになっても、handle と stop、drain、実行と query の抑止を維持する。
以前のセッションの process に signal は送らず、この中断手順は現在保持している handle にだけ適用する。

## Alternatives Considered

- **query 言語を入力する**: target を探すための言語習得を要求するため、snapshot の検索と pattern に絞る。

## Caveats

- hard kill 後も server の処理が残る場合や、対象 binary が daemonize してグループを抜ける場合は回収しきれない。
- 外部の Bazel が lock を持つ場合も原因は断定せず、届いた log を示す。他クライアントを止める `bazel shutdown` は行わない。

## Security Concerns

引数配列による実行と process group の管理は [overview](./overview.md#security-concerns) に従う。
固定 cmd、Zeal が生成する flag、対象の単一引数を別々に渡し、shell や文字列評価を使わない。
空白拒否は入力の単純化であり、shell injection の防御は [Command.arg の逐語的な引数渡し](https://doc.rust-lang.org/std/process/struct.Command.html#method.arg)による。
実行先の target 自体はリポジトリのコードであり、この入力制限でその動作を隔離するものではない。

## Privacy Concerns

入力と出力は local の run 記録に保存し、共有や自動送信は設けない。
子プロセスの既存設定と通信は [overview](./overview.md#privacy-concerns) に従う。

## Test Plan

- 長い label の末尾検索、一致位置の表示、キーボードでの候補確定、file から owner への挿入を検証する。案内なしに検索、tree、pattern から対象を見つけ、操作を選んで実行できれば PRD M3 の到達とする。
- label と pattern を展開せず1引数で渡し、build/test/run の成功とエラーをそのまま表示できることを実際の Bazel で確かめる。
- UI と core で空白、改行、NUL、先頭 `-`、許可外 cmd を拒否し、対象が変更なく1引数になることを検証する。shell 記号は解釈せず、取り込み中の抑止、取得失敗後、cmd 選択保持、初回出力前後の無出力も確認する。
- SIGINT と SIGKILL のグループ回収、wait エラーと終了未確認時の送信抑止、再起動時に以前の process へ signal を送らないことを検証する。

## 入力検証の実装

実行は build/test に限る。target は label または pattern の1文字列とし、空文字、空白、制御文字、先頭の `-` を拒否する。
shell を介さず1引数で渡し、trim、分割、quote の解釈はしない。
