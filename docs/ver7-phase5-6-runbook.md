# Ver7.0 Phase 5・6 の手順書(バックアップの確認・移行のリハーサルと移行の日)

2026-11-22 作成。元は設計書 `design-shiori-ver7-20261121-2` の Phase 5・6。
**本物のSSD・余りのPCへ書く手順は、あなたが実行する(または、確認してから私が実行する)。**
`shiori-server` は、余りのPCに配置したもの(`scripts/server/build-release.ps1` の出力)を使う。開発機なら `target\release\shiori-server.exe`。

## 0. できているもの

| 内容 | 状態 |
|---|---|
| 移行ツール `migrate-plan`・`migrate-apply`・`migrate-verify` | 済(本物のlibraryで、作業場所へ試した) |
| 毎日 05:00 の世代(`serve` が作る。止まっていた分は起動時に追いつく)、毎日7・毎週4の整理 | 済 |
| `restore`・`verify`(= `backup-verify`)で、世代の復元と照合 | 済 |
| SSDへ写すボタン・7日の知らせ・バックアップ画面(P-09・P-12) | 未(7.0の後に回す) |
| 会話の記録(`conversations.db`)を世代に入れる | しない(決定) |

## 1. バックアップの確認(P-08。移行の前に必ず通す)

世代を作って、別の場所へ復元し、元と一致することを見る。

1. サーバーを動かし、世代を1つ作る(トークンは自分の端末のもの)。
   ```powershell
   curl.exe -X POST -H "Authorization: Bearer <トークン>" http://<サーバーのアドレス>/api/v1/backup
   curl.exe -H "Authorization: Bearer <トークン>" http://<サーバーのアドレス>/api/v1/backups
   ```
2. 世代のzipを、`<data>\backups\` から、別の場所へ復元して照らす(サーバーは止めなくてよい)。
   ```powershell
   shiori-server restore <data>\backups\gen-YYYYMMDD-HHMMSS.zip --dest C:\restore-check --against-library <library> --against-tasks <tasks>
   ```
3. 「zipの中身はmanifestと一致した」「復元した中身はmanifestと一致した」「元のフォルダと一致した」が出れば合格。
4. SSDへの写しは、7.0では手動(世代のzipとその `.sha256` を、`ShioriFolio\shiori-backups\` へコピー)。コピーしたら、SSD側で `restore` を同じように通して確かめる。
5. 確認が済んだら `C:\restore-check` を消す。

## 2. リハーサル(Phase 6-1。SSDのlibraryの写しで行う。SSDには書かない)

1. SSDの `portable\library` を、作業場所(例 `C:\migrate-rehearsal\src`)へコピーする(`._` ファイルは、ツールが除外する)。
2. 予行(何も書かない)。警告が無いことを見る。
   ```powershell
   shiori-server migrate-plan C:\migrate-rehearsal\src C:\migrate-rehearsal\dst
   ```
   - 見るところ: 写す件数、置き場が変わる件数、`type: area` の書き換え件数、旧journal → 日誌の件数、警告(あれば、適用できない)。
   - 11/22 の本物のlibrary(予行): 写す506・除外217(`._*`)・置き場が変わる391・`type: area` 245・journal 168件 → 日誌85日、警告なし。
3. 適用 → 照合。
   ```powershell
   shiori-server migrate-apply  C:\migrate-rehearsal\src C:\migrate-rehearsal\dst
   shiori-server migrate-verify C:\migrate-rehearsal\src C:\migrate-rehearsal\dst
   ```
4. 試験用のサーバー(`dst` を library にした設定、`layout: v7`)を、`--no-children` で動かし、アプリ・CLIから、申し送り・memory・日誌・規約を確かめる。
5. 済んだら、作業場所を消す。

## 3. 移行の日(Phase 6-2)

順に。各段階の終わりに、止まって確認する。**移行は、バックアップの確認(1)と、リハーサル(2)が通ってから。**

1. 両方の端末(WindowsとMac)で、v6.2のアプリ・CLIを閉じる。
2. SSDから、最終の写しを取る(`portable\library`・`knowledge`・タスクの置き場)。
3. `migrate-plan` の報告を、リハーサルとの差分だけ確認する。
4. `migrate-apply`(照合に通るたびに、段階が進む)→ `migrate-verify`。行き先は、サーバーの正本の置き場(空であること)。
5. 索引を入れる(大きな索引は、開発機で作って送る)。
6. AIの入口を切り替える(`docs/ver7-drafts/` の入口3つ、skill 3つ、`_system/CLAUDE.md` の改訂)。
6-2. 試験の間に使ったトークンは、失効させて作り直す(`shiori-server token revoke <端末名> --data <data>` → `token add`)。作り直した値は、チャットなどに貼らず、`server.json` に自分で置く。
6-3. 端末のWindowsのアプリを、内蔵(`C:\ShioriFolio7\`)に置く。`npm.cmd run tauri build -- --no-bundle` で作った `shiori-folio.exe` と、リポジトリの `config.json`、`data\server.json` の3つを置く(`C:\ShioriFolio7\shiori-folio.exe`・`config.json`・`data\server.json`)。開発版のアプリは、先に閉じる(ホットキーを取り合う)。右クリック → タスクバーに固定。Macは `~/ShioriFolio7/`(アプリは、別途、`.app` の形で確認する)。
7. 両方の端末の `data\server.json`(`{url, token}`)を置き、アプリ・CLIをサーバーへつなぐ。
8. SSDの `portable\` に、凍結の目印を置く(書き込まない旨)。
9. サーバーで、最初の世代を作り、SSDへ写す(1と同じ手順で、復元の確認まで)。
10. 成功条件を、WindowsとMacの両方で通す(企画書の成功条件の一覧に、端末ごとの結果を書く)。

**戻すとき**: サーバーの最新の世代を `portable\library` などへ `restore` して、v6.2を使い直す。v6.2のlibraryは、凍結の目印を置くまで、そのまま残っている。

## 4. 注意

- 古いパス(`10-projects/…` など)が記録の本文に残る記事が61件ある。書き換えない(設計どおり)。
- 日付が日付だけ(時刻なし)の旧journalは、その日付の 00:00 として入る(1件。`session-journal-20261005.md`)。
- `._*`・`.DS_Store`・`Thumbs.db` は、移行で写さない。
- 毎日の世代の区切りは、日誌と同じ午前5時。サーバーの時計(Windowsの時刻同期)が狂うと、世代の日時も狂う。
