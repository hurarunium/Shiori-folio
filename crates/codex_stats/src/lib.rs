//! Codexの利用統計(詩織Ver7.0.2)。各PCのCodexの履歴(`~/.codex/sessions/`と`archived_sessions/`の
//! `*.jsonl`)を差分で読み、端末(ホスト名)別の台帳を作る。サーバーは端末の差分を受け取り、全端末を合算する。
//! 形は`claude_stats`にそろえるが、トークンの意味が違うので、台帳も集計も分ける。
//!
//! 履歴の使う行(2026-11-27、MacのCLI 0.167・0.171で確認)。
//! - `session_meta`: `session_id`。`source.subagent`があればサブエージェント(`session_id`は親と同じ)。
//! - `turn_context`: `turn_id`ごとのモデル名。
//! - `token_usage_record`: 応答1回の使用量。`response_id`で重複を除く。
//!   `cached_input_tokens`は入力の内数、`reasoning_output_tokens`は出力の内数。
//! - `event_msg`/`item_completed`: `UserMessage`・`AgentMessage`の発言。`item.id`で重複を除く。
//! - `event_msg`/`token_count`: レート制限の使用率と、コンテキスト窓の大きさ。
//!
//! サブエージェントの分は、トークン数にもメッセージ数にも含める。
//! 台帳には会話の中身・作業フォルダ・gitの情報・アカウントIDを入れない。

use chrono::{DateTime, Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const LEDGER_VERSION: u32 = 1;
/// 履歴を読むフォルダ(Codexの設定フォルダからの相対)。アーカイブされた会話は後者へ移る。
const HISTORY_DIRS: [&str; 2] = ["sessions", "archived_sessions"];

#[derive(Serialize, Deserialize, Clone, Copy, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub input: u64,
    /// 入力のうち、キャッシュから読んだ分(内数)。
    pub cached_input: u64,
    pub cache_write: u64,
    pub output: u64,
    /// 出力のうち、推論の分(内数)。
    pub reasoning_output: u64,
}

impl Tokens {
    /// 合計は入力+出力。キャッシュと推論は内数なので足さない。
    pub fn total(&self) -> u64 {
        self.input + self.output
    }

    fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.cached_input += other.cached_input;
        self.cache_write += other.cache_write;
        self.output += other.output;
        self.reasoning_output += other.reasoning_output;
    }
}

/// レート制限の枠1つ。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitWindow {
    pub used_percent: f64,
    pub window_minutes: Option<u64>,
    /// リセットされる時刻(秒)。
    pub resets_at: Option<i64>,
}

/// ある時点で観測したレート制限。足し合わせず、`limit_id`ごとにいちばん新しいものを使う。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitSnapshot {
    pub limit_id: String,
    /// 観測した時刻(秒)。
    pub observed_at: i64,
    pub plan_type: Option<String>,
    pub primary: Option<LimitWindow>,
    pub secondary: Option<LimitWindow>,
}

/// モデルのコンテキスト窓の大きさ。モデルごとにいちばん新しいものを使う。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContextWindow {
    pub model: String,
    pub observed_at: i64,
    pub tokens: u64,
}

/// 応答1回分。JSONでは配列。(日時[秒], モデル, セッション番号, 量, サブエージェントか)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct ResponseRecord(i64, String, u32, Tokens, bool);

/// 発言1つ分。(日時[秒], セッション番号, サブエージェントか)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct MessageRecord(i64, u32, bool);

/// 履歴ファイル1つの読み取り状態。差分で読むと先頭の行や前の回の行を読み直さないので、
/// あとの行の解釈に要る値をここに残す。
#[derive(Serialize, Deserialize, Default, Clone)]
struct FileState {
    /// 読み終えた位置(バイト)。
    offset: u64,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    subagent: bool,
    /// `turn_id`ごとのモデル名。
    #[serde(default)]
    turn_models: BTreeMap<String, String>,
    #[serde(default)]
    last_model: Option<String>,
    /// 使用量のある`token_count`があった。
    #[serde(default)]
    has_token_count: bool,
    /// `token_usage_record`があった。
    #[serde(default)]
    has_usage_record: bool,
}

impl FileState {
    /// `token_count`はあるのに使用量の行が無い(数えられていない)。
    fn is_unsupported(&self) -> bool {
        self.has_token_count && !self.has_usage_record
    }
}

/// 端末1台分の台帳。`data/codex-stats/<ホスト名>.json`に置く。
#[derive(Serialize, Deserialize, Default)]
struct Ledger {
    version: u32,
    host: String,
    /// 最後に取り込んだ時刻(ミリ秒)。
    updated_at: u64,
    /// 履歴ファイル(Codexの設定フォルダからの相対パス)ごとの読み取り状態。端末側だけが使う。
    #[serde(default)]
    files: BTreeMap<String, FileState>,
    sessions: Vec<String>,
    responses: HashMap<String, ResponseRecord>,
    messages: HashMap<String, MessageRecord>,
    #[serde(default)]
    limits: BTreeMap<String, LimitSnapshot>,
    #[serde(default)]
    context_windows: BTreeMap<String, ContextWindow>,
    /// `token_count`はあるのに`token_usage_record`が無いファイルの数(使用量を数えられていない)。
    #[serde(default)]
    unsupported_files: u64,
    /// サーバーへ送る準備ができているか。無ければ、台帳の全記録を送る対象にする。falseに戻すと全部を再送する。
    #[serde(default)]
    sync_initialized: bool,
    #[serde(default)]
    dirty_responses: BTreeSet<String>,
    #[serde(default)]
    dirty_messages: BTreeSet<String>,
    #[serde(default)]
    dirty_limits: BTreeSet<String>,
    #[serde(default)]
    dirty_windows: BTreeSet<String>,
    #[serde(default)]
    dirty_unsupported: bool,
}

impl Ledger {
    fn new() -> Self {
        Ledger { version: LEDGER_VERSION, ..Ledger::default() }
    }

    fn session_lookup(&self) -> HashMap<String, u32> {
        self.sessions.iter().enumerate().map(|(i, s)| (s.clone(), i as u32)).collect()
    }

    fn session_index(&mut self, session_id: &str, lookup: &mut HashMap<String, u32>) -> u32 {
        if let Some(&i) = lookup.get(session_id) {
            return i;
        }
        let i = self.sessions.len() as u32;
        self.sessions.push(session_id.to_string());
        lookup.insert(session_id.to_string(), i);
        i
    }

    fn session_name(&self, i: u32) -> &str {
        self.sessions.get(i as usize).map(String::as_str).unwrap_or("")
    }

    /// 応答を入れる。新しいか、値が変わったらtrue。
    fn put_response(&mut self, id: &str, record: ResponseRecord) -> bool {
        if self.responses.get(id) == Some(&record) {
            return false;
        }
        self.responses.insert(id.to_string(), record);
        true
    }

    /// 観測が新しい(同じ時刻を含む)ときだけ置き換える。変わったらtrue。
    fn put_limit(&mut self, s: LimitSnapshot) -> bool {
        match self.limits.get(&s.limit_id) {
            Some(old) if old.observed_at > s.observed_at || *old == s => false,
            _ => {
                self.limits.insert(s.limit_id.clone(), s);
                true
            }
        }
    }

    fn put_window(&mut self, w: ContextWindow) -> bool {
        match self.context_windows.get(&w.model) {
            Some(old) if old.observed_at > w.observed_at || *old == w => false,
            _ => {
                self.context_windows.insert(w.model.clone(), w);
                true
            }
        }
    }
}

fn read_ledger(path: &Path) -> Result<Option<Ledger>, String> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<Ledger>(&text)
            .map(Some)
            .map_err(|e| format!("台帳が壊れていて読めません({}): {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("台帳を読めません({}): {e}", path.display())),
    }
}

fn write_ledger(path: &Path, ledger: &Ledger) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("台帳の置き場所を作れません: {e}"))?;
    }
    let text = serde_json::to_string(ledger).map_err(|e| format!("台帳の書き出しに失敗: {e}"))?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&tmp, text).map_err(|e| format!("台帳を書けません: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("台帳を置き換えられません: {e}")
    })
}

/// Codexの設定フォルダ。`CODEX_HOME`があればそれ、無ければホームの`.codex`。
pub fn default_codex_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    Some(PathBuf::from(home).join(".codex"))
}

/// 台帳のファイル名に使えるよう、ホスト名の英数字・`-`・`_`・`.`以外を`_`にする。
pub fn ledger_file_name(host: &str) -> String {
    let safe: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' })
        .collect();
    format!("{safe}.json")
}

// ---- 取り込み(端末側) ----

/// 取り込みの結果。
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct RefreshOutcome {
    pub host: String,
    pub files_read: usize,
    pub bytes_read: u64,
    pub new_responses: usize,
    pub new_messages: usize,
    /// JSONとして読めず飛ばした行の数。
    pub skipped_lines: usize,
}

/// 履歴の1行。使わない中身は、読んでも台帳へ入れない。
#[derive(Deserialize)]
struct HistoryLine {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    payload: Value,
}

/// `codex_home`の履歴のうち、前回から増えた分を、このPCの台帳へ取り込む。
/// 台帳が壊れていて読めない場合は、何も書かずにエラーを返す。
/// `for_server`がtrueのときは、サーバーへ送る準備をする。
pub fn refresh_with(
    codex_home: &Path,
    ledger_dir: &Path,
    host: &str,
    now_ms: u64,
    for_server: bool,
) -> Result<RefreshOutcome, String> {
    let ledger_path = ledger_dir.join(ledger_file_name(host));
    let mut ledger = read_ledger(&ledger_path)?.unwrap_or_else(Ledger::new);
    ledger.host = host.to_string();
    if for_server && !ledger.sync_initialized {
        ledger.dirty_responses = ledger.responses.keys().cloned().collect();
        ledger.dirty_messages = ledger.messages.keys().cloned().collect();
        ledger.dirty_limits = ledger.limits.keys().cloned().collect();
        ledger.dirty_windows = ledger.context_windows.keys().cloned().collect();
        ledger.dirty_unsupported = true;
        ledger.sync_initialized = true;
    }

    let mut outcome = RefreshOutcome { host: host.to_string(), ..RefreshOutcome::default() };
    let mut sessions = ledger.session_lookup();

    let mut files = Vec::new();
    for dir in HISTORY_DIRS {
        collect_jsonl(&codex_home.join(dir), &mut files);
    }
    files.sort();
    // いまある履歴のうち、使用量を読めない形式のもの。
    let mut unsupported = 0u64;
    for path in files {
        let key = path.strip_prefix(codex_home).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let Ok(len) = fs::metadata(&path).map(|m| m.len()) else {
            continue;
        };
        let mut state = ledger.files.get(&key).cloned().unwrap_or_default();
        // 縮んだファイルは書き直されたものとして最初から読む。記録はIDで重複を除くので二重にならない。
        if len < state.offset {
            state = FileState::default();
        }
        if len == state.offset {
            unsupported += u64::from(state.is_unsupported());
            continue;
        }
        let Ok(chunk) = read_from(&path, state.offset) else {
            unsupported += u64::from(state.is_unsupported());
            continue;
        };
        // 書き込み途中の最後の行は、次回に回す。
        let Some(last_newline) = chunk.iter().rposition(|&b| b == b'\n') else {
            unsupported += u64::from(state.is_unsupported());
            continue;
        };
        let complete = &chunk[..=last_newline];
        outcome.files_read += 1;
        outcome.bytes_read += complete.len() as u64;
        for line in complete.split(|&b| b == b'\n') {
            if line.iter().all(|b| b.is_ascii_whitespace()) {
                continue;
            }
            match serde_json::from_slice::<HistoryLine>(line) {
                Ok(parsed) => ingest_line(&mut ledger, &mut sessions, &mut state, parsed, &mut outcome),
                Err(_) => outcome.skipped_lines += 1,
            }
        }
        state.offset += complete.len() as u64;
        unsupported += u64::from(state.is_unsupported());
        ledger.files.insert(key, state);
    }

    if unsupported != ledger.unsupported_files {
        ledger.unsupported_files = unsupported;
        ledger.dirty_unsupported |= ledger.sync_initialized;
    }

    ledger.version = LEDGER_VERSION;
    ledger.updated_at = now_ms;
    write_ledger(&ledger_path, &ledger)?;
    Ok(outcome)
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().is_some_and(|e| e == "jsonl")
            && !entry.file_name().to_string_lossy().starts_with("._")
        {
            out.push(path);
        }
    }
}

fn read_from(path: &Path, offset: u64) -> std::io::Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn number(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn limit_window(v: Option<&Value>) -> Option<LimitWindow> {
    let v = v?;
    Some(LimitWindow {
        used_percent: v.get("used_percent")?.as_f64()?,
        window_minutes: v.get("window_minutes").and_then(Value::as_u64),
        resets_at: v.get("resets_at").and_then(Value::as_i64),
    })
}

fn ingest_line(
    ledger: &mut Ledger,
    sessions: &mut HashMap<String, u32>,
    state: &mut FileState,
    line: HistoryLine,
    outcome: &mut RefreshOutcome,
) {
    let p = &line.payload;
    let at = line
        .timestamp
        .as_deref()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.timestamp());
    let track = ledger.sync_initialized;
    match line.kind.as_str() {
        // 写された会話の`session_meta`が続くことに備え、ファイルの最初のものを使う。
        "session_meta" if state.session_id.is_none() => {
            state.session_id = text(p, "session_id").or_else(|| text(p, "id")).map(str::to_string);
            state.subagent = p.get("source").and_then(|s| s.get("subagent")).is_some();
        }
        "turn_context" => {
            if let Some(model) = text(p, "model") {
                state.last_model = Some(model.to_string());
                if let Some(turn) = text(p, "turn_id") {
                    state.turn_models.insert(turn.to_string(), model.to_string());
                }
            }
        }
        "token_usage_record" => {
            state.has_usage_record = true;
            let (Some(at), Some(id), Some(usage)) = (at, text(p, "response_id"), p.get("usage")) else {
                return;
            };
            let Some(session) = text(p, "session_id").map(str::to_string).or_else(|| state.session_id.clone())
            else {
                return;
            };
            let model = text(p, "turn_id")
                .and_then(|t| state.turn_models.get(t))
                .or(state.last_model.as_ref())
                .cloned()
                .unwrap_or_default();
            let tokens = Tokens {
                input: number(usage, "input_tokens"),
                cached_input: number(usage, "cached_input_tokens"),
                cache_write: number(usage, "cache_write_input_tokens"),
                output: number(usage, "output_tokens"),
                reasoning_output: number(usage, "reasoning_output_tokens"),
            };
            let index = ledger.session_index(&session, sessions);
            let is_new = !ledger.responses.contains_key(id);
            // 写しで同じ応答がもう一度出ても、最初に取り込んだ値を保つ(行き来して送り直さないように)。
            if is_new && ledger.put_response(id, ResponseRecord(at, model, index, tokens, state.subagent)) {
                outcome.new_responses += 1;
                if track {
                    ledger.dirty_responses.insert(id.to_string());
                }
            }
        }
        "event_msg" => match text(p, "type") {
            Some("item_completed") => {
                let Some(item) = p.get("item") else { return };
                if !matches!(text(item, "type"), Some("UserMessage" | "AgentMessage")) {
                    return;
                }
                let (Some(at), Some(id), Some(session)) = (at, text(item, "id"), state.session_id.clone())
                else {
                    return;
                };
                if ledger.messages.contains_key(id) {
                    return;
                }
                let index = ledger.session_index(&session, sessions);
                ledger.messages.insert(id.to_string(), MessageRecord(at, index, state.subagent));
                outcome.new_messages += 1;
                if track {
                    ledger.dirty_messages.insert(id.to_string());
                }
            }
            Some("token_count") => {
                let Some(at) = at else { return };
                if let Some(info) = p.get("info").filter(|i| !i.is_null()) {
                    state.has_token_count = true;
                    let tokens = number(info, "model_context_window");
                    if let (true, Some(model)) = (tokens > 0, state.last_model.clone()) {
                        let window = ContextWindow { model: model.clone(), observed_at: at, tokens };
                        // 大きさが同じでも観測時刻は進める(端末をまたいで、どちらが新しいかを正しく比べるため)。
                        if ledger.put_window(window) && track {
                            ledger.dirty_windows.insert(model);
                        }
                    }
                }
                let Some(limits) = p.get("rate_limits").filter(|r| !r.is_null()) else {
                    return;
                };
                let snapshot = LimitSnapshot {
                    limit_id: text(limits, "limit_id").unwrap_or("codex").to_string(),
                    observed_at: at,
                    plan_type: text(limits, "plan_type").map(str::to_string),
                    primary: limit_window(limits.get("primary")),
                    secondary: limit_window(limits.get("secondary")),
                };
                if snapshot.primary.is_none() && snapshot.secondary.is_none() {
                    return;
                }
                let id = snapshot.limit_id.clone();
                if ledger.put_limit(snapshot) && track {
                    ledger.dirty_limits.insert(id);
                }
            }
            _ => {}
        },
        _ => {}
    }
}

// ---- サーバーへ送る差分 ----
//
// 端末は、台帳のうちまだ送っていない・変わった記録だけを送る。サーバーはIDで上書きして足す
// (同じ差分を2回受け取っても結果は同じ)。レート制限とコンテキスト窓は、観測の新しい方を残す。

/// 差分の形式の版。
pub const DELTA_VERSION: u32 = 1;
/// 差分1回に入れる記録の最大数。1件が約250バイトなので、約2.5MBに収まる(窓口の上限は4MB)。
pub const MAX_DELTA_RECORDS: usize = 10_000;

/// (応答ID, 日時[秒], モデル, セッションID, 量, サブエージェントか)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DeltaResponse(pub String, pub i64, pub String, pub String, pub Tokens, pub bool);

/// (発言ID, 日時[秒], セッションID, サブエージェントか)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DeltaMessage(pub String, pub i64, pub String, pub bool);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Delta {
    pub version: u32,
    pub responses: Vec<DeltaResponse>,
    pub messages: Vec<DeltaMessage>,
    pub limits: Vec<LimitSnapshot>,
    pub context_windows: Vec<ContextWindow>,
    /// 変わったときだけ入れる。
    pub unsupported_files: Option<u64>,
}

impl Delta {
    pub fn len(&self) -> usize {
        self.responses.len()
            + self.messages.len()
            + self.limits.len()
            + self.context_windows.len()
            + usize::from(self.unsupported_files.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// サーバーが差分を取り込んだ結果。
#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct ApplyOutcome {
    pub responses: usize,
    pub messages: usize,
}

/// この端末の台帳から、まだ送っていない記録を最大`max_records`件取り出す。台帳は変えない。
pub fn take_delta(ledger_dir: &Path, host: &str, max_records: usize) -> Result<Delta, String> {
    let mut delta = Delta { version: DELTA_VERSION, ..Delta::default() };
    let Some(ledger) = read_ledger(&ledger_dir.join(ledger_file_name(host)))? else {
        return Ok(delta);
    };
    if !ledger.sync_initialized {
        return Ok(delta);
    }
    for id in &ledger.dirty_responses {
        if delta.len() >= max_records {
            return Ok(delta);
        }
        if let Some(ResponseRecord(at, model, session, tokens, sub)) = ledger.responses.get(id) {
            let session = ledger.session_name(*session).to_string();
            delta.responses.push(DeltaResponse(id.clone(), *at, model.clone(), session, *tokens, *sub));
        }
    }
    for id in &ledger.dirty_messages {
        if delta.len() >= max_records {
            return Ok(delta);
        }
        if let Some(MessageRecord(at, session, sub)) = ledger.messages.get(id) {
            let session = ledger.session_name(*session).to_string();
            delta.messages.push(DeltaMessage(id.clone(), *at, session, *sub));
        }
    }
    for id in &ledger.dirty_limits {
        if delta.len() >= max_records {
            return Ok(delta);
        }
        delta.limits.extend(ledger.limits.get(id).cloned());
    }
    for model in &ledger.dirty_windows {
        if delta.len() >= max_records {
            return Ok(delta);
        }
        delta.context_windows.extend(ledger.context_windows.get(model).cloned());
    }
    if ledger.dirty_unsupported && delta.len() < max_records {
        delta.unsupported_files = Some(ledger.unsupported_files);
    }
    Ok(delta)
}

/// 送れたあとに呼び、送った記録を「送る対象」から外す。送っている間に変わった記録は残す。
pub fn mark_sent(ledger_dir: &Path, host: &str, delta: &Delta) -> Result<(), String> {
    let path = ledger_dir.join(ledger_file_name(host));
    let Some(mut ledger) = read_ledger(&path)? else {
        return Ok(());
    };
    for DeltaResponse(id, at, model, session, tokens, sub) in &delta.responses {
        let same = ledger.responses.get(id).is_some_and(|r| {
            r.0 == *at && r.1 == *model && ledger.session_name(r.2) == session && r.3 == *tokens && r.4 == *sub
        });
        if same {
            ledger.dirty_responses.remove(id);
        }
    }
    for DeltaMessage(id, at, session, sub) in &delta.messages {
        let same = ledger
            .messages
            .get(id)
            .is_some_and(|m| m.0 == *at && ledger.session_name(m.1) == session && m.2 == *sub);
        if same {
            ledger.dirty_messages.remove(id);
        }
    }
    for s in &delta.limits {
        if ledger.limits.get(&s.limit_id) == Some(s) {
            ledger.dirty_limits.remove(&s.limit_id);
        }
    }
    for w in &delta.context_windows {
        if ledger.context_windows.get(&w.model) == Some(w) {
            ledger.dirty_windows.remove(&w.model);
        }
    }
    if delta.unsupported_files == Some(ledger.unsupported_files) {
        ledger.dirty_unsupported = false;
    }
    write_ledger(&path, &ledger)
}

/// サーバー側。端末から届いた差分を、その端末の台帳へ足す。台帳が壊れていれば何も書かない。
pub fn apply_delta(ledger_dir: &Path, host: &str, delta: &Delta, now_ms: u64) -> Result<ApplyOutcome, String> {
    if delta.version != DELTA_VERSION {
        return Err(format!("差分の版が合いません(届いた版: {}、扱える版: {DELTA_VERSION})", delta.version));
    }
    let path = ledger_dir.join(ledger_file_name(host));
    let mut ledger = read_ledger(&path)?.unwrap_or_else(Ledger::new);
    ledger.host = host.to_string();
    let mut sessions = ledger.session_lookup();
    for DeltaResponse(id, at, model, session, tokens, sub) in &delta.responses {
        let s = ledger.session_index(session, &mut sessions);
        ledger.put_response(id, ResponseRecord(*at, model.clone(), s, *tokens, *sub));
    }
    for DeltaMessage(id, at, session, sub) in &delta.messages {
        let s = ledger.session_index(session, &mut sessions);
        ledger.messages.insert(id.clone(), MessageRecord(*at, s, *sub));
    }
    for s in &delta.limits {
        ledger.put_limit(s.clone());
    }
    for w in &delta.context_windows {
        ledger.put_window(w.clone());
    }
    if let Some(n) = delta.unsupported_files {
        ledger.unsupported_files = n;
    }
    ledger.version = LEDGER_VERSION;
    ledger.updated_at = now_ms;
    write_ledger(&path, &ledger)?;
    Ok(ApplyOutcome { responses: delta.responses.len(), messages: delta.messages.len() })
}

/// 端末側。まだ送っていない記録を`max_records`件ずつ`send`で送りきる。`send`が失敗したら止めてエラーを返し、
/// 送れていない記録は送る対象に残る。返すのは送れた記録の数。
pub fn push_pending<F>(ledger_dir: &Path, host: &str, max_records: usize, mut send: F) -> Result<usize, String>
where
    F: FnMut(&Delta) -> Result<(), String>,
{
    let mut sent = 0;
    loop {
        let delta = take_delta(ledger_dir, host, max_records)?;
        if delta.is_empty() {
            return Ok(sent);
        }
        send(&delta)?;
        mark_sent(ledger_dir, host, &delta)?;
        sent += delta.len();
    }
}

// ---- 集計 ----

/// 集計する期間。
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum StatsRange {
    All,
    Days30,
    Days7,
}

impl StatsRange {
    /// サーバーの窓口(`?range=`)に渡す名前。
    pub fn as_query(&self) -> &'static str {
        match self {
            StatsRange::All => "all",
            StatsRange::Days30 => "days30",
            StatsRange::Days7 => "days7",
        }
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub host: String,
    pub updated_at: u64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DailyTokens {
    /// ローカル日付(YYYY-MM-DD)。
    pub date: String,
    pub total: u64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelTokens {
    pub model: String,
    pub total: u64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CodexStats {
    pub hosts: Vec<HostInfo>,
    /// 期間の初日(ローカル日付)。「すべて」のときは最初の記録の日。記録が無ければ今日。
    pub since: String,
    pub until: String,
    pub sessions: usize,
    pub messages: usize,
    pub active_days: usize,
    pub tokens: Tokens,
    pub total_tokens: u64,
    pub daily: Vec<DailyTokens>,
    /// 量の多い順。
    pub models: Vec<ModelTokens>,
    /// レート制限の最新の値。期間と端末の絞り込みは掛けない(アカウントの値なので)。
    pub limits: Vec<LimitSnapshot>,
    /// モデルごとのコンテキスト窓の最新の値。同じく絞り込まない。
    pub context_windows: Vec<ContextWindow>,
    pub warnings: Vec<String>,
}

/// `ledger_dir`の全端末の台帳を合算する。`host`を指定するとその端末だけ。日付の区切りは`tz`で決める。
pub fn summarize<Tz: TimeZone>(
    ledger_dir: &Path,
    range: StatsRange,
    host: Option<&str>,
    now_ms: u64,
    tz: &Tz,
) -> CodexStats {
    let mut warnings = Vec::new();
    let mut ledgers = Vec::new();
    if let Ok(entries) = fs::read_dir(ledger_dir) {
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                name.ends_with(".json") && !name.starts_with("._")
            })
            .collect();
        paths.sort();
        for path in paths {
            match read_ledger(&path) {
                Ok(Some(ledger)) => ledgers.push(ledger),
                Ok(None) => {}
                Err(e) => warnings.push(e),
            }
        }
    }

    let hosts = ledgers.iter().map(|l| HostInfo { host: l.host.clone(), updated_at: l.updated_at }).collect();

    // レート制限とコンテキスト窓は、全端末で観測の新しいものを選ぶ。
    let mut latest = Ledger::new();
    for ledger in &ledgers {
        for s in ledger.limits.values() {
            latest.put_limit(s.clone());
        }
        for w in ledger.context_windows.values() {
            latest.put_window(w.clone());
        }
        if ledger.unsupported_files > 0 {
            warnings.push(format!(
                "{}: 使用量を読めない形式の履歴が{}件あります(数に入っていません)",
                ledger.host, ledger.unsupported_files
            ));
        }
    }

    let local_date = |secs: i64| -> NaiveDate {
        tz.timestamp_opt(secs, 0).single().map(|t| t.date_naive()).unwrap_or(NaiveDate::MIN)
    };
    let today = local_date((now_ms / 1000) as i64);
    let since_limit = match range {
        StatsRange::All => None,
        StatsRange::Days30 => Some(today - chrono::Days::new(29)),
        StatsRange::Days7 => Some(today - chrono::Days::new(6)),
    };
    let in_range = |date: NaiveDate| since_limit.is_none_or(|s| date >= s) && date <= today;

    let mut seen_responses = HashSet::new();
    let mut seen_messages = HashSet::new();
    let mut sessions = HashSet::new();
    let mut days = HashSet::new();
    let mut first_day: Option<NaiveDate> = None;
    let mut tokens = Tokens::default();
    let mut daily: BTreeMap<NaiveDate, u64> = BTreeMap::new();
    let mut models: HashMap<String, u64> = HashMap::new();

    for ledger in ledgers.iter().filter(|l| host.is_none_or(|h| l.host == h)) {
        for (id, MessageRecord(at, session, _)) in &ledger.messages {
            let date = local_date(*at);
            if !in_range(date) || !seen_messages.insert(id.as_str()) {
                continue;
            }
            sessions.insert(ledger.session_name(*session));
            days.insert(date);
            first_day = Some(first_day.map_or(date, |d| d.min(date)));
        }
        for (id, ResponseRecord(at, model, session, t, _)) in &ledger.responses {
            let date = local_date(*at);
            if !in_range(date) || !seen_responses.insert(id.as_str()) {
                continue;
            }
            tokens.add(t);
            *daily.entry(date).or_default() += t.total();
            if !model.is_empty() {
                *models.entry(model.clone()).or_default() += t.total();
            }
            // 発言が無く応答だけのセッションや日も数える。
            sessions.insert(ledger.session_name(*session));
            days.insert(date);
            first_day = Some(first_day.map_or(date, |d| d.min(date)));
        }
    }

    let since = since_limit.or(first_day).unwrap_or(today);
    let mut models: Vec<ModelTokens> =
        models.into_iter().map(|(model, total)| ModelTokens { model, total }).collect();
    models.sort_by(|a, b| b.total.cmp(&a.total).then(a.model.cmp(&b.model)));

    CodexStats {
        hosts,
        since: since.to_string(),
        until: today.to_string(),
        sessions: sessions.len(),
        messages: seen_messages.len(),
        active_days: days.len(),
        tokens,
        total_tokens: tokens.total(),
        daily: daily.into_iter().map(|(d, total)| DailyTokens { date: d.to_string(), total }).collect(),
        models,
        limits: latest.limits.into_values().collect(),
        context_windows: latest.context_windows.into_values().collect(),
        warnings,
    }
}

/// 表示用。このPCのローカル時刻で集計する。
pub fn summarize_local(ledger_dir: &Path, range: StatsRange, host: Option<&str>, now_ms: u64) -> CodexStats {
    summarize(ledger_dir, range, host, now_ms, &Local)
}

#[cfg(test)]
mod tests;
