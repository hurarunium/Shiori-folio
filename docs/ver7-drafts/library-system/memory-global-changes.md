# `memory-global.md` と `library-organizing` の改訂案(Ver7.0の下書き)

移行の日に入れる。それまでは、v6.2のまま。`memory-global.md` は `kind: memory` なので、更新は `shiori-save --expect-updated` 経由(移行後は `10-common/memory-global.md`)。

## 1. `memory-global.md`(本文の、書き換える箇所)

| 箇所 | 旧 | 新 |
|---|---|---|
| 概要の1行目 | 「…プロジェクトの `memory.md` を読む(`10-projects/{project}/memory.md` または `20-areas/{project}/memory.md`)」 | 「…`library/20-projects/{project}/memory.md` を読む(`read_document`)」 |
| 概要の2行目(タスク) | 「`tasks/INDEX.md`(Windows `E:\portable\tasks\INDEX.md`、Mac `/Volumes/…`)を読む」 | 「`shiori-task list` で読む(全プロジェクトの未完了タスク)」 |
| 概要の置き場所 | 「詩織の実機のlibrary `40-profile/memory-global.md`(Windows `E:\portable\library\`、Mac …)」 | 「サーバーの正本 `library/10-common/memory-global.md`」 |
| 概要の更新 | 「`shiori-save --expect-updated` 経由のみ。詳細は `library/_system/CLAUDE.md` の「共通記憶MD」」 | (そのまま。「日誌」の節も参照に足す) |
| 詩織の環境 | 「実機は外付けSSD `ShioriFolio` の `portable/`」「本番のlibraryは実機の `portable/library/`」「SSDが接続されていないとき…」 | 「実機は、余りのPCで動くサーバー(`C:\\ShioriServer\\`)。SSDは、バックアップ専用」「SSDの代わりに、サーバーにつながらないとき、黙って別の動作をせず、その旨を伝える」 |
| 開発機 | (そのまま) | 「開発機の `library/` は、ダミー。検証は `scripts/server/make-test-env.ps1` で作る試験環境」を足す |
| 約束事に足す | | 「**日誌は、日次のファイルへの追記だけ**(`shiori-save --journal`)。節の見出しは、サーバーが付ける」「**memoryに、長さの上限は無い**。内容の密度(具体的か・1行1事実・曖昧語を単独で使わない・再掲しない)を重視する。外した項目は、サーバーが日誌へ自動で記録する」「規則: `rules-journal-memory-ver7-20261122`」 |
| 道具とskill | `30-resources/tools-and-skills-catalog.md`(そのまま) | `list_documents`・`read_document`・`shiori-save --journal` を足す |

## 2. 全プロジェクトの `memory.md`

- 移行のとき、ファイルが `20-areas/{p}/` または `10-projects/{p}/` から `20-projects/{p}/` へ移る。**本文は変えない**(`type: area` → `type: project` の1行だけ)。
- 各プロジェクトのリポジトリの `AGENTS.md`・`CLAUDE.md` は、「作業開始時にmemoryを読む」とだけ書いてある(中身はmemoryが正本)。**パスを書いているものだけ**、`20-projects/` へ直す(移行の日の確認で、一覧を出す)。

## 3. `library/_system/library-organizing/`(閲覧の参考・進め方・判断基準の3本)と `check_library.py`

機械的な置き換えだけ(意味の変更は無い)。

| ファイル | 該当 | 置き換え |
|---|---|---|
| `01-reading-guide.md` | 15行 | `10-projects/`・`20-areas/` → `20-projects/` |
| `02-procedure.md` | 5行 | 同上。SSDのパス(`E:\portable\…`)を含む箇所は、サーバーの正本の説明に直す |
| `03-criteria.md` | 12行 | `10-projects/`・`20-areas/` → `20-projects/` |
| `check_library.py` | SSDのパス | 一斉整理の点検用のスクリプト。サーバーの正本に対して動かす方法(サーバー上で実行する)を、決める必要がある(未解決の課題。`system-CLAUDE-changes.md` の11) |

## 4. 一斉整理(`library-organizing`)の扱い

移行後、一斉整理は、サーバーの**バックアップを取ってから**、サーバー上で行う(進め方の「バックアップ」の手順は、Phase 5 の世代のバックアップに置き換わる)。この手順の更新は、Phase 5 で行う。
