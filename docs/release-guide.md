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

Draft ReleaseのDMGをApplicationsへコピーし、Finderから起動します。
信頼できるworkspaceで、実Bazelを使って確認します。

- buildとtestが成功し、結果とログを見られる。
- 実行中のbuildをキャンセルでき、その後もbuildできる。
- 再起動後もworkspace、scope、履歴が残る。

Releaseの説明にBazelのバージョン、使用したworkspaceとtarget、各項目の結果を記録し、すべて確認したDMGを公開します。
更新は新版DMGの手動インストールです。
