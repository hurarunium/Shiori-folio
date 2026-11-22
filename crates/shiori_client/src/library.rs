//! 蔵書の読み出し(一覧・検索・本文・棚)の窓口への呼び出し。アプリの図書館・棚の画面が使う。
//! 結果は、サーバーが返したJSONのまま返す。パスは、`library/…`・`knowledge/…` の相対パス。

use serde_json::Value;

use crate::{encode_component, Client, ClientError};

type R = Result<Value, ClientError>;

/// 蔵書の一覧。各項目に `preview`(属性と冒頭の文)が付く。
pub fn index(c: &Client) -> R {
    c.get("/api/v1/library-index")
}

/// 図書館の検索。`body` は `{query, limit, offset, filter?, mode?}`。各結果に `preview` が付く。
pub fn search_library(c: &Client, body: &Value) -> R {
    c.post("/api/v1/search/library", body)
}

/// 棚の検索。`body` は `{query, limit, offset, topic?}`。
pub fn search_shelf(c: &Client, body: &Value) -> R {
    c.post("/api/v1/search/shelf", body)
}

/// 記事の本文(`{path, content}`)。一覧の `source_category`・`source` から探す。
pub fn source(c: &Client, category: &str, source: &str) -> R {
    c.get(&format!("/api/v1/source?category={}&source={}", encode_component(category), encode_component(source)))
}

/// 棚の記事の一覧。
pub fn shelf_list(c: &Client) -> R {
    c.get("/api/v1/shelf")
}

/// 棚の記事の本文(`{name, content}`)。
pub fn shelf_doc(c: &Client, name: &str) -> R {
    c.get(&format!("/api/v1/shelf/doc?name={}", encode_component(name)))
}

/// 文書の本文(`{path, content, rev}`)。`path` は `library/…`・`knowledge/…`。
pub fn doc(c: &Client, path: &str) -> R {
    c.get(&format!("/api/v1/doc?path={}", encode_component(path)))
}

/// フォルダの一覧(`{path, entries}`)。
pub fn list(c: &Client, path: &str, limit: usize) -> R {
    c.get(&format!("/api/v1/list?path={}&limit={limit}", encode_component(path)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn 窓口ごとの要求の形() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                log.lock().unwrap().push(format!("{} {} {body}", req.method(), req.url()));
                let _ = req.respond(tiny_http::Response::from_string("[]"));
            }
        });
        let c = Client::new(&url, "t", "app");
        index(&c).unwrap();
        search_library(&c, &serde_json::json!({"query": "詩織", "limit": 5})).unwrap();
        search_shelf(&c, &serde_json::json!({"query": "数学"})).unwrap();
        source(&c, "20-projects", "shiori/memory.md").unwrap();
        shelf_list(&c).unwrap();
        shelf_doc(&c, "超実数.md").unwrap();
        doc(&c, "library/a b.md").unwrap();
        list(&c, "library/20-projects/shiori/journal", 3).unwrap();
        let log = seen.lock().unwrap().clone();
        assert_eq!(log[0], "GET /api/v1/library-index ");
        assert_eq!(log[1], r#"POST /api/v1/search/library {"limit":5,"query":"詩織"}"#);
        assert_eq!(log[2], r#"POST /api/v1/search/shelf {"query":"数学"}"#);
        assert_eq!(log[3], "GET /api/v1/source?category=20-projects&source=shiori/memory.md ");
        assert_eq!(log[4], "GET /api/v1/shelf ");
        assert!(log[5].starts_with("GET /api/v1/shelf/doc?name=%E8%B6%85"));
        assert_eq!(log[6], "GET /api/v1/doc?path=library/a%20b.md ");
        assert_eq!(log[7], "GET /api/v1/list?path=library/20-projects/shiori/journal&limit=3 ");
    }
}
