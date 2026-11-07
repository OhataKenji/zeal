# Target の段階的な取得

_Last update: 2026/11/02_

## 取得の流れ

Zeal は workspace を開いたとき、Scope 内の rule を `bazel query --output=label_kind` で取得する。
この結果にはツリーと検索に必要な label と rule class が入る。
取得した一覧をメモリに保持し、refresh が成功したときに一括で置き換える。

利用者が target を選ぶと、Zeal はその rule と直接依存する source file だけを proto 形式で取得する。
Bazel の query schema から生成した型で decode し、選択 target の属性、入出力ファイル、定義位置を求める。
壊れた byte 列は error とし、`--keep_going` の正常な部分結果はこれまでどおり受理する。
既定の出力では `select()` の scalar 属性が値なしで届くことがあり、その場合は関係を作らない。
UI へ渡すのは選択 target の詳細だけであり、query 結果全体は保持しない。
一覧の先頭 target を自動選択しないため、クリック前に詳細 query は走らない。

両 query は build、test、run、Query Editor と同じ Bazel の排他枠を使う。
詳細取得中に別の target を選んだ場合、UI は最後の選択だけを待機させる。
workspace、Scope、一覧の世代、選択 target が変わった後に届いた詳細は表示しない。

## 判断の根拠

grpc/grpc `7088419`、Bazel 8.7.0、Scope `//...` の warm query を OpenTelemetry SDK で計測した。
8 組の交互実行では、旧実装の Scope 全体の XML は中央値 1,326 ms、ツリー用 `label_kind` は 194 ms だった。
両方とも 11,398 件の label が一致し、出力はそれぞれ 67.5 MB と 0.91 MB だった。
この計測は Bazel の query 実行だけを含み、Zeal の画面表示や詳細クリック後の待ち時間は含まない。

使用側の表示要件を取り下げたため、選択時に Scope 全体を再取得する必要がなくなった。
選択した rule の属性に加え、直接の source file を取得して入力ファイルを正しく分類する。

変更前は Zeal `e304207`、変更後は同じ revision に本変更を適用した debug build を使った。
両方とも grpc/grpc `7088419`、Bazel 8.7.0、Scope `//...`、一覧取得後の warm cache で、`//:add_port` と `//:alts_util` を各版で計 8 回選択した。
旧版の順序は alts / add / add / alts / add / alts / add / alts、新版は add / alts の交互だった。
OTEL は各選択の `target.detail.request` span の終了時刻と開始時刻の差を取った。
画面観測は操作ツールの click 開始から、macOS の accessibility tree に選択 target の見出しが現れるまでを測った。操作ツールの待ち時間も含む。
旧実装の `target.detail.request` は中央値 3,573 ms (範囲 1,969–5,046 ms)、変更後は 161 ms (158–217 ms) だった。
画面で詳細を確認できるまでの観測値は中央値 3,739 ms (2,057–5,147 ms) から 562 ms (538–678 ms) になった。
OTEL 値は backend の所要時間であり、画面表示時間は別の観測値である。
変更前は測定中に負荷が揺れたため、前後の中央値は参考値とする。変更後の `bazel.query` は中央値 160 ms、`snapshot.build` は 0.1 ms 以下だった。

Query Editor の外部 label と属性名の補完は、従来の全件 XML の辺に依存していた。
初回一覧にはその情報がないため、これらの入力は手入力とする。
Scope 内の target label と rule class の補完は一覧から作る。
