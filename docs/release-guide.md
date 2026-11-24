# Release guide

1. `src-tauri/tauri.conf.json`と`src-tauri/Cargo.toml`のバージョンをそろえ、次のコマンドで更新した`Cargo.lock`と一緒に`main`へマージします。

   ```sh
   cargo update --manifest-path src-tauri/Cargo.toml --workspace
   ```

2. `Actions → Release → Run workflow`で`main`を選びます。
   既存タグ、公開済みリリース、同名ドラフトの重複があるとビルドは開始されません。
3. このバージョンの最新のRelease実行で3環境すべてが成功し、同じコミットの`CI`と`Build`も成功したことを確認します。
   更新中や一部失敗の状態では公開せず、失敗したジョブは`Re-run failed jobs`で再試行します。
4. ドラフトのタグ対象が最新実行のコミットSHAと一致することを確認します。
   同じバージョンで再実行するとパッケージは置き換わりますが、タグ対象は変わりません。
   不一致のまま公開せず、修正できなければ新しいバージョンでやり直します。
5. ドラフトの5ファイルを以下の手順で検証し、変更内容を追記して`Publish release`を押します。
   検証中に再実行した場合は手順3からやり直します。

## パッケージの確認

- **macOS（Apple Silicon）:** DMGからApplicationsへコピーし、Finderで起動します。初回起動をブロックされた場合は[Tauriのad-hoc署名ガイド](https://v2.tauri.app/distribute/sign/macos/#ad-hoc-signing)を参照してください。
- **Linux（x86_64、ARM64）:** 各環境でAppImageに実行権限を付けて起動し、`.deb`もファイル名を指定してインストールし、起動します（`sudo apt install ./ファイル名.deb`）。
- **全パッケージ:** Bazelの検出、ターゲット検索、ビルドとキャンセル、再起動後の履歴を確認します。
