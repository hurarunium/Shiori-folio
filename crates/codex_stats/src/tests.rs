use super::*;
use chrono::FixedOffset;
use serde_json::json;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

// 2026-11-27 12:00:00 JST
const NOW_MS: u64 = 1_795_748_400_000;
const TS: &str = "2026-11-27T01:00:00.000Z";

fn jst() -> FixedOffset {
    FixedOffset::east_opt(9 * 3600).unwrap()
}

/// 試験ごとの一時フォルダ。(root, Codexの設定フォルダ, 台帳の置き場)
fn dirs(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("shiori-codex-stats-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    (root.clone(), root.join("codex"), root.join("ledgers"))
}

fn line(kind: &str, ts: &str, payload: Value) -> String {
    json!({"timestamp": ts, "type": kind, "payload": payload}).to_string()
}

fn meta(session: &str, subagent: bool) -> String {
    let source = if subagent { json!({"subagent": {"other": "guardian"}}) } else { json!("vscode") };
    line("session_meta", TS, json!({"session_id": session, "source": source, "cwd": "秘密のフォルダ",
        "creator_account_id": "秘密のアカウント", "git": {"repository_url": "秘密のURL"}}))
}

fn turn(turn: &str, model: &str) -> String {
    line("turn_context", TS, json!({"turn_id": turn, "model": model}))
}

fn usage(ts: &str, id: &str, session: &str, turn: &str) -> String {
    line("token_usage_record", ts, json!({"response_id": id, "session_id": session, "turn_id": turn,
        "usage": {"input_tokens": 100, "cached_input_tokens": 60, "cache_write_input_tokens": 0,
                  "output_tokens": 20, "reasoning_output_tokens": 5, "total_tokens": 120}}))
}

fn message(kind: &str, id: &str) -> String {
    line("event_msg", TS, json!({"type": "item_completed",
        "item": {"type": kind, "id": id, "content": "秘密の本文"}}))
}

fn token_count(ts: &str, used: f64, window: u64) -> String {
    line("event_msg", ts, json!({"type": "token_count", "info": {"model_context_window": window},
        "rate_limits": {"limit_id": "codex", "plan_type": "free", "secondary": null,
                        "primary": {"used_percent": used, "window_minutes": 43200, "resets_at": 1797754172}}}))
}

fn append(path: &Path, lines: &[String]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
}

fn refresh(home: &Path, ledgers: &Path, host: &str, for_server: bool) -> RefreshOutcome {
    refresh_with(home, ledgers, host, NOW_MS, for_server).unwrap()
}

fn summary(ledgers: &Path, range: StatsRange, host: Option<&str>) -> CodexStats {
    summarize(ledgers, range, host, NOW_MS, &jst())
}

fn as_json(s: CodexStats) -> Value {
    serde_json::to_value(s).unwrap()
}

#[test]
fn 合計は入力と出力で_内数は足さない() {
    let (root, home, ledgers) = dirs("total");
    append(&home.join("sessions/2026/11/27/a.jsonl"),
        &[meta("s1", false), turn("t1", "gpt-x"), usage(TS, "r1", "s1", "t1")]);
    refresh(&home, &ledgers, "pc1", false);
    let s = summary(&ledgers, StatsRange::All, None);
    assert_eq!(s.total_tokens, 120);
    assert_eq!((s.tokens.cached_input, s.tokens.reasoning_output), (60, 5));
    assert_eq!((s.models[0].model.as_str(), s.models[0].total), ("gpt-x", 120));
    assert_eq!((s.sessions, s.active_days), (1, 1));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn サブエージェントはトークンもメッセージも数え_セッションは親にまとまる() {
    let (root, home, ledgers) = dirs("sub");
    append(&home.join("sessions/a.jsonl"), &[meta("s1", false), turn("t1", "main"),
        message("UserMessage", "u1"), message("AgentMessage", "a1"), message("Reasoning", "x1"),
        message("CommandExecution", "x2"), usage(TS, "r1", "s1", "t1")]);
    append(&home.join("sessions/b.jsonl"), &[meta("s1", true), turn("t2", "review"),
        message("UserMessage", "u2"), message("AgentMessage", "a2"), usage(TS, "r2", "s1", "t2")]);
    refresh(&home, &ledgers, "pc1", false);
    let s = summary(&ledgers, StatsRange::All, None);
    assert_eq!((s.sessions, s.messages, s.total_tokens), (1, 4, 240));
    assert_eq!(s.models.len(), 2);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn アーカイブへ移った履歴や写しは二重に数えない() {
    let (root, home, ledgers) = dirs("archive");
    let lines = [meta("s1", false), turn("t1", "m"), message("UserMessage", "u1"), usage(TS, "r1", "s1", "t1")];
    let live = home.join("sessions/2026/11/27/a.jsonl");
    append(&live, &lines);
    refresh(&home, &ledgers, "pc1", true);
    push_pending(&ledgers, "pc1", 100, |_| Ok(())).unwrap();
    fs::create_dir_all(home.join("archived_sessions")).unwrap();
    fs::rename(&live, home.join("archived_sessions/a.jsonl")).unwrap();
    let second = refresh(&home, &ledgers, "pc1", true);
    assert_eq!((second.new_responses, second.new_messages), (0, 0));
    assert!(take_delta(&ledgers, "pc1", 100).unwrap().is_empty(), "移しただけでは送り直さない");
    let s = summary(&ledgers, StatsRange::All, None);
    assert_eq!((s.total_tokens, s.messages), (120, 1));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn 差分で読んでも前の回のセッションとモデルを使う() {
    let (root, home, ledgers) = dirs("incr");
    let file = home.join("sessions/a.jsonl");
    append(&file, &[meta("s1", true), turn("t1", "m1")]);
    refresh(&home, &ledgers, "pc1", false);
    append(&file, &[message("AgentMessage", "a1"), usage(TS, "r1", "s1", "t1")]);
    let o = refresh(&home, &ledgers, "pc1", false);
    assert_eq!((o.new_responses, o.new_messages), (1, 1));
    let s = summary(&ledgers, StatsRange::All, None);
    assert_eq!((s.sessions, s.messages, s.models[0].model.as_str()), (1, 1, "m1"));
    assert_eq!(refresh(&home, &ledgers, "pc1", false).files_read, 0);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn 書き込み途中の行は次回に回し_壊れた行は飛ばす() {
    let (root, home, ledgers) = dirs("partial");
    let file = home.join("sessions/a.jsonl");
    append(&file, &[meta("s1", false), "{壊れた行".to_string()]);
    let full = usage(TS, "r1", "s1", "t1");
    let (head, tail) = full.split_at(30);
    fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(head.as_bytes()).unwrap();
    assert_eq!(refresh(&home, &ledgers, "pc1", false).skipped_lines, 1);
    assert_eq!(summary(&ledgers, StatsRange::All, None).total_tokens, 0);
    append(&file, &[tail.to_string()]);
    refresh(&home, &ledgers, "pc1", false);
    assert_eq!(summary(&ledgers, StatsRange::All, None).total_tokens, 120);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn 台帳に会話の中身や作業フォルダを入れない() {
    let (root, home, ledgers) = dirs("secret");
    append(&home.join("sessions/a.jsonl"), &[meta("s1", false), turn("t1", "m"),
        message("UserMessage", "u1"), usage(TS, "r1", "s1", "t1"), token_count(TS, 1.0, 1000)]);
    refresh(&home, &ledgers, "pc1", true);
    let text = fs::read_to_string(ledgers.join("pc1.json")).unwrap();
    let sent = serde_json::to_string(&take_delta(&ledgers, "pc1", 100).unwrap()).unwrap();
    assert!(!text.contains("秘密") && !sent.contains("秘密"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn レート制限とコンテキスト窓は端末をまたいで新しい方を使い_絞り込まない() {
    let (root, home, ledgers) = dirs("limits");
    let other = root.join("codex2");
    append(&home.join("sessions/a.jsonl"),
        &[meta("s1", false), turn("t1", "m"), token_count("2026-11-20T00:00:00Z", 10.0, 1000)]);
    append(&other.join("sessions/b.jsonl"),
        &[meta("s2", false), turn("t2", "m"), token_count("2026-11-26T00:00:00Z", 40.0, 2000),
          token_count("2026-11-25T00:00:00Z", 30.0, 2000)]);
    refresh(&home, &ledgers, "pc1", false);
    refresh(&other, &ledgers, "pc2", false);
    for (range, host) in [(StatsRange::All, None), (StatsRange::Days7, Some("pc1"))] {
        let s = summary(&ledgers, range, host);
        assert_eq!(s.limits.len(), 1);
        assert_eq!(s.limits[0].primary.as_ref().unwrap().used_percent, 40.0);
        assert_eq!(s.context_windows[0].tokens, 2000);
    }
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn 同じ大きさの窓でも観測時刻を進め_あとで見た値が勝つ() {
    let (root, home, ledgers) = dirs("window");
    let other = root.join("codex2");
    let a = home.join("sessions/a.jsonl");
    append(&a, &[meta("s1", false), turn("t1", "m"), token_count("2026-11-20T00:00:00Z", 1.0, 2000)]);
    append(&other.join("sessions/b.jsonl"),
        &[meta("s2", false), turn("t2", "m"), token_count("2026-11-23T00:00:00Z", 1.0, 1000)]);
    refresh(&home, &ledgers, "pc1", false);
    refresh(&other, &ledgers, "pc2", false);
    append(&a, &[token_count("2026-11-26T00:00:00Z", 1.0, 2000)]);
    refresh(&home, &ledgers, "pc1", false);
    assert_eq!(summary(&ledgers, StatsRange::All, None).context_windows[0].tokens, 2000);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn 使用量の行が無い形式は警告する() {
    let (root, home, ledgers) = dirs("unsupported");
    append(&home.join("sessions/a.jsonl"), &[meta("s1", false), turn("t1", "m"), token_count(TS, 1.0, 1000)]);
    refresh(&home, &ledgers, "pc1", false);
    let s = summary(&ledgers, StatsRange::All, None);
    assert_eq!(s.warnings.len(), 1);
    assert!(s.warnings[0].contains("1件"));
    // アーカイブへ移しても、件数は増えない。
    fs::create_dir_all(home.join("archived_sessions")).unwrap();
    fs::rename(home.join("sessions/a.jsonl"), home.join("archived_sessions/a.jsonl")).unwrap();
    refresh(&home, &ledgers, "pc1", false);
    refresh(&home, &ledgers, "pc1", false);
    assert!(summary(&ledgers, StatsRange::All, None).warnings[0].contains("1件"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn 履歴が無い端末は空で_壊れた台帳は上書きしない() {
    let (root, home, ledgers) = dirs("empty");
    refresh(&home, &ledgers, "pc1", false);
    let s = summary(&ledgers, StatsRange::All, None);
    assert_eq!((s.sessions, s.messages, s.total_tokens, s.warnings.len()), (0, 0, 0, 0));
    fs::write(ledgers.join("pc1.json"), "{broken").unwrap();
    assert!(refresh_with(&home, &ledgers, "pc1", NOW_MS, true).is_err());
    assert!(apply_delta(&ledgers, "pc1", &Delta { version: DELTA_VERSION, ..Delta::default() }, NOW_MS).is_err());
    assert_eq!(fs::read_to_string(ledgers.join("pc1.json")).unwrap(), "{broken");
    assert_eq!(summary(&ledgers, StatsRange::All, None).warnings.len(), 1);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn 期間で絞り込み_日付はローカル時刻で区切る() {
    let (root, home, ledgers) = dirs("range");
    append(&home.join("sessions/a.jsonl"), &[meta("s1", false), turn("t1", "m"),
        // UTCでは11/26だが、JSTでは11/27。
        usage("2026-11-26T23:30:00Z", "r1", "s1", "t1"),
        usage("2026-11-10T01:00:00Z", "r2", "s1", "t1"),
        usage("2026-09-01T01:00:00Z", "r3", "s1", "t1")]);
    refresh(&home, &ledgers, "pc1", false);
    let all = summary(&ledgers, StatsRange::All, None);
    assert_eq!((all.active_days, all.since.as_str(), all.until.as_str()), (3, "2026-09-01", "2026-11-27"));
    assert_eq!(summary(&ledgers, StatsRange::Days30, None).total_tokens, 240);
    let d7 = summary(&ledgers, StatsRange::Days7, None);
    assert_eq!(d7.daily.iter().map(|d| d.date.as_str()).collect::<Vec<_>>(), vec!["2026-11-27"]);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn 分割して送り_サーバーの集計は端末と同じで_2回送っても変わらない() {
    let (root, home, ledgers) = dirs("sync");
    let server = root.join("server");
    append(&home.join("sessions/a.jsonl"), &[meta("s1", false), turn("t1", "m"),
        message("UserMessage", "u1"), message("AgentMessage", "a1"),
        usage(TS, "r1", "s1", "t1"), usage(TS, "r2", "s1", "t1"), token_count(TS, 5.0, 1000)]);
    // サーバーなしで取り込んだ後にサーバー接続にしても、過去の分が全部送られる。
    refresh(&home, &ledgers, "pc1", false);
    assert!(take_delta(&ledgers, "pc1", 100).unwrap().is_empty());
    refresh(&home, &ledgers, "pc1", true);

    let mut deltas = Vec::new();
    let sent = push_pending(&ledgers, "pc1", 2, |d| {
        assert!(d.len() <= 2);
        deltas.push(d.clone());
        apply_delta(&server, "pc1", d, NOW_MS).map(|_| ())
    })
    .unwrap();
    // 応答2 + 発言2 + レート制限1 + 窓1 + 読めない件数1
    assert_eq!(sent, 7);
    let expected = as_json(summary(&ledgers, StatsRange::All, None));
    assert_eq!(as_json(summary(&server, StatsRange::All, None)), expected);
    for d in &deltas {
        apply_delta(&server, "pc1", d, NOW_MS).unwrap();
    }
    assert_eq!(as_json(summary(&server, StatsRange::All, None)), expected);
    // 差分はJSONにして戻しても同じ(窓口を通る形)。
    for d in &deltas {
        let back: Delta = serde_json::from_str(&serde_json::to_string(d).unwrap()).unwrap();
        assert_eq!(&back, d);
    }
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn 送れなかった分は残り_古い観測はサーバーの新しい値を戻さない() {
    let (root, home, ledgers) = dirs("fail");
    let server = root.join("server");
    append(&home.join("sessions/a.jsonl"), &[meta("s1", false), turn("t1", "m"),
        usage(TS, "r1", "s1", "t1"), token_count("2026-11-20T00:00:00Z", 10.0, 1000)]);
    refresh(&home, &ledgers, "pc1", true);
    let before = take_delta(&ledgers, "pc1", 100).unwrap().len();
    assert_eq!(push_pending(&ledgers, "pc1", 100, |_| Err("つながらない".into())).unwrap_err(), "つながらない");
    assert_eq!(take_delta(&ledgers, "pc1", 100).unwrap().len(), before);

    let old = take_delta(&ledgers, "pc1", 100).unwrap();
    append(&home.join("sessions/a.jsonl"), &[token_count("2026-11-26T00:00:00Z", 50.0, 1000)]);
    refresh(&home, &ledgers, "pc1", true);
    push_pending(&ledgers, "pc1", 100, |d| apply_delta(&server, "pc1", d, NOW_MS).map(|_| ())).unwrap();
    apply_delta(&server, "pc1", &old, NOW_MS).unwrap();
    let s = summary(&server, StatsRange::All, None);
    assert_eq!(s.limits[0].primary.as_ref().unwrap().used_percent, 50.0);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn 版が違う差分は断る() {
    let (root, _, ledgers) = dirs("version");
    let d = Delta { version: DELTA_VERSION + 1, ..Delta::default() };
    assert!(apply_delta(&ledgers, "pc1", &d, NOW_MS).is_err());
    assert!(!ledgers.join("pc1.json").exists());
    fs::remove_dir_all(&root).unwrap();
}

/// 実物の履歴を一時フォルダの台帳へ取り込み、数字だけを表示する(手動確認用)。
/// `cargo test real_history -- --ignored --nocapture`
#[test]
#[ignore]
fn real_history() {
    let (root, _, ledgers) = dirs("real");
    let home = default_codex_home().unwrap();
    let o = refresh(&home, &ledgers, "real", true);
    let s = summary(&ledgers, StatsRange::All, None);
    println!("files={} skipped={} responses={} messages={}", o.files_read, o.skipped_lines, o.new_responses, o.new_messages);
    println!("sessions={} messages={} days={} total={} tokens={:?}", s.sessions, s.messages, s.active_days, s.total_tokens, s.tokens);
    for m in &s.models {
        println!("model {} {}", m.model, m.total);
    }
    println!("limits={:?}", s.limits);
    println!("windows={:?}", s.context_windows);
    println!("warnings={}", s.warnings.len());
    let server = root.join("server");
    let sent = push_pending(&ledgers, "real", MAX_DELTA_RECORDS, |d| apply_delta(&server, "real", d, NOW_MS).map(|_| ())).unwrap();
    let a = serde_json::to_value(summary(&ledgers, StatsRange::All, None)).unwrap();
    let b = serde_json::to_value(summarize(&server, StatsRange::All, None, NOW_MS, &jst())).unwrap();
    println!("sent={sent} server_equals_local={}", a == b);
    println!("ledger_bytes={}", fs::metadata(ledgers.join("real.json")).unwrap().len());
    fs::remove_dir_all(&root).unwrap();
}
