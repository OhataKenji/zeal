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
   パッケージのビルド前に、そのバージョンのドラフトが1つだけ選択または新規作成されます。
   同じバージョンのタグまたは公開済みReleaseが既にある場合は失敗するため、新しいバージョンを使います。
   同じタグ名のドラフトが複数ある場合も、どれかを選ばずに失敗します。
3. このバージョンの最新のRelease実行で、macOS、Linux x86_64、Linux ARM64の3環境すべてが成功したことを確認します。
   同じバージョンで再実行すると、各ビルドが完了した時点でドラフトの同名ファイルが置き換わります。
   更新中や一部のビルドが失敗した状態では公開せず、失敗したジョブは「Re-run failed jobs」で再試行します。
4. ドラフトのタグの対象コミットが、手順3の実行のコミットSHAと一致することを確認します。
   別のコミットから再実行しても、既存ドラフトのタグの対象は自動では変わりません。
   一致しない場合は公開しません。
   対象を修正できない場合は、新しいバージョンで手順1からやり直します。
5. そのドラフトからDMG 1個と、Linuxの各アーキテクチャ向けのAppImageと`.deb`（計5ファイル）をダウンロードし、以下の手順で検証します。
6. 5ファイルすべてを確認したら、Releaseの説明に変更内容を追記し、`Publish release`を押します。
   検証中に同じバージョンを再実行した場合は、手順3からやり直します。
   タグは公開時に作成されるため、先に作る必要はありません。

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
- ダウンロードしたZelithの`.deb`をファイル名で指定してインストールし（`sudo apt install ./ファイル名.deb`）、アプリケーションメニューまたは`zelith`コマンドで起動できることを確認します。

## Verify on both platforms

- Bazelを検出し、target検索、build、キャンセルが動く。
- 再起動後も履歴が残る。

更新は各プラットフォームで対応するパッケージ（macOSはDMG、LinuxはAppImageまたは.deb）の手動インストールです。
