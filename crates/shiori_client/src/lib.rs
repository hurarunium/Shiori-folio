//! 詩織のサーバー(`shiori-server`)に話しかける側の部品。
//!
//! 端末側(アプリ・CLI・`mcp_server`)は、`<アプリ>/data/server.json`(`{"url": "...", "token": "..."}`)を読む。
//! **このファイルがあれば、サーバーへ送る**(移行の日以降)。無ければ、いままでどおり手元で動く(v6.2)。
//! 環境変数 `SHIORI_LOCAL=1` を付けると、`server.json` があっても手元で動く(復旧・比較のため)。
//! サーバーにつながらないとき、手元の動きへ黙って切り替えない(正本が2つになるのを防ぐ)。

use serde_json::Value;
use std::fmt;
use std::path::Path;
use std::time::Duration;

pub const SERVER_JSON: &str = "server.json";

#[derive(Debug)]
pub enum ClientError {
    /// サーバーにつながらない・通信に失敗した
    Connect(String),
    /// サーバーが断った(409 衝突、422 不正、404、401 など)
    Status { code: u16, detail: String, body: Value },
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Connect(m) => write!(f, "詩織のサーバーにつながれません: {m}"),
            ClientError::Status { detail, .. } => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl ClientError {
    /// 衝突(409)か。
    pub fn is_conflict(&self) -> bool {
        matches!(self, ClientError::Status { code: 409, .. })
    }
}

#[derive(Debug, Clone)]
pub struct Client {
    url: String,
    token: String,
    /// `X-Shiori-Client` に入れる値(`app`・`cli`・`mcp`)
    kind: String,
}

impl Client {
    pub fn new(url: &str, token: &str, kind: &str) -> Client {
        Client { url: url.trim_end_matches('/').to_string(), token: token.to_string(), kind: kind.to_string() }
    }

    /// `<data_dir>/server.json` から読む。無い・`SHIORI_LOCAL=1`・読めないときは `None`(手元で動く)。
    pub fn load(data_dir: &Path, kind: &str) -> Option<Client> {
        if std::env::var("SHIORI_LOCAL").is_ok_and(|v| v == "1") {
            return None;
        }
        let text = std::fs::read_to_string(data_dir.join(SERVER_JSON)).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        let url = v.get("url")?.as_str()?;
        let token = v.get("token")?.as_str()?;
        if url.is_empty() || token.is_empty() {
            return None;
        }
        Some(Client::new(url, token, kind))
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    fn request(&self, method: &str, path: &str) -> ureq::Request {
        ureq::request(method, &format!("{}{path}", self.url))
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("X-Shiori-Client", &self.kind)
            .timeout(Duration::from_secs(120))
    }

    fn finish(result: Result<ureq::Response, ureq::Error>) -> Result<Value, ClientError> {
        match result {
            Ok(r) => Ok(r.into_json().unwrap_or(Value::Null)),
            Err(ureq::Error::Status(code, r)) => {
                let body: Value = r.into_json().unwrap_or(Value::Null);
                let detail = if code == 401 {
                    "サーバーの認証に失敗しました(トークンが違う、または失効しています。data/server.json を確認してください)".to_string()
                } else {
                    body.get("detail")
                        .and_then(Value::as_str)
                        .map(String::from)
                        .unwrap_or_else(|| format!("サーバーが{code}を返しました"))
                };
                Err(ClientError::Status { code, detail, body })
            }
            Err(e) => Err(ClientError::Connect(e.to_string())),
        }
    }

    pub fn get(&self, path: &str) -> Result<Value, ClientError> {
        Self::finish(self.request("GET", path).call())
    }

    pub fn delete(&self, path: &str) -> Result<Value, ClientError> {
        Self::finish(self.request("DELETE", path).call())
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value, ClientError> {
        Self::finish(self.request("POST", path).send_json(body))
    }

    pub fn put(&self, path: &str, body: &Value) -> Result<Value, ClientError> {
        Self::finish(self.request("PUT", path).send_json(body))
    }

    /// ファイルの中身を、そのまま本文にして送る(素材の実体。大きくてもメモリに全部は載せない)。
    pub fn put_file(&self, path: &str, file: &Path) -> Result<Value, ClientError> {
        let f = std::fs::File::open(file).map_err(|e| ClientError::Connect(format!("{}を開けません: {e}", file.display())))?;
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        Self::finish(self.request("PUT", path).set("Content-Length", &len.to_string()).send(f))
    }
}

/// URLの値として安全な形にする(日本語・記号を `%XX` にする)。
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn mock() -> (String, Arc<Mutex<Vec<String>>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let auth = req.headers().iter().find(|h| h.field.equiv("Authorization")).map(|h| h.value.to_string()).unwrap_or_default();
                let kind = req.headers().iter().find(|h| h.field.equiv("X-Shiori-Client")).map(|h| h.value.to_string()).unwrap_or_default();
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                log.lock().unwrap().push(format!("{} {} [{auth}] [{kind}] {body}", req.method(), req.url()));
                let (code, out) = match req.url() {
                    "/conflict" => (409, r#"{"detail":"他の端末が先に書きました"}"#),
                    "/plain-error" => (500, "not json"),
                    _ => (200, r#"{"ok":true}"#),
                };
                let _ = req.respond(tiny_http::Response::from_string(out).with_status_code(code));
            }
        });
        (url, seen)
    }

    #[test]
    fn server_jsonが無ければ手元で動く() {
        let d = tempfile::tempdir().unwrap();
        assert!(Client::load(d.path(), "cli").is_none());
        std::fs::write(d.path().join("server.json"), r#"{"url":"http://x:8090/","token":"t"}"#).unwrap();
        let c = Client::load(d.path(), "cli").unwrap();
        assert_eq!(c.url(), "http://x:8090", "末尾の/は取る");
        // 壊れた・空の設定は、無いものとして扱う
        std::fs::write(d.path().join("server.json"), r#"{"url":"","token":"t"}"#).unwrap();
        assert!(Client::load(d.path(), "cli").is_none());
        std::fs::write(d.path().join("server.json"), "壊れている").unwrap();
        assert!(Client::load(d.path(), "cli").is_none());
    }

    #[test]
    fn トークンとクライアントの種類を付けて送る() {
        let (url, seen) = mock();
        let c = Client::new(&url, "tok123", "mcp");
        assert_eq!(c.post("/api/v1/x", &serde_json::json!({"a": 1})).unwrap()["ok"], true);
        assert_eq!(c.get("/api/v1/y").unwrap()["ok"], true);
        let log = seen.lock().unwrap().clone();
        assert_eq!(log[0], r#"POST /api/v1/x [Bearer tok123] [mcp] {"a":1}"#);
        assert!(log[1].starts_with("GET /api/v1/y [Bearer tok123] [mcp]"));
    }

    #[test]
    fn サーバーの断りは理由つきの誤りになり衝突を見分けられる() {
        let (url, _) = mock();
        let c = Client::new(&url, "t", "cli");
        let e = c.get("/conflict").unwrap_err();
        assert!(e.is_conflict());
        assert_eq!(e.to_string(), "他の端末が先に書きました");
        let e = c.get("/plain-error").unwrap_err();
        assert!(!e.is_conflict());
        assert!(e.to_string().contains("500"));
    }

    #[test]
    fn つながらないときは手元へ切り替えず誤りにする() {
        let c = Client::new("http://127.0.0.1:1", "t", "cli");
        assert!(matches!(c.get("/api/v1/status"), Err(ClientError::Connect(_))));
        assert!(c.get("/api/v1/status").unwrap_err().to_string().contains("つながれません"));
    }

    #[test]
    fn ファイルをそのまま送る() {
        let (url, seen) = mock();
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.bin");
        std::fs::write(&f, "中身").unwrap();
        Client::new(&url, "t", "cli").put_file("/api/v1/upload?name=a", &f).unwrap();
        assert!(seen.lock().unwrap()[0].ends_with("中身"));
    }

    #[test]
    fn url用の符号化() {
        assert_eq!(encode_component("a b/日本語.zip"), "a%20b/%E6%97%A5%E6%9C%AC%E8%AA%9E.zip");
    }
}
