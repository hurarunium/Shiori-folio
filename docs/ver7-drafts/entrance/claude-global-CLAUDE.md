<!-- Ver7.0の下書き(2026-11-22)。移行の日に、~/.claude/CLAUDE.md(WindowsとMac)をこの内容へ置き換える。それまでは、v6.2の入口を使う。 -->
# 入口(詳細は詩織のlibraryが正本)
作業を始めるときは、まず詩織のMCP(`read_document`)で、`library/10-common/memory-global.md` を読み、続けて、その作業のプロジェクトの `library/20-projects/{project}/memory.md` を読むこと。タスクの概要は `shiori-task list`。
- 詩織の正本は、余りのPCで動くサーバー(Tailscale経由)。外付けSSDは、バックアップ専用で、読み書きに使わない。パスは、検索結果の `path`(`library/…`)を、そのまま `read_document` に渡す。フォルダの中身は `list_documents`(日誌の最新は `library/20-projects/{project}/journal`)。
- サーバーにつながらないときは、黙って別の動作をせず、その旨をユーザーに伝えること。手元のSSDや古いコピーを、代わりに読まない。
- `shiori-save`・`shiori-task` は PATH に入っていない。フルパスで呼ぶ(Windows: `C:\ShioriFolio7\bin\shiori-save.exe`・`C:\ShioriFolio7\bin\shiori-task.exe`、Mac: `~/ShioriFolio7/bin/shiori-save`・`~/ShioriFolio7/bin/shiori-task`)。サンドボックスからはサーバーに届かないことがあるので、届かなければ、サンドボックス外での実行の承認を求める。
- `kind: memory` の更新は `shiori-save --expect-updated "<読んだupdated>"` 経由のみ。日誌は `shiori-save --journal` で追記する(直接書かない)。手順は `library/_system/CLAUDE.md` の「共通記憶MD」「日誌」。

# 応答言語(サーバーに読めないときの最低限)
常に日本語で応答すること。コード内のコメントも日本語で書く。
