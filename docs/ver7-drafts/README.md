# Ver7.0 の skill・AIの入口・規約の下書き

2026-11-22、Phase 3(CLIとAIの接続)の作業。**移行の日に切り替える**もの。それまでは、v6.2の入口・手順(libraryとホームディレクトリにあるもの)を使い続ける。

## 一覧

| 下書き | 切り替え先(移行の日) | 内容 |
|---|---|---|
| `skills/work-start.md` | library `30-resources/skills/work-start.md` | 前回内容の把握を、`read_document`・`list_documents`・`shiori-task list` で行う。直近の日誌・全体の日誌の目次を読む。サーバーにつながらないときの扱い |
| `skills/session-close.md` | 同 `session-close.md` | 記録を、日誌への追記(`shiori-save --journal`)に。memoryの更新を、長さの上限なし・密度の規則・自己点検・外した項目の自動記録に |
| `skills/completion-report.md` | 同 `completion-report.md` | 詩織への記録を、日誌への追記に |
| `entrance/claude-global-CLAUDE.md` | `~/.claude/CLAUDE.md`(WindowsとMac) | 入口(`read_document` で `10-common/memory-global.md` と `20-projects/{project}/memory.md`) |
| `entrance/codex-AGENTS.md` | `~/.codex/AGENTS.md`(WindowsとMac) | 同上。サンドボックスのネットワーク制限の扱い |
| `entrance/repo-AGENTS.md` | このリポジトリの `AGENTS.md`(`CLAUDE.md` は `@AGENTS.md` のまま) | 同上 |
| `library-system/system-CLAUDE-changes.md` | library `_system/CLAUDE.md` | 改訂案(貼れる文言つき)。直接書き込みの廃止、`type: area` の廃止、新しい置き場、日誌の節、memoryの内容の規則 |
| `library-system/memory-global-changes.md` | `memory-global.md` と `library-organizing/` | 改訂の一覧 |

規則そのものは、`../ver7-writing-rules-draft.md`(承認済み。libraryにも `rules-journal-memory-ver7-20261122` として保存済み)。

`skills/` の3つは、現行の本体から、**変更する節だけを差し替えた全文**(構造は現行のまま)。現行との差分は、`diff` で確かめられる:

```powershell
diff (Get-Content E:\portable\library\30-resources\skills\session-close.md) (Get-Content docs\ver7-drafts\skills\session-close.md)
```

## 新しい道具(この下書きのために足した)

- MCP `list_documents(path, limit)` と、サーバーの `GET /api/v1/list?path=…`: フォルダのファイル一覧(名前の降順。日誌なら新しい順)。`work-start` が直近の日誌を探すために必要だった。
- MCP `read_document(path)`: 記事の本文(`library/…`・`knowledge/…`)。

## 移行の日に切り替える手順(案)

1. 両方の端末(WindowsとMac)で、v6.2を閉じ、サーバー・CLI・`mcp_server` のv7.0を配置し、`data/server.json` を置く。
2. library: `_system/CLAUDE.md`・`memory-global.md`・skillの本体を、この下書きで置き換える(`shiori-save --doc-get` / `--doc` を使う。`memory-global.md` は `shiori-save --expect-updated`)。
3. 入口: `~/.claude/CLAUDE.md`・`~/.codex/AGENTS.md`(WindowsとMac)、各リポジトリの `AGENTS.md` を置き換える。
4. 新しいセッションの Claude Code・Codex で、入口のとおりに読めること(`work-start` が、memory・直近の日誌・タスクを読めること)を確かめる。

## 解決済みの課題: skill の本体・規約の更新

Ver7.0では、libraryが端末から見えず、サーバーの窓口に、skill の本体・規約を書く道が無かった。**memoryと同じ考え方**で、`shiori-save --doc-get <path> --out <作業用のファイル>`(読む。版を表示)と `shiori-save --doc <path> --expect-rev <版> <作業用のファイル>`(読んだ版のときだけ書く。旧版は `_system/doc-history/` に3世代退避)で更新できるようにした(2026-11-22)。範囲・拡張子の制限・貼れる文言は、`library-system/system-CLAUDE-changes.md` の「11」。

## 確認していないこと

- この下書きの手順を、**実際のセッションで通した確認は、まだ無い**(サーバーが余りのPCに配置されたあとの、Phase 3 の完了条件)。
- `list_documents` の結果の `path` を `read_document` に渡す流れは、試験で確かめた(サーバーの結合試験と、MCPのJSON-RPC)。
