# Release guide

Apple Silicon向けのDMGを配布します。
Apple Developer Programへの加入やSecretsの登録は不要です。

## Create a release

1. `src-tauri/tauri.conf.json`の`version`と`src-tauri/Cargo.toml`のpackage versionを同じ新しい版に更新します。
   リポジトリのルートで次のコマンドを実行し、更新された`Cargo.lock`も含めてmainへマージします。

   ```sh
   cargo update --manifest-path src-tauri/Cargo.toml --workspace
   ```

2. GitHubの`Actions → Release → Run workflow`で`main`を選んで実行し、その実行対象コミットの`CI`と`Build`が成功していることを確認します。
3. 作成されたDraft ReleaseからDMGをダウンロードし、下の確認を行います。
4. Releaseの説明に変更内容を追記し、`Publish release`を押します。
   タグは自動で作成されるため、先に作る必要はありません。

## Verify the DMG

このDMGはad-hoc署名で、Appleの公証はありません。
初回起動時にmacOSがブロックする場合があります。
配布元を信頼できる場合だけ、システム設定の「プライバシーとセキュリティ」から起動を許可します。
詳細は[Tauriのad-hoc署名ガイド](https://v2.tauri.app/distribute/sign/macos/#ad-hoc-signing)を参照してください。
Releaseの説明にも、公証がないことを残してください。

### 実Bazelによる手動受け入れ確認

Draft Releaseからダウンロードした候補DMGを使い、以下を手動で確認します。
これは確認手順であり、実施済みの証拠やCIのカバレッジを示すものではありません。

信頼できる既知の公開workspaceを用意し、正確なrevisionを固定します。
buildが成功するtarget、成功するtest target、実行中にキャンセルできる十分に長いbuild targetを選び、各labelを記録します。
キャンセル用buildは、キャッシュ未使用など、即座に完了しない条件を用意して記録します。
候補のcommitとDMGファイル名、macOSのバージョンとアーキテクチャ、アプリが実際に使うBazelのバージョンも記録します。
Bazelisk経由の場合は、Bazeliskのバージョンだけでなく、対象workspaceで解決されたBazelのバージョンを記録します。

1. 候補DMGのアプリをApplicationsへコピーし、Finderから起動します。
2. 用意したworkspaceを開き、Bazelが検出され、指定したscopeで期待するtargetが見つかることを確認します。
3. アプリからbuildを実行し、成功結果とログを確認します。
4. アプリからtestを実行し、テストの成功結果がアプリに表示されることを確認します。
5. 用意した長いbuildをアプリから開始し、実行中にキャンセルします。
   buildの実行が停止したことを確認し、続けて別のbuildを実行して成功することを確認します。
   キャンセル前に完了した場合は合格にせず、条件を変えて再確認します。
6. 候補アプリを終了してFinderから再起動し、同じworkspaceとscopeが復元され、先ほどの履歴と結果を閲覧できることを確認します。

各項目の結果と証拠をDraft Releaseの説明に以下の形式で残します。
スクリーンショットとログは公開前に確認し、私的なパス、認証情報、非公開データを除いたものへのリンクだけを記載します。
未加工の私的なログは添付しません。

```text
実Bazel手動受け入れ確認
実施日: <YYYY-MM-DD>
確認者: <公開可能な名前>
候補commit / DMG: <完全なSHA> / <ファイル名>
macOS / arch: <バージョン> / <アーキテクチャ>
Bazel: <実際のバージョン、Bazel直接またはBazelisk経由>
公開workspace URL / revision: <URL> / <完全なSHA>
scope: <対象pattern>
build / test / cancel用label: <label> / <label> / <label>
cancel用buildの条件: <キャッシュ未使用などの条件と使用した引数>
各項目は PASS / FAIL / 未実施 と、サニタイズ済み証拠リンクを記入:
1. Finder起動: <結果、リンク>
2. workspace / Bazel検出 / target発見: <結果、リンク>
3. build成功とログ: <結果、リンク>
4. test成功表示: <結果、リンク>
5. 実行中キャンセルと停止 / 次のbuild成功: <各結果、リンク>
6. 再起動後のworkspace / scope / 履歴と結果: <各結果、リンク>
```

各項目がすべてPASSになった同じDMGだけを公開します。
失敗または未実施の項目があれば公開せず、候補DMGを差し替えた場合はすべて再確認します。
更新は新版DMGの手動インストールです。
