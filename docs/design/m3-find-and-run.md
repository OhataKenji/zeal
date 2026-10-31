# Zeal 設計 M3: target を見つけて実行する

_OhataKenji_
_Last update: 2026/10/31_
_Status: Approved_

## Goal

既知の一つのリポジトリで、検索、tree、pattern から対象を見つけ、`build` / `test` / `run` を実行できる試作を作る。
PRD シナリオ3の「label を覚えていなくても対象を見つけて実行できる」を先に確かめる。

### Non Goals

- 子孫プロセスの完全な回収と終了不明からの自動復旧
- 実行後の自動 refresh、query の予約、実行キュー
- bazel flag、プログラム引数、複数 label/pattern の指定
- query 言語全般、Windows、複数リポジトリでの動作保証

## Overview

実行の入口は composer(cmd 選択、target 欄、Send)のみとする。
tree と dropdown は target 欄を置き換え、cmd は利用者が `build` / `test` / `run` から選ぶ。
入力の展開と対象の最終判断は Bazel に委ねる。

```mermaid
flowchart LR
    pick["検索 / tree / pattern"] --> input["target 欄"]
    input --> send["入力を検証しIDを予約"]
    send --> bazel["Bazel を起動"]
    bazel -->|"共通BES receiver"| result["BEP事実とログ込みeventを保存"]
    result --> view["保存したログと結果を表示"]
```

## Detailed Design

### 索引と検索

[M1](./m1-targets.md) の snapshot に含まれる target を fuzzysort に渡す。
部分列と空白区切りの複数語で検索し、照合、スコア、一致位置を自作しない。
検索候補は target のフル label のみとし、rule class を添える。
選択した label をそのまま target 欄に挿入する。

↑↓で候補を選び、Enter で target 欄全体を置き換える。
Enter は候補の確定だけに使い、矢印キーを操作した履歴を持たない。
label と pattern の実行は Send に統一する。
候補が閉じている場合と候補がない場合、入力欄の Enter は何もしない。
Esc は候補を閉じる。
IME 変換中のキー操作では確定や送信を行わない。

### 入力と起動

target 欄は label または pattern を表す1文字列とする。
空文字、空白文字、制御文字、先頭が `-` の入力を UI と core で拒否する。
cmdは `build` / `test` / `run` を受け付ける。
対象全体を shell を介さず `Command.arg` の1引数で渡し、分割、trim、quote の解釈をしない。
`//...` / `:all` / `:*` の意味と構文エラーは Bazel に委ねる。

workspace と bazel が利用可能で、取り込みと実行のどちらも動いていなければ Send を受け付ける。
snapshot がなくても手入力で実行できる。
ID予約、擬似端末(**PTY**)、Bazelイベントを受けるreceiverの準備に失敗したらspawnしない。
cwdはworkspace root、stdinはPTYで扱い、観測flagはZealが組み立てる([M2](./m2-run-detail.md))。

### ログと更新

live出力はPTYのConsoleへ表示し、端末出力を `terminal.log` に残す。
履歴はBuild Event Service(**BES**) receiverが、Bazelのイベント列(**BEP**)から保存する。
process終了からビルド結果を補完しない。
Bazel clientのwaitはlive実行枠を解放し、UIへ状態を通知する。
保存済みのBEPとprofileは詳細要求時に読む。

target の取り込みは workspace を開いたときと手動 refresh のみ行う。
query または run の実行中の refresh は予約せず拒否し、run が終わっても自動取り込みしない。
workspace 切替も処理中は拒否する。
古い snapshot と部分結果は状態と時刻を示す。

### 中断と不明な結果

stop は現在保持しているプロセスグループへ SIGINT を送り、3秒後も run を保持していれば SIGKILL を送る。
さらに2秒経っても終了を確認できなければ「停止を確認できません」と示し、実行枠を解放する。
wait エラーでも理由を示して実行枠を解放する。
この場合、processの終了は不明と示す。
履歴のビルド結果はBEPの事実を維持し、processの状態から補完しない。
自動再実行、自動 query、無期限の復旧ループを持たず、残存プロセスへの対処を利用者に任せる。
以前のセッションのプロセスには signal を送らない。
アプリ終了時も現在の run に限り SIGINT と短い猶予後の SIGKILL を試みる。

## Caveats

- build/test の子孫が残る場合もある。停止は best-effort であり、client の終了は子孫すべての終了を意味しない。
- 子孫が残っても、受信済みBEPのログにその後の出力が含まれるとは限らない。Consoleや履歴で全出力の保存を保証しない。
- 実行枠を解放しても残存プロセスは止まらない。再実行と refresh は利用者が状況を確認して行う。
- 外部の Bazel が lock を持つ場合も原因を推測しない。他クライアントを止める `bazel shutdown` は行わない。
- 大きな snapshot と大量のログの表示は、この試作の保証範囲に含めない。

## Security and Privacy

入力制約と shell を介さない引数渡しは [overview](./overview.md#security-concerns) に従う。
成果物は local の run ディレクトリに保存し、Zeal から外部へ共有しない。
対象のコード自体を隔離するものではないため、既知のリポジトリで試す。

## Test Plan

- 検索、tree、pattern から対象を選び、build/test の結果を確認する。
- 入力制約、IME 操作、cmd 選択保持、取り込み失敗後の手入力を確認する。
- PTYのConsoleとBES保存、準備失敗時のspawn抑止を検証する。
- 子孫が出力先を保持しても client の終了確認を妨げないことを検証する。
- SIGINT と SIGKILL、wait エラー、停止未確認時の不明表示と枠解放を検証する。
- 実行後と停止未確認時に query や再実行を自動で起動しないことを検証する。
- Enter は候補確定だけを行い、確定後の再入力や候補なしの Enter で実行しないことを検証する。
