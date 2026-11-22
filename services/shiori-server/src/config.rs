//! サーバーの設定(`server.json`、または起動時の引数)。

use library_store::Layout;
use serde::Deserialize;
use std::path::PathBuf;

fn default_embed_port() -> u16 {
    8082
}
fn default_rag_port() -> u16 {
    8083
}
fn default_layout() -> String {
    "v7".to_string()
}

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    /// 待ち受けるアドレス(`Tailscaleのアドレス:8090` や `127.0.0.1:8090`)。`0.0.0.0` は断る。
    pub bind: String,
    /// 正本: library・知識の棚・タスク
    pub library: PathBuf,
    pub knowledge: PathBuf,
    pub tasks: PathBuf,
    /// 窓口の状態の置き場(`tokens.json`・`audit/`・`uploads/`・`backups/`・索引・ログ)
    pub data: PathBuf,
    /// 子(埋め込みの llama-server・shiori-rag)を持つ実行環境。`no_children` なら要らない。
    #[serde(default)]
    pub runtime: Option<PathBuf>,
    #[serde(default = "default_embed_port")]
    pub embed_port: u16,
    #[serde(default = "default_rag_port")]
    pub rag_port: u16,
    #[serde(default)]
    pub no_children: bool,
    /// 置き場の並び: `v7`(`20-projects`・`10-common`)または `legacy`(v6.2)
    #[serde(default = "default_layout")]
    pub layout: String,
}

impl Config {
    pub fn layout(&self) -> Result<Layout, String> {
        match self.layout.as_str() {
            "v7" => Ok(Layout::V7),
            "legacy" => Ok(Layout::Legacy),
            other => Err(format!("layout は v7 か legacy: {other}")),
        }
    }

    pub fn load(path: &std::path::Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("設定を読めない {}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("設定の形が正しくない {}: {e}", path.display()))
    }
}
