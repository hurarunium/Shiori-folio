//! 窓口の結合テスト。サーバーを 127.0.0.1 の空きポートで立て、HTTPで呼ぶ(RAGは、テスト内の偽物)。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, FixedOffset};
use serde_json::{json, Value};
use shiori_server::{auth, Config, Running};

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    srv: Running,
    base: String,
    win: String,
    mac: String,
    now: Arc<Mutex<DateTime<FixedOffset>>>,
}

fn at(s: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(s).unwrap()
}

impl Env {
    fn new() -> Env {
        Env::with(|_| {})
    }

    fn with(tweak: impl FnOnce(&mut Config)) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let mut cfg = Config {
            bind: "127.0.0.1:0".into(),
            library: root.join("library"),
            knowledge: root.join("knowledge"),
            tasks: root.join("tasks"),
            data: root.join("data"),
            runtime: None,
            embed_port: 1,
            rag_port: 1, // 何も待っていないポート(RAGなし)
            no_children: true,
            layout: "v7".into(),
        };
        tweak(&mut cfg);
        std::fs::create_dir_all(cfg.library.join("_system")).unwrap();
        std::fs::write(cfg.library.join("_system/projects.yaml"), "# 台帳\nprojects:\n  - id: shiori\n    status: confirmed\n  - id: common\n    status: confirmed\n").unwrap();
        std::fs::write(cfg.library.join("_system/tags.yaml"), "tags:\n  - canonical: 詩織\n    aliases: [shiori]\n    status: confirmed\n").unwrap();
        std::fs::create_dir_all(&cfg.data).unwrap();
        let win = auth::add(&cfg.data.join("tokens.json"), "win").unwrap();
        let mac = auth::add(&cfg.data.join("tokens.json"), "mac").unwrap();
        let now = Arc::new(Mutex::new(at("2026-11-22T14:30:00+09:00")));
        let clock_now = now.clone();
        let srv = shiori_server::start_with_clock(cfg, Arc::new(move || *clock_now.lock().unwrap())).unwrap();
        let base = format!("http://{}", srv.addr);
        Env { _dir: dir, root, srv, base, win, mac, now }
    }

    fn lib(&self) -> PathBuf {
        self.root.join("library")
    }

    fn call(&self, method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> (u16, Value) {
        let mut req = ureq::request(method, &format!("{}{path}", self.base));
        if let Some(t) = token {
            req = req.set("Authorization", &format!("Bearer {t}"));
        }
        let res = match body {
            Some(b) => req.send_json(b),
            None => req.call(),
        };
        match res {
            Ok(r) => (r.status(), r.into_json().unwrap_or(Value::Null)),
            Err(ureq::Error::Status(c, r)) => (c, r.into_json().unwrap_or(Value::Null)),
            Err(e) => panic!("通信に失敗: {e}"),
        }
    }

    fn win(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.call(method, path, Some(&self.win.clone()), body)
    }

    fn save(&self, file_name: &str, content: &str) -> (u16, Value) {
        self.win("POST", "/api/v1/save", Some(json!({"file_name": file_name, "content": content})))
    }
}

const ARTICLE: &str = "---\ntitle: 資料\ntype: project\nkind: resource\nproject: shiori\ntags: [shiori, 新タグ]\n---\n本文\n";

#[test]
fn healthは認証なしで他は認証が要る() {
    let e = Env::new();
    assert_eq!(e.call("GET", "/health", None, None), (200, json!({"status": "ok", "api": "v1"})));
    assert_eq!(e.call("GET", "/api/v1/status", None, None).0, 401);
    assert_eq!(e.call("GET", "/api/v1/status", Some("wrong-token"), None).0, 401);
    let (code, v) = e.win("GET", "/api/v1/status", None);
    assert_eq!((code, v["layout"].as_str(), v["api"].as_str()), (200, Some("v7"), Some("v1")));
}

#[test]
fn 失効したトークンは即座に使えなくなる() {
    let e = Env::new();
    assert_eq!(e.win("GET", "/api/v1/status", None).0, 200);
    auth::revoke(&e.srv.app.tokens, "win").unwrap();
    assert_eq!(e.win("GET", "/api/v1/status", None).0, 401);
    assert_eq!(e.call("GET", "/api/v1/status", Some(&e.mac.clone()), None).0, 200, "ほかの端末は使える");
}

#[test]
fn 全部のアドレスでは待ち受けない() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        bind: "0.0.0.0:0".into(),
        library: dir.path().join("l"),
        knowledge: dir.path().join("k"),
        tasks: dir.path().join("t"),
        data: dir.path().join("d"),
        runtime: None,
        embed_port: 1,
        rag_port: 1,
        no_children: true,
        layout: "v7".into(),
    };
    assert!(shiori_server::start(cfg).is_err());
}

#[test]
fn 受信箱への新規と上書きと衝突と削除() {
    let e = Env::new();
    let p = "library/00-inbox/a.md";
    let (code, v) = e.win("PUT", "/api/v1/doc", Some(json!({"path": p, "content": "1"})));
    assert_eq!(code, 200, "{v}");
    let rev1 = v["rev"].as_str().unwrap().to_string();
    // 新規のつもりで、同じパスは断る
    assert_eq!(e.win("PUT", "/api/v1/doc", Some(json!({"path": p, "content": "x"}))).0, 409);
    // 読むと、本文と版
    let (code, v) = e.win("GET", &format!("/api/v1/doc?path={p}"), None);
    assert_eq!((code, v["content"].as_str(), v["rev"].as_str()), (200, Some("1"), Some(rev1.as_str())));
    // 上書き(読んだ版)
    let (code, v) = e.win("PUT", "/api/v1/doc", Some(json!({"path": p, "content": "2", "expect_rev": rev1})));
    assert_eq!(code, 200);
    let rev2 = v["rev"].as_str().unwrap().to_string();
    // 古い版では409で、今の版が返り、中身は変わらない
    let (code, v) = e.win("PUT", "/api/v1/doc", Some(json!({"path": p, "content": "3", "expect_rev": rev1})));
    assert_eq!((code, v["current_rev"].as_str()), (409, Some(rev2.as_str())));
    assert_eq!(std::fs::read_to_string(e.lib().join("00-inbox/a.md")).unwrap(), "2");
    // 版が違えば消せない。合えば消せる
    assert_eq!(e.win("DELETE", &format!("/api/v1/doc?path={p}&expect_rev=0000"), None).0, 409);
    assert_eq!(e.win("DELETE", &format!("/api/v1/doc?path={p}&expect_rev={rev2}"), None).0, 200);
    assert_eq!(e.win("GET", &format!("/api/v1/doc?path={p}"), None).0, 404);
}

#[test]
fn 受信箱の外へは直接書けずパスの不正は断る() {
    let e = Env::new();
    let put = |p: &str| e.win("PUT", "/api/v1/doc", Some(json!({"path": p, "content": "x"}))).0;
    assert_eq!(put("library/20-projects/shiori/memory.md"), 403);
    assert_eq!(put("knowledge/a.md"), 403);
    assert_eq!(put("tasks/a.md"), 403);
    assert_eq!(put("etc/passwd"), 400);
    assert_eq!(put("library/00-inbox/../_system/tags.yaml"), 422, "..は断る");
    assert_eq!(put("library/00-inbox/._x.md"), 422);
    assert_eq!(e.win("GET", "/api/v1/doc?path=library/..%2F..%2Fsecret", None).0, 422);
    assert_eq!(e.win("DELETE", "/api/v1/doc?path=library/_system/tags.yaml", None).0, 403);
    assert!(e.lib().join("_system/tags.yaml").exists());
}

#[test]
fn 記事の保存はv7の置き場へ入り相対パスで返る() {
    let e = Env::new();
    let (code, v) = e.save("a.md", ARTICLE);
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["destination"], "library/20-projects/shiori/resource/a.md");
    assert_eq!(v["newly_pending_tags"], json!(["新タグ"]));
    assert_eq!(v["indexed"], false, "RAGが無いので索引は更新されない");
    let text = std::fs::read_to_string(e.lib().join("20-projects/shiori/resource/a.md")).unwrap();
    assert!(text.contains("- 詩織") && text.contains("date: 2026-11-22T14:30:00+09:00"), "{text}");
    // 同名は連番
    assert_eq!(e.save("a.md", ARTICLE).1["destination"], "library/20-projects/shiori/resource/a-2.md");
    // 不備の記事は、理由つきでinboxへ
    let (_, v) = e.save("b.md", "---\ntitle: x\ntype: resource\ntags: []\n---\nx\n");
    assert_eq!((v["destination"].as_str(), v["inbox_reason"].as_str()), (Some("library/00-inbox/b.md"), Some("tagsが空です")));
}

#[test]
fn type_areaと日誌の通常の保存は断る() {
    let e = Env::new();
    let (code, v) = e.save("a.md", "---\ntitle: x\ntype: area\nkind: resource\nproject: shiori\ntags: [a]\n---\nx\n");
    assert_eq!(code, 422);
    assert!(v["detail"].as_str().unwrap().contains("area"), "{v}");
    let (code, v) = e.save("j.md", "---\ntitle: x\ntype: project\nkind: journal\nproject: shiori\ntags: [a]\n---\nx\n");
    assert_eq!(code, 422);
    assert!(v["detail"].as_str().unwrap().contains("--journal"), "{v}");
    assert_eq!(e.save("c.md", "frontmatterの無い文").0, 422);
    assert!(!e.lib().join("20-projects").exists() && !e.lib().join("00-inbox").exists());
}

#[test]
fn memoryの保存は版の確認つきで衝突を断る() {
    let e = Env::new();
    let mem = |body: &str| format!("---\ntitle: shiori memory\ntype: project\nkind: memory\nproject: shiori\ntags: [詩織]\n---\n{body}\n");
    let (code, v) = e.save("memory.md", &mem("1"));
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["destination"], "library/20-projects/shiori/memory.md");
    let u1 = v["updated"].as_str().unwrap().to_string();
    // 既存の更新に、expectなし
    assert_eq!(e.save("memory.md", &mem("2")).0, 422);
    // 古い(でたらめな)updatedは409
    let (code, v) = e.win("POST", "/api/v1/save", Some(json!({"file_name": "memory.md", "content": mem("2"), "expect_updated": "でたらめ"})));
    assert_eq!((code, v["stale"].clone()), (409, json!(true)));
    // 合っていれば通り、旧版が退避される
    *e.now.lock().unwrap() = at("2026-11-22T15:00:00+09:00");
    let (code, v) = e.win("POST", "/api/v1/save", Some(json!({"file_name": "memory.md", "content": mem("3"), "expect_updated": u1})));
    assert_eq!(code, 200, "{v}");
    assert!(v["history_file"].as_str().unwrap().starts_with("library/_system/memory-history/shiori-memory-2026-11-22T14-30-00"), "{v}");
    // 全体用は10-commonへ
    let g = "---\ntitle: 全体\ntype: profile\nkind: memory\ntags: [詩織]\n---\n全体\n";
    assert_eq!(e.save("memory-global.md", g).1["destination"], "library/10-common/memory-global.md");
}

#[test]
fn 知識の棚への保存は棚へ入りlibraryには置かない() {
    let e = Env::new();
    let sh = |status: &str| format!("---\ntitle: 数学\ntype: knowledge\ntags: [数学]\ntopic: 数学\nlearned: 2026-11-01\nsource: 教科書\nstatus: {status}\n---\n本文\n");
    let (code, v) = e.save("math.md", &sh("new"));
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["destination"], "knowledge/math.md");
    assert!(e.root.join("knowledge/math.md").is_file() && !e.lib().join("00-inbox").exists());
    // 上書きは、旧版を退避する
    let (_, v) = e.save("math.md", &sh("new"));
    assert_eq!(v["history_file"], "knowledge/_system/history/math-2026-11-01.md");
    // 下書きは専用の場所
    assert_eq!(e.save("d.md", &sh("draft")).1["destination"], "knowledge/_system/drafts/d.md");
}

#[test]
fn 素材は実体を先に送りsha256が合うときだけ取り込む() {
    let e = Env::new();
    let bytes = b"PK-asset-bytes".to_vec();
    let sha = library_store::fsx::rev_of(&bytes);
    let up = |name: &str, sha: &str, body: Vec<u8>| -> (u16, Value) {
        let res = ureq::put(&format!("{}/api/v1/upload?name={name}&sha256={sha}", e.base))
            .set("Authorization", &format!("Bearer {}", e.win))
            .send_bytes(&body);
        match res {
            Ok(r) => (r.status(), r.into_json().unwrap()),
            Err(ureq::Error::Status(c, r)) => (c, r.into_json().unwrap_or(Value::Null)),
            Err(e) => panic!("{e}"),
        }
    };
    // 中身と合わないsha256は断る(置かれない)
    assert_eq!(up("a.zip", &"0".repeat(64), bytes.clone()).0, 422);
    assert_eq!(up("a.zip", "短い", bytes.clone()).0, 422);
    assert_eq!(up("../a.zip", &sha, bytes.clone()).0, 422);
    assert!(!e.root.join("data/uploads").join(&sha).exists());
    // 実体なしに.metaだけを送ると断る
    let meta = "---\ntitle: 素材\ntype: resource\ntags: [詩織]\n---\n説明\n";
    assert_eq!(e.save("a.zip.meta", meta).0, 422);
    assert_eq!(e.win("POST", "/api/v1/save", Some(json!({"file_name": "a.zip.meta", "content": meta, "asset_upload": sha}))).0, 404);
    // 合う実体を送って、取り込む
    let (code, v) = up("a.zip", &sha, bytes.clone());
    assert_eq!((code, v["upload_id"].as_str()), (200, Some(sha.as_str())));
    let (code, v) = e.win("POST", "/api/v1/save", Some(json!({"file_name": "a.zip.meta", "content": meta, "asset_upload": sha})));
    assert_eq!(code, 200, "{v}");
    assert_eq!((v["destination"].as_str(), v["asset_destination"].as_str()), (Some("library/30-resources/files/a.zip.meta"), Some("library/30-resources/files/a.zip")));
    assert_eq!(std::fs::read(e.lib().join("30-resources/files/a.zip")).unwrap(), bytes);
    let m = std::fs::read_to_string(e.lib().join("30-resources/files/a.zip.meta")).unwrap();
    assert!(m.contains(&format!("sha256: {sha}")) && m.contains("size: 14"), "{m}");
    assert!(!e.root.join("data/uploads").join(&sha).exists(), "取り込んだあとは、受け取りの置き場から消える");
    // 秘密情報の名前は断る
    let k = b"secret".to_vec();
    let ksha = library_store::fsx::rev_of(&k);
    assert_eq!(up("server.pem", &ksha, k).0, 200);
    assert_eq!(e.win("POST", "/api/v1/save", Some(json!({"file_name": "server.pem.meta", "content": meta, "asset_upload": ksha}))).0, 422);
}

#[test]
fn 日誌の追記は午前5時で日が変わり目次と端末名が付く() {
    let e = Env::new();
    let append = |token: &str, at_time: &str, body: Value| -> (u16, Value) {
        *e.now.lock().unwrap() = at(at_time);
        e.call("POST", "/api/v1/journal/append", Some(token), Some(body))
    };
    let (code, v) = append(&e.win.clone(), "2026-11-22T04:59:00+09:00", json!({"project": "shiori", "content": "夜更かし", "ai": "claude-code", "summary": "深夜の作業"}));
    assert_eq!(code, 200, "{v}");
    assert_eq!((v["path"].as_str(), v["day"].as_str(), v["heading"].as_str()), (Some("library/20-projects/shiori/journal/2026-11-21.md"), Some("2026-11-21"), Some("11/22 04:59 win・claude-code")));
    let (_, v) = append(&e.mac.clone(), "2026-11-22T05:00:00+09:00", json!({"project": "shiori", "content": "朝", "ai": "codex"}));
    assert_eq!((v["day"].as_str(), v["heading"].as_str()), (Some("2026-11-22"), Some("05:00 mac・codex")));
    let toc = std::fs::read_to_string(e.lib().join("10-common/journal/2026-11-21.md")).unwrap();
    assert!(toc.contains("## 目次\n\n- 04:59 [shiori] 深夜の作業\n"), "{toc}");
    // 不正は断る
    assert_eq!(append(&e.win.clone(), "2026-11-22T10:00:00+09:00", json!({"project": "../x", "content": "x"})).0, 422);
    assert_eq!(append(&e.win.clone(), "2026-11-22T10:00:00+09:00", json!({"project": "shiori", "content": " "})).0, 422);
}

#[test]
fn 日誌への同時の追記でも全部の節が残る() {
    let e = Env::new();
    let base = e.base.clone();
    let tokens = [e.win.clone(), e.mac.clone()];
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let (base, token) = (base.clone(), tokens[i % 2].clone());
            std::thread::spawn(move || {
                ureq::post(&format!("{base}/api/v1/journal/append"))
                    .set("Authorization", &format!("Bearer {token}"))
                    .send_json(json!({"project": "shiori", "content": format!("節{i}"), "summary": format!("要約{i}")}))
                    .map(|r| r.status())
            })
        })
        .collect();
    for h in handles {
        assert_eq!(h.join().unwrap().unwrap(), 200);
    }
    let text = std::fs::read_to_string(e.lib().join("20-projects/shiori/journal/2026-11-22.md")).unwrap();
    for i in 0..8 {
        assert_eq!(text.matches(&format!("\n節{i}\n")).count(), 1, "節{i}");
    }
    let toc = std::fs::read_to_string(e.lib().join("10-common/journal/2026-11-22.md")).unwrap();
    for i in 0..8 {
        assert_eq!(toc.matches(&format!("[shiori] 要約{i}")).count(), 1, "目次{i}");
    }
}

/// 偽のRAG。検索には絶対パスを返し、索引の更新の要求を記録する。
fn fake_rag(lib: PathBuf, shelf: PathBuf) -> (u16, Arc<Mutex<Vec<String>>>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let url = req.url().to_string();
            let mut body = String::new();
            let _ = req.as_reader().read_to_string(&mut body);
            let out = match url.as_str() {
                "/health" => json!({"status": "ok"}),
                "/search_library" => json!([{"path": lib.join("20-projects/shiori/memory.md").to_string_lossy(), "asset_path": "", "text": "本文"}]),
                "/search_shelf" => json!([{"path": shelf.join("math.md").to_string_lossy(), "text": "棚"}]),
                _ => {
                    log.lock().unwrap().push(format!("{url} {body}"));
                    json!({})
                }
            };
            let _ = req.respond(tiny_http::Response::from_string(out.to_string()));
        }
    });
    (port, seen)
}

#[test]
fn 検索の中継は絶対パスを相対パスに直す() {
    let dir = tempfile::tempdir().unwrap();
    let (port, seen) = fake_rag(dir.path().join("library"), dir.path().join("knowledge"));
    let _ = seen;
    let e = Env::with(|c| {
        c.library = dir.path().join("library");
        c.knowledge = dir.path().join("knowledge");
        c.tasks = dir.path().join("tasks");
        c.data = dir.path().join("data");
        c.rag_port = port;
    });
    let (code, v) = e.win("POST", "/api/v1/search/library", Some(json!({"query": "詩織"})));
    assert_eq!((code, v[0]["path"].as_str(), v[0]["text"].as_str()), (200, Some("library/20-projects/shiori/memory.md"), Some("本文")));
    let (_, v) = e.win("POST", "/api/v1/search/shelf", Some(json!({"query": "数学"})));
    assert_eq!(v[0]["path"], "knowledge/math.md");
    assert_eq!(e.win("POST", "/api/v1/search/library", Some(json!({"x": 1}))).0, 422);
}

#[test]
fn 保存のあとに索引の更新がragへ送られる() {
    let dir = tempfile::tempdir().unwrap();
    let (port, seen) = fake_rag(dir.path().join("library"), dir.path().join("knowledge"));
    let e = Env::with(|c| {
        c.library = dir.path().join("library");
        c.knowledge = dir.path().join("knowledge");
        c.tasks = dir.path().join("tasks");
        c.data = dir.path().join("data");
        c.rag_port = port;
    });
    let (_, v) = e.save("a.md", ARTICLE);
    assert_eq!(v["indexed"], true);
    let (_, v) = e.win("POST", "/api/v1/journal/append", Some(json!({"project": "shiori", "content": "x", "summary": "y"})));
    assert_eq!(v["indexed"], true);
    let log = seen.lock().unwrap().clone();
    assert!(log.iter().any(|l| l.starts_with("/reindex_file") && l.contains("20-projects/shiori/resource/a.md")), "{log:?}");
    assert!(log.iter().any(|l| l.contains("20-projects/shiori/journal/2026-11-22.md")), "{log:?}");
    assert!(log.iter().any(|l| l.contains("10-common/journal/2026-11-22.md")), "{log:?}");
}

#[test]
fn 検索はragが無ければ503で返る() {
    let e = Env::new();
    assert_eq!(e.win("POST", "/api/v1/search/library", Some(json!({"query": "x"}))).0, 503);
}

#[test]
fn 操作は月ごとの監査ログに残り本文は残らない() {
    let e = Env::new();
    e.save("a.md", ARTICLE);
    e.win("GET", "/api/v1/status", None);
    let dir = e.root.join("data/audit");
    let file = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    let text = std::fs::read_to_string(file).unwrap();
    assert!(text.contains("/api/v1/save") && text.contains("\"device\":\"win\""), "{text}");
    assert!(!text.contains("本文"), "本文は残さない");
}

#[test]
fn 世代のバックアップを作り一覧と取得ができる() {
    let e = Env::new();
    e.save("a.md", ARTICLE);
    let (code, v) = e.win("POST", "/api/v1/backup", None);
    assert_eq!(code, 200, "{v}");
    let name = v["name"].as_str().unwrap().to_string();
    let (_, list) = e.win("GET", "/api/v1/backups", None);
    assert_eq!(list[0]["name"], name.as_str());
    let res = ureq::get(&format!("{}/api/v1/backup?name={name}", e.base)).set("Authorization", &format!("Bearer {}", e.win)).call().unwrap();
    let mut bytes = Vec::new();
    res.into_reader().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len() as u64, v["size"].as_u64().unwrap());
    assert_eq!(e.win("GET", "/api/v1/backup?name=../x", None).0, 400);
}

#[test]
fn タスクの追加から完了と削除までの一巡() {
    let e = Env::new();
    // フォルダ
    let (code, v) = e.win("POST", "/api/v1/task-folders", Some(json!({"folder": "詩織"})));
    assert_eq!((code, v["folders"].clone()), (200, json!(["詩織"])));
    // 追加
    let (code, t) = e.win("POST", "/api/v1/tasks", Some(json!({"summary": "設計を読む", "folder": "詩織", "due": "2026-12-15", "detail": "本文"})));
    assert_eq!(code, 200, "{t}");
    let id = t["id"].as_str().unwrap().to_string();
    assert!(e.root.join("tasks/INDEX.md").is_file(), "一覧(INDEX.md)が更新される");
    // 一覧
    let (_, v) = e.win("GET", "/api/v1/tasks", None);
    assert_eq!((v["tasks"].as_array().unwrap().len(), v["tasks"][0]["id"].as_str()), (1, Some(id.as_str())));
    // 編集(概要と期日の取り消し)・移動
    let (code, t) = e.win("PUT", &format!("/api/v1/tasks/{id}"), Some(json!({"summary": "設計を読み直す", "due": "none"})));
    assert_eq!((code, t["summary"].as_str()), (200, Some("設計を読み直す")));
    let (code, _) = e.win("PUT", &format!("/api/v1/tasks/{id}"), Some(json!({"folder": ""})));
    assert_eq!(code, 200);
    assert_eq!(e.win("PUT", &format!("/api/v1/tasks/{id}"), Some(json!({}))).0, 422, "変更する項目が無い");
    // 完了 → アーカイブに入り、戻せる
    assert_eq!(e.win("POST", &format!("/api/v1/tasks/{id}/done"), None).0, 200);
    assert_eq!(e.win("GET", "/api/v1/tasks", None).1["tasks"].as_array().unwrap().len(), 0);
    assert_eq!(e.win("GET", "/api/v1/tasks?archived=1", None).1["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(e.win("POST", &format!("/api/v1/tasks/{id}/restore"), None).0, 200);
    // 削除
    assert_eq!(e.win("DELETE", &format!("/api/v1/tasks/{id}"), None).1["deleted"], id.as_str());
    assert_eq!(e.win("DELETE", &format!("/api/v1/tasks/{id}"), None).0, 404);
    // フォルダの名前変更と削除
    assert_eq!(e.win("PUT", "/api/v1/task-folders", Some(json!({"from": "詩織", "to": "詩織2"}))).1["folders"], json!(["詩織2"]));
    assert_eq!(e.win("DELETE", "/api/v1/task-folders?folder=%E8%A9%A9%E7%B9%942", None).1["folders"], json!([]));
}

#[test]
fn タスクの不正な入力は断る() {
    let e = Env::new();
    assert_eq!(e.win("POST", "/api/v1/tasks", Some(json!({"summary": ""}))).0, 422);
    assert_eq!(e.win("POST", "/api/v1/tasks", Some(json!({"summary": "x", "due": "来週"}))).0, 422);
    assert_eq!(e.win("POST", "/api/v1/tasks", Some(json!({"x": 1}))).0, 422);
    assert_eq!(e.win("POST", "/api/v1/tasks/無い/done", None).0, 404);
    assert_eq!(e.call("GET", "/api/v1/tasks", None, None).0, 401);
}

#[test]
fn 日誌の追記の結果に書き方の規則の警告が付く() {
    let e = Env::new();
    let (code, v) = e.win("POST", "/api/v1/journal/append", Some(json!({"project": "shiori", "content": "### 次にやること\n\n- 設計を読む\n"})));
    assert_eq!(code, 200, "保存は止まらない: {v}");
    let ids: Vec<&str> = v["warnings"].as_array().unwrap().iter().map(|w| w["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"J1") && ids.contains(&"J2"), "{v}");
    let w = &v["warnings"][0];
    assert!(w["severity"] == "normal" || w["severity"] == "weak", "{w}");
    assert!(w["message"].as_str().unwrap().len() > 5);
    assert!(std::fs::read_to_string(e.lib().join("20-projects/shiori/journal/2026-11-22.md")).unwrap().contains("設計を読む"));
}

#[test]
fn memoryを整理して外した項目は日誌へ自動で記録され長さの警告は出ない() {
    let e = Env::new();
    let mem = |body: &str| format!("---\ntitle: shiori memory\ntype: project\nkind: memory\nproject: shiori\ntags: [詩織]\n---\n## やること\n\n{body}");
    let (_, v) = e.save("memory.md", &mem("- Phase 1 を進める(11/22)\n- 4-6の2回目を集計し、合否を決める(11/22)\n- 外のネットワークから、4-1・4-8を試す。(11/20)\n"));
    let updated = v["updated"].as_str().unwrap().to_string();
    assert_eq!(v["removed_items"], 0);

    // 「4-6…」を外し、長い本文にする
    *e.now.lock().unwrap() = at("2026-11-22T15:00:00+09:00");
    let big: String = (0..400).map(|i| format!("- 項目{i}: `file{i}.rs` の状態を記録した(11/22)\n")).collect();
    let body = mem(&format!("- Phase 1 を進める(11/23)\n- 外のネットワークから、4-1・4-8を試す。(11/20)\n{big}"));
    let (code, v) = e.win("POST", "/api/v1/save", Some(json!({"file_name": "memory.md", "content": body, "expect_updated": updated})));
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["removed_items"], 1);
    assert!(v["size"]["chars"].as_u64().unwrap() > 6000, "大きさは情報として返る: {v}");
    assert!(v["warnings"].as_array().unwrap().is_empty(), "長くても警告しない: {}", v["warnings"]);
    let rec = &v["removed_recorded_to"];
    assert_eq!(rec["path"], "library/20-projects/shiori/journal/2026-11-22.md", "{v}");
    assert_eq!(rec["heading"], "15:00 win・memory整理");
    let journal = std::fs::read_to_string(e.lib().join("20-projects/shiori/journal/2026-11-22.md")).unwrap();
    assert!(journal.contains("### memoryから外した項目\n\n- 4-6の2回目を集計し、合否を決める(11/22)\n"), "{journal}");

    // 全体用のmemoryから外した項目は、全体の日誌へ
    let g = |b: &str| format!("---\ntitle: 全体\ntype: profile\nkind: memory\ntags: [詩織]\n---\n## 約束事\n\n{b}");
    let (_, v) = e.save("memory-global.md", &g("- 古い決まり(11/01)\n- 残す決まり(11/02)\n"));
    let u = v["updated"].as_str().unwrap().to_string();
    *e.now.lock().unwrap() = at("2026-11-22T16:00:00+09:00");
    let (_, v) = e.win("POST", "/api/v1/save", Some(json!({"file_name": "memory-global.md", "content": g("- 残す決まり(11/02)\n"), "expect_updated": u})));
    assert_eq!(v["removed_recorded_to"]["path"], "library/10-common/journal/2026-11-22.md", "{v}");
}

#[test]
fn フォルダの一覧は新しい日付の順で返り件数を絞れる() {
    let e = Env::new();
    for (time, body) in [("2026-11-20T10:00:00+09:00", "a"), ("2026-11-21T10:00:00+09:00", "b"), ("2026-11-22T10:00:00+09:00", "c")] {
        *e.now.lock().unwrap() = at(time);
        assert_eq!(e.win("POST", "/api/v1/journal/append", Some(json!({"project": "shiori", "content": format!("`{body}.rs` を直した。試験を足して、112件が通った。確認の結果は良好。")}))).0, 200);
    }
    let (code, v) = e.win("GET", "/api/v1/list?path=library/20-projects/shiori/journal", None);
    let names: Vec<&str> = v["entries"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    assert_eq!((code, names), (200, vec!["2026-11-22.md", "2026-11-21.md", "2026-11-20.md"]));
    assert_eq!(v["entries"][0]["path"], "library/20-projects/shiori/journal/2026-11-22.md", "read_documentにそのまま渡せる");
    let (_, v) = e.win("GET", "/api/v1/list?path=library/20-projects/shiori/journal&limit=1", None);
    assert_eq!(v["entries"].as_array().unwrap().len(), 1);
    // フォルダが無ければ空(エラーにしない)。不正なパスは断る。認証が要る
    assert_eq!(e.win("GET", "/api/v1/list?path=library/20-projects/無い/journal", None).1["entries"], json!([]));
    assert_eq!(e.win("GET", "/api/v1/list?path=library/..%2Fx", None).0, 422);
    assert_eq!(e.win("GET", "/api/v1/list?path=etc", None).0, 400);
    assert_eq!(e.call("GET", "/api/v1/list?path=library/20-projects", Some("wrong-token"), None).0, 401);
}

#[allow(dead_code)]
fn _unused(_: &Path) {}
