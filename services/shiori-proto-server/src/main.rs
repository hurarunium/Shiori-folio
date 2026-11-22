//! 詩織Ver7.0 Phase 0の試作: サーバー側の窓口(shiori-proto-server)。
//!
//! 使い方:
//!   shiori-proto-server serve --bind <IP:ポート> --library <DIR> --runtime <DIR> --data <DIR>
//!                             [--tasks <DIR>] [--embed-port 8082] [--rag-port 8083] [--no-children]
//!   shiori-proto-server token-add <端末名> --data <DIR>      (トークンを1回だけ表示する)
//!   shiori-proto-server token-revoke <端末名> --data <DIR>
//!
//! 窓口:
//!   GET  /health                (認証なし。生きているかだけ)
//!   GET  /status                (認証あり。子の健康)
//!   GET  /doc?path=<論理パス>    → {path, content, rev}
//!   PUT  /doc  {path, content, expect_rev}  → {rev}。違えば409(現在のrevつき)
//!   POST /search_library        (RAGへ中継。結果から、サーバーの絶対パスは外す)
//!   POST /backup                (世代のzipを作る。manifestつき。作ったあとに照らす)
//!   GET  /backups               (世代の一覧)
//!   GET  /backup?name=<世代名>   (世代のzipを渡す)
//!   shiori-proto-server backup-verify <zip> --dest <空のDIR> [--against-library <DIR>] [--against-tasks <DIR>]
//!
//! 待ち受けは引数で渡したアドレスだけ。0.0.0.0 などの全部のアドレスは拒否する。

mod audit;
mod auth;
mod backup;
mod children;
mod store;

use std::collections::HashMap;
use std::io::Read;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response};

use store::{Store, StoreError};

const MAX_BODY: u64 = 2 * 1024 * 1024;

struct App {
    store: Store,
    tokens: PathBuf,
    audit: audit::Audit,
    rag_port: u16,
    health: Arc<children::Health>,
    no_children: bool,
    backups: PathBuf,
    tasks_dir: PathBuf,
}

/// 窓口の返事。ふつうはJSON。世代のzipを渡すときだけ、ファイルの中身そのもの
enum Out {
    Json(Reply),
    Zip(Vec<u8>),
}

type Reply = (u16, Value);

fn options(args: &[String]) -> Result<(Vec<String>, HashMap<String, String>), String> {
    let mut pos = Vec::new();
    let mut map = HashMap::new();
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        if let Some(k) = a.strip_prefix("--") {
            if k == "no-children" {
                map.insert(k.to_string(), "1".into());
            } else {
                let v = it.next().ok_or_else(|| format!("--{k} の値が無い"))?;
                map.insert(k.to_string(), v.clone());
            }
        } else {
            pos.push(a.clone());
        }
    }
    Ok((pos, map))
}

fn need<'a>(o: &'a HashMap<String, String>, k: &str) -> Result<&'a String, String> {
    o.get(k).ok_or_else(|| format!("--{k} が必要"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match run(&args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    };
    std::process::exit(code);
}

fn run(args: &[String]) -> Result<(), String> {
    let (pos, o) = options(args.get(1..).unwrap_or(&[]))?;
    match args.first().map(String::as_str) {
        Some("serve") => serve(&o),
        Some("token-add") => {
            let name = pos.first().ok_or("端末名が必要")?;
            let data = PathBuf::from(need(&o, "data")?);
            std::fs::create_dir_all(&data).map_err(|e| e.to_string())?;
            let token = auth::add(&data.join("tokens.json"), name)?;
            println!("端末 {name} のトークン(この1回だけ表示。控えてください):\n{token}");
            Ok(())
        }
        Some("backup-verify") => backup_verify(&pos, &o),
        Some("token-revoke") => {
            let name = pos.first().ok_or("端末名が必要")?;
            auth::revoke(&PathBuf::from(need(&o, "data")?).join("tokens.json"), name)?;
            println!("{name} を失効した");
            Ok(())
        }
        _ => Err("使い方: shiori-proto-server serve|token-add|token-revoke …(ファイル先頭の説明を参照)".into()),
    }
}

fn serve(o: &HashMap<String, String>) -> Result<(), String> {
    let bind: SocketAddr = need(o, "bind")?.parse().map_err(|e| format!("--bind が正しくない: {e}"))?;
    if bind.ip().is_unspecified() {
        return Err("全部のアドレス(0.0.0.0 / ::)では待ち受けない。Tailscaleのアドレスか 127.0.0.1 を指定する".into());
    }
    let library = PathBuf::from(need(o, "library")?);
    let runtime = PathBuf::from(need(o, "runtime")?);
    let data = PathBuf::from(need(o, "data")?);
    let tasks = o.get("tasks").map(PathBuf::from).unwrap_or_else(|| library.parent().unwrap_or(&library).join("tasks"));
    let embed_port: u16 = o.get("embed-port").map(|s| s.parse().unwrap_or(8082)).unwrap_or(8082);
    let rag_port: u16 = o.get("rag-port").map(|s| s.parse().unwrap_or(8083)).unwrap_or(8083);
    if !library.is_dir() {
        return Err(format!("library が無い: {}", library.display()));
    }
    std::fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&tasks).map_err(|e| e.to_string())?;

    let health = Arc::new(children::Health::default());
    let no_children = o.contains_key("no-children");
    if !no_children {
        children::supervise(
            children::Paths { runtime, library: library.clone(), data: data.clone(), embed_port, rag_port },
            health.clone(),
        );
    }
    let app = Arc::new(App {
        backups: data.join("backups"),
        tasks_dir: tasks.clone(),
        store: Store::new(library, tasks),
        tokens: data.join("tokens.json"),
        audit: audit::Audit::open(&data.join("audit.jsonl"))?,
        rag_port,
        health,
        no_children,
    });

    let server = Arc::new(tiny_http::Server::http(bind).map_err(|e| format!("待ち受けられない {bind}: {e}"))?);
    eprintln!("待ち受け: http://{bind}(子の起動: {})", if no_children { "しない" } else { "する" });
    let mut handles = Vec::new();
    for _ in 0..8 {
        let (server, app) = (server.clone(), app.clone());
        handles.push(std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let t = Instant::now();
                let method = req.method().to_string();
                let remote = req.remote_addr().map(|a| a.ip().to_string()).unwrap_or_default();
                let url = req.url().to_string();
                let (device, out) = handle(&app, &mut req);
                let code = match &out {
                    Out::Json((c, _)) => *c,
                    Out::Zip(_) => 200,
                };
                app.audit.record(&device, &remote, &method, &url_path(&url), code, t.elapsed().as_millis() as u64);
                match out {
                    Out::Json((code, body)) => {
                        let resp = Response::from_string(body.to_string())
                            .with_status_code(code)
                            .with_header(Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap());
                        let _ = req.respond(resp);
                    }
                    Out::Zip(bytes) => {
                        let resp = Response::from_data(bytes)
                            .with_header(Header::from_bytes("Content-Type", "application/zip").unwrap());
                        let _ = req.respond(resp);
                    }
                }
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

fn url_path(url: &str) -> String {
    // 監査には、問い合わせの中の記事のパスまでを残す(本文は残さない)
    url.to_string()
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

fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == key).and_then(|(_, v)| percent_decode(v))
}

fn err(code: u16, detail: &str) -> Reply {
    (code, json!({"detail": detail}))
}

fn store_err(e: StoreError) -> Reply {
    match e {
        StoreError::BadPath => err(400, "パスが正しくない"),
        StoreError::Forbidden => err(403, "ここには書けない"),
        StoreError::NotFound => err(404, "記事が無い"),
        StoreError::NotUtf8 => err(415, "UTF-8の文章ではない"),
        StoreError::TooLarge => err(413, "大きすぎる"),
        StoreError::Conflict { current } => (409, json!({"detail": "conflict", "current_rev": current})),
        StoreError::Io(m) => err(500, &format!("書き込めない: {m}")),
    }
}

#[derive(Deserialize)]
struct PutDoc {
    path: String,
    content: String,
    expect_rev: String,
}

/// 世代のzipを渡す窓口(GET /backup?name=)だけは、JSONではないので、ここで分ける
fn handle(app: &App, req: &mut Request) -> (String, Out) {
    let url = req.url().to_string();
    if *req.method() == Method::Get && url.split('?').next() == Some("/backup") {
        let Some(device) = auth::authenticate(&app.tokens, header_value(req, "Authorization").as_deref()) else {
            return ("-".into(), Out::Json(err(401, "unauthorized")));
        };
        let out = match query_param(&url, "name") {
            Some(n) if backup::is_generation_name(&n) => match std::fs::read(app.backups.join(&n)) {
                Ok(b) => Out::Zip(b),
                Err(_) => Out::Json(err(404, "その世代は無い")),
            },
            _ => Out::Json(err(400, "name が正しくない")),
        };
        return (device, out);
    }
    let (device, reply) = handle_json(app, req);
    (device, Out::Json(reply))
}

/// 認証して、窓口を呼ぶ。返り値の先頭は、監査に残す端末名
fn handle_json(app: &App, req: &mut Request) -> (String, Reply) {
    let method = req.method().clone();
    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    if method == Method::Get && path == "/health" {
        return ("-".into(), (200, json!({"status": "ok"})));
    }
    let Some(device) = auth::authenticate(&app.tokens, header_value(req, "Authorization").as_deref()) else {
        return ("-".into(), err(401, "unauthorized"));
    };

    let mut body = String::new();
    if method == Method::Put || method == Method::Post {
        if req.as_reader().take(MAX_BODY + 1).read_to_string(&mut body).is_err() {
            return (device, err(400, "本文を読めない"));
        }
        if body.len() as u64 > MAX_BODY {
            return (device, err(413, "大きすぎる"));
        }
    }

    let reply = match (&method, path.as_str()) {
        (Method::Get, "/status") => (
            200,
            json!({
                "status": "ok",
                "embed": app.health.embed.load(std::sync::atomic::Ordering::Relaxed),
                "rag": app.health.rag.load(std::sync::atomic::Ordering::Relaxed),
                "children_managed": !app.no_children,
            }),
        ),
        (Method::Get, "/doc") => match query_param(&url, "path") {
            None => err(400, "path が必要"),
            Some(p) => match app.store.read(&p) {
                Ok((content, rev)) => (200, json!({"path": p, "content": content, "rev": rev})),
                Err(e) => store_err(e),
            },
        },
        (Method::Put, "/doc") => match serde_json::from_str::<PutDoc>(&body) {
            Err(e) => err(422, &format!("本文の形が正しくない: {e}")),
            Ok(d) => match app.store.write(&d.path, &d.content, &d.expect_rev) {
                Ok(rev) => (200, json!({"rev": rev})),
                Err(e) => store_err(e),
            },
        },
        (Method::Post, "/search_library") => search_library(app, &body),
        (Method::Post, "/backup") => {
            let lib = app.store.library.clone();
            match backup::create(&[("library", &lib), ("tasks", &app.tasks_dir)], &app.backups) {
                Ok(g) => (200, json!({"name": g.name, "size": g.size, "sha256": g.sha256, "files": g.files})),
                Err(e) => err(500, &format!("世代を作れない: {e}")),
            }
        }
        (Method::Get, "/backups") => {
            let v: Vec<Value> = backup::list(&app.backups)
                .into_iter()
                .map(|(name, size, sha256)| json!({"name": name, "size": size, "sha256": sha256}))
                .collect();
            (200, json!(v))
        }
        _ => err(404, "Not Found"),
    };
    (device, reply)
}

fn search_library(app: &App, body: &str) -> Reply {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return err(422, "本文がJSONではない") };
    if v.get("query").and_then(Value::as_str).is_none() {
        return err(422, "query が必要");
    }
    let url = format!("http://127.0.0.1:{}/search_library", app.rag_port);
    match ureq::post(&url).timeout(Duration::from_secs(120)).send_json(&v) {
        Ok(r) => match r.into_json::<Value>() {
            Ok(mut items) => {
                // サーバーの絶対パスは端末へ渡さない。libraryからの相対パス(read_document にそのまま渡せる形)に直す。
                // RAGの source はファイル名だけなので、path が相対パスの役目をする。直せないものは空にする
                let base = app.store.library.to_string_lossy().replace('\\', "/");
                let base = format!("{}/", base.trim_end_matches('/'));
                if let Some(arr) = items.as_array_mut() {
                    for it in arr {
                        if let Some(o) = it.as_object_mut() {
                            for key in ["path", "asset_path"] {
                                let rel = o
                                    .get(key)
                                    .and_then(Value::as_str)
                                    .map(|s| s.replace('\\', "/"))
                                    .and_then(|s| s.strip_prefix(&base).map(String::from))
                                    .unwrap_or_default();
                                o.insert(key.into(), Value::String(rel));
                            }
                        }
                    }
                }
                (200, items)
            }
            Err(e) => err(502, &format!("検索の応答を読めない: {e}")),
        },
        Err(ureq::Error::Status(c, _)) => err(502, &format!("検索が{c}を返した")),
        Err(e) => err(503, &format!("検索につながらない: {e}")),
    }
}

/// shiori-proto-server backup-verify <zip> --dest <空のDIR> [--against-library <DIR>] [--against-tasks <DIR>]
/// zipを照らし、行き先へ復元し、manifestと一致するか調べる。元のフォルダが渡されれば、それとも照らす。
fn backup_verify(pos: &[String], o: &HashMap<String, String>) -> Result<(), String> {
    let zip = PathBuf::from(pos.first().ok_or("zipのパスが必要")?);
    let dest = PathBuf::from(need(o, "dest")?);
    let n = backup::verify_zip(&zip)?;
    println!("zipの中身はmanifestと一致した({n} ファイル)");
    let n = backup::restore(&zip, &dest)?;
    println!("復元した中身はmanifestと一致した({n} ファイル): {}", dest.display());
    for (key, sub) in [("against-library", "library"), ("against-tasks", "tasks")] {
        if let Some(orig) = o.get(key) {
            let diff = backup::diff_dirs(&dest.join(sub), &PathBuf::from(orig))?;
            if diff.is_empty() {
                println!("{sub}: 元のフォルダと一致した");
            } else {
                return Err(format!("{sub}: 元のフォルダと違う: {}", diff.join(" / ")));
            }
        }
    }
    Ok(())
}
