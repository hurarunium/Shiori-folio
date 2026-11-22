# Ver7.0 Phase 2・3 の進み具合(不在中の作業の記録)

2026-11-22、ふらるの外出中に、Phase 2(サーバー本体)と Phase 3(CLI・AIの接続)を、ダミーのlibraryに対して進めた記録。
**コミット・プッシュはしていない**(決まりで、内容を示して確認を取ってから行うため)。変更はすべて作業ツリーにある。
実機のSSD・余りのPC・本物のlibraryには、一切書いていない。

## 判断(帰宅後に確認してほしいこと)

1. サーバーは、設計書の「`src-tauri/src/bin/shiori_server.rs`」ではなく、**Tauriに依存しない別クレート `services/shiori-server`** にした(余りのPCで軽くビルドでき、`library_store` をそのまま使えるため)。試作(`services/shiori-proto-server`)は、動いている余りのPCのために、そのまま残した。
2. `library_store` に、置き場の並び `Layout`(`Legacy` = v6.2、`V7` = `20-projects`・`10-common`)を足した。アプリ・CLIの直接の動きは `Legacy` のまま、サーバーが `V7` で動く。切り替えは移行の日。
3. CLI・`mcp_server` は、`server.json` があれば**サーバーへ送り**、無ければ**いままでどおり直接動く**形にした(移行の日まで、v6.2を壊さないため)。

## 進み具合

(作業のたびに更新する)

- [x] A. `library_store`: `Layout`(V7の置き場)、日誌の追記(`journal`。午前5時の区切り、全体の日誌の目次)。テスト86件。
- [x] B. サーバー本体 `services/shiori-server`: 窓口(REST API v1)・認証(端末ごとのトークン、失効は即時)・監査(月ごと)・保存の振り分け(記事・memory・棚・素材)・素材のアップロード(SHA-256照合)・日誌の追記・検索の中継(絶対パス→相対パス)・保存後の索引更新・世代のバックアップ。テスト: 単体8件+結合17件(127.0.0.1の空きポートで実際にHTTPで呼ぶ)。
- [x] C. タスク: `src-tauri/src/tasks.rs`(1,052行)を `library_store::tasks` へ移した(アプリ・`shiori-task`・サーバーが同じ処理を使う。移した試験25件は、Windowsで初めて動いた)。サーバーに `/api/v1/tasks`・`/api/v1/task-folders` を足した。結合テスト+2件(計19件)。ログは `library_store::log::set_sink` でアプリのログへ差し込む。
- [x] D. 端末側(Phase 3の送信と中継): 新しい `crates/shiori_client`(`data/server.json` を読んでサーバーを呼ぶ。テスト6件)。`shiori-save`(記事・memory・棚・素材・`--journal`)、`shiori-task`、`mcp_server`(`search_library`・`search_shelf`・`get_context_profile` の中継と、新しい `read_document`)を、server.jsonがあればサーバーへ送る形にした。無ければ、いままでどおり手元で動く。`SHIORI_LOCAL=1` で、server.jsonがあっても手元で動く。実際のサーバーに対する通し試験で確認(記事・memory・棚・素材・日誌・タスク一式・MCPのJSON-RPC)。
- [x] E. 検索の新しさ(P-16): `shiori-rag` の `recency_date` を、日誌(`kind: journal`)でも `updated` にした。索引の版 `SCHEMA_VERSION` を 1→2(次の起動で全件を作り直す)。テスト42件。
- [x] F. 配置の手順と通し試験: `scripts/server/`(`build-release.ps1`・`start-server.ps1`・`register-task.ps1`・`check-server.ps1`・`make-test-env.ps1`・`server.example.json`・`e2e.py`)。`e2e.py` は29件、実際のサーバーとCLIで通る。PowerShellは、構文の確認と、手元での起動・点検まで(余りのPCでの実行は未)。
- [x] G. 帰宅後の確認の手順書: `docs/ver7-phase2-3-runbook.md`。

## 試験の結果(2026-11-22 夜)

| 対象 | 結果 |
|---|---|
| `crates/library_store` | 111件OK |
| `crates/shiori_client` | 6件OK |
| `services/shiori-server` | 単体8件 + 結合19件 OK |
| `services/shiori-rag` | 42件OK |
| `scripts/server/e2e.py`(実サーバー + 3つのCLI) | 29/29 OK |
| v6.2の手元の動き(記事・棚・素材) | 変更前のバイナリと、完全に一致 |
| v6.2の手元の動き(memory) | ファイル・退避・台帳は一致。エラーのパスが相対になる差だけ |

## 未着手・未完了

`docs/ver7-phase2-3-runbook.md` の「まだやっていないこと」を参照。

## 目視テストで見つかった不具合(2026-11-22 午後)

- 日誌の節の本文・タスクの詳細に、PowerShellの `Set-Content -Encoding utf8` が付ける先頭のBOMが残っていた(記事は、読み飛ばしていた)。直した(`library_store::journal`・サーバーのタスクの窓口)。試験を足した(`library_store` 112件)。
- T1(手元でサーバーを動かし、手でCLIを使う)は、記事・日誌・タスク・停止中の失敗・トークン失効まで、期待どおり。

## Phase 1-C(書き方の規則)と、その実装(2026-11-22 午後)

- 規則を決めた(`docs/ver7-writing-rules-draft.md`。承認済み)。方針: **長さの上限で削らず、内容の密度を重視する。**
- 実装(`crates/library_store/src/rules.rs`): 警告の条件 J1〜J3・D1〜D4・M1(すべて警告。保存は止めない)。実機のlibrary(journal 168件・memory 17件)に当てて、しきい値を調整した。
- サーバー: 日誌の追記の結果に `warnings`。memory(V7)の保存の結果に `warnings`・`size`(大きさは情報だけ。長さの上限・警告は無い)。
- **外した項目の自動記録**: memory(V7)を保存すると、前の版から消えた項目(書き換えは除く)を、そのプロジェクトの日誌へ「memoryから外した項目」の節として記録する(全体用は全体の日誌へ)。
- CLI: 警告と、外した項目の記録を、標準エラーに表示する。
- 試験: `library_store` 127件、サーバー 結合21件、通し試験 33件。

## Phase 3 の下書き(2026-11-22 夕方)

- 新しい道具: サーバーの `GET /api/v1/list?path=…`(フォルダのファイル一覧。名前の降順=日誌なら新しい順)と、MCPの `list_documents`。`work-start` が「直近の日誌」を探すために必要だった(`read_document` は、パスが分かるものしか読めないため)。`library_store::fsx::list_entries`。試験: `library_store` 128件、サーバー結合22件、通し試験36件。
- 下書き(`docs/ver7-drafts/`。移行の日に切り替える): skill 3つ(`work-start`・`session-close`・`completion-report`。現行の本体から、変更する節だけを差し替えた全文)、AIの入口3つ(`~/.claude/CLAUDE.md`・`~/.codex/AGENTS.md`・リポジトリの `AGENTS.md`)、`_system/CLAUDE.md` と `memory-global.md`・`library-organizing` の改訂案。
- **未解決の課題**(ふらるの判断が要る): skill の本体・`_system/` の資料・libraryの中のスクリプトを、Ver7.0でどう更新するか(サーバーの窓口に、これらを書く道が無い)。案と推奨は `docs/ver7-drafts/library-system/system-CLAUDE-changes.md` の「11」。
- 未確認: 下書きの手順を、実際のセッションで通した確認(サーバーが余りのPCへ配置されたあと)。

## 手順書・規約の更新(2026-11-22 夕方。案B+`shiori-save --doc`)

- `library_store::docs`: 窓口から書ける場所を3か所に限る(受信箱 / `30-resources/skills/`=新規・上書き / `_system/CLAUDE.md`・`_system/library-organizing/`=**上書きのみ**)。台帳・退避・そのほかは不可。拡張子の制限(.md・.sh・.json・.txt・.yaml・.yml・.ps1・.py)と、秘密情報の名前の拒否。上書きは、読んだ版(`rev`)のときだけ。旧版を `_system/doc-history/` へ3世代退避。負けた側(競合)は、退避を残さない。試験9件(`library_store` 137件)。
- サーバー: `PUT /api/v1/doc` をこの部品へ通した(結合試験 計25件)。削除は、受信箱だけ。
- CLI: `shiori-save --doc-get <path> --out <ファイル>`(読んで、版を表示)、`shiori-save --doc <path> [--expect-rev <版>] <ファイル>`(書く)。衝突のとき、「先に書き換えました。読み直して…」と案内する。
- 通し試験 44件(手順書の新規・読み出し・上書き・退避・衝突・台帳の拒否・規約の新規の拒否を追加)。


## Phase 4・5(2026-11-22 夜)

- Phase 4(アプリ): タスク・図書館・棚・検索・メモ・保留・規約・申し送り・空き容量・素材を開く、をサーバー経由にした。T5(目視テスト)は合格。詳細は `docs/ver7-phase4-survey.md`。
- Phase 5-B(移行ツール): `shiori-server migrate-plan|migrate-apply|migrate-verify`(`library_store::migrate`)。本物のlibraryで予行と、作業場所への適用・照合に通った(警告なし)。
- Phase 5(バックアップ・最小限): 毎日 05:00(日誌と同じ区切り)の世代、毎日7・毎週4の整理、`restore`・`verify`。SSDへ写すボタン・知らせ・画面は、7.0の後。会話の記録は世代に入れない。
- 次の手順は `docs/ver7-phase5-6-runbook.md`。
