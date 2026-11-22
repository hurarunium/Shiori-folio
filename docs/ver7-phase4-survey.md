# Phase 4(アプリの差し替え)の洗い出し

2026-11-22。アプリ(`src-tauri/src/lib.rs` の `#[tauri::command]` 60件ほど)のうち、libraryに触れるものを、サーバー経由にする順に分けた。

## 方針

- `data/server.json` があれば、サーバーへ送る。無ければ、いままでどおり手元(CLIと同じ二つの動き)。
- つながらないときは、手元に切り替えず、画面に「サーバーにつながりません」を出す(正本を2つにしない)。
- 「ローカルのもの」(LLM・音声・RAGの起動・設定・ペット・統計)は、そのまま手元。

## A. すでにサーバーに窓口がある(差し替えるだけ)

| アプリのコマンド | サーバー側 |
|---|---|
| `list_tasks` / `add_task` / `save_task` / `complete_task` / `restore_task` / `delete_task` | `/api/v1/tasks`(`shiori-task` が使用済み) |
| `list_task_folders` / `add_task_folder` / `rename_task_folder` / `remove_task_folder` | `/api/v1/task-folders` |
| `search_library` / `search_shelf`(画面)、`build_passive_recall_context`(受け身の想起) | `/api/v1/search/library`・`/shelf`(MCPが使用済み) |
| `list_shelf_docs` / `read_shelf_doc` / `get_source_document` / `read_system_doc` / `list_system_docs` | `/api/v1/doc`・`/api/v1/list` |

## B. サーバーに窓口が無い(足す必要がある)

| アプリのコマンド | 足すもの |
|---|---|
| `list_library_projects` | プロジェクト一覧(`registry`)の読み出し |
| `list_pending_items` / `resolve_pending_tag` / `resolve_pending_project` / `resolve_pending_inbox_item` | 保留の一覧と、解決(承認・却下)。書き込みなので監査・排他に乗せる |
| `get_latest_handoff` | V7では日誌(`list` + `doc`)で代替できる。画面の見せ方を決める |
| `list_all_knowledge`(画面の一覧)、`get_source_frontmatter` | 記事の一覧・frontmatterの要約(`list` の拡張) |
| `open_library_asset` / `reveal_library_asset` | 素材の実体を端末で開く。サーバー経由なら、ダウンロード(`GET /api/v1/upload` 相当)してから開く。「フォルダで表示」は意味がなくなる |
| `get_portable_storage` | サーバーの空き容量(`/api/v1/status` に足す) |

## C. 会話の記録(`log_conversation`)

手元のSQLite(`conversations`)に書いている。V7の方針(会話の記録の送信)は未設計。選択肢:
1. 手元のSQLiteのまま(アプリの端末ごとの記録として残す。サーバーには送らない)。**最小。**
2. サーバーへ送る(`/api/v1/conversations` を足す)。端末をまたいで見られるが、窓口・容量・個人情報の扱いの設計が要る。

→ 11/24までは1で十分。2は運用後。

## D. 手元のまま(差し替えない)

LLM・埋め込み・RAG(手元用)の起動と設定、音声(whisper・piper)、ペット、統計(`claude_stats`)、`hub`、モデル切り替え、`eject`、診断。

注意: 手元のRAG(8083)は、サーバー接続時は**使わない**(検索はサーバーのRAG)。アプリの起動時に、手元のRAGを起こさない分岐が要る。

## E. 画面(フロント)

- 接続状態の表示(つながっている・つながらない・トークンが違う)。
- つながらないときの表示。書き込みの操作は無効にして、理由を出す。
- フォルダ名・色(V7の置き場 `20-projects`・`10-common` への対応)。

## 順番(案)

1. 共通の入口: アプリ側に `server_client`(`shiori_client::Client::load(data_dir, "app")`)を持たせ、状態(接続中・不達・認証失敗)を保持する。
2. A(差し替えるだけ)。タスク → 検索 → 文書の順。
3. B で足す窓口(サーバー側)と差し替え。保留の解決が、いちばん大きい。
4. E(画面の表示)。
5. C は、1(手元のまま)で確定。

## 未確認

- アプリの `cargo test --lib` はWindowsで動かないため、差し替えは**目視で確かめる**ことになる(ダミーの library・ローカルのサーバーで。`scripts/server/make-test-env.ps1`)。
