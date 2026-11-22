//! 窓口(REST API v1)。認証・振り分け・応答。保存の規則は `library_store`、判断は `saver`、検索の中継は `relay`。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, FixedOffset};
use library_store::{docs, fsx, journal, Layout, StoreError};
use serde::Deserialize;
use serde_json::{json, Value};
use tiny_http::{Method, Request};

use crate::audit::Audit;
use crate::config::Config;
use crate::saver::{self, Reindex, Roots, SaveReq};
use crate::{auth, backup, children, relay, tasks_api};

pub const MAX_BODY: u64 = 4 * 1024 * 1024;
pub const MAX_UPLOAD: u64 = 512 * 1024 * 1024;

pub type Reply = (u16, Value);
pub type Clock = Arc<dyn Fn() -> DateTime<FixedOffset> + Send + Sync>;

pub enum Out {
    Json(Reply),
    Zip(Vec<u8>),
}

pub struct App {
    pub cfg: Config,
    pub layout: Layout,
    pub roots: Roots,
    pub tokens: PathBuf,
    pub audit: Audit,
    pub uploads: PathBuf,
    pub backups: PathBuf,
    pub health: Arc<children::Health>,
    pub clock: Clock,
    pub started: Instant,
}

/// 窓口のパスの先頭(どの正本か)
#[derive(Clone, Copy, PartialEq, Eq)]
enum Area {
    Library,
    Knowledge,
    Tasks,
}

pub fn err(code: u16, detail: &str) -> Reply {
    (code, json!({ "detail": detail }))
}

pub fn store_err(e: StoreError) -> Reply {
    match e {
        StoreError::Conflict { current_rev } => (409, json!({"detail": "conflict", "current_rev": current_rev})),
        StoreError::Stale(m) => (409, json!({"detail": m, "stale": true})),
        StoreError::Exists(m) => (409, json!({"detail": format!("すでにあります: {m}"), "exists": true})),
        StoreError::NotFound(m) => err(404, &format!("見つかりません: {m}")),
        StoreError::Invalid(m) => err(422, &m),
        StoreError::Io(m) => err(500, &m),
    }
}

fn header_value(req: &Request, name: &'static str) -> Option<String> {
    req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.to_string())
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let h = std::str::from_utf8(&b[i + 1..i + 3]).ok()?;
                out.push(u8::from_str_radix(h, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

pub fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == key).and_then(|(_, v)| percent_decode(v))
}

fn split_area(path: &str) -> Option<(Area, &str)> {
    let (head, rest) = path.split_once('/')?;
    let area = match head {
        "library" => Area::Library,
        "knowledge" => Area::Knowledge,
        "tasks" => Area::Tasks,
        _ => return None,
    };
    if rest.is_empty() {
        return None;
    }
    Some((area, rest))
}

impl App {
    fn root_of(&self, area: Area) -> &Path {
        match area {
            Area::Library => &self.roots.library,
            Area::Knowledge => &self.roots.knowledge,
            Area::Tasks => &self.roots.tasks,
        }
    }

    /// 窓口の `doc` から消せるのは、libraryの受信箱だけ(手順書・規約は、消せず、版つきの上書きだけ)。
    fn deletable(&self, area: Area, rel: &str) -> bool {
        area == Area::Library && docs::zone_of(rel) == Some(docs::Zone::Inbox)
    }

    fn reindex(&self, target: &Reindex) -> bool {
        match target {
            Reindex::Library(rel) => relay::reindex(self.cfg.rag_port, "reindex_file", rel),
            Reindex::Shelf(rel) => relay::reindex(self.cfg.rag_port, "reindex_shelf_file", rel),
        }
    }
}

/// 認証して、窓口を呼ぶ。返り値は、監査に残す(端末名・クライアント・応答)。
pub fn handle(app: &App, req: &mut Request) -> (String, String, Out) {
    let method = req.method().clone();
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();

    if method == Method::Get && path == "/health" {
        return ("-".into(), "-".into(), Out::Json((200, json!({"status": "ok", "api": "v1"}))));
    }
    let Some(device) = auth::authenticate(&app.tokens, header_value(req, "Authorization").as_deref()) else {
        return ("-".into(), "-".into(), Out::Json(err(401, "unauthorized")));
    };
    let client = header_value(req, "X-Shiori-Client").unwrap_or_else(|| "-".into());

    // JSONではない応答・要求
    if method == Method::Get && path == "/api/v1/backup" {
        let out = match query_param(&url, "name") {
            Some(n) if backup::is_generation_name(&n) => match std::fs::read(app.backups.join(&n)) {
                Ok(b) => Out::Zip(b),
                Err(_) => Out::Json(err(404, "その世代は無い")),
            },
            _ => Out::Json(err(400, "name が正しくない")),
        };
        return (device, client, out);
    }
    if method == Method::Put && path == "/api/v1/upload" {
        return (device, client, Out::Json(upload(app, req, &url)));
    }

    let mut body = String::new();
    if method == Method::Put || method == Method::Post {
        if req.as_reader().take(MAX_BODY + 1).read_to_string(&mut body).is_err() {
            return (device, client, Out::Json(err(400, "本文を読めない")));
        }
        if body.len() as u64 > MAX_BODY {
            return (device, client, Out::Json(err(413, "大きすぎる")));
        }
    }
    let reply = route(app, &method, &path, &url, &body, &device);
    (device, client, Out::Json(reply))
}

fn parse<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, Reply> {
    serde_json::from_str(body).map_err(|e| err(422, &format!("本文の形が正しくない: {e}")))
}

fn route(app: &App, method: &Method, path: &str, url: &str, body: &str, device: &str) -> Reply {
    if path.starts_with("/api/v1/tasks") || path.starts_with("/api/v1/task-folders") {
        return tasks_api::route(app, method, path, url, body);
    }
    match (method, path) {
        (Method::Get, "/api/v1/status") => (
            200,
            json!({
                "status": "ok", "api": "v1", "version": env!("CARGO_PKG_VERSION"),
                "layout": app.cfg.layout,
                "embed": app.health.embed.load(std::sync::atomic::Ordering::Relaxed),
                "rag": app.health.rag.load(std::sync::atomic::Ordering::Relaxed),
                "children_managed": !app.cfg.no_children,
                "uptime_seconds": app.started.elapsed().as_secs(),
            }),
        ),
        (Method::Get, "/api/v1/doc") => doc_get(app, url),
        (Method::Get, "/api/v1/list") => doc_list(app, url),
        (Method::Get, "/api/v1/library-index") => library_index(app),
        (Method::Get, "/api/v1/pending") => match library_store::pending::list(&app.roots.library) {
            Ok(p) => (200, json!(p)),
            Err(m) => err(500, &m),
        },
        (Method::Post, "/api/v1/pending/resolve") => pending_resolve(app, url),
        // 規約画面の、libraryにある書類(memory・スキル・規約・台帳・プロフィール)。許可リストのidだけ読める
        (Method::Get, "/api/v1/system-docs") => (200, json!(library_store::system_docs::list_library(&app.roots.library, app.layout))),
        (Method::Get, "/api/v1/system-docs/read") => match query_param(url, "id") {
            None => err(400, "id が必要"),
            Some(id) => match library_store::system_docs::read_library(&app.roots.library, app.layout, &id) {
                Ok(content) => (200, json!({"id": id, "content": content})),
                Err(m) => err(404, &m),
            },
        },
        (Method::Get, "/api/v1/source") => source_get(app, url),
        (Method::Get, "/api/v1/shelf") => (200, json!(library_store::shelf_view::list(&app.roots.knowledge))),
        (Method::Get, "/api/v1/shelf/doc") => match query_param(url, "name") {
            None => err(400, "name が必要"),
            Some(n) => match library_store::shelf_view::read(&app.roots.knowledge, &n) {
                Ok(content) => (200, json!({"name": n, "content": content})),
                Err(m) => err(404, &m),
            },
        },
        (Method::Put, "/api/v1/doc") => doc_put(app, body),
        (Method::Delete, "/api/v1/doc") => doc_delete(app, url),
        (Method::Post, "/api/v1/save") => save(app, body, device),
        (Method::Post, "/api/v1/journal/append") => journal_append(app, body, device),
        (Method::Post, "/api/v1/search/library") => search(app, "search_library", body),
        (Method::Post, "/api/v1/search/shelf") => search(app, "search_shelf", body),
        (Method::Post, "/api/v1/search") => search(app, "search", body),
        (Method::Post, "/api/v1/backup") => {
            let lib = app.roots.library.clone();
            match backup::create(
                &[("library", &lib), ("knowledge", &app.roots.knowledge), ("tasks", &app.roots.tasks)],
                &app.backups,
            ) {
                Ok(g) => (200, json!({"name": g.name, "size": g.size, "sha256": g.sha256, "files": g.files})),
                Err(e) => err(500, &format!("世代を作れない: {e}")),
            }
        }
        (Method::Get, "/api/v1/backups") => {
            let v: Vec<Value> = backup::list(&app.backups)
                .into_iter()
                .map(|(name, size, sha256)| json!({"name": name, "size": size, "sha256": sha256}))
                .collect();
            (200, json!(v))
        }
        _ => err(404, "Not Found"),
    }
}

/// フォルダの中のファイルの一覧(`GET /api/v1/list?path=library/20-projects/shiori/journal[&limit=N]`)。
/// 名前の降順(日付の名前なら、新しい順)。フォルダが無ければ空。直近の日誌を探すときなどに使う。
fn doc_list(app: &App, url: &str) -> Reply {
    let Some(p) = query_param(url, "path") else { return err(400, "path が必要") };
    let p = p.trim_end_matches('/').to_string();
    let Some((area, rel)) = split_area(&p) else { return err(400, "パスは library/… knowledge/… tasks/… の形") };
    let limit = query_param(url, "limit").and_then(|s| s.parse::<usize>().ok()).unwrap_or(100).min(1000);
    match fsx::list_entries(app.root_of(area), rel) {
        Ok(entries) => {
            let items: Vec<Value> = entries
                .into_iter()
                .take(limit)
                .map(|e| json!({"name": e.name, "path": format!("{p}/{}", e.name), "size": e.size, "modified": e.modified}))
                .collect();
            (200, json!({"path": p, "entries": items}))
        }
        Err(e) => store_err(e),
    }
}

fn doc_get(app: &App, url: &str) -> Reply {
    let Some(p) = query_param(url, "path") else { return err(400, "path が必要") };
    let Some((area, rel)) = split_area(&p) else { return err(400, "パスは library/… knowledge/… tasks/… の形") };
    match fsx::read(app.root_of(area), rel) {
        Ok((bytes, rev)) => match String::from_utf8(bytes) {
            Ok(content) => (200, json!({"path": p, "content": content, "rev": rev})),
            Err(_) => err(415, "UTF-8の文章ではない"),
        },
        Err(e) => store_err(e),
    }
}

#[derive(Deserialize)]
struct PutDoc {
    path: String,
    content: String,
    /// あれば、読んだ版のときだけ上書きする。無ければ、新規(同じパスがあれば断る)。
    expect_rev: Option<String>,
}

fn doc_put(app: &App, body: &str) -> Reply {
    let d: PutDoc = match parse(body) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let Some((area, rel)) = split_area(&d.path) else { return err(400, "パスは library/… knowledge/… tasks/… の形") };
    // 直接書けるのは、受信箱・手順書(`30-resources/skills/`)・規約(`_system/CLAUDE.md` など。上書きだけ)の3か所だけ。
    // ほかは、規則を通る保存・日誌の追記・タスクの窓口から。上書きは、読んだ版(expect_rev)のときだけ。
    let zone = if area == Area::Library { docs::zone_of(rel) } else { None };
    let Some(zone) = zone else {
        return err(403, "ここには直接書けない(受信箱・手順書・規約だけ。ほかは、保存・日誌の追記・タスクの窓口を使う)");
    };
    let stamp = (app.clock)().format("%Y%m%dT%H%M%S").to_string();
    match docs::put(app.root_of(area), rel, d.content.as_bytes(), d.expect_rev.as_deref(), &stamp) {
        Ok(p) => {
            // `_system/` は索引の対象外
            let indexed = zone != docs::Zone::System && app.reindex(&Reindex::Library(rel.to_string()));
            (
                200,
                json!({
                    "rev": p.rev, "indexed": indexed, "created": p.created,
                    "history_file": p.history_rel.as_ref().map(|h| format!("library/{h}")),
                }),
            )
        }
        Err(e) => store_err(e),
    }
}

fn doc_delete(app: &App, url: &str) -> Reply {
    let Some(p) = query_param(url, "path") else { return err(400, "path が必要") };
    let Some((area, rel)) = split_area(&p) else { return err(400, "パスは library/… knowledge/… tasks/… の形") };
    if !app.deletable(area, rel) {
        return err(403, "ここは直接消せない(消せるのは、受信箱だけ)");
    }
    match fsx::remove(app.root_of(area), rel, query_param(url, "expect_rev").as_deref()) {
        Ok(()) => (200, json!({"deleted": p})),
        Err(e) => store_err(e),
    }
}

#[derive(Deserialize)]
struct SaveBody {
    file_name: String,
    content: String,
    expect_updated: Option<String>,
    asset_upload: Option<String>,
}

fn save(app: &App, body: &str, device: &str) -> Reply {
    let b: SaveBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let req = SaveReq { file_name: b.file_name, content: b.content, expect_updated: b.expect_updated, asset_upload: b.asset_upload };
    let now = (app.clock)();
    match saver::save(&app.roots, app.layout, &app.uploads, &req, device, &now) {
        Ok((mut out, reindex)) => {
            // 索引の更新が要る対象が無い(下書きの棚など)ときは false。全部更新できたときだけ true
            let indexed = !reindex.is_empty() && reindex.iter().all(|t| app.reindex(t));
            if let Some(o) = out.as_object_mut() {
                o.insert("indexed".into(), json!(indexed));
            }
            (200, out)
        }
        Err(e) => store_err(e),
    }
}

#[derive(Deserialize)]
struct JournalBody {
    project: String,
    content: String,
    ai: Option<String>,
    summary: Option<String>,
}

fn journal_append(app: &App, body: &str, device: &str) -> Reply {
    let b: JournalBody = match parse(body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let now = (app.clock)();
    let req = journal::AppendRequest {
        project: &b.project,
        content: &b.content,
        device,
        ai: b.ai.as_deref(),
        summary: b.summary.as_deref(),
    };
    match journal::append(&app.roots.library, app.layout, &req, &now) {
        Ok(a) => {
            let mut indexed = app.reindex(&Reindex::Library(a.rel.clone()));
            if a.toc_added {
                let common = format!("{}/{}.md", app.layout.journal_dir(library_store::layout::COMMON_PROJECT), a.day);
                indexed &= app.reindex(&Reindex::Library(common));
            }
            (
                200,
                json!({
                    "path": format!("library/{}", a.rel), "day": a.day, "heading": a.heading, "rev": a.rev,
                    "toc_added": a.toc_added, "toc_error": a.toc_error, "indexed": indexed,
                    "warnings": a.warnings,
                }),
            )
        }
        Err(e) => store_err(e),
    }
}

fn search(app: &App, endpoint: &str, body: &str) -> Reply {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return err(422, "本文がJSONではない"),
    };
    let bases: [(&Path, &str); 2] = [(&app.roots.library, "library"), (&app.roots.knowledge, "knowledge")];
    match relay::search(app.cfg.rag_port, endpoint, &v, &bases) {
        Ok(mut r) => {
            // 図書館の検索結果には、一覧に出す属性と冒頭の文を添える
            if endpoint == "search_library" {
                relay::add_previews(&mut r, &bases);
            }
            (200, r)
        }
        Err((code, m)) => err(code, &m),
    }
}

/// 要確認の解決(`POST /api/v1/pending/resolve?kind=tag|project|inbox&key=<対象>&action=confirm|defer|reject`)。
/// 対象と操作をURLに置くのは、監査ログ(URLを残す)に、何を解決したかが残るようにするため。
fn pending_resolve(app: &App, url: &str) -> Reply {
    let (Some(kind), Some(key), Some(action)) = (query_param(url, "kind"), query_param(url, "key"), query_param(url, "action")) else {
        return err(400, "kind・key・action が必要");
    };
    let Some(kind) = library_store::pending::Kind::parse(&kind) else { return err(422, "kind は tag・project・inbox のどれか") };
    match library_store::pending::resolve(&app.roots.library, kind, &key, &action) {
        Ok(()) => {
            // 受信箱の記事の状態が変わった・消えたときは、索引も合わせる
            let indexed = kind == library_store::pending::Kind::Inbox && app.reindex(&Reindex::Library(format!("00-inbox/{key}")));
            (200, json!({"resolved": key, "action": action, "indexed": indexed}))
        }
        Err(e) => store_err(e),
    }
}

/// 記事の本文(`GET /api/v1/source?category=<source_category>&source=<source>`)。アプリの参照資料の表示用。
/// 一覧の `source_category`・`source` から実ファイルを探す(アプリの手元の版と同じ処理)。libraryの外は読まない。
fn source_get(app: &App, url: &str) -> Reply {
    let (Some(category), Some(source)) = (query_param(url, "category"), query_param(url, "source")) else {
        return err(400, "category と source が必要");
    };
    // 手元の版と同じ。`..` などは、探し方の側で受け付けない
    let root = &app.roots.library;
    let search_root = if category == "uncategorized" { root.clone() } else { root.join(&category) };
    if category.contains(['/', '\\']) || category == ".." {
        return err(400, "category が正しくない");
    }
    let Some(file) = library_store::source_path::resolve_source_file(&search_root, &source) else {
        return err(404, "指定された参照資料が見つかりませんでした。");
    };
    let (Ok(canon_root), Ok(canon)) = (root.canonicalize(), file.canonicalize()) else {
        return err(404, "指定された参照資料が見つかりませんでした。");
    };
    if !canon.starts_with(&canon_root) {
        return err(403, "不正なパスが指定されました。");
    }
    match std::fs::read(&canon).map(String::from_utf8) {
        Ok(Ok(content)) => {
            let rel = canon.strip_prefix(&canon_root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            (200, json!({"path": format!("library/{rel}"), "content": content}))
        }
        Ok(Err(_)) => err(415, "UTF-8の文章ではない"),
        Err(e) => err(500, &format!("読み込みに失敗: {e}")),
    }
}

/// 蔵書の一覧(`GET /api/v1/library-index`)。アプリの図書館の画面用。各項目に `preview` を添える。
fn library_index(app: &App) -> Reply {
    let bases: [(&Path, &str); 2] = [(&app.roots.library, "library"), (&app.roots.knowledge, "knowledge")];
    match relay::list_library(app.cfg.rag_port, &bases) {
        Ok(mut r) => {
            relay::add_previews(&mut r, &bases);
            (200, r)
        }
        Err((code, m)) => err(code, &m),
    }
}

/// 素材の実体を受け取る(`PUT /api/v1/upload?name=<素材の名前>&sha256=<SHA-256>`、本文は実体そのもの)。
/// 受け取りながらSHA-256を求め、渡された値と合うときだけ置く。置き場は `data/uploads/<sha256>/<名前>`。
fn upload(app: &App, req: &mut Request, url: &str) -> Reply {
    let (Some(name), Some(sha)) = (query_param(url, "name"), query_param(url, "sha256")) else {
        return err(400, "name と sha256 が必要");
    };
    if name.is_empty() || name.contains(['/', '\\', ':']) || name.starts_with('.') {
        return err(422, "素材の名前が正しくない");
    }
    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return err(422, "sha256 が正しくない");
    }
    let sha = sha.to_ascii_lowercase();
    let tmp_dir = app.uploads.join("_tmp");
    if std::fs::create_dir_all(&tmp_dir).is_err() {
        return err(500, "受け取りの置き場を作れない");
    }
    let mut seed = [0u8; 8];
    let _ = getrandom::getrandom(&mut seed);
    let tmp = tmp_dir.join(format!("{}.part", seed.iter().map(|b| format!("{b:02x}")).collect::<String>()));
    let result = (|| -> Result<(String, u64), String> {
        use sha2::{Digest, Sha256};
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        let mut buf = vec![0u8; 1024 * 1024];
        let reader = req.as_reader();
        loop {
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > MAX_UPLOAD {
                return Err("大きすぎる".into());
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        }
        Ok((hasher.finalize().iter().map(|b| format!("{b:02x}")).collect(), total))
    })();
    match result {
        Err(m) => {
            let _ = std::fs::remove_file(&tmp);
            if m == "大きすぎる" { err(413, &m) } else { err(500, &format!("受け取れない: {m}")) }
        }
        Ok((got, _)) if got != sha => {
            let _ = std::fs::remove_file(&tmp);
            err(422, &format!("SHA-256が一致しない(受け取った中身: {got})"))
        }
        Ok((_, size)) => {
            let dir = app.uploads.join(&sha);
            let dest = dir.join(&name);
            let moved = std::fs::create_dir_all(&dir).and_then(|_| std::fs::rename(&tmp, &dest));
            match moved {
                Ok(()) => (200, json!({"upload_id": sha, "size": size})),
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    err(500, &format!("置けない: {e}"))
                }
            }
        }
    }
}
