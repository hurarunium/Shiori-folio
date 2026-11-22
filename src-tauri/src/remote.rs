//! アプリがサーバー(shiori-server)へつなぐ入口(詩織Ver7.0 Phase 4)。
//!
//! `<アプリ>/data/server.json` があれば、libraryへの読み書きをサーバーへ送る。無ければ、いままでどおり手元(v6.2)。
//! つながらないときは、手元の動きへ黙って切り替えず、画面へ理由を返す(正本を2つにしない)。

use serde::Serialize;
use serde_json::Value;
use shiori_client::{Client, ClientError};

/// 設定があれば、サーバーへ話しかける道具。呼ぶたびに読み直す(設定を直せば、再起動なしで効く)。
pub fn client() -> Option<Client> {
    Client::load(&crate::project_root().join("data"), "app")
}

/// 画面に出す理由の文にする。
pub fn message(e: ClientError) -> String {
    e.to_string()
}

/// サーバーの応答を、画面が使う型にする。
pub fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, String> {
    serde_json::from_value(v).map_err(|e| format!("サーバーの応答を読めません: {e}"))
}

/// サーバーとの接続の状態(画面の表示用)。
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatusDto {
    /// `local`(手元で動く)・`connected`(つながっている)・`unreachable`(つながらない)・`unauthorized`(トークンが違う)・`error`
    pub state: String,
    pub url: String,
    pub version: String,
    pub layout: String,
    pub message: String,
}

fn local() -> ServerStatusDto {
    ServerStatusDto { state: "local".into(), url: String::new(), version: String::new(), layout: String::new(), message: String::new() }
}

/// サーバーに `/api/v1/status` を尋ねて、状態にする。
pub fn status(client: Option<&Client>) -> ServerStatusDto {
    let Some(c) = client else { return local() };
    let base = ServerStatusDto { url: c.url().to_string(), ..local() };
    match c.get("/api/v1/status") {
        Ok(v) => ServerStatusDto {
            state: "connected".into(),
            version: v["version"].as_str().unwrap_or_default().into(),
            layout: v["layout"].as_str().unwrap_or_default().into(),
            ..base
        },
        Err(ClientError::Connect(m)) => ServerStatusDto { state: "unreachable".into(), message: format!("サーバーにつながれません: {m}"), ..base },
        Err(ClientError::Status { code: 401, detail, .. }) => ServerStatusDto { state: "unauthorized".into(), message: detail, ..base },
        Err(e) => ServerStatusDto { state: "error".into(), message: e.to_string(), ..base },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 設定が無ければ手元で動く状態() {
        assert_eq!(status(None).state, "local");
    }

    #[test]
    fn つながらないときは不達の状態になり理由が付く() {
        let c = Client::new("http://127.0.0.1:1", "t", "app");
        let s = status(Some(&c));
        assert_eq!(s.state, "unreachable");
        assert!(s.message.contains("つながれません"));
        assert_eq!(s.url, "http://127.0.0.1:1");
    }
}
