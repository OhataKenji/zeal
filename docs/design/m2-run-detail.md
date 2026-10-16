# Zeal 設計 M2: 一回の run の説明

_OhataKenji_
_Last update: 2026/10/16_
_Status: Approved_

## Goal

一回の run について、phase(loading / analysis / execution)の流れと、実際に実行された action の記録を提示し、利用者が自分のコマンドと Bazel の内部処理を結びつけられるようにする(PRD シナリオ2・M2)。

### Non Goals

- remote cache・remote execution の詳細な内訳
- action の内部ログ(stdout/stderr の中身)の再生
- 「実行され得たが起きなかった action」の提示(action graph の候補集合との照合)

## Background

Bazel の一回の呼び出しは、定義を読む loading、処理を組み立てる analysis、出力を得る execution の phase で進む(Bazel の評価モデル)。
この build が内部で何をしたかを示す観測源は役割が異なる。

- **BEP**: 呼び出し単位のイベント列。呼び出しの成否と target ごとの結果を与える
- **exec log**: 実際に起こった spawn の記録。action 単位の owner target・mnemonic・出力・runner・cache 情報を持つ
- **profile**: phase の区切りと時系列の計測結果(action ごとの位置と所要時間を含む)

重要な前提として、local のみの環境では action cache hit は spawn に至らず、exec log には出ない。BEP には件数だけが出る。
「再利用」を個々の action の cache hit として観測できるのは remote/disk cache 使用時に限られる。
したがって local 環境で見せられるのは「実際に実行された action」と再利用の件数だけであり、どの action が実行を免れたかは観測できない。観測できないものは推測せず「不明」とするのがこの文書の核である。

## Overview

run の起動時に観測用の flag を付けて bazel を spawn し、成果物を run の dir に残す。run の詳細を開くたびに BEP・exec log・profile を parse する。
action の表示は exec log の spawn 記録(実際に実行されたもの)に限定し、cache hit は spawn 記録が示す場合にのみ付記する。
実行中は timing を出せないため(mock どおり `not available`)、stdout/stderr の log だけを streaming する。

```mermaid
flowchart LR
    SEND["Send(composer)"] --> SPAWN["bazel spawn<br/>+ 観測 flag"]
    SPAWN -->|"実行中: stdout/stderr"| LOG["log 欄(streaming)"]
    SPAWN -->|"完了後"| ART["成果物<br/>bep.json / execlog / profile.gz"]
    ART --> INGEST["parse(表示のたび)"]
    INGEST --> VIEW["run 詳細: timing / log / phase / action 一覧"]
```

## Detailed Design

### Send の範囲(M2)

M2 の Send は `build` / `test` だけを起動する。stop・中断・`bazel run` は [m3](./m3-find-and-run.md) で扱う。
bazel が使用中(snapshot の query 中か run 中)の間は Send を disabled にし、待ち行列は持たない。

### 付与する flag

run 起動時に以下の固定セットを付与する(必須/optional の区別はしない)。成果物は run の UUID ディレクトリ(`<app data dir>/ws-<fnv64>/runs/<uuid>/`)に出力させる。

- `--build_event_json_file=<dir>/bep.json`
- `--execution_log_binary_file=<dir>/execlog`
- `--profile=<dir>/profile.gz`
- `--experimental_profile_include_primary_output`
- `--noslim_profile`
- `--tool_tag=zeal`

失敗の扱いは一本に絞る。`unknown option` 等で run が起動しない場合(exit 2)も含め、失敗は失敗の run として記録するだけで、自動リトライも観測 flag を落とした再実行も行わない。flag が受理されない version は v1 の対象外であり、実際に発生したら Zeal 側のバグとして対応する。
v1 の対象は Bazel 8.7 以上を目安とし、この flag 群が受理されることを前提とする。flag の旧名対応や version ごとの対応表は持たない。parse 側も version 差の読み分けは持たない。

### 実行中の表示

log 欄は spawn した子プロセスの stdout/stderr を runner が常時 drain し、1 行ずつ event で流す。
bazel 自身が進捗行(`[34 / 120] …` 系)を stderr に出すため、mock の実行中画面はこれだけで足り、BEP file の tail のような構造化経路は別途持たない。
timing 欄は exec log/profile が完了後にしか出ないため、実行中は `not available` と表示する。

### 保存と parse

M2 では SQLite を入れない。run ごとに次の file を置き、run の詳細を開く(`get_run`)たびに 3 成果物を parse する。parse 結果は保存しない。

```
<app data dir>/ws-<fnv64(正規化した workspace path)>/runs/<uuid>/
  run.json  log.txt              # Zeal が書く
  bep.json  execlog  profile.gz  # bazel が書く。手を加えない
```

- `run.json`: id・workspace・cmd・target・開始/終了時刻・exit code。run の成否は exit code で決める
- BEP: target ごとの結果(test 結果を含む)と action cache hit の件数。行単位で parse し、末尾が欠けていても読める範囲を採用する
- exec log: spawn ごとの action(owner target・mnemonic・primary output・runner・cache hit の有無)
- profile: phase marker と、action ごとの位置・所要時間

成果物ごとの状態(ok / incomplete / missing / parse error)は parse のたびに決め、詳細画面のセクションごとの可用性に反映する。
DB への run record の保存と lifecycle(retention・導出記号)は [m4](./m4-run-history.md) が持つ。

### action の記録(exec log)

action の一覧は exec log の spawn 記録だけで作る。「実行され得たが起きなかった action」は観測源がなく示さない。
exec log は巨大になり得るため streaming で読み、spawn ごとに action の行(owner target・mnemonic・primary output の exec path・runner・cache hit の有無)を作る。
action の所要時間は一覧に出さず、timing の tooltip でだけ見せる。

- **cache hit**: `cache_hit` が true、または `runner` が ` cache hit` で終わる場合に付記する。disk/remote の区別まではしない(Non Goal 参照)が、runner の生値はそのまま出す
- **local の再利用**: local の action cache hit は exec log に出ないが、BEP の `actionCacheStatistics.hits` に件数が出る。run 単位で「action cache hit: N 件(内訳は不明)」と出す
- 同一 exec path に複数の spawn 記録が来ることがある(flaky test の retry 等)。行はまとめず記録どおりに残し、実行回数として数えられるようにする

action 間を入力/出力の artifact で結ぶグラフ(PRD の「入力と出力を結ぶ Action のグラフ」)は M2 の表示対象に含めない。spawn 記録と owner target の往復までを提供し、action 間の辺を辿る UI は後続の検討とする。

### timing 表示

mock の frame chart(exec-0..3 の行と bar)は profile だけで描く。
profile の action event が名前・mnemonic・位置・所要時間・スレッドを持つので、スレッドごとに行を作り、bar の tooltip に名前と所要時間を出す。
exec log とは join しない。test の primary output は exec log に無く、disk cache hit の開始時刻は前回の実行時刻のままで、照合のキーにならないためである。
phase の帯は profile の phase marker を Bazel の名前のまま並べる。Bazel 8 の既定(Skymeld)では marker が `Evaluate target patterns` → `Load, analyze dependencies and build artifacts` → `Complete build` となり、analysis と execution は重なって進むため分けられない。帯にはその旨を注記する。
フィールドが欠けた場合は、該当部分だけを落として timing 表示を degrade する。

### target・phase・action の結び方

run の詳細では、target から関連する action 一覧へ、action から owner target へ遡れる。
phase 内訳と action 一覧を同じ run に紐づけることで、「loading→analysis→execution」の流れと個々の action の記録を行き来できる。

## Alternatives Considered

- **BEP だけで完結させる**: BEP の action 実行イベントはデフォルトで失敗分しか出ず(全件には `--build_event_publish_all_actions` が要る)、cache hit の判別情報も持たない。exec log のほうが per-action の判別に必要な情報を網羅しており、二本立てとした。
- **profile から action を推定する**: profile は計測用であり網羅的でない。「観測できないものは不明と示す」原則に反して精度を偽ることになる。
- **aquery で候補集合を取り「実行されなかった action」を分類する**: 「実行され得た vs 実際起きた」の対比は再利用の理解に資するが、完了後に別呼び出しで action graph を再推定する機構(quote・config 再現・照合・ずれ検出)は設計全体で最も複雑であり、mock にも対応する画面がない。実装の単純さを優先して v1 では削り、「再利用」は exec log で観測できる cache hit(remote/disk cache 使用時)に限定して示す。local のみの環境では再利用の件数だけを示し、内訳は不明と表示する。

## Caveats

- exec log と profile は呼び出し完了後にしか得られない。exec log は完了時に書き出されるため、中断・強制終了では file 自体が出ないことが多い。実行中の画面は log のみになる。
- build が execution に達しない run では action 一覧は作れない。analysis の失敗では exec log は 0 byte、loading の失敗では file 自体が作られない。どちらも「phase 未到達」として action なしの部分 detail にする。
- 観測 flag が受理されない場合 run は失敗として記録される(v1 の対象外。実際に発生したら Zeal 側のバグとして対応する)。
- 観測 flag には overhead がある(profile の計測、exec log の書き出しサイズ)。M2 では常時付与とし、肥大が問題になれば run 単位で off にする選択肢を検討する。

## Test Plan

- cache hit のフィールド、profile の event、disk cache 使用時の挙動は spike(Bazel 8.7.0)で確かめ、本文に反映した。
- exec log・BEP・profile の各 parser に golden file test を用意する。fixture には末尾が欠けた BEP、execution 未到達の run、成果物の一部が欠けた run を含める。
- 観測 flag 付きで build を回し、成否と所要時間への影響(overhead)を確認する。

## spike 後の改訂(2026/10/16)

Bazel 8.7.0 での spike の結果とオーナーの決定に合わせて、次を改めた。

- timing は profile だけで描き、exec log と join しない(`args.out` の記述を削除)
- action の所要時間は timing の tooltip でだけ見せ、action 一覧には出さない
- exec log の `walltime` / `metrics` の presence による読み分けと、「cache 判別不能」の注記を削除(field 17 は reserved)
- cache hit の判定を 1 条件にし、local の action cache hit は BEP の件数だけを「内訳は不明」として出す
- phase の帯は Bazel の marker 名のままにし、分けられない旨を注記する
- M2 では SQLite を入れず、run dir の file を表示のたびに parse する。DB は m4 で扱う
- M2 の Send は build / test だけ。stop・中断・`bazel run` は m3 で扱う
