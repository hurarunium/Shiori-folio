//! タスクの窓口。処理の中身は `library_store::tasks`(アプリ・`shiori-task` と同じ)。
//!
//! - `GET    /api/v1/tasks[?archived=1]`        → `{tasks, folders}`
//! - `POST   /api/v1/tasks`                      `{summary, folder?, due?, detail?}` → タスク
//! - `PUT    /api/v1/tasks/{id}`                 `{summary?, due?("none"で外す), detail?, folder?(""で未分類), expected_mtime?}` → タスク
//! - `POST   /api/v1/tasks/{id}/done` / `/restore` → タスク
//! - `DELETE /api/v1/tasks/{id}`                 → `{deleted}`
//! - `GET    /api/v1/task-folders`               → `{folders}`
//! - `POST   /api/v1/task-folders`               `{folder}` / `PUT` `{from, to}` / `DELETE ?folder=`

use std::sync::Mutex;

use library_store::tasks::{self, NewTask, TaskEdit};
use serde::Deserialize;
use serde_json::{json, Value};
use tiny_http::Method;

use crate::api::{err, query_param, App, Reply};

/// タスクの変更は1本の列に並べる(idの採番・INDEX.mdの更新が重ならないように)。
static LOCK: Mutex<()> = Mutex::new(());

fn parse<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, Reply> {
    serde_json::from_str(body).map_err(|e| err(422, &format!("本文の形が正しくない: {e}")))
}

/// タスクの処理の失敗(文章)を、応答にする。見つからない・不正は、利用者の誤りとして4xxにする。
fn fail(message: String) -> Reply {
    let code = if message.contains("見つかりません") || message.contains("見つかりませんでした") {
        404
    } else if message.contains("変わりました") || message.contains("すでに") || message.contains("書き換えられ") {
        409
    } else {
        422
    };
    err(code, &message)
}

#[derive(Deserialize)]
struct NewBody {
    summary: String,
    #[serde(default)]
    folder: String,
    due: Option<String>,
    #[serde(default)]
    detail: String,
}

#[derive(Deserialize)]
struct EditBody {
    summary: Option<String>,
    due: Option<String>,
    detail: Option<String>,
    folder: Option<String>,
    expected_mtime: Option<f64>,
}

#[derive(Deserialize)]
struct FolderBody {
    folder: String,
}

#[derive(Deserialize)]
struct RenameBody {
    from: String,
    to: String,
}

pub fn route(app: &App, method: &Method, path: &str, url: &str, body: &str) -> Reply {
    let root = &app.roots.tasks;
    if path == "/api/v1/task-folders" {
        return match method {
            Method::Get => (200, json!({"folders": tasks::folders(root)})),
            Method::Post => match parse::<FolderBody>(body) {
                Err(r) => r,
                Ok(b) => {
                    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
                    match tasks::folder_add(root, &b.folder) {
                        Ok(_) => (200, json!({"folders": tasks::folders(root)})),
                        Err(m) => fail(m),
                    }
                }
            },
            Method::Put => match parse::<RenameBody>(body) {
                Err(r) => r,
                Ok(b) => {
                    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
                    match tasks::folder_rename(root, &b.from, &b.to) {
                        Ok(_) => (200, json!({"folders": tasks::folders(root)})),
                        Err(m) => fail(m),
                    }
                }
            },
            Method::Delete => match query_param(url, "folder") {
                None => err(400, "folder が必要"),
                Some(f) => {
                    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
                    match tasks::folder_remove(root, &f) {
                        Ok(()) => (200, json!({"folders": tasks::folders(root)})),
                        Err(m) => fail(m),
                    }
                }
            },
            _ => err(404, "Not Found"),
        };
    }

    let rest = path.strip_prefix("/api/v1/tasks").unwrap_or("");
    let parts: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    match (method, parts.as_slice()) {
        (Method::Get, []) => {
            let archived = matches!(query_param(url, "archived").as_deref(), Some("1" | "true"));
            (200, json!({"tasks": tasks::list(root, archived), "folders": tasks::folders(root)}))
        }
        (Method::Post, []) => match parse::<NewBody>(body) {
            Err(r) => r,
            Ok(b) => {
                let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
                // 先頭のBOM(PowerShellが付ける)は、詳細に残さない
                let detail = b.detail.trim_start_matches('\u{feff}');
                let new = NewTask { summary: &b.summary, folder: &b.folder, due: b.due.as_deref(), detail };
                match tasks::add(root, &new) {
                    Ok(t) => (200, json!(t)),
                    Err(m) => fail(m),
                }
            }
        },
        (Method::Put, [id]) => match parse::<EditBody>(body) {
            Err(r) => r,
            Ok(b) => {
                let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
                if b.summary.is_none() && b.due.is_none() && b.detail.is_none() && b.folder.is_none() {
                    return err(422, "変更する項目(summary・due・detail・folder)を指定してください");
                }
                let mut result: Result<Value, String> = Ok(Value::Null);
                if b.summary.is_some() || b.due.is_some() || b.detail.is_some() {
                    let change = TaskEdit {
                        summary: b.summary.as_deref(),
                        due: b.due.as_deref().map(|d| if d == "none" { None } else { Some(d) }),
                        detail: b.detail.as_deref().map(|d| d.trim_start_matches('\u{feff}')),
                    };
                    result = tasks::edit(root, id, &change, b.expected_mtime).map(|t| json!(t));
                }
                if let (Ok(_), Some(folder)) = (&result, &b.folder) {
                    result = tasks::move_to(root, id, folder).map(|t| json!(t));
                }
                match result {
                    Ok(v) => (200, v),
                    Err(m) => fail(m),
                }
            }
        },
        (Method::Post, [id, "done"]) => {
            let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
            match tasks::complete(root, id) {
                Ok(t) => (200, json!(t)),
                Err(m) => fail(m),
            }
        }
        (Method::Post, [id, "restore"]) => {
            let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
            match tasks::restore(root, id) {
                Ok(t) => (200, json!(t)),
                Err(m) => fail(m),
            }
        }
        (Method::Delete, [id]) => {
            let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
            match tasks::delete(root, id) {
                Ok(()) => (200, json!({"deleted": id})),
                Err(m) => fail(m),
            }
        }
        _ => err(404, "Not Found"),
    }
}
