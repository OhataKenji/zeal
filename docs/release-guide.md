# Release guide

Apple Silicon向けのDMGを配布します。
Apple Developer Programへの加入やSecretsの登録は不要です。

## Create a release

1. `src-tauri/tauri.conf.json`の`version`、`src-tauri/Cargo.toml`のpackage version、`src-tauri/Cargo.lock`の`zeal`のversionを同じ新しい版に更新し、mainへマージします。
2. 対象コミットの`CI`と`Build`が成功したら、GitHubの`Actions → Release → Run workflow`で`main`を選びます。
   実行対象のコミットがCIに成功していることを確認してください。
3. 作成されたDraft ReleaseからDMGをダウンロードし、下の確認を行います。
4. Releaseの説明に変更内容を追記し、`Publish release`を押します。
   タグは自動で作成されるため、先に作る必要はありません。

## Verify the DMG

このDMGはad-hoc署名で、Appleの公証はありません。
初回起動時にmacOSがブロックする場合があります。
配布元を信頼できる場合だけ、システム設定の「プライバシーとセキュリティ」から起動を許可します。
詳細は[Tauriのad-hoc署名ガイド](https://v2.tauri.app/distribute/sign/macos/#ad-hoc-signing)を参照してください。
Releaseの説明にも、公証がないことを残してください。

- Applicationsへコピーし、Finderから起動できる。
- Bazelを検出し、target検索、build、キャンセルが動く。
- 再起動後も履歴が残る。

すべて確認できたDMGをそのまま公開します。
更新は新版DMGの手動インストールです。
