//! タスクの窓口(`/api/v1/tasks`・`/api/v1/task-folders`)への呼び出し。アプリの画面が使う。
//! 結果は、サーバーが返したJSONのまま返す(タスクの形は `library_store::tasks::Task` と同じ)。

use serde_json::{json, Value};

use crate::{encode_component, Client, ClientError};

type R = Result<Value, ClientError>;

/// `{tasks, folders}`。
pub fn list(c: &Client, archived: bool) -> R {
    c.get(&format!("/api/v1/tasks{}", if archived { "?archived=1" } else { "" }))
}

pub fn add(c: &Client, summary: &str, folder: &str, due: Option<&str>, detail: &str) -> R {
    let mut body = json!({ "summary": summary, "folder": folder, "detail": detail });
    if let Some(d) = due {
        body["due"] = json!(d);
    }
    c.post("/api/v1/tasks", &body)
}

/// 編集の欄の中身をまとめて保存する。`due` が `None` なら期限を外す。
/// `expected_mtime` は編集を始めたときのmtimeで、変わっていればサーバーが断る(409)。
pub fn save(c: &Client, id: &str, summary: &str, folder: &str, due: Option<&str>, detail: &str, expected_mtime: f64) -> R {
    let body = json!({
        "summary": summary, "folder": folder, "due": due.unwrap_or("none"), "detail": detail,
        "expected_mtime": expected_mtime,
    });
    c.put(&format!("/api/v1/tasks/{}", encode_component(id)), &body)
}

pub fn complete(c: &Client, id: &str) -> R {
    c.post(&format!("/api/v1/tasks/{}/done", encode_component(id)), &json!({}))
}

pub fn restore(c: &Client, id: &str) -> R {
    c.post(&format!("/api/v1/tasks/{}/restore", encode_component(id)), &json!({}))
}

pub fn delete(c: &Client, id: &str) -> R {
    c.delete(&format!("/api/v1/tasks/{}", encode_component(id)))
}

/// `{folders}`。
pub fn folders(c: &Client) -> R {
    c.get("/api/v1/task-folders")
}

pub fn folder_add(c: &Client, folder: &str) -> R {
    c.post("/api/v1/task-folders", &json!({ "folder": folder }))
}

pub fn folder_rename(c: &Client, from: &str, to: &str) -> R {
    c.put("/api/v1/task-folders", &json!({ "from": from, "to": to }))
}

pub fn folder_remove(c: &Client, folder: &str) -> R {
    c.delete(&format!("/api/v1/task-folders?folder={}", encode_component(folder)))
}

#[cfg(test)]
mod tests {
    use super::*;
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
        list(&c, true).unwrap();
        add(&c, "概要", "Unity/Tanker", Some("2026-11-30"), "詳細").unwrap();
        save(&c, "t 1", "新", "", None, "本文", 12.5).unwrap();
        complete(&c, "a").unwrap();
        restore(&c, "a").unwrap();
        delete(&c, "a").unwrap();
        folder_add(&c, "x").unwrap();
        folder_rename(&c, "x", "y").unwrap();
        folder_remove(&c, "日本").unwrap();
        let log = seen.lock().unwrap().clone();
        assert_eq!(log[0], "GET /api/v1/tasks?archived=1 ");
        assert_eq!(log[1], r#"POST /api/v1/tasks {"detail":"詳細","due":"2026-11-30","folder":"Unity/Tanker","summary":"概要"}"#);
        assert_eq!(
            log[2],
            r#"PUT /api/v1/tasks/t%201 {"detail":"本文","due":"none","expected_mtime":12.5,"folder":"","summary":"新"}"#,
            "期限なしは none、idは符号化"
        );
        assert_eq!(log[3], "POST /api/v1/tasks/a/done {}");
        assert_eq!(log[4], "POST /api/v1/tasks/a/restore {}");
        assert_eq!(log[5], "DELETE /api/v1/tasks/a ");
        assert_eq!(log[6], r#"POST /api/v1/task-folders {"folder":"x"}"#);
        assert_eq!(log[7], r#"PUT /api/v1/task-folders {"from":"x","to":"y"}"#);
        assert_eq!(log[8], "DELETE /api/v1/task-folders?folder=%E6%97%A5%E6%9C%AC ");
    }
}
