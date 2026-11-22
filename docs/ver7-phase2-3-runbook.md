# Ver7.0 Phase 2・3 の確認の手順書(帰宅後の目視テスト)

2026-11-22の外出中に、Phase 2(サーバー本体)と Phase 3(CLI・AIの接続)を、ダミーのデータだけで進めた。
その確認の手順。進み具合と判断は `docs/ver7-phase2-3-progress.md`。**変更はすべて、コミット前**(作業ツリーにある)。

## 0. 何ができたか

| 部品 | 場所 | 中身 |
|---|---|---|
| サーバー | `services/shiori-server/` | REST API v1(読み・受信箱への書き込み・削除・保存・素材の受け取り・日誌の追記・検索の中継・タスク・世代のバックアップ)。端末ごとのトークン、月ごとの監査ログ |
| 書き込みの部品 | `crates/library_store/` | 置き場の並び `Layout`(v6.2 / V7)、日誌の追記(午前5時の区切り・全体の日誌の目次)、タスクの処理(`tasks.rs` を移した) |
| 話しかける側 | `crates/shiori_client/` | `data/server.json` を読んでサーバーを呼ぶ |
| 端末のCLI | `src-tauri/src/bin/` | `shiori-save`(+`--journal`)・`shiori-task`・`mcp_server`(+`read_document`)。**`server.json` があればサーバーへ送る。無ければ、いままでどおり手元で動く** |
| 配置の手順 | `scripts/server/` | `build-release.ps1`・`start-server.ps1`・`register-task.ps1`・`check-server.ps1`・`make-test-env.ps1`・`e2e.py` |

## 1. まず、自動の試験(約1分)

開発機で。すべて通ることを確かめる。

```powershell
cd C:\15_MyProjects\Others\shiori-folio\services\shiori-server
cargo build
cd ..\..\src-tauri
cargo build --bin shiori_save --bin shiori_task --bin mcp_server
cd ..
python scripts\server\e2e.py
```

期待: 最後に `29/29 件OK`。(実際のサーバーを立て、`shiori-save`・`shiori-task`・`mcp_server` を動かして、記事・memory・棚・素材・日誌・タスク・MCP・認証・監査を確かめる試験)

単体の試験(それぞれ、末尾が `ok`):

```powershell
cd crates\library_store; cargo test       # 111件
cd ..\shiori_client; cargo test           # 6件
cd ..\..\services\shiori-server; cargo test   # 8件 + 19件
cd ..\shiori-rag; cargo test              # 42件
```

## 2. 目視テスト(あなたに見てほしいもの)

### T1. 手元でサーバーを動かし、手でCLIを使う

1. 試験環境を作る(`C:\shiori-server-test` に、ダミーのlibrary・設定・トークン・端末側ができる):

   ```powershell
   cd C:\15_MyProjects\Others\shiori-folio
   powershell -ExecutionPolicy Bypass -File scripts\server\make-test-env.ps1
   ```
2. 表示された手順に従い、**別のPowerShellの窓**でサーバーを起動する(止めるときは `Ctrl+C`)。
3. `Invoke-RestMethod http://127.0.0.1:18090/health` が `status: ok` を返す。
4. 記事を保存する(表示された例のとおり)。見るところ:
   - `C:\shiori-server-test\server\library\20-projects\shiori\resource\a.md` ができる(**`20-projects`** に入る)。
   - 手元の原本(`portable\work\a.md`)が消える。
   - 同じ名前でもう一度保存すると `a-2.md` になる。
5. 日誌に追記する:

   ```powershell
   "昼に実装した。" | Set-Content -Encoding utf8 C:\shiori-server-test\portable\work\j.md
   & C:\shiori-server-test\portable\bin\shiori_save.exe --journal --project shiori --ai claude-code --summary "実装した" C:\shiori-server-test\portable\work\j.md
   ```

   見るところ: `server\library\20-projects\shiori\journal\<今日の日付>.md` に `## 14:30 win・claude-code` の見出しの節ができ、`server\library\10-common\journal\` の日誌の `## 目次` に `- 14:30 [shiori] 実装した` が1行入る。もう一度追記すると、**末尾に節が足される**(既存の節は変わらない)。
6. タスク: `shiori_task.exe add --summary "試験" --folder 詩織`、`list`、`done <id>`、`delete <id> --yes`。`server\tasks\` にmdと `INDEX.md` ができる。
7. サーバーを止めてから `shiori_save.exe` を動かすと、**「詩織のサーバーにつながれません」と失敗する**(手元の保存に黙って切り替えない)。
8. トークンの失効: `shiori-server.exe token revoke win --data C:\shiori-server-test\server\data` のあと、サーバーを止めずに保存すると「認証に失敗しました」。

### T2. Claude Code から `read_document` を使う(任意)

試験用の `mcp_server` を、**別の名前**で登録する(本物の `shiori-library` には触れない)。

```powershell
claude mcp add shiori-test --scope local -- C:\shiori-server-test\portable\bin\mcp_server.exe
```

新しいセッションで、`read_document` に `library/20-projects/shiori/resource/a.md` を渡して、本文が返ること。終わったら `claude mcp remove shiori-test`。
(検索 `search_library` は、RAG(`shiori-rag`)を動かしていないと、「検索につながらない」と返るのが正しい。RAGを含む検索は T4。)

### T3. Mac から、Tailscale経由で使う

Phase 3 の完了条件の1つ。サーバーは開発機(または余りのPC)で動かし、Macの `shiori-save` から送る。

1. サーバー側の `server.json` の `bind` を、そのPCの Tailscale のアドレスと待ち受けポートにする(例 `100.117.249.122:18090`)。0.0.0.0 は断られる。初めて待ち受けるとき、Windowsのファイアウォールの許可を求められたら、「プライベートネットワーク」を許可する。
2. Mac用のトークンを作る: `shiori-server token add mac --data <dataのフォルダ>`(トークンは**1回だけ**表示される)。
3. Macで `cargo build --bin shiori_save --bin shiori_task --bin mcp_server`(`src-tauri` で)。
4. Macの置き場(例 `~/ShioriFolio7/`)に、`config.json`(`{}`)、`bin/` に3つの実行ファイル、`data/server.json`(`{"url":"http://<Tailscaleのアドレス>:18090","token":"<トークン>"}`、権限は `chmod 600`)を置く。
5. Macから記事・日誌を送る。見るところ: **見出しが `mac・…`** になる(トークンの名前が端末名になる)。
6. WindowsとMacから、ほぼ同時に同じプロジェクトの日誌へ追記し、**両方の節が残る**こと(自動試験では8並列で確認済み)。

### T4. 余りのPCへの配置と、RAGを含む検索

試作(`ShioriProtoServer`、ポート9090)とは別物。**先に試作のタスクを止める**(8082・8083番を取り合うため)。

1. 開発機で `powershell -File scripts\server\build-release.ps1` → `dist\ShioriServer\` ができる。
2. 余りのPCの `C:\ShioriServer\` へ写す(Taildropなど)。`server.example.json` を `server.json` にして直し、`runtime`(`shiori-rag`・`llama-server`・モデル。試作の `C:\shiori-proto\runtime` が使える)を置く。
3. `scripts\start-server.ps1` で起動して、`scripts\check-server.ps1` が全部OKになること(時計の同期・タイムゾーン・Tailscale・窓口・空き容量。日誌の日の区切りは、サーバーの時計が基準)。
4. 管理者権限で `scripts\register-task.ps1`。再起動して、ログインしなくても、1分以内に窓口が戻ること(試験4-5の再実施)。
5. RAGが動いたら、端末から `search_library` が通り、結果の `path` が `library/…` の相対パスになること。

## 3. 判断してほしいこと

1. **サーバーは、設計書の `src-tauri/src/bin/` ではなく、Tauriに依存しない別クレート `services/shiori-server`** にした(余りのPCで軽くビルドでき、`library_store` をそのまま使える)。試作 `services/shiori-proto-server` は、動いている余りのPCのために、そのまま残した。
2. サーバーは **V7の置き場**(`20-projects`・`10-common`)で動く。CLI・アプリの手元の動きは **v6.2の置き場のまま**(移行の日まで)。`type: area` はV7で断り、`kind: journal` の通常の保存はV7で断る(`--journal` へ案内)。
3. **`data/server.json` があれば、CLI・`mcp_server` はサーバーへ送り、つながらなければ失敗する**(手元へ黙って切り替えない)。`SHIORI_LOCAL=1` で、手元の動きを強制できる。
4. サーバーの `PUT /api/v1/doc`(任意のパスへの書き込み)は、**`library/00-inbox/` だけ**に絞った。ほかは、規則を通る窓口(保存・日誌・タスク)から。アプリの画面の機能(Phase 4)で、ほかが必要になったら足す。
5. **`shiori-rag` の索引の版を上げた**(`SCHEMA_VERSION` 1→2。日誌でも `updated` を新しさに使うため)。このRAGを起動すると、次の起動で索引を全部作り直す(設計書のとおり。大きな索引は、サーバーでなく開発機で作って送る)。v6.2の実機のRAGは、再ビルドして配置するまで変わらない。
6. タスクの処理(`tasks.rs`)を `library_store` へ移した(アプリ・`shiori-task`・サーバーが同じ処理を使う)。アプリ側は `pub use` で、これまでの呼び出しがそのまま通る。
7. `read_document` のパスは `library/…`・`knowledge/…`(サーバーは `tasks/…` も)。手元(v6.2)の `mcp_server` でも、同じ形で読める。

## 4. まだやっていないこと

- **Phase 1-C(書き方の規則)に依存するもの**: 規則による警告(日誌の節の見出しに「申し送り」、節が長すぎる、など)、`session-close`・`work-start`・`completion-report` の手順の下書き、AIの入口(`~/.claude/CLAUDE.md`・`AGENTS.md` など)の下書き。規則を、あなたと決めてから。
- Phase 2: 統計・会話の記録のAPI(Phase 4でアプリが使う)、`projects.yaml` への `common` の追加(移行ツールで)、子の見張り(試作の流用。余りのPCでの再確認は T4)。
- Phase 3: Codexのサンドボックスの中から、サーバーへ送れるかの確認(ネットワークが塞がれていれば、承認を求める決まりを `AGENTS.md` に足す)。libraryの中のスクリプト(`cross-review.sh` など)の扱いの決定。
- Phase 4(アプリの差し替え)以降: 未着手。

## 5. コミットの案(確認をお願いします)

1つの内容ごとに、次の順で分ける案。メッセージは1行(`[Add]`/`[Update]`)。

1. `[Add] 置き場の並び(Layout)と日誌の追記をlibrary_storeに足した`
2. `[Update] タスクの処理をlibrary_storeへ移した`
3. `[Add] サーバーに話しかける側の部品(shiori_client)を足した`
4. `[Add] 詩織のサーバー(shiori-server)を足した`
5. `[Update] shiori-save・shiori-task・mcp_serverを、サーバーにも話せるようにした`
6. `[Update] 日誌も更新日を新しさに使うようにした(索引の版を上げた)`
7. `[Add] サーバー配置の手順と通し試験を足した`
