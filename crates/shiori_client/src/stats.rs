//! 利用統計の窓口(`/api/v1/stats/claude`)への呼び出し(詩織Ver7.0.1)。アプリのClaudeタブが使う。
//! 差分と集計の形は `claude_stats` の型と同じ。ここはJSONのまま運ぶ(この部品は `claude_stats` に依存しない)。

use serde_json::Value;

use crate::{encode_component, Client, ClientError};

type R = Result<Value, ClientError>;

/// 端末の台帳の差分をサーバーへ送る。返すのは `{responses, messages}`(取り込まれた件数)。
pub fn claude_push(c: &Client, host: &str, delta: &Value) -> R {
    c.put(&format!("/api/v1/stats/claude/{}", encode_component(host)), delta)
}

/// 全端末の合算を取る。`range` は `all`・`days30`・`days7`、`tz` はUTCからの時差(分)、
/// `host` を渡すとその端末だけ。返すのは `ClaudeStats` のJSON。
pub fn claude_summary(c: &Client, range: &str, host: Option<&str>, tz_minutes: i32) -> R {
    let mut path = format!("/api/v1/stats/claude?range={}&tz={tz_minutes}", encode_component(range));
    if let Some(h) = host {
        path.push_str(&format!("&host={}", encode_component(h)));
    }
    c.get(&path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    fn mock() -> (Client, Arc<Mutex<Vec<String>>>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.server_addr().to_ip().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                log.lock().unwrap().push(format!("{} {} {body}", req.method(), req.url()));
                let _ = req.respond(tiny_http::Response::from_string("{}"));
            }
        });
        (Client::new(&url, "t", "app"), seen)
    }

    #[test]
    fn 窓口ごとの要求の形() {
        let (c, seen) = mock();
        claude_push(&c, "MacBook.local", &json!({"version": 1, "responses": [], "messages": []})).unwrap();
        claude_push(&c, "日本 PC", &json!({"version": 1})).unwrap();
        claude_summary(&c, "days30", None, 540).unwrap();
        claude_summary(&c, "all", Some("Mac Book"), -300).unwrap();
        let log = seen.lock().unwrap().clone();
        assert_eq!(log[0], r#"PUT /api/v1/stats/claude/MacBook.local {"messages":[],"responses":[],"version":1}"#);
        assert_eq!(log[1], r#"PUT /api/v1/stats/claude/%E6%97%A5%E6%9C%AC%20PC {"version":1}"#, "ホスト名は符号化");
        assert_eq!(log[2], "GET /api/v1/stats/claude?range=days30&tz=540 ");
        assert_eq!(log[3], "GET /api/v1/stats/claude?range=all&tz=-300&host=Mac%20Book ");
    }
}
