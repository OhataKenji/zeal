# Release guide

1. `src-tauri/tauri.conf.json`と`src-tauri/Cargo.toml`のバージョンをそろえ、次のコマンドで更新した`Cargo.lock`と一緒に`main`へマージします。

   ```sh
   cargo update --manifest-path src-tauri/Cargo.toml --workspace
   ```

2. `Actions → Release → Run workflow`で`main`を選びます。
   実行ごとに新しいドラフトを作成します。公開済みのタグは使えません。
3. 同じコミットの`CI`と`Build`、この実行の3環境のビルドがすべて成功したことを確認します。
   パッケージのビルドが失敗した場合は、同じ実行の`Re-run failed jobs`で同じドラフトにアップロードし直します。
4. この実行で作成したドラフトのタグ対象が実行のコミットSHAと一致することを確認します。
   そのドラフトの5ファイルを以下の手順で検証し、変更内容を追記して`Publish release`を押します。

## パッケージの確認

- **macOS（Apple Silicon）:** DMGからApplicationsへコピーし、Finderで起動します。初回起動をブロックされた場合は[Tauriのad-hoc署名ガイド](https://v2.tauri.app/distribute/sign/macos/#ad-hoc-signing)を参照してください。
- **Linux（x86_64、ARM64）:** 各環境でAppImageに実行権限を付けて起動し、`.deb`もファイル名を指定してインストールし、起動します（`sudo apt install ./ファイル名.deb`）。
- **全パッケージ:** Bazelの検出、ターゲット検索、ビルドとキャンセル、再起動後の履歴を確認します。
