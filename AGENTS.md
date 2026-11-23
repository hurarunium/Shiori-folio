# 入口(詳細は詩織のlibraryが正本)

このリポジトリは詩織(project id: `shiori`)の開発機。AIの種類(Claude Code、Codexなど)を問わず、次に従うこと。

作業を始めるときは、まず詩織のMCP(`read_document`)で、次の2つを読む。

1. `library/10-common/memory-global.md`(ふらるとAI全体の約束事と、いま有効な状態)
2. `library/20-projects/shiori/memory.md`(詩織の、いま有効な状態・やること・申し送り)

- 詩織の正本は、余りのPCで動くサーバー(Tailscale経由)。外付けSSDは、バックアップ専用で、読み書きに使わない。このリポジトリの `library/` は、開発用の空(ダミー)のもので、読み書きに使わない。
- サーバーにつながらないときは、黙って別の動作をせず、その旨をユーザーに伝える。
- `shiori-save`・`shiori-task` は PATH に入っていない。フルパスで呼ぶ(Windows: `C:\ShioriFolio7\bin\shiori-save.exe`・`C:\ShioriFolio7\bin\shiori-task.exe`、Mac: `~/ShioriFolio7/bin/shiori-save`・`~/ShioriFolio7/bin/shiori-task`)。サンドボックスからはサーバーに届かないことがあるので、届かなければ、サンドボックス外での実行の承認を求める。
- `kind: memory` の更新は `shiori-save --expect-updated "<読んだupdated>"` 経由のみ。日誌は `shiori-save --journal` で追記する。手順は `library/_system/CLAUDE.md` の「共通記憶MD」「日誌」。
- skillの手順の本体は、libraryの `30-resources/skills/`(例: `work-start.md`、`git-workflow.md`)。`read_document` で読む。
- 配置と検証の注意点は、libraryの `deploy-and-verify-pitfalls-20261008` を先に読む。
- 開発中の Ver7.0 の資料: `docs/ver7-phase2-3-runbook.md`(確認の手順)、`docs/ver7-writing-rules-draft.md`(日誌・memoryの書き方の規則)。

# 応答言語(サーバーに読めないときの最低限)

常に日本語で応答すること。コード内のコメントも日本語で書く。
