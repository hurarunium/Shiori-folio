//! 詩織Ver2.0(セカンドブレイン化)のMCPサーバー(設計指示書v3、9章)。
//!
//! Claude Code等のAIエージェントから、詩織のライブラリ(library/)を検索専用で
//! 参照するためのstdioベースMCPサーバー。書き込みツールは持たない(書き込みは
//! library/への直接ファイル操作+決定的ルーティングで行う、7章)。
//!
//! 会話UI(shiori-folioバイナリ)とは別プロセス・別バイナリ(このクレート内の
//! 追加[[bin]]ターゲット)として実装している。会話UIが起動していなくても、
//! embedding用llama-server・RAG Pythonサーバーを共有デーモンとして起動し
//! (shared_daemon::ensure_daemon_running、4章)、単体で検索が完結する。
//!
//! 起動: `cargo build --bin mcp_server`でビルドすると`target/{debug,release}/
//! mcp_server`が生成される。Claude Code側からはstdioで直接このバイナリを
//! 起動する設定を想定している。

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, ContentBlock, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
    ServiceExt,
};

use shiori_client::{encode_component, Client};
use shiori_folio_lib::shiori_log;
use shiori_folio_lib::{
    build_embedding_command, build_rag_command, library_root, load_search_backend_config, project_root,
    rag_client, shared_daemon, shelf_root,
};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchLibraryArgs {
    /// 検索クエリ(日本語可)
    query: String,
    /// 返すファイル数の上限(1回あたり)。スコア閾値による足切りは行わないため、
    /// 欲しい情報が見つからなければoffsetを増やして同じクエリで再検索すること
    #[serde(default = "default_limit")]
    limit: u32,
    /// ページングの開始位置。1回目は省略(0)、続きが欲しければ前回の
    /// offset+limitを指定して再検索する。返却件数がlimit未満ならそれ以上
    /// 候補が無いことを意味する
    #[serde(default)]
    offset: u32,
    /// frontmatterのauthor(human/claude-code)で絞り込む。省略可
    #[serde(default)]
    author: Option<String>,
    /// frontmatterのtypeで絞り込む。省略可
    #[serde(default)]
    r#type: Option<String>,
    /// frontmatterのprojectで絞り込む。省略可
    #[serde(default)]
    project: Option<String>,
    /// 経緯モード(詩織Ver3.3、時間認識検索)。trueにすると、
    /// status: deprecatedやアーカイブ済みの記事も含め、新しい順に
    /// 並べ替えて返す。「前はどうだったか」「過去と現在を見比べたい」
    /// といった時系列・変遷が重要な質問のときに使う。falseの場合は
    /// deprecatedを除外し、新しい記事を優先する通常の検索(デフォルト)
    #[serde(default)]
    include_history: bool,
}

fn default_limit() -> u32 {
    20
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchShelfArgs {
    /// 検索クエリ(日本語可)
    query: String,
    /// 返すファイル数の上限。省略すると5件
    #[serde(default = "default_shelf_limit")]
    limit: u32,
    /// ページングの開始位置。続きが欲しければ前回のoffset+limitを指定する
    #[serde(default)]
    offset: u32,
    /// 分野(例: 数学)で絞り込む。省略可
    #[serde(default)]
    topic: Option<String>,
}

fn default_shelf_limit() -> u32 {
    5
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ReadDocumentArgs {
    /// 読む記事のパス。検索結果の `path` をそのまま渡す(`library/…` または `knowledge/…`)
    path: String,
}

/// data/server.json があれば、サーバーへ中継する(詩織Ver7.0)。無ければ、いままでどおり手元で動く。
fn remote() -> Option<Client> {
    Client::load(&project_root().join("data"), "mcp")
}

fn relay_error(e: shiori_client::ClientError) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

fn pretty(value: &serde_json::Value) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("結果のJSON化に失敗: {e}"), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
}

#[derive(Clone)]
struct ShioriLibrary {
    // #[tool_router]/#[tool_handler]マクロが展開するコード内部で参照される
    // (実機のJSON-RPC疎通テストでtools/list・tools/callとも正常動作を確認済み)。
    // rustcの未使用フィールド検出はマクロ展開後のコードまで追えないため警告が
    // 出るが、動作上は問題ない。
    #[allow(dead_code)]
    tool_router: ToolRouter<ShioriLibrary>,
}

#[tool_router]
impl ShioriLibrary {
    fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }

    // library/への物理パスの存在確認。設計指示書v3、2章「SSD未接続時の挙動」の
    // 通り、各ツールの最初のステップとして行い、無ければ即座にエラーを返す
    // (embeddingデーモンの起動処理などを試みる前に、ここで打ち切る)。
    fn ensure_library_present(&self) -> Result<(), McpError> {
        if library_root().is_dir() {
            Ok(())
        } else {
            Err(McpError::internal_error(
                "詩織のSSDが接続されていません(library/が見つかりません)",
                None,
            ))
        }
    }

    #[tool(
        description = "詩織のTier1プロファイル(_system/tier1-profile.md)を丸ごと返す。セッション開始時に検索を挟まず最初に呼ぶことを想定している"
    )]
    async fn get_context_profile(&self) -> Result<CallToolResult, McpError> {
        if let Some(client) = remote() {
            let doc = client
                .get(&format!("/api/v1/doc?path={}", encode_component("library/_system/tier1-profile.md")))
                .map_err(relay_error)?;
            let content = doc.get("content").and_then(|c| c.as_str()).unwrap_or_default().to_string();
            return Ok(CallToolResult::success(vec![ContentBlock::text(content)]));
        }
        self.ensure_library_present()?;
        let path = library_root().join("_system/tier1-profile.md");
        let content = std::fs::read_to_string(&path).map_err(|e| {
            McpError::internal_error(format!("tier1-profile.mdの読み込みに失敗: {e}"), None)
        })?;
        Ok(CallToolResult::success(vec![ContentBlock::text(content)]))
    }

    #[tool(
        description = "詩織のライブラリ(library/)をクエリで検索し、ファイル単位に集約した見出し一覧を返す(本文は含まない。棚の場所を教えるだけで内容の合成はしない)。各結果には実ファイルへの絶対パス(path)が含まれるため、本文を読みたい場合はReadツール等で直接開ける。素材ファイル(zip・png・pdf等)は説明用の「.meta」サイドカーがヒットし、pathは.meta自体、asset_pathは素材の実体を指す(素材には実体が無い場合や.meta以外の結果では空文字列)。スコアによる足切りは行わないため、返却件数がlimit未満になるまでoffsetを増やして同じクエリで呼び直すと、関連しうる候補を漏らさず確認できる。「前はどうだったか」のように時系列・変遷が重要な質問にはinclude_history: trueを指定すると、deprecated/アーカイブ済みの記事も含め新しい順に並べ替えて返す"
    )]
    async fn search_library(
        &self,
        Parameters(args): Parameters<SearchLibraryArgs>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(client) = remote() {
            // サーバーの窓口へ中継する(結果の path は、library/… の相対パス。read_document に渡せる)
            let mut body = serde_json::json!({ "query": args.query, "limit": args.limit, "offset": args.offset });
            if args.author.is_some() || args.r#type.is_some() || args.project.is_some() {
                let mut filter = serde_json::Map::new();
                if let Some(a) = &args.author {
                    filter.insert("author".into(), serde_json::json!(a));
                }
                if let Some(k) = &args.r#type {
                    filter.insert("type".into(), serde_json::json!(k));
                }
                if let Some(p) = &args.project {
                    filter.insert("project".into(), serde_json::json!(p));
                }
                body["filter"] = serde_json::Value::Object(filter);
            }
            if args.include_history {
                body["mode"] = serde_json::json!("history");
            }
            let results = client.post("/api/v1/search/library", &body).map_err(|e| {
                shiori_log::error("MCP", "検索に失敗した", &[("道具", &"search_library"), ("語", &args.query), ("理由", &e.to_string())]);
                relay_error(e)
            })?;
            shiori_log::info("MCP", "検索した(サーバー経由)", &[("道具", &"search_library"), ("語", &args.query)]);
            return pretty(&results);
        }
        self.ensure_library_present()?;

        let root = project_root();
        let backend =
            load_search_backend_config(&root).map_err(|e| McpError::internal_error(e, None))?;

        // embedding用llama-server・RAG Pythonサーバーの両方を共有デーモンとして
        // 起動確認する(設計指示書v3、4章)。会話UIが起動していなくてもここで
        // 単体完結する。戻り値のChildは(起動した場合)このプロセスの終了と共に
        // デタッチされる(shared_daemon.rs参照、意図した設計)。
        shared_daemon::ensure_daemon_running(
            &root,
            backend.embedding_port,
            ".shiori-embed.lock",
            30,
            || {
                build_embedding_command(
                    &root,
                    &backend.embedding_model_path,
                    backend.embedding_port,
                    backend.embedding_context_size,
                    true,
                )
            },
        )
        .map_err(|e| McpError::internal_error(e, None))?;

        // 索引の再登録が長引いても、進捗が更新されている間はkillせずに待つ(表示先は無い)。
        let progress = shared_daemon::DaemonProgress {
            file: shiori_folio_lib::rag_vectordb_dir(&root).join(".index_progress.json"),
            on_label: &|_| {},
        };
        shared_daemon::ensure_daemon_running_with_progress(
            &root,
            backend.rag_port,
            ".shiori-rag.lock",
            60,
            || build_rag_command(&root, backend.rag_port, true),
            Some(&progress),
        )
        .map_err(|e| McpError::internal_error(e, None))?;

        let filter = if args.author.is_some() || args.r#type.is_some() || args.project.is_some() {
            Some(rag_client::LibrarySearchFilter {
                author: args.author.as_deref(),
                kind: args.r#type.as_deref(),
                project: args.project.as_deref(),
                ..Default::default()
            })
        } else {
            None
        };

        let results = rag_client::search_library(
            backend.rag_port,
            &args.query,
            args.limit,
            args.offset,
            filter,
            args.include_history,
        )
        .map_err(|e| {
            shiori_log::error("MCP", "検索に失敗した", &[("道具", &"search_library"), ("語", &args.query), ("理由", &e)]);
            McpError::internal_error(e, None)
        })?;
        shiori_log::info(
            "MCP",
            "検索した",
            &[
                ("道具", &"search_library"),
                ("語", &args.query),
                ("経緯", &args.include_history),
                ("offset", &args.offset),
                ("件数", &results.len()),
                ("上位", &shiori_log::summarize_hits(results.iter().map(|r| (r.title.as_str(), r.source.as_str(), r.best_score)), 3)),
            ],
        );

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("結果のJSON化に失敗: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(
        description = "知識の棚(knowledge/)を検索する。棚には、出典で確かめた十分に信頼できる知識だけが入っている(libraryの記録とは別。search_libraryには出ない)。ふらるとAIが共有する知識の正本として使う。各結果には出典(cited_from)・学んだ日(learned)・実ファイルへの絶対パス(path)が付く。本文はReadツール等でpathを開いて読む。出典間で食い違う点は、記事の本文に両論が書かれている。棚に該当が無ければ、自分の知識で補ってよいが、棚の知識とは区別して伝える"
    )]
    async fn search_shelf(
        &self,
        Parameters(args): Parameters<SearchShelfArgs>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(client) = remote() {
            let mut body = serde_json::json!({ "query": args.query, "limit": args.limit, "offset": args.offset });
            if let Some(topic) = &args.topic {
                body["topic"] = serde_json::json!(topic);
            }
            let results = client.post("/api/v1/search/shelf", &body).map_err(|e| {
                shiori_log::error("MCP", "検索に失敗した", &[("道具", &"search_shelf"), ("語", &args.query), ("理由", &e.to_string())]);
                relay_error(e)
            })?;
            shiori_log::info("MCP", "検索した(サーバー経由)", &[("道具", &"search_shelf"), ("語", &args.query)]);
            return pretty(&results);
        }
        if !shelf_root().is_dir() {
            return Err(McpError::internal_error(
                "知識の棚(knowledge/)が見つかりません。SSDが未接続か、棚がまだ作られていません",
                None,
            ));
        }

        let root = project_root();
        let backend =
            load_search_backend_config(&root).map_err(|e| McpError::internal_error(e, None))?;

        shared_daemon::ensure_daemon_running(
            &root,
            backend.embedding_port,
            ".shiori-embed.lock",
            30,
            || {
                build_embedding_command(
                    &root,
                    &backend.embedding_model_path,
                    backend.embedding_port,
                    backend.embedding_context_size,
                    true,
                )
            },
        )
        .map_err(|e| McpError::internal_error(e, None))?;

        let progress = shared_daemon::DaemonProgress {
            file: shiori_folio_lib::rag_vectordb_dir(&root).join(".index_progress.json"),
            on_label: &|_| {},
        };
        shared_daemon::ensure_daemon_running_with_progress(
            &root,
            backend.rag_port,
            ".shiori-rag.lock",
            60,
            || build_rag_command(&root, backend.rag_port, true),
            Some(&progress),
        )
        .map_err(|e| McpError::internal_error(e, None))?;

        let results = rag_client::search_shelf(
            backend.rag_port,
            &args.query,
            args.limit,
            args.offset,
            args.topic.as_deref(),
        )
        .map_err(|e| {
            shiori_log::error("MCP", "検索に失敗した", &[("道具", &"search_shelf"), ("語", &args.query), ("理由", &e)]);
            McpError::internal_error(e, None)
        })?;
        shiori_log::info(
            "MCP",
            "検索した",
            &[
                ("道具", &"search_shelf"),
                ("語", &args.query),
                ("件数", &results.len()),
                ("上位", &shiori_log::summarize_hits(results.iter().map(|r| (r.title.as_str(), r.source.as_str(), r.best_score)), 3)),
            ],
        );

        let json = serde_json::to_string_pretty(&results)
            .map_err(|e| McpError::internal_error(format!("結果のJSON化に失敗: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
    }

    #[tool(
        description = "記事の本文を読む。search_library・search_shelfの結果の `path`(library/… または knowledge/…)をそのまま渡す。読み取り専用。サーバーにつないでいるときは、サーバーの正本を読む(SSDのパスを直接読まない)"
    )]
    async fn read_document(
        &self,
        Parameters(args): Parameters<ReadDocumentArgs>,
    ) -> Result<CallToolResult, McpError> {
        if let Some(client) = remote() {
            let doc = client
                .get(&format!("/api/v1/doc?path={}", encode_component(&args.path)))
                .map_err(relay_error)?;
            let content = doc.get("content").and_then(|c| c.as_str()).unwrap_or_default().to_string();
            return Ok(CallToolResult::success(vec![ContentBlock::text(content)]));
        }
        // 手元(v6.2)の動き。library/…・knowledge/… の形のパスだけを、その置き場の中で読む(.. などは断る)
        let (root, rel) = match args.path.split_once('/') {
            Some(("library", rel)) => (library_root(), rel),
            Some(("knowledge", rel)) => (shelf_root(), rel),
            _ => {
                return Err(McpError::invalid_params(
                    "パスは library/… または knowledge/… の形で指定してください(検索結果の path をそのまま渡す)",
                    None,
                ))
            }
        };
        if !root.is_dir() {
            return Err(McpError::internal_error("詩織のSSDが接続されていません", None));
        }
        let (bytes, _rev) = library_store::fsx::read(&root, rel).map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let content = String::from_utf8(bytes)
            .map_err(|_| McpError::invalid_params("UTF-8の文章ではないため、読めません", None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(content)]))
    }
}

#[tool_handler]
impl ServerHandler for ShioriLibrary {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "詩織のライブラリ(セカンドブレイン)を検索するためのMCPサーバーです。\
                 セッション開始時にまずget_context_profileを呼び、その後は必要に応じて\
                 search_libraryで検索してください。ふらるが学んで自分の知識にしたこと(知識の棚)は\
                 search_shelfで検索します。記事の本文は、検索結果のpathをread_documentに渡して読みます。書き込みはこのMCPサーバー経由ではなく\
                 library/への直接ファイル操作(frontmatter付きmd。素材は同名の.metaを添えて\
                 shiori-saveで取り込む)で行います。保存時の\
                 配置ルールはlibrary/_system/CLAUDE.mdを参照してください。"
                    .to_string(),
            )
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdioはMCPプロトコル通信専用のため、記録はログのファイルにだけ書く(標準出力には出さない)。
    let root = project_root();
    shiori_log::init(
        &root.join("data").join("logs"),
        shiori_log::LogKind::Mcp,
        &shiori_log::LogSettings::from_config(&root),
    );
    shiori_log::info("MCP", "起動した", &[("版", &env!("CARGO_PKG_VERSION"))]);
    let service = ShioriLibrary::new().serve(stdio()).await?;
    service.waiting().await?;
    shiori_log::info("MCP", "終了した", &[]);
    Ok(())
}
