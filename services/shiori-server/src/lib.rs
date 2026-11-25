//! 詩織Ver7.0のサーバー。正本(library・知識の棚・タスク)への書き込みを一元化し、検索を中継し、世代のバックアップを作る。
//! 端末(アプリ・CLI・mcp_server)は、Tailscale経由のHTTPで、トークンを付けて呼ぶ。

pub mod api;
pub mod audit;
pub mod auth;
pub mod backup;
pub mod children;
pub mod config;
pub mod keep_awake;
pub mod relay;
pub mod saver;
pub mod stats_api;
pub mod tasks_api;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

pub use api::Clock;
pub use config::Config;

/// 動いているサーバー。`stop` で止める(試験用。本番はプロセスごと止める)。
pub struct Running {
    pub addr: SocketAddr,
    pub app: Arc<api::App>,
    server: Arc<tiny_http::Server>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl Running {
    pub fn stop(self) {
        self.server.unblock();
        for _ in 0..self.handles.len() {
            self.server.unblock();
        }
        for h in self.handles {
            let _ = h.join();
        }
    }

    /// プロセスが終わるまで待つ。
    pub fn wait(self) {
        for h in self.handles {
            let _ = h.join();
        }
    }
}

pub fn local_clock() -> Clock {
    Arc::new(|| chrono::Local::now().fixed_offset())
}

pub fn start(cfg: Config) -> Result<Running, String> {
    start_with_clock(cfg, local_clock())
}

/// 時計を渡せる版(日誌の午前5時の前後などを、試験で確かめるため)。
pub fn start_with_clock(cfg: Config, clock: Clock) -> Result<Running, String> {
    let bind: SocketAddr = cfg.bind.parse().map_err(|e| format!("bind が正しくない: {e}"))?;
    if bind.ip().is_unspecified() {
        return Err("全部のアドレス(0.0.0.0 / ::)では待ち受けない。Tailscaleのアドレスか 127.0.0.1 を指定する".into());
    }
    let layout = cfg.layout()?;
    for (name, dir) in [("library", &cfg.library), ("knowledge", &cfg.knowledge), ("tasks", &cfg.tasks)] {
        std::fs::create_dir_all(dir).map_err(|e| format!("{name} を用意できない {}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(&cfg.data).map_err(|e| format!("data を用意できない: {e}"))?;

    let health = Arc::new(children::Health::default());
    if !cfg.no_children {
        let runtime = cfg.runtime.clone().ok_or("子を動かすには runtime が必要(動かさないなら no_children)")?;
        children::supervise(
            children::Paths {
                runtime,
                library: cfg.library.clone(),
                data: cfg.data.clone(),
                embed_port: cfg.embed_port,
                rag_port: cfg.rag_port,
            },
            health.clone(),
        );
    }

    let app = Arc::new(api::App {
        layout,
        roots: saver::Roots { library: cfg.library.clone(), knowledge: cfg.knowledge.clone(), tasks: cfg.tasks.clone() },
        tokens: cfg.data.join("tokens.json"),
        audit: audit::Audit::open(&cfg.data.join("audit"))?,
        uploads: cfg.data.join("uploads"),
        backups: cfg.data.join("backups"),
        health,
        clock,
        started: Instant::now(),
        cfg,
    });

    let server = Arc::new(tiny_http::Server::http(bind).map_err(|e| format!("待ち受けられない {bind}: {e}"))?);
    let addr = server.server_addr().to_ip().ok_or("待ち受けのアドレスを取れない")?;
    let mut handles = Vec::new();
    for _ in 0..8 {
        let (server, app) = (server.clone(), app.clone());
        handles.push(std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let t = Instant::now();
                let method = req.method().to_string();
                let remote = req.remote_addr().map(|a| a.ip().to_string()).unwrap_or_default();
                let url = req.url().to_string();
                let (device, client, out) = api::handle(&app, &mut req);
                let code = match &out {
                    api::Out::Json((c, _)) => *c,
                    api::Out::Zip(_) | api::Out::Bytes(_) => 200,
                };
                app.audit.record(&device, &client, &remote, &method, &url, code, t.elapsed().as_millis() as u64);
                use tiny_http::{Header, Response};
                match out {
                    api::Out::Json((code, body)) => {
                        let resp = Response::from_string(body.to_string())
                            .with_status_code(code)
                            .with_header(Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap());
                        let _ = req.respond(resp);
                    }
                    api::Out::Bytes(bytes) => {
                        let resp = Response::from_data(bytes).with_header(Header::from_bytes("Content-Type", "application/octet-stream").unwrap());
                        let _ = req.respond(resp);
                    }
                    api::Out::Zip(bytes) => {
                        let resp = Response::from_data(bytes).with_header(Header::from_bytes("Content-Type", "application/zip").unwrap());
                        let _ = req.respond(resp);
                    }
                }
            }
        }));
    }
    Ok(Running { addr, app, server, handles })
}
