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

### 実Bazelで確認する

Draft ReleaseのDMGをApplicationsへコピーし、Finderから起動します。
信頼できるworkspaceを開き、実際のBazelで次を確認します。

- targetを検索し、アプリからbuildとtestを実行して、成功結果とログを確認する。
- 実行中のbuildをキャンセルし、停止後にもう一度buildできる。
  キャンセル前に完了した場合は、十分に時間のかかるtargetでやり直す。
- アプリを再起動してもworkspaceとscopeが復元され、履歴と結果を閲覧できる。

Draft Releaseの説明に、確認したDMGとcommit、macOSとBazelのバージョン、workspaceのrevisionと使用したtarget、各項目の結果を短く記録します。
Bazeliskを使う場合も、対象workspaceで実行されたBazelのバージョンを記録します。
必要に応じてログや画像を添え、非公開情報は載せません。

すべて確認できたDMGをそのまま公開します。
未確認や失敗があれば公開せず、DMGを差し替えた場合は再確認します。
更新は新版DMGの手動インストールです。
