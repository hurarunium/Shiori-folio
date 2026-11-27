//! 利用統計の窓口(詩織Ver7.0.1のClaude、Ver7.0.2のCodex)。端末が取り込んだ台帳の差分を受け取り、全端末の合算を返す。
//! 台帳の取り込み・集計の中身は `claude_stats`・`codex_stats`(アプリと同じ部品)。
//!
//! - `PUT /api/v1/stats/claude/{ホスト名}`  本文は差分 `{version, responses, messages}` → `{responses, messages}`(取り込んだ件数)
//! - `GET /api/v1/stats/claude?range=all|days30|days7[&host=..][&tz=分]` → `ClaudeStats`
//! - `PUT /api/v1/stats/codex/{ホスト名}`  本文は `codex_stats::Delta` → `{responses, messages}`
//! - `GET /api/v1/stats/codex?(Claudeと同じ引数)` → `CodexStats`
//!
//! 台帳は `data/claude-stats/<ホスト名>.json` と `data/codex-stats/<ホスト名>.json`。履歴の中身
//! (プロンプト・応答)は受け取らない。どちらも世代のバックアップには入れない(端末の台帳が写し)。

use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, FixedOffset};
use serde::de::DeserializeOwned;
use serde_json::json;
use tiny_http::Method;

use crate::api::{err, percent_decode, query_param, App, Reply};

/// 台帳の書き込みは1本の列に並べる(同じ端末の台帳を、2つの要求が同時に読み書きして、片方の分を失わないように)。
static LOCK: Mutex<()> = Mutex::new(());

const CLAUDE_PATH: &str = "/api/v1/stats/claude";
const CODEX_PATH: &str = "/api/v1/stats/codex";
/// ホスト名の長さの上限(文字数)。
const MAX_HOST_CHARS: usize = 128;

pub fn ledger_dir(app: &App) -> PathBuf {
    app.cfg.data.join("claude-stats")
}

pub fn codex_ledger_dir(app: &App) -> PathBuf {
    app.cfg.data.join("codex-stats")
}

pub fn route(app: &App, method: &Method, path: &str, url: &str, body: &str) -> Reply {
    if *method == Method::Get {
        match path {
            CLAUDE_PATH => return claude_summary(app, url),
            CODEX_PATH => return codex_summary(app, url),
            _ => {}
        }
    }
    if *method == Method::Put {
        if let Some(host) = path.strip_prefix(&format!("{CLAUDE_PATH}/")) {
            return claude_push(app, host, body);
        }
        if let Some(host) = path.strip_prefix(&format!("{CODEX_PATH}/")) {
            return codex_push(app, host, body);
        }
    }
    err(404, "Not Found")
}

/// 差分の受け取りの、ClaudeとCodexに共通する検査。ホスト名と、本文の形を確かめる。
fn parse_push<D: DeserializeOwned>(host: &str, body: &str) -> Result<(String, D), Reply> {
    let Some(host) = percent_decode(host) else {
        return Err(err(422, "ホスト名が正しくない"));
    };
    if host.is_empty() || host.chars().count() > MAX_HOST_CHARS || host.chars().any(char::is_control) {
        return Err(err(422, "ホスト名が正しくない(空・長すぎる・制御文字)"));
    }
    match serde_json::from_str(body) {
        Ok(d) => Ok((host, d)),
        Err(e) => Err(err(422, &format!("本文の形が正しくない: {e}"))),
    }
}

fn check_version(got: u32, supported: u32) -> Result<(), Reply> {
    if got == supported {
        return Ok(());
    }
    Err(err(422, &format!("差分の版が合いません(届いた版: {got}、扱える版: {supported})")))
}

fn now_ms(now: &DateTime<FixedOffset>) -> u64 {
    now.timestamp_millis().max(0) as u64
}

/// 取り込んだ件数、または失敗の理由を応答にする。
fn applied(result: Result<(usize, usize), String>) -> Reply {
    match result {
        Ok((responses, messages)) => (200, json!({"responses": responses, "messages": messages})),
        Err(m) => err(500, &m),
    }
}

fn claude_push(app: &App, host: &str, body: &str) -> Reply {
    let (host, delta): (String, claude_stats::Delta) = match parse_push(host, body) {
        Ok(v) => v,
        Err(reply) => return reply,
    };
    if let Err(reply) = check_version(delta.version, claude_stats::DELTA_VERSION) {
        return reply;
    }
    let now = now_ms(&(app.clock)());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    applied(claude_stats::apply_delta(&ledger_dir(app), &host, &delta, now).map(|o| (o.responses, o.messages)))
}

fn codex_push(app: &App, host: &str, body: &str) -> Reply {
    let (host, delta): (String, codex_stats::Delta) = match parse_push(host, body) {
        Ok(v) => v,
        Err(reply) => return reply,
    };
    if let Err(reply) = check_version(delta.version, codex_stats::DELTA_VERSION) {
        return reply;
    }
    let now = now_ms(&(app.clock)());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    applied(codex_stats::apply_delta(&codex_ledger_dir(app), &host, &delta, now).map(|o| (o.responses, o.messages)))
}

/// 集計の引数。期間の名前、端末の絞り込み、日付の区切りに使う時差。
struct SummaryQuery {
    range: Range,
    host: Option<String>,
    offset: FixedOffset,
    now_ms: u64,
}

#[derive(Clone, Copy)]
enum Range {
    All,
    Days30,
    Days7,
}

fn summary_query(app: &App, url: &str) -> Result<SummaryQuery, Reply> {
    let range = match query_param(url, "range").as_deref() {
        None | Some("all") => Range::All,
        Some("days30") => Range::Days30,
        Some("days7") => Range::Days7,
        Some(other) => return Err(err(422, &format!("range が正しくない: {other}(all・days30・days7)"))),
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
                return Err(err(422, "tz が正しくない(UTC からの時差を、分で)"));
            };
            offset
        }
    };
    let host = query_param(url, "host").filter(|h| !h.is_empty());
    Ok(SummaryQuery { range, host, offset, now_ms: now_ms(&now) })
}

fn stats_reply<T: serde::Serialize>(stats: T) -> Reply {
    match serde_json::to_value(stats) {
        Ok(v) => (200, v),
        Err(e) => err(500, &format!("集計を返せません: {e}")),
    }
}

fn claude_summary(app: &App, url: &str) -> Reply {
    let q = match summary_query(app, url) {
        Ok(q) => q,
        Err(reply) => return reply,
    };
    let range = match q.range {
        Range::All => claude_stats::StatsRange::All,
        Range::Days30 => claude_stats::StatsRange::Days30,
        Range::Days7 => claude_stats::StatsRange::Days7,
    };
    stats_reply(claude_stats::summarize(&ledger_dir(app), range, q.host.as_deref(), q.now_ms, &q.offset))
}

fn codex_summary(app: &App, url: &str) -> Reply {
    let q = match summary_query(app, url) {
        Ok(q) => q,
        Err(reply) => return reply,
    };
    let range = match q.range {
        Range::All => codex_stats::StatsRange::All,
        Range::Days30 => codex_stats::StatsRange::Days30,
        Range::Days7 => codex_stats::StatsRange::Days7,
    };
    stats_reply(codex_stats::summarize(&codex_ledger_dir(app), range, q.host.as_deref(), q.now_ms, &q.offset))
}
