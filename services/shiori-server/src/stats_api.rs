//! 利用統計の窓口(詩織Ver7.0.1)。端末が取り込んだ台帳の差分を受け取り、全端末の合算を返す。
//! 台帳の取り込み・集計の中身は `claude_stats`(アプリと同じ部品)。
//!
//! - `PUT /api/v1/stats/claude/{ホスト名}`  本文は差分 `{version, responses, messages}` → `{responses, messages}`(取り込んだ件数)
//! - `GET /api/v1/stats/claude?range=all|days30|days7[&host=..][&tz=分]` → `ClaudeStats`
//!
//! 台帳は `data/claude-stats/<ホスト名>.json`。履歴の中身(プロンプト・応答)は受け取らない
//! (差分に入るのは、ID・日時・モデル・トークン数・sessionIdだけ)。

use std::path::PathBuf;
use std::sync::Mutex;

use chrono::FixedOffset;
use claude_stats::{Delta, StatsRange, DELTA_VERSION};
use serde_json::json;
use tiny_http::Method;

use crate::api::{err, percent_decode, query_param, App, Reply};

/// 台帳の書き込みは1本の列に並べる(同じ端末の台帳を、2つの要求が同時に読み書きして、片方の分を失わないように)。
static LOCK: Mutex<()> = Mutex::new(());

const CLAUDE_PATH: &str = "/api/v1/stats/claude";
/// ホスト名の長さの上限(文字数)。
const MAX_HOST_CHARS: usize = 128;

pub fn ledger_dir(app: &App) -> PathBuf {
    app.cfg.data.join("claude-stats")
}

pub fn route(app: &App, method: &Method, path: &str, url: &str, body: &str) -> Reply {
    if path == CLAUDE_PATH && *method == Method::Get {
        return summary(app, url);
    }
    if let Some(host) = path.strip_prefix(&format!("{CLAUDE_PATH}/")) {
        if *method == Method::Put {
            return push(app, host, body);
        }
    }
    err(404, "Not Found")
}

fn push(app: &App, host: &str, body: &str) -> Reply {
    let Some(host) = percent_decode(host) else {
        return err(422, "ホスト名が正しくない");
    };
    if host.is_empty() || host.chars().count() > MAX_HOST_CHARS || host.chars().any(char::is_control) {
        return err(422, "ホスト名が正しくない(空・長すぎる・制御文字)");
    }
    let delta: Delta = match serde_json::from_str(body) {
        Ok(d) => d,
        Err(e) => return err(422, &format!("本文の形が正しくない: {e}")),
    };
    if delta.version != DELTA_VERSION {
        return err(
            422,
            &format!("差分の版が合いません(届いた版: {}、扱える版: {DELTA_VERSION})", delta.version),
        );
    }
    let now_ms = (app.clock)().timestamp_millis().max(0) as u64;
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    match claude_stats::apply_delta(&ledger_dir(app), &host, &delta, now_ms) {
        Ok(o) => (200, json!({"responses": o.responses, "messages": o.messages})),
        Err(m) => err(500, &m),
    }
}

fn summary(app: &App, url: &str) -> Reply {
    let range = match query_param(url, "range").as_deref() {
        None | Some("all") => StatsRange::All,
        Some("days30") => StatsRange::Days30,
        Some("days7") => StatsRange::Days7,
        Some(other) => return err(422, &format!("range が正しくない: {other}(all・days30・days7)")),
    };
    let now = (app.clock)();
    // 日付の区切りは、見ている端末の時差で決める。無ければ、サーバーの時計の時差。
    let offset = match query_param(url, "tz") {
        None => *now.offset(),
        Some(s) => {
            let Some(offset) = s
                .parse::<i32>()
                .ok()
                .and_then(|minutes| minutes.checked_mul(60))
                .and_then(FixedOffset::east_opt)
            else {
                return err(422, "tz が正しくない(UTC からの時差を、分で)");
            };
            offset
        }
    };
    let host = query_param(url, "host").filter(|h| !h.is_empty());
    let now_ms = now.timestamp_millis().max(0) as u64;
    let stats = claude_stats::summarize(&ledger_dir(app), range, host.as_deref(), now_ms, &offset);
    match serde_json::to_value(stats) {
        Ok(v) => (200, v),
        Err(e) => err(500, &format!("集計を返せません: {e}")),
    }
}
