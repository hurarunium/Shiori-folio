//! 試作の中継: AI(Claude Code・Codex)からは stdio のMCPサーバーに見え、中でサーバー側の窓口へHTTPで問い合わせる。
//!
//! 環境変数:
//!   SHIORI_SERVER_URL  窓口のURL(例: http://100.105.22.70:9090)
//!   SHIORI_TOKEN       この端末のトークン
//! ツール: search_library / read_document

use std::time::Duration;

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData as McpError, ServerHandler, ServiceExt,
};
use serde_json::{json, Value};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchArgs {
    /// 検索クエリ(日本語可)
    query: String,
    /// 返すファイル数の上限。省略すると20
    #[serde(default = "default_limit")]
    limit: u32,
    /// ページングの開始位置
    #[serde(default)]
    offset: u32,
    /// 経緯モード(古い記事も含め、新しい順)
    #[serde(default)]
    include_history: bool,
}

fn default_limit() -> u32 {
    20
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ReadArgs {
    /// libraryからの相対パス(検索結果の path)。タスクは `@tasks/` で始める
    path: String,
}

#[derive(Clone)]
struct Relay {
    #[allow(dead_code)]
    tool_router: ToolRouter<Relay>,
}

fn config() -> Result<(String, String), McpError> {
    let url = std::env::var("SHIORI_SERVER_URL")
        .map_err(|_| McpError::internal_error("SHIORI_SERVER_URL が設定されていない", None))?;
    let token = std::env::var("SHIORI_TOKEN")
        .map_err(|_| McpError::internal_error("SHIORI_TOKEN が設定されていない", None))?;
    Ok((url.trim_end_matches('/').to_string(), token))
}

fn http_err(e: ureq::Error) -> McpError {
    match e {
        ureq::Error::Status(code, r) => {
            let detail = r.into_string().unwrap_or_default();
            McpError::internal_error(format!("サーバーが{code}を返した: {detail}"), None)
        }
        other => McpError::internal_error(format!("サーバーにつながらない: {other}"), None),
    }
}

fn text(v: &Value) -> Result<CallToolResult, McpError> {
    let s = serde_json::to_string_pretty(v).map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(s)]))
}

#[tool_router]
impl Relay {
    fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }

    #[tool(description = "詩織のライブラリを検索し、ファイル単位の見出し一覧を返す(本文は含まない)。結果の path(libraryからの相対パス)を read_document に渡すと本文を読める")]
    async fn search_library(&self, Parameters(a): Parameters<SearchArgs>) -> Result<CallToolResult, McpError> {
        let (url, token) = config()?;
        let mut body = json!({"query": a.query, "limit": a.limit, "offset": a.offset});
        if a.include_history {
            body["mode"] = json!("history");
        }
        let v: Value = tokio::task::spawn_blocking(move || {
            ureq::post(&format!("{url}/search_library"))
                .set("Authorization", &format!("Bearer {token}"))
                .timeout(Duration::from_secs(130))
                .send_json(&body)
                .map_err(http_err)?
                .into_json::<Value>()
                .map_err(|e| McpError::internal_error(e.to_string(), None))
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))??;
        text(&v)
    }

    #[tool(description = "記事を1件、libraryからの相対パスで読む。本文と、書き込みに使う rev を返す")]
    async fn read_document(&self, Parameters(a): Parameters<ReadArgs>) -> Result<CallToolResult, McpError> {
        let (url, token) = config()?;
        let v: Value = tokio::task::spawn_blocking(move || {
            ureq::get(&format!("{url}/doc"))
                .query("path", &a.path)
                .set("Authorization", &format!("Bearer {token}"))
                .timeout(Duration::from_secs(30))
                .call()
                .map_err(http_err)?
                .into_json::<Value>()
                .map_err(|e| McpError::internal_error(e.to_string(), None))
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))??;
        text(&v)
    }
}

#[tool_handler]
impl ServerHandler for Relay {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions("詩織のサーバー側の窓口(試作)を呼ぶ中継です。search_libraryで検索し、read_documentで本文を読んでください。".to_string())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = Relay::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
