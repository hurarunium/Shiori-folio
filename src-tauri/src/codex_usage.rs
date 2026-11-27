//! アプリ側のCodexの利用統計(詩織Ver7.0.2)。このPCのCodexの履歴を`data/codex-stats/`の台帳へ取り込み、
//! サーバー接続のときは変わった分を送って、全端末の合算をサーバーから取る。台帳の中身は`codex_stats`。
//!
//! 流れはClaude(`lib.rs`の`refresh_claude_stats_blocking`)と同じ。状態と命令は分けてあり、
//! Codexの失敗はClaudeの取り込み・表示に影響しない。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use codex_stats::{RefreshOutcome, StatsRange};
use shiori_client::Client;

/// 直近に成功した取り込みの時刻(ミリ秒)。起動時と画面を開いたときの取り込みを重ねない。
static REFRESH: Mutex<Option<u64>> = Mutex::new(None);

fn ledger_dir() -> PathBuf {
    crate::project_root().join("data").join("codex-stats")
}

/// 履歴を取り込み、`remote`があれば、まだ送っていない記録を送りきる。返すのは(取り込みの結果, 送った件数)。
/// 送れなければエラーにする(手元の集計へ黙って切り替えない)。送れていない記録は台帳に残り、次に送り直す。
fn sync(
    codex_home: &Path,
    ledger_dir: &Path,
    host: &str,
    now_ms: u64,
    remote: Option<&Client>,
) -> Result<(RefreshOutcome, usize), String> {
    let outcome = codex_stats::refresh_with(codex_home, ledger_dir, host, now_ms, remote.is_some())?;
    let Some(c) = remote else {
        return Ok((outcome, 0));
    };
    let sent = codex_stats::push_pending(ledger_dir, host, codex_stats::MAX_DELTA_RECORDS, |delta| {
        let body = serde_json::to_value(delta).map_err(|e| format!("差分を作れません: {e}"))?;
        shiori_client::stats::codex_push(c, host, &body)
            .map(|_| ())
            .map_err(shiori_client::stats::explain_codex)
    })?;
    Ok((outcome, sent))
}

/// 集計。`remote`があればサーバーが全端末を合算した結果、無ければこのPCの`ledger_dir`の集計。
fn summary(
    ledger_dir: &Path,
    range: StatsRange,
    host: Option<&str>,
    now_ms: u64,
    remote: Option<&Client>,
) -> Result<serde_json::Value, String> {
    match remote {
        // 日付の区切りは、このPCの時差で。
        Some(c) => {
            let tz_minutes = chrono::Local::now().offset().local_minus_utc() / 60;
            shiori_client::stats::codex_summary(c, range.as_query(), host, tz_minutes)
                .map_err(shiori_client::stats::explain_codex)
        }
        None => serde_json::to_value(codex_stats::summarize_local(ledger_dir, range, host, now_ms))
            .map_err(|e| format!("Codexの利用統計の集計に失敗: {e}")),
    }
}

/// 取り込みを省いたときは`None`を返す。
pub(crate) fn refresh_blocking(force: bool) -> Result<Option<RefreshOutcome>, String> {
    let mut last = REFRESH.lock().map_err(|_| "Codexの利用統計の取り込みの状態が壊れています".to_string())?;
    let now_ms = crate::unix_now_ms()?;
    if !force && last.is_some_and(|t| now_ms.saturating_sub(t) < crate::CLAUDE_STATS_REFRESH_INTERVAL_MS) {
        return Ok(None);
    }
    let home = codex_stats::default_codex_home().ok_or("Codexの履歴の場所(ホームフォルダ)が分かりません")?;
    let remote = crate::remote::client();
    let (outcome, sent) = sync(&home, &ledger_dir(), &crate::stats_host_name(), now_ms, remote.as_ref())?;
    if sent > 0 {
        crate::shiori_log::info("APP", "Codexの利用統計をサーバーへ送った", &[("件数", &sent)]);
    }
    // 失敗したときは`last`を更新しないので、次に開いたときに送り直す。
    *last = Some(now_ms);
    Ok(Some(outcome))
}

#[tauri::command]
pub async fn refresh_codex_stats(force: bool) -> Result<Option<RefreshOutcome>, String> {
    crate::run_blocking(move || refresh_blocking(force)).await
}

#[tauri::command]
pub async fn get_codex_stats(range: StatsRange, host: Option<String>) -> Result<serde_json::Value, String> {
    let now_ms = crate::unix_now_ms()?;
    crate::run_blocking(move || {
        summary(&ledger_dir(), range, host.as_deref(), now_ms, crate::remote::client().as_ref())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    const NOW_MS: u64 = 1_795_748_400_000;

    /// 応答1つと発言1つの履歴を持つ一時フォルダ。(root, Codexの設定フォルダ, 台帳の置き場)
    fn fixture(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("shiori-codex-app-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("codex");
        fs::create_dir_all(home.join("sessions")).unwrap();
        let line = |kind: &str, payload: serde_json::Value| {
            json!({"timestamp": "2026-11-27T01:00:00Z", "type": kind, "payload": payload}).to_string() + "\n"
        };
        let text = line("session_meta", json!({"session_id": "s1", "source": "vscode"}))
            + &line("turn_context", json!({"turn_id": "t1", "model": "m"}))
            + &line("event_msg", json!({"type": "item_completed", "item": {"type": "UserMessage", "id": "u1"}}))
            + &line("token_usage_record", json!({"response_id": "r1", "session_id": "s1", "turn_id": "t1",
                "usage": {"input_tokens": 100, "output_tokens": 20}}));
        fs::write(home.join("sessions/a.jsonl"), text).unwrap();
        let ledgers = root.join("ledgers");
        (root, home, ledgers)
    }

    #[test]
    fn サーバーの設定が無ければ手元で取り込んで集計する() {
        let (root, home, ledgers) = fixture("local");
        let (outcome, sent) = sync(&home, &ledgers, "pc1", NOW_MS, None).unwrap();
        assert_eq!((outcome.new_responses, outcome.new_messages, sent), (1, 1, 0));
        let s = summary(&ledgers, StatsRange::All, None, NOW_MS, None).unwrap();
        assert_eq!((s["totalTokens"].as_u64(), s["messages"].as_u64()), (Some(120), Some(1)));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn サーバーにつながらないときは手元へ切り替えず_送れなかった分が残る() {
        let (root, home, ledgers) = fixture("down");
        let down = Client::new("http://127.0.0.1:1", "t", "app");
        let e = sync(&home, &ledgers, "pc1", NOW_MS, Some(&down)).unwrap_err();
        assert!(e.contains("つながれません"), "{e}");
        // 取り込みは手元で進み、送る対象に残る。
        assert!(!codex_stats::take_delta(&ledgers, "pc1", 100).unwrap().is_empty());
        let e = summary(&ledgers, StatsRange::All, None, NOW_MS, Some(&down)).unwrap_err();
        assert!(e.contains("つながれません"), "{e}");
        // あとでサーバーの設定を外しても、取り込んだ分は手元にある。
        let s = summary(&ledgers, StatsRange::All, None, NOW_MS, None).unwrap();
        assert_eq!(s["totalTokens"], 120);
        fs::remove_dir_all(&root).unwrap();
    }

    /// 試験用のサーバー(本番ではないもの)へ、このPCの実物の履歴を送り、合算が手元の集計と同じことを確かめる。
    /// 数字だけを表示する。`SHIORI_TEST_URL=http://127.0.0.1:<port> SHIORI_TEST_TOKEN=… cargo test --lib
    /// 実物の履歴を試験用サーバー -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn 実物の履歴を試験用サーバーへ送ると手元の集計と同じになる() {
        let url = std::env::var("SHIORI_TEST_URL").expect("SHIORI_TEST_URL");
        let token = std::env::var("SHIORI_TEST_TOKEN").expect("SHIORI_TEST_TOKEN");
        assert!(url.starts_with("http://127.0.0.1:"), "本番へは送らない: {url}");
        let root = std::env::temp_dir().join(format!("shiori-codex-app-real-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let ledgers = root.join("ledgers");
        let home = codex_stats::default_codex_home().unwrap();
        let now_ms = crate::unix_now_ms().unwrap();
        let c = Client::new(&url, &token, "app");
        let (outcome, sent) = sync(&home, &ledgers, "real-test", now_ms, Some(&c)).unwrap();
        let remote = summary(&ledgers, StatsRange::All, None, now_ms, Some(&c)).unwrap();
        let local = summary(&ledgers, StatsRange::All, None, now_ms, None).unwrap();
        for key in ["sessions", "messages", "activeDays", "totalTokens", "tokens", "daily", "models", "limits", "contextWindows", "warnings"] {
            assert_eq!(remote[key], local[key], "{key}");
        }
        println!(
            "files={} sent={sent} sessions={} messages={} totalTokens={} hosts={}",
            outcome.files_read, remote["sessions"], remote["messages"], remote["totalTokens"],
            remote["hosts"].as_array().map_or(0, Vec::len)
        );
        // 送り直すものは無い。
        assert!(codex_stats::take_delta(&ledgers, "real-test", 100).unwrap().is_empty());
        fs::remove_dir_all(&root).unwrap();
    }
}
