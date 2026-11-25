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
                "/list_all_library" => json!([
                    {"source": "memory.md", "path": lib.join("20-projects/shiori/memory.md").to_string_lossy(), "asset_path": "", "title": "詩織"},
                    {"source": "gone.md", "path": lib.join("20-projects/shiori/gone.md").to_string_lossy(), "asset_path": "", "title": "消えた"},
                    {"source": "x.md", "path": "D:/outside/x.md", "asset_path": "", "title": "外"}
                ]),
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
fn 蔵書の一覧と検索結果に属性と冒頭の文が添えられる() {
    let dir = tempfile::tempdir().unwrap();
    let lib = dir.path().join("library");
    std::fs::create_dir_all(lib.join("20-projects/shiori")).unwrap();
    std::fs::write(
        lib.join("20-projects/shiori/memory.md"),
        "---\ntitle: 詩織\nproject: shiori\nkind: memory\nsummary: 詩織の今の状態\n---\n本文",
    )
    .unwrap();
    let (port, _) = fake_rag(lib.clone(), dir.path().join("knowledge"));
    let e = Env::with(|c| {
        c.library = lib.clone();
        c.knowledge = dir.path().join("knowledge");
        c.tasks = dir.path().join("tasks");
        c.data = dir.path().join("data");
        c.rag_port = port;
    });
    let (code, v) = e.win("GET", "/api/v1/library-index", None);
    assert_eq!(code, 200);
    assert_eq!(v[0]["path"], "library/20-projects/shiori/memory.md");
    assert_eq!(v[0]["preview"]["summary"], "詩織の今の状態");
    assert_eq!(v[0]["preview"]["project"], "shiori");
    assert_eq!(v[0]["preview"]["entry_kind"], "memory");
    assert!(v[1].get("preview").is_none(), "読めない記事には添えない(一覧は止めない)");
    assert_eq!(v[2]["path"], "", "置き場の外の絶対パスは外へ出さない");
    // 検索結果にも添える
    let (_, v) = e.win("POST", "/api/v1/search/library", Some(json!({"query": "詩織"})));
    assert_eq!(v[0]["preview"]["summary"], "詩織の今の状態");
}

#[test]
fn 棚の一覧と本文を返し一覧に無い名前は読めない() {
    let e = Env::new();
    let shelf = e.root.join("knowledge");
    std::fs::create_dir_all(&shelf).unwrap();
    std::fs::write(shelf.join("a.md"), "---\ntitle: 超実数\ntopic: 数学\nlearned: '2026-10-17'\n---\n\n本文\n").unwrap();
    std::fs::write(e.root.join("secret.md"), "秘密").unwrap();
    let (code, v) = e.win("GET", "/api/v1/shelf", None);
    assert_eq!((code, v[0]["name"].as_str(), v[0]["title"].as_str(), v[0]["topic"].as_str()), (200, Some("a.md"), Some("超実数"), Some("数学")));
    let (code, v) = e.win("GET", "/api/v1/shelf/doc?name=a.md", None);
    assert_eq!(code, 200);
    assert!(v["content"].as_str().unwrap().contains("本文"));
    assert_eq!(e.win("GET", "/api/v1/shelf/doc?name=..%2Fsecret.md", None).0, 404);
    assert_eq!(e.win("GET", "/api/v1/shelf/doc?name=none.md", None).0, 404);
    assert_eq!(e.win("GET", "/api/v1/shelf/doc", None).0, 400);
}

#[test]
fn 参照資料の本文をカテゴリと名前から返し外は読めない() {
    let e = Env::new();
    std::fs::create_dir_all(e.lib().join("20-projects/shiori/resource")).unwrap();
    std::fs::write(e.lib().join("20-projects/shiori/resource/a.md"), "---\ntitle: 資料\n---\n本文").unwrap();
    std::fs::write(e.lib().join("20-projects/shiori/memory.md"), "---\ntitle: m\n---\nmemory本文").unwrap();
    std::fs::write(e.root.join("secret.md"), "秘密").unwrap();
    let (code, v) = e.win("GET", "/api/v1/source?category=20-projects&source=a.md", None);
    assert_eq!((code, v["path"].as_str()), (200, Some("library/20-projects/shiori/resource/a.md")));
    assert!(v["content"].as_str().unwrap().contains("本文"));
    // memory は source が「project/memory.md」
    let (code, v) = e.win("GET", "/api/v1/source?category=20-projects&source=shiori%2Fmemory.md", None);
    assert_eq!(code, 200);
    assert!(v["content"].as_str().unwrap().contains("memory本文"));
    // 外へは出られない
    assert_eq!(e.win("GET", "/api/v1/source?category=20-projects&source=..%2F..%2Fsecret.md", None).0, 404);
    assert_eq!(e.win("GET", "/api/v1/source?category=..&source=secret.md", None).0, 400);
    assert_eq!(e.win("GET", "/api/v1/source?category=20-projects&source=none.md", None).0, 404);
    assert_eq!(e.win("GET", "/api/v1/source?category=20-projects", None).0, 400);
}

#[test]
fn 要確認の一覧と解決は窓口から行え解決の対象は監査に残る() {
    let e = Env::new();
    std::fs::write(
        e.lib().join("_system/tags.yaml"),
        "# 台帳\ntags:\n  - canonical: 詩織\n    status: confirmed\n  - canonical: 新タグ\n    status: pending\n",
    )
    .unwrap();
    std::fs::create_dir_all(e.lib().join("00-inbox")).unwrap();
    std::fs::write(e.lib().join("00-inbox/a.md"), "---\ntitle: 受信A\nreason: 理由\n---\n本文").unwrap();
    let (code, v) = e.win("GET", "/api/v1/pending", None);
    assert_eq!(code, 200);
    assert_eq!(v["tags"][0]["canonical"], "新タグ");
    assert_eq!(v["inbox"][0]["filename"], "a.md");
    assert_eq!(v["inbox"][0]["reason"], "理由");
    // 承認(タグ)・保留(受信箱)
    let (code, _) = e.win("POST", "/api/v1/pending/resolve?kind=tag&key=%E6%96%B0%E3%82%BF%E3%82%B0&action=confirm", None);
    assert_eq!(code, 200);
    assert_eq!(e.win("POST", "/api/v1/pending/resolve?kind=inbox&key=a.md&action=defer", None).0, 200);
    let (_, v) = e.win("GET", "/api/v1/pending", None);
    assert_eq!((v["tags"].as_array().unwrap().len(), v["inbox"][0]["reviewStatus"].as_str()), (0, Some("deferred")));
    // 却下(受信箱は消える)
    assert_eq!(e.win("POST", "/api/v1/pending/resolve?kind=inbox&key=a.md&action=reject", None).0, 200);
    assert!(!e.lib().join("00-inbox/a.md").exists());
    // 断るもの
    assert_eq!(e.win("POST", "/api/v1/pending/resolve?kind=inbox&key=..%2F_system%2Ftags.yaml&action=reject", None).0, 422);
    assert_eq!(e.win("POST", "/api/v1/pending/resolve?kind=x&key=a&action=confirm", None).0, 422);
    assert_eq!(e.win("POST", "/api/v1/pending/resolve?kind=tag&key=a&action=bogus", None).0, 422);
    assert_eq!(e.win("POST", "/api/v1/pending/resolve?kind=tag&key=無い&action=confirm", None).0, 404);
    assert_eq!(e.win("POST", "/api/v1/pending/resolve?kind=tag", None).0, 400);
    // 何を解決したかが、監査のURLに残る
    let dir = e.root.join("data/audit");
    let file = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    assert!(std::fs::read_to_string(file).unwrap().contains("pending/resolve?kind=inbox&key=a.md&action=reject"));
}

#[test]
fn ホームの申し送りはmemoryの最新から返し記事の本文へつながる() {
    let e = Env::new();
    assert_eq!(e.win("GET", "/api/v1/handoff", None), (200, Value::Null), "何も無ければnull");
    std::fs::create_dir_all(e.lib().join("20-projects/shiori")).unwrap();
    std::fs::write(
        e.lib().join("20-projects/shiori/memory.md"),
        "---\ntitle: 詩織\nupdated: 2026-11-22T10:00:00+09:00\n---\n## やること\n\n- [ ] Phase 4 を進める\n\n## 申し送り・待ち\n\n- Macの確認待ち\n",
    )
    .unwrap();
    let (code, v) = e.win("GET", "/api/v1/handoff", None);
    assert_eq!(code, 200);
    assert_eq!((v["project"].as_str(), v["fromMemory"].as_bool(), v["date"].as_str()), (Some("shiori"), Some(true), Some("2026-11-22")));
    assert_eq!(v["todos"][0], "Phase 4 を進める");
    assert_eq!(v["notes"][0], "Macの確認待ち");
    // 画面が、この source・sourceCategory で本文を開ける
    let url = format!(
        "/api/v1/source?category={}&source={}",
        v["sourceCategory"].as_str().unwrap(),
        v["source"].as_str().unwrap().replace('/', "%2F")
    );
    assert_eq!(e.win("GET", &url, None).0, 200);
}

#[test]
fn 素材の実体はlibraryの中だけそのまま返す() {
    let e = Env::new();
    std::fs::create_dir_all(e.lib().join("20-projects/shiori/files")).unwrap();
    let bytes: Vec<u8> = vec![0, 159, 146, 150, 255, 1, 2, 3];
    std::fs::write(e.lib().join("20-projects/shiori/files/a.bin"), &bytes).unwrap();
    std::fs::write(e.root.join("secret.txt"), "秘密").unwrap();
    let get = |path: &str, token: Option<&str>| {
        let mut req = ureq::get(&format!("{}/api/v1/file?path={path}", e.base));
        if let Some(t) = token {
            req = req.set("Authorization", &format!("Bearer {t}"));
        }
        match req.call() {
            Ok(r) => {
                let mut b = Vec::new();
                r.into_reader().read_to_end(&mut b).unwrap();
                (200, b)
            }
            Err(ureq::Error::Status(c, _)) => (c, Vec::new()),
            Err(e) => panic!("{e}"),
        }
    };
    let win = e.win.clone();
    assert_eq!(get("library/20-projects/shiori/files/a.bin", Some(&win)), (200, bytes), "バイト列のまま返る");
    assert_eq!(get("library/20-projects/shiori/files/a.bin", None).0, 401, "認証が要る");
    assert_eq!(get("library/_system/projects.yaml", Some(&win)).0, 403, "規約・台帳は返さない");
    assert_eq!(get("library/..%2Fsecret.txt", Some(&win)).0, 400);
    assert_eq!(get("knowledge/a.md", Some(&win)).0, 400, "library の外は返さない");
    assert_eq!(get("library/20-projects/none.bin", Some(&win)).0, 404);
}

#[test]
fn 状態に置き場の空き容量が入る() {
    let e = Env::new();
    let (_, v) = e.win("GET", "/api/v1/status", None);
    let (free, total) = (v["storage"]["free_gb"].as_f64().unwrap(), v["storage"]["total_gb"].as_f64().unwrap());
    assert!(total > 0.0 && free >= 0.0 && free <= total);
}

#[test]
fn 規約画面の書類はv7の置き場から出し許可リスト以外は読めない() {
    let e = Env::new();
    std::fs::create_dir_all(e.lib().join("10-common")).unwrap();
    std::fs::write(e.lib().join("10-common/memory-global.md"), "---\nupdated: 2026-11-01T00:00:00+09:00\n---\n全体のmemory").unwrap();
    std::fs::create_dir_all(e.lib().join("20-projects/shiori")).unwrap();
    std::fs::write(e.lib().join("20-projects/shiori/memory.md"), "---\nupdated: 2026-11-22T00:00:00+09:00\n---\nshioriのmemory").unwrap();
    std::fs::write(e.lib().join("_system/CLAUDE.md"), "# 規約").unwrap();
    std::fs::write(e.root.join("secret.txt"), "秘密").unwrap();
    let (code, v) = e.win("GET", "/api/v1/system-docs", None);
    assert_eq!(code, 200);
    let ids: Vec<&str> = v.as_array().unwrap().iter().map(|d| d["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"memory/global") && ids.contains(&"memory/project/shiori") && ids.contains(&"library/CLAUDE.md"), "{ids:?}");
    assert!(!ids.iter().any(|i| i.starts_with("prompts/") || i.starts_with("config/")), "手元の設定は出さない");
    let (code, v) = e.win("GET", "/api/v1/system-docs/read?id=memory%2Fproject%2Fshiori", None);
    assert_eq!(code, 200);
    assert!(v["content"].as_str().unwrap().contains("shioriのmemory"));
    for bad in ["..%2Fsecret.txt", "memory%2Fproject%2F..%2F..%2F..%2Fsecret.txt", "config%2Fconfig.json", "ledger%2Ftags.yaml.bak"] {
        assert_eq!(e.win("GET", &format!("/api/v1/system-docs/read?id={bad}"), None).0, 404, "{bad}");
    }
    assert_eq!(e.win("GET", "/api/v1/system-docs/read", None).0, 400);
}

#[test]
fn 蔵書の一覧はragが無ければ503で返る() {
    assert_eq!(Env::new().win("GET", "/api/v1/library-index", None).0, 503);
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

#[test]
fn skillの手順書は読んだ版で上書きでき旧版が退避される() {
    let e = Env::new();
    let path = "library/30-resources/skills/work-start.md";
    let (code, v) = e.win("PUT", "/api/v1/doc", Some(json!({"path": path, "content": "# v1\n"})));
    assert_eq!((code, v["created"].clone()), (200, json!(true)), "{v}");
    // 同じパスへの新規は断る
    assert_eq!(e.win("PUT", "/api/v1/doc", Some(json!({"path": path, "content": "x"}))).0, 409);
    // 読んで版を得て、その版で上書きする
    let (_, got) = e.win("GET", &format!("/api/v1/doc?path={path}"), None);
    let rev = got["rev"].as_str().unwrap().to_string();
    *e.now.lock().unwrap() = at("2026-11-22T16:00:00+09:00");
    let (code, v) = e.win("PUT", "/api/v1/doc", Some(json!({"path": path, "content": "# v2\n", "expect_rev": rev})));
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["history_file"], "library/_system/doc-history/30-resources__skills__work-start.md@20261122T160000.md");
    assert_eq!(std::fs::read_to_string(e.lib().join("_system/doc-history/30-resources__skills__work-start.md@20261122T160000.md")).unwrap(), "# v1\n");
    assert_eq!(std::fs::read_to_string(e.lib().join("30-resources/skills/work-start.md")).unwrap(), "# v2\n");
    // 古い版では、409で今の版が返り、中身は変わらない
    let (code, v) = e.win("PUT", "/api/v1/doc", Some(json!({"path": path, "content": "# v3\n", "expect_rev": rev})));
    assert_eq!(code, 409);
    assert!(v["current_rev"].is_string(), "{v}");
    assert_eq!(std::fs::read_to_string(e.lib().join("30-resources/skills/work-start.md")).unwrap(), "# v2\n");
}

#[test]
fn 規約は上書きだけで新規には作れず台帳や退避には書けない() {
    let e = Env::new();
    // 新規は断る
    let (code, v) = e.win("PUT", "/api/v1/doc", Some(json!({"path": "library/_system/CLAUDE.md", "content": "x"})));
    assert_eq!(code, 422, "{v}");
    // あるものは、読んだ版で上書きできる(_systemは索引の対象外なので indexed は false)
    std::fs::write(e.lib().join("_system/CLAUDE.md"), "旧規約\n").unwrap();
    let (_, got) = e.win("GET", "/api/v1/doc?path=library/_system/CLAUDE.md", None);
    let (code, v) = e.win("PUT", "/api/v1/doc", Some(json!({"path": "library/_system/CLAUDE.md", "content": "新規約\n", "expect_rev": got["rev"]})));
    assert_eq!((code, v["indexed"].clone()), (200, json!(false)), "{v}");
    assert!(v["history_file"].as_str().unwrap().starts_with("library/_system/doc-history/_system__CLAUDE.md@"));
    // 台帳・退避・ほかの置き場には、書けない
    for p in ["library/_system/projects.yaml", "library/_system/tags.yaml", "library/_system/memory-history/a.md", "library/_system/doc-history/a.md", "library/20-projects/shiori/memory.md"] {
        assert_eq!(e.win("PUT", "/api/v1/doc", Some(json!({"path": p, "content": "x"}))).0, 403, "{p}");
    }
    assert!(std::fs::read_to_string(e.lib().join("_system/projects.yaml")).unwrap().contains("# 台帳"), "台帳は変わらない");
}

#[test]
fn 手順書は拡張子と秘密情報の名前を検査し消せない() {
    let e = Env::new();
    let put = |p: &str| e.win("PUT", "/api/v1/doc", Some(json!({"path": p, "content": "x"}))).0;
    assert_eq!(put("library/30-resources/skills/a.exe"), 422);
    assert_eq!(put("library/30-resources/skills/server.pem"), 422);
    assert_eq!(put("library/30-resources/skills/cross-review.sh"), 200);
    assert_eq!(put("library/30-resources/a.md"), 403, "skills の外の 30-resources は不可");
    // 手順書は、消せない(受信箱だけが消せる)
    assert_eq!(e.win("DELETE", "/api/v1/doc?path=library/30-resources/skills/cross-review.sh", None).0, 403);
    assert!(e.lib().join("30-resources/skills/cross-review.sh").exists());
}

// ---- 利用統計(Ver7.0.1) ----

fn tokens(output: u64) -> claude_stats::Tokens {
    claude_stats::Tokens { input: 1, cache_creation: 10, cache_read: 100, output }
}

/// 応答1つ(id・日時・出力トークン数)と発言1つを持つ差分。
fn delta_of(response: &str, at_secs: i64, output: u64, message: &str) -> Value {
    let d = claude_stats::Delta {
        version: claude_stats::DELTA_VERSION,
        responses: vec![claude_stats::DeltaResponse(
            response.into(), at_secs, "claude-opus-5-5".into(), "s1".into(), tokens(output), tokens(output),
        )],
        messages: vec![claude_stats::DeltaMessage(message.into(), at_secs, "s1".into(), 1)],
    };
    serde_json::to_value(d).unwrap()
}

#[test]
fn 統計の窓口は認証が要る() {
    let e = Env::new();
    let d = delta_of("r1", at("2026-11-22T10:00:00+09:00").timestamp(), 5, "m1");
    assert_eq!(e.call("PUT", "/api/v1/stats/claude/pc1", None, Some(d)).0, 401);
    assert_eq!(e.call("GET", "/api/v1/stats/claude", None, None).0, 401);
}

#[test]
fn 端末ごとの差分を送ると全端末の合算が返り_同じ差分の再送で増えない() {
    let e = Env::new();
    let t = at("2026-11-22T10:00:00+09:00").timestamp();
    let (code, v) = e.win("PUT", "/api/v1/stats/claude/pc1", Some(delta_of("r1", t, 5, "m1")));
    assert_eq!((code, v["responses"].as_u64(), v["messages"].as_u64()), (200, Some(1), Some(1)), "{v}");
    // 2台目(mac)。r1は1台目と同じ会話の写し、r2は自分だけの応答。
    let mut d2 = delta_of("r2", t, 7, "m2");
    let same = delta_of("r1", t, 5, "m1");
    d2["responses"].as_array_mut().unwrap().extend(same["responses"].as_array().unwrap().clone());
    d2["messages"].as_array_mut().unwrap().extend(same["messages"].as_array().unwrap().clone());
    assert_eq!(e.call("PUT", "/api/v1/stats/claude/pc2", Some(&e.mac.clone()), Some(d2)).0, 200);

    let (code, s) = e.win("GET", "/api/v1/stats/claude?range=all&tz=540", None);
    assert_eq!(code, 200, "{s}");
    assert_eq!(s["hosts"].as_array().unwrap().len(), 2);
    assert_eq!(s["messages"], 2, "端末をまたぐ重複は1回だけ");
    assert_eq!(s["actual"]["output"], 12);
    assert_eq!(s["until"], "2026-11-22");

    // 同じ差分をもう一度送っても、集計は変わらない。
    assert_eq!(e.win("PUT", "/api/v1/stats/claude/pc1", Some(delta_of("r1", t, 5, "m1"))).0, 200);
    let (_, again) = e.win("GET", "/api/v1/stats/claude?range=all&tz=540", None);
    assert_eq!((again["messages"].clone(), again["actual"].clone()), (s["messages"].clone(), s["actual"].clone()));

    // 端末で絞り込める。
    let (_, only) = e.win("GET", "/api/v1/stats/claude?range=all&tz=540&host=pc2", None);
    assert_eq!((only["messages"].as_u64(), only["actual"]["output"].as_u64()), (Some(2), Some(12)));
    let (_, one) = e.win("GET", "/api/v1/stats/claude?range=all&tz=540&host=pc1", None);
    assert_eq!((one["messages"].as_u64(), one["actual"]["output"].as_u64()), (Some(1), Some(5)));
}

#[test]
fn 日付の区切りは要求の時差で決まり_台帳はサーバーのdataに置かれる() {
    let e = Env::new();
    // 日本時間 11/22 00:30 = UTC 11/21 15:30
    let t = at("2026-11-22T00:30:00+09:00").timestamp();
    assert_eq!(e.win("PUT", "/api/v1/stats/claude/pc1", Some(delta_of("r1", t, 5, "m1"))).0, 200);
    let (_, jp) = e.win("GET", "/api/v1/stats/claude?range=all&tz=540", None);
    let (_, utc) = e.win("GET", "/api/v1/stats/claude?range=all&tz=0", None);
    assert_eq!(jp["daily"][0]["date"], "2026-11-22");
    assert_eq!(utc["daily"][0]["date"], "2026-11-21");
    assert!(e.root.join("data/claude-stats/pc1.json").exists());
    // 古い日は、期間で絞られる(サーバーの時計は 2026-11-22)。
    let old = at("2026-09-01T10:00:00+09:00").timestamp();
    assert_eq!(e.win("PUT", "/api/v1/stats/claude/pc1", Some(delta_of("r0", old, 3, "m0"))).0, 200);
    let (_, d7) = e.win("GET", "/api/v1/stats/claude?range=days7&tz=540", None);
    let (_, all) = e.win("GET", "/api/v1/stats/claude?range=all&tz=540", None);
    assert_eq!((d7["messages"].as_u64(), all["messages"].as_u64()), (Some(1), Some(2)));
}

#[test]
fn 統計の不正な入力は断り_台帳を壊さない() {
    let e = Env::new();
    let t = at("2026-11-22T10:00:00+09:00").timestamp();
    // 名前が空
    assert_eq!(e.win("PUT", "/api/v1/stats/claude/", Some(delta_of("r1", t, 5, "m1"))).0, 422);
    // 制御文字(%0A)
    assert_eq!(e.win("PUT", "/api/v1/stats/claude/a%0Ab", Some(delta_of("r1", t, 5, "m1"))).0, 422);
    // 本文の形が違う
    assert_eq!(e.win("PUT", "/api/v1/stats/claude/pc1", Some(json!({"hello": 1}))).0, 422);
    // 版が違う
    let mut old = delta_of("r1", t, 5, "m1");
    old["version"] = json!(99);
    let (code, v) = e.win("PUT", "/api/v1/stats/claude/pc1", Some(old));
    assert_eq!(code, 422, "{v}");
    assert!(!e.root.join("data/claude-stats/pc1.json").exists(), "断ったときは台帳を作らない");
    // 集計の引数
    assert_eq!(e.win("GET", "/api/v1/stats/claude?range=year", None).0, 422);
    assert_eq!(e.win("GET", "/api/v1/stats/claude?tz=abc", None).0, 422);
    assert_eq!(e.win("GET", "/api/v1/stats/claude?tz=99999999", None).0, 422);
    // 窓口に無い
    assert_eq!(e.win("GET", "/api/v1/stats/codex", None).0, 404);
    assert_eq!(e.win("DELETE", "/api/v1/stats/claude/pc1", None).0, 404);
    // 記録が1件も無くても、集計は空で返る
    let (code, s) = e.win("GET", "/api/v1/stats/claude", None);
    assert_eq!((code, s["messages"].as_u64(), s["hosts"].as_array().map(Vec::len)), (200, Some(0), Some(0)));
}

#[allow(dead_code)]
fn _unused(_: &Path) {}
