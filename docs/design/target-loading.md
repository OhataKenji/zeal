# Target の段階的な取得

_Last update: 2026/11/02_

## 取得の流れ

Zeal は workspace を開いたとき、Scope 内の rule を `bazel query --output=label_kind` で取得する。
この結果にはツリーと検索に必要な label と rule class が入る。
取得した一覧をメモリに保持し、refresh が成功したときに一括で置き換える。

利用者が target を選ぶと、Zeal は現行の Scope query を XML 形式で実行する。
XML から選択 target の属性、入出力ファイル、定義位置、Scope 内の直接の使用側を求める。
UI へ渡すのは選択 target の詳細だけであり、XML 全体は保持しない。
一覧の先頭 target を自動選択しないため、クリック前に詳細 query は走らない。

両 query は build、test、run、Query Editor と同じ Bazel の排他枠を使う。
詳細取得中に別の target を選んだ場合、UI は最後の選択だけを待機させる。
workspace、Scope、一覧の世代、選択 target が変わった後に届いた詳細は表示しない。

## 判断の根拠

grpc/grpc `7088419`、Bazel 8.7.0、Scope `//...` の warm query を OpenTelemetry SDK で計測した。
8 組の交互実行では、現行 XML は中央値 1,326 ms、ツリー用 `label_kind` は 194 ms だった。
両方とも 11,398 件の label が一致し、出力はそれぞれ 67.5 MB と 0.91 MB だった。
この計測は Bazel の query 実行だけを含み、Zeal の画面表示や詳細クリック後の待ち時間は含まない。

選択 target の使用側を `rdeps` で取る案は採用しなかった。
grpc/grpc では Scope の依存閉包が外部 Python 依存まで評価され、query が失敗した。
現行の浅い XML query から逆向きの辺を選ぶ方法なら、これまでの表示と同じ意味を保てる。

Query Editor の外部 label と属性名の補完は、従来の全件 XML の辺に依存していた。
初回一覧にはその情報がないため、これらの入力は手入力とする。
Scope 内の target label と rule class の補完は一覧から作る。
