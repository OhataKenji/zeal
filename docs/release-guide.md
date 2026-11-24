# Release guide

macOS (Apple Silicon) 向けのDMGと、Linux (x86_64、ARM64) 向けのAppImageと.debを配布します。
Apple Developer Programへの加入やSecretsの登録は不要です。

## Create a release

1. `src-tauri/tauri.conf.json`の`version`と`src-tauri/Cargo.toml`のpackage versionを同じ新しい版に更新します。
   リポジトリのルートで次のコマンドを実行し、更新された`Cargo.lock`も含めてmainへマージします。

   ```sh
   cargo update --manifest-path src-tauri/Cargo.toml --workspace
   ```

2. GitHubの`Actions → Release → Run workflow`で`main`を選んで実行し、その実行対象コミットの`CI`と`Build`が成功していることを確認します。
3. Release workflowの実行が成功するのを待ち、作成されたDraft ReleaseからDMGと、x86_64とARM64両方のAppImageと.debをすべてダウンロードして下の確認を行います。
4. Releaseの説明に変更内容を追記し、`Publish release`を押します。
   タグは自動で作成されるため、先に作る必要はありません。

Linuxジョブだけが失敗した場合は、Draft Releaseを公開せず、同じ実行の「Re-run failed jobs」でLinuxジョブを再実行します。

## Verify the DMG

このDMGはad-hoc署名で、Appleの公証はありません。
初回起動時にmacOSがブロックする場合があります。
配布元を信頼できる場合だけ、システム設定の「プライバシーとセキュリティ」から起動を許可します。
詳細は[Tauriのad-hoc署名ガイド](https://v2.tauri.app/distribute/sign/macos/#ad-hoc-signing)を参照してください。
Releaseの説明にも、公証がないことを残してください。

- Applicationsへコピーし、Finderから起動できる。

## Verify Linux packages

x86_64とARM64の各環境で、対応するパッケージを確認します。

- AppImageは`chmod +x`して実行できる。
- `.deb`は`sudo apt install ./*_$(dpkg --print-architecture).deb`のようにローカルファイルを指定してインストールでき、アプリケーションメニューまたは`zelith`コマンドで起動できる。

## Verify on both platforms

- Bazelを検出し、target検索、build、キャンセルが動く。
- 再起動後も履歴が残る。

すべてのパッケージを確認できたら、そのまま公開します。
更新は各プラットフォームで対応するパッケージ（macOSはDMG、LinuxはAppImageまたは.deb）の手動インストールです。
