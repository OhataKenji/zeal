# Release guide

Apple Silicon向けのDMGを配布します。
Apple Developer Programへの加入やSecretsの登録は不要です。

## リリースする

1. `src-tauri/tauri.conf.json`の`version`を更新し、mainへマージします。
   Cargo側の版も揃える場合は、`src-tauri/Cargo.toml`と`src-tauri/Cargo.lock`の`zeal`を更新します。
2. 対象コミットの`CI`と`Build`が成功したら、GitHubの`Actions → Release → Run workflow`で`main`を選びます。
   実行対象のコミットがCIに成功していることを確認してください。
3. 作成されたDraft ReleaseからDMGをダウンロードし、下の確認を行います。
4. Releaseの説明に変更内容を追記し、`Publish release`を押します。
   タグは自動で作成されるため、先に作る必要はありません。

## 起動と動作の確認

このDMGはad-hoc署名で、Appleの公証はありません。
初回起動時にmacOSがブロックする場合があります。
配布元を信頼できる場合だけ、システム設定の「プライバシーとセキュリティ」から起動を許可します。
詳細は[Tauriのad-hoc署名ガイド](https://v2.tauri.app/distribute/sign/macos/#ad-hoc-signing)を参照してください。
Releaseの説明にも、公証がないことを残してください。

- Applicationsへコピーし、Finderから起動できる。
- Bazelを検出し、target検索、build、キャンセルが動く。
- 再起動後も履歴が残る。旧版がある場合は上書き更新でも確認する。

Finder起動でBazelを検出できない場合は、公開せず修正します。
GUIアプリはシェル設定の`PATH`をそのまま引き継ぎません。
確認したDMGをそのまま公開します。更新は新版DMGの手動インストールです。

## 失敗したら

一時的な失敗は、作成途中の未公開Draft Releaseがあれば削除し、元の実行画面から`Re-run jobs`を選びます。
既存のReleaseやタグと同じ版では停止します。
公開済みの版は変更せず、修正を新しい版番号で配布します。
