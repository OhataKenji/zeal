# Release guide

Apple Silicon向けのmacOS DMGをGitHub Releasesで配布します。
利用者は新版のDMGをダウンロードし、アプリを上書きして更新します。

```text
版番号を更新 → mainへマージ → CI成功を確認 → Releaseを手動実行
  → Draft ReleaseのDMGを確認 → 公開
```

## 初回の署名設定

Apple Developer Programのアカウントで、`Developer ID Application`証明書を用意します。
秘密鍵を含む証明書をKeychain Accessからパスワード付きの`.p12`として書き出します。
手順は[TauriのmacOS署名ガイド](https://v2.tauri.app/distribute/sign/macos/)を参照してください。

GitHubのリポジトリ設定で、`Settings → Secrets and variables → Actions`に次のrepository secretsを登録します。

| Secret                       | 値                                                     |
| ---------------------------- | ------------------------------------------------------ |
| `APPLE_CERTIFICATE`          | `.p12`ファイルをBase64に変換した文字列                 |
| `APPLE_CERTIFICATE_PASSWORD` | `.p12`の書き出し時に設定したパスワード                 |
| `APPLE_SIGNING_IDENTITY`     | `Developer ID Application: ... (TEAMID)`形式の証明書名 |
| `APPLE_ID`                   | 公証に使うApple Accountのメールアドレス                |
| `APPLE_PASSWORD`             | Apple Accountで発行したアプリ用パスワード              |
| `APPLE_TEAM_ID`              | Apple Developer ProgramのTeam ID                       |

`APPLE_CERTIFICATE`の値は、ローカルで次のように作成できます。
出力先はリポジトリの外に置き、登録後に削除してください。

```sh
openssl base64 -A -in /path/to/certificate.p12 -out /path/to/certificate-base64.txt
```

証明書、秘密鍵、パスワードをコミットしないでください。
`GITHUB_TOKEN`はActionsが発行するため、別途登録する必要はありません。

## リリースの作成

1. `src-tauri/tauri.conf.json`の`version`を次の版へ更新します（例：`0.1.1`）。
   `src-tauri/Cargo.toml`のpackage versionと、`src-tauri/Cargo.lock`の`zeal`パッケージのversionも揃えます。
   `package.json`は非公開のフロントエンド用パッケージで、アプリの版には使いません。
2. 変更をmainへマージし、対象コミットの`CI`と`Build`が成功したことを確認します。
3. GitHubの`Actions → Release → Run workflow`で`main`を選び、実行します。
   実行画面のコミットが、CI成功を確認したコミットと同じことを確認します。
   異なる場合は、そのコミットのCI成功を確認してから公開へ進みます。
4. ワークフローの成功後、`Releases`で`Zeal v<version>`のDraft Releaseを開きます。
   `tauri-action`がアプリの版から`v<version>`を指定するため、タグを先に作る必要はありません。
5. Releaseの説明に変更内容、既知の制約、Apple Silicon向けであることを記載します。

ビルドは`macos-15`上で`aarch64-apple-darwin`を指定し、署名と公証を行います。
公開はワークフローでは行いません。
構成の元になった例は[TauriのGitHub Actionsガイド](https://v2.tauri.app/distribute/pipelines/github/)にあります。

## 公開前の確認

Draft Releaseに添付されたDMGをApple SiliconのMacへダウンロードして、次を確認します。

- DMGからApplicationsへコピーし、FinderからZealを起動できる。
- BazelまたはBazeliskを検出し、workspaceを開いてtargetを検索できる。
- buildを実行でき、実行中の処理をキャンセルできる。
- 実行履歴を保存し、アプリを再起動しても表示できる。
- 旧版がある場合は、終了してから上書き更新し、設定と履歴が残る。

Finderからの起動でBazelを検出できない場合は、公開せず修正します。
GUIアプリはシェル設定の`PATH`をそのまま引き継がないため、ターミナルからの起動だけでは確認になりません。
詳しくは[Tauriの注意事項](https://v2.tauri.app/distribute/appimage/)を参照してください。

確認できたら`Publish release`を押します。
確認したDMGをそのまま公開し、公開のために再ビルドしません。

## 失敗した場合

署名の認証情報や一時的な通信障害を直した場合は、元の実行画面の`Re-run jobs`から同じコミットを再実行します。
mainから新しく実行すると、進んだ別のコミットを選ぶことがあります。
同じ版の下書きがある場合は、対象コミットが一致するときだけ再実行できます。

コードを修正した場合は、新しい版番号で作り直します。
不要になった下書きは、未公開であることを確認して削除します。
同じ版のタグが既にある場合や、Releaseが公開済みの場合、ワークフローは停止します。
公開済みのタグや添付ファイルは変更せず、修正版を新しい版として公開します。
