<!-- Ver7.0の下書き(2026-11-22)。移行の日に、~/.codex/AGENTS.md(WindowsとMac)をこの内容へ置き換える。それまでは、v6.2の入口を使う。 -->
# Codex 共通指示

- 常に日本語で応答し、コード内コメントも日本語にする。
- 作業開始時に、詩織のMCP(`read_document`)で、`library/10-common/memory-global.md` と、対象プロジェクトの `library/20-projects/{project}/memory.md` を読む。タスクの概要は `shiori-task list`。
- 詩織のサーバーにつながらない場合は、サンドボックスのネットワーク制限でないかを疑い、サンドボックス外での実行の承認を求める。それでもつながらなければ、その旨を伝えて続行してよいか確認する。手元のSSDや古いコピーを、代わりに読まない。
- `shiori-save`・`shiori-task` は PATH に入っていない。フルパスで呼ぶ(Windows: `C:\ShioriFolio7\bin\shiori-save.exe`・`C:\ShioriFolio7\bin\shiori-task.exe`、Mac: `~/ShioriFolio7/bin/shiori-save`・`~/ShioriFolio7/bin/shiori-task`)。サンドボックスからはサーバーに届かないことがあるので、届かなければ、サンドボックス外での実行の承認を求める。
- `kind: memory` は直接書き換えず、`shiori-save --expect-updated "<読んだupdated>"` 経由のみで更新する。日誌は `shiori-save --journal` で追記する。
