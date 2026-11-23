# Ver7.0 本番移行の手順書(移行の日)

2026-11-22 作成。**この手順書は、本番(SSDの本物のlibrary)を、サーバーへ移す日に、上から順に行う。**
リハーサルは行わない(2026-11-22、ふらるの判断)。そのかわり、**各段階で確認点を通ってから、次へ進む。通らなければ、そこで止まる。**

関連: 設計 `design-shiori-ver7-20261121-2`(Phase 6)、`ver7-phase5-6-runbook.md`(バックアップの確認とリハーサルの手順。この手順書の元)、`ver7-drafts/`(入口・skillの下書き)。

## 方針

- **凍結の目印(段階 K)を置くまで、SSDの `portable\` は、そのまま残る。** 1〜J のどこで止まっても、v6.2 に戻れる。
- 変換は、元を書き換えない(`migrate-apply` は、行き先に書くだけ)。
- 移行のあいだ、**v6.2 では、保存しない**(最終の写しのあとに保存したものは、移せない)。
- 迷ったら、止まる。止まったときの状態を書き留めて、私(Claude)に見せる。

### リハーサルをしなかったことで、まだ通していないもの(本番で初めて行う)

| 項目 | 本番での扱い |
|---|---|
| 本物のlibraryの最終の写しでの、`migrate-apply` のサーバーへの配置 | 段階 C〜E。各段階で照合する |
| 索引づくり(11/22に約1時間かかった) | 段階 E。起動して、待つ。待っている間に、段階 F 以降の準備をする |
| AIの入口の切り替え(実際のセッションで通していない) | 段階 I。**控えを取ってから**。1台で確かめてから、もう1台 |
| Macのアプリ本体(`.app`) | 段階 H。CLIの確認は済んでいる。アプリは、通れば通る、通らなければ後回し(7.0の成功条件はMacのCLI・AI入口で満たす) |
| 外(自宅の外)からのTailscale経由 | 段階 J。移行のあと、スマホのテザリングなどで確かめる |

## 0. 前の日までに済ませておくこと

- [ ] 余りのPC: `C:\ShioriServer\bin\shiori-server.exe` が、最新(`backup`・`migrate` を含む)のリリース版。古ければ、`services\shiori-server` で `cargo build --release` して、`bin\` へ置き換える(Taildrop)。
- [ ] 余りのPC: 空き容量が 10GB 以上(`C:\ShioriServer\scripts\check-server.ps1`)。
- [ ] 開発機: `services\shiori-server\target\release\shiori-server.exe` が、最新。
- [ ] 開発機: `C:\ShioriFolio7\` に、リリース版のアプリ・`config.json`・`data\server.json` が置いてある(11/22に確認済み)。
- [ ] 時間を確保する。**索引づくりに約1時間**かかる。全体で、3〜4時間の見込み。
- [ ] 両端末で、v6.2 のアプリ・CLI・Claude Code・Codex のセッションを、閉じる準備をする(最終の写しのあとは、保存しない)。

## A. 両端末で v6.2 を閉じる

1. Windows・Mac の詩織のアプリを閉じる。開いている Claude Code・Codex のセッションは、保存を終えて閉じる。
2. SSDを、どちらの端末でも使っていない状態にする(書き込み中でない)。

**確認点**: どちらの端末にも、詩織のアプリのプロセスが無い(`tasklist | findstr shiori` が空)。

## B. 第2のバックアップ(SSDの写し)

SSDの `portable\` は、元として、そのまま残る。念のため、別の場所へ写しを取る。開発機の内蔵(空きがあること)に取る。

```powershell
$d = "C:\shiori-premigration-$(Get-Date -Format yyyyMMdd)"
robocopy E:\portable\library $d\library /E /XF "._*" ".DS_Store" /NFL /NDL /NP
robocopy E:\portable\knowledge $d\knowledge /E /XF "._*" ".DS_Store" /NFL /NDL /NP
robocopy E:\portable\tasks $d\tasks /E /XF "._*" ".DS_Store" /NFL /NDL /NP
robocopy E:\portable\data $d\data shiori.db /NFL /NDL /NP
```

`robocopy` の終了コードは、0〜7 が成功(8以上が失敗)。

**確認点**: 件数が合う。

```powershell
(Get-ChildItem E:\portable\library -Recurse -File | Where-Object { $_.Name -notlike '._*' -and $_.Name -ne '.DS_Store' }).Count
(Get-ChildItem $d\library -Recurse -File).Count
```

2つの数が同じ。違えば、止まる。

## C. 変換(開発機。SSDには書かない)

作業場所は `C:\shiori-migrate\`(開発機の内蔵)。

```powershell
$m = "C:\15_MyProjects\Others\shiori-folio\services\shiori-server\target\release\shiori-server.exe"
& $m migrate-plan E:\portable\library C:\shiori-migrate\library
```

**確認点(予行)**:
- 「警告: なし(適用できる)」が出る。
- 件数が、11/22の報告(写す506・置き場が変わる391・`type: area` 245・旧journal 168 → 日誌85日)と、新しい記事の分だけ、近い。大きく違えば、止まる。

```powershell
& $m migrate-apply E:\portable\library C:\shiori-migrate\library
& $m migrate-verify E:\portable\library C:\shiori-migrate\library
```

**確認点**: 「適用して、3つの段階の照合に通りました」と「照合に通りました: すべて合っています」。通らなければ、`C:\shiori-migrate` を捨てて、止まる(SSDは、無傷)。

知識の棚とタスクは、変換せず、そのまま写す。

```powershell
robocopy E:\portable\knowledge C:\shiori-migrate\knowledge /E /XF "._*" ".DS_Store" /NFL /NDL /NP
robocopy E:\portable\tasks C:\shiori-migrate\tasks /E /XF "._*" ".DS_Store" /NFL /NDL /NP
```

## D. サーバーへ送る

1. 変換した3つを zip にして、ハッシュを控える。

   ```powershell
   Compress-Archive -Path C:\shiori-migrate\library, C:\shiori-migrate\knowledge, C:\shiori-migrate\tasks -DestinationPath C:\shiori-migrate\data.zip
   (Get-FileHash C:\shiori-migrate\data.zip -Algorithm SHA256).Hash
   tailscale file cp C:\shiori-migrate\data.zip laptop-5h6ft2jk:
   ```
2. 余りのPCで、受け取り、ハッシュを照らす。

   ```powershell
   tailscale file get $env:USERPROFILE\Downloads
   (Get-FileHash $env:USERPROFILE\Downloads\data.zip -Algorithm SHA256).Hash
   ```

**確認点**: 2つのハッシュが同じ。違えば、止まる(送り直す)。

## E. サーバーの置き換えと索引

余りのPCで(管理者権限のPowerShell)。

1. サーバーを止める(自動起動を一時無効にし、プロセスを止める)。

   ```powershell
   Disable-ScheduledTask -TaskName ShioriServer
   Stop-ScheduledTask -TaskName ShioriServer
   Get-Process shiori-server,shiori-rag,llama-server -ErrorAction SilentlyContinue | Stop-Process -Force
   ```
2. 試験用のデータを、**消さずに、名前を変えて**退ける(戻せるように)。

   ```powershell
   cd C:\ShioriServer
   Rename-Item library library.rehearsal
   Rename-Item knowledge knowledge.rehearsal
   Rename-Item tasks tasks.rehearsal
   Rename-Item data\rag-index rag-index.rehearsal
   ```
3. 変換したデータを、展開する。

   ```powershell
   Expand-Archive $env:USERPROFILE\Downloads\data.zip -DestinationPath C:\ShioriServer
   ```
4. **トークンを作り直す**(試験のものを、失効させる)。

   ```powershell
   C:\ShioriServer\bin\shiori-server.exe token revoke win --data C:\ShioriServer\data
   C:\ShioriServer\bin\shiori-server.exe token revoke win2 --data C:\ShioriServer\data
   C:\ShioriServer\bin\shiori-server.exe token revoke mac --data C:\ShioriServer\data
   C:\ShioriServer\bin\shiori-server.exe token add win7 --data C:\ShioriServer\data
   C:\ShioriServer\bin\shiori-server.exe token add mac7 --data C:\ShioriServer\data
   ```

   **失効(`revoke`)は、印を付けるだけで、名前は残る。同じ名前で `token add` すると「すでにある」で失敗する。**新しい名前(ここでは `win7`・`mac7`)で作る。端末の名前は、日誌の見出し(`## HH:MM 端末・AI`)にも出る。

   表示されたトークンは、**チャットに貼らず**、それぞれの `server.json` に、自分で置く。
5. 自動起動を戻し、起動する。

   ```powershell
   Enable-ScheduledTask -TaskName ShioriServer
   Start-ScheduledTask -TaskName ShioriServer
   ```
6. 索引づくりを待つ(約1時間)。進み具合を見る。

   ```powershell
   curl.exe -m 10 -s -H "Authorization: Bearer <トークン>" http://100.105.22.70:8090/api/v1/status
   ```

**確認点**: `embed: true`・`rag: true`。待っている間は、F・G の準備をしてよい(索引が要らない確認は、先に進めてよい)。

## F. バックアップの確認(P-08)

1. 世代を作る。

   ```powershell
   curl.exe -X POST -H "Authorization: Bearer <トークン>" http://100.105.22.70:8090/api/v1/backup
   ```
2. 別の場所へ復元して、元と一致するか。

   ```powershell
   C:\ShioriServer\bin\shiori-server.exe restore C:\ShioriServer\data\backups\<世代のzip> --dest C:\restore-check --against-library C:\ShioriServer\library --against-tasks C:\ShioriServer\tasks
   ```

**確認点**: 「zipの中身はmanifestと一致」「復元した中身はmanifestと一致」「元のフォルダと一致」。済んだら `C:\restore-check` を消す。

3. SSDへ写す(手動)。世代のzipと、その `.sha256` を、SSDの `ShioriFolio\shiori-backups\` へ(開発機から取ってくる)。SSD側で、同じく `restore` を通す。

## G. 端末Windows(開発機)の確認

1. 開発機の `C:\ShioriFolio7\data\server.json` に、新しいトークンを置く(自分で)。
2. `C:\ShioriFolio7\shiori-folio.exe` を起動する(開発版のアプリを、閉じておく)。

**確認点**:

| 見る所 | 期待 |
|---|---|
| ホーム | 「サーバー 接続中」。伝言に、shiori の memory の、やること・申し送り |
| 図書館 | 「プロジェクト」「共通」の下に、フォルダ。記事が開く |
| 検索 | 結果が出る(新しい記事も) |
| 素材 | 開ける |
| タスク | 一覧が出る。追加・完了ができる |
| **旧journalの対応** | `symphony-kill-chord-homepage` の日誌(20節が 2026-09-16 にまとまる)、`shiori` の日誌(81件 → 38日)を、図書館で開き、旧記事の内容が節に入っていることを、**数件、実際に見る**。旧記事が `90-archive/20-projects/` にあり、`status: outdated` になっていること |
| 規約 | memory・スキル・規約が出る |

通らなければ、止まる(戻し方は、後述)。

## H. 端末Mac

1. `~/ShioriFolio7/data/server.json` に、新しいトークン(Mac用)を置く(自分で)。
2. CLI を確認する(`shiori_save --journal` で、日誌に追記。見出しが `mac・…`)。
3. アプリ本体(`.app`)は、ビルドして、動けば確認する。動かなければ、7.0の移行は止めず、後で直す(CLI・AIの入口が通れば、成功条件の「Macで確かめる」は、満たせる)。

**確認点**: Macから、記事・日誌が、サーバーに入る。

## I. AIの入口の切り替え(控えを取ってから)

**この段階で、初めて、実際のセッションで通す。** 1台で確かめてから、もう1台へ。

1. 控えを取る(戻せるように)。

   ```powershell
   $b = "$env:USERPROFILE\.claude\ver7-backup-$(Get-Date -Format yyyyMMdd)"
   New-Item -ItemType Directory $b | Out-Null
   Copy-Item $env:USERPROFILE\.claude\CLAUDE.md $b\
   Copy-Item $env:USERPROFILE\.codex\AGENTS.md $b\
   Copy-Item $env:USERPROFILE\.claude.json $b\
   ```
2. `C:\ShioriFolio7\bin\` に、`mcp_server.exe`・`shiori_save.exe`・`shiori_task.exe` を置く(`src-tauri\target\release\` から)。
3. library の中の、`_system\CLAUDE.md`・`memory-global.md`・skill の本体を、下書き(`docs/ver7-drafts/`)で置き換える。手順は、`docs/ver7-drafts/README.md` の「移行の日に切り替える手順(案)」の 2。`kind: memory` は、`shiori-save --expect-updated` 経由。
4. `~/.claude.json` の `shiori-library` の `command` を、`C:\ShioriFolio7\bin\mcp_server.exe` に変える。
5. `~/.claude/CLAUDE.md` を、`docs/ver7-drafts/entrance/claude-global-CLAUDE.md` に置き換える。
6. **新しいセッション**の Claude Code を開き、「作業を始めます」と言って、`work-start` が、memory・直近の日誌・タスクを読めることを確かめる。
7. Codex も、同じく(`~/.codex/AGENTS.md`)。サンドボックスで、サーバー(Tailscale)に届くかも見る。届かなければ、Codex の設定(ネットワーク)を、確認する。
8. リポジトリの `AGENTS.md` を、`docs/ver7-drafts/entrance/repo-AGENTS.md` に置き換える。
9. 1台で通ったら、もう1台(Mac の `~/.claude/CLAUDE.md`・`~/.codex/AGENTS.md`・MCP)も、同じ内容に切り替える。

**確認点**: 新しいセッションで、入口のとおりに、memory-global と プロジェクトの memory が読める。

**戻し方**: 控え(`$b`)の3つを、元の場所へ書き戻す。数分。

## J. 成功条件(企画書の8項目)

WindowsとMacの両方で通す。「Windowsで通った」だけで完了にしない。

- [ ] WindowsとMacで、同時に別の作業をして、両方の保存がサーバーに入る。
- [ ] 自宅の外(スマホのテザリング等)から、Tailscale経由で、アプリ・Claude Code・Codex から使える。
- [ ] サーバーの再起動のあと、人が操作せず、戻る。
- [ ] SSDのバックアップから、別の場所へ復元して、一致する(F-3)。
- [ ] 同じ日に、2台から同じプロジェクトの日誌へ追記して、節が残る。
- [ ] 旧journalのすべての内容を、日誌からたどれる(G)。
- [ ] 新しいパスで、AIの入口から、memory・memory-global が読める(I)。
- [ ] 上を、WindowsとMacの両方で確かめた。

## K. 凍結の目印と、仕上げ(**ここまで通ってから**)

1. SSDの `portable\` に、凍結の目印を置く(書き込まない旨)。

   ```powershell
   Set-Content E:\portable\FROZEN-v6.2.txt "2026-11-xx に、Ver7.0 へ移行した。v6.2 のlibraryは、ここで凍結。書き込まない。最新はサーバー。" -Encoding UTF8
   ```
2. 最初の世代を作り、SSDへ写す(F と同じ)。
3. 版を 7.0.0 にして、タグを付ける(私が行う。コミットの確認つき)。
4. 試験用の退けたデータ(`library.rehearsal` など)と、`C:\shiori-migrate`・`C:\shiori-premigration-*` は、**7.0で問題なく使えると確かめてから**、消す。

## 戻し方(段階ごと)

| どこで止まったか | 戻し方 |
|---|---|
| A〜D | 何もしない(SSDは無傷。`C:\shiori-migrate` を消す) |
| E〜H | サーバーを止め、`library.rehearsal` などを、元の名前に戻す。v6.2 のSSDを使い続ける |
| I | 控えの3つを、元の場所へ書き戻す |
| K の後 | 凍結の目印を消し、サーバーの最新の世代を `portable\library` などへ `restore` して、v6.2 を使い直す(それまでに、サーバーに入った分は、世代から戻る) |

## 止まったときに、書き留めること

- どの段階の、どの確認点か。
- 出た出力(トークンは、伏せる)。
- そのときの状態(サーバーは、動いているか、止まっているか)。

これらを、私(Claude)に見せる。
