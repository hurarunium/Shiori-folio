//! Claudeの利用統計(詩織Ver3.9。Ver7.0.1でTauri非依存のクレートに切り出した)。各PCのClaude Codeの履歴(`~/.claude/projects/**/*.jsonl`)
//! を差分で読み、応答単位の台帳を端末(ホスト名)別にSSDの`data/claude-stats/`へ貯め、
//! 全端末の台帳を合算して返す。
//!
//! 履歴には2種類の重複がある(2026-10-07、実物で確認)。
//! - 応答1回が中身のブロックごとに別の行になり、各行に応答全体のusageがそのまま付く。
//! - セッションを再開すると、前の会話が新しいファイルへ写される(IDはそのまま)。
//! そのため「実際の量」は応答ID・発言IDで重複を除いて数える。デスクトップアプリの
//! 統計パネルは行ごとに足しているため、比較用にその値(アプリ式)も別に持つ。
//!
//! 台帳にはプロンプト・応答の中身を入れない(ID・日時・モデル・トークン数・sessionIdのみ)。

use chrono::{DateTime, Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const LEDGER_VERSION: u32 = 1;
/// 中身が0の内部的な応答に付くモデル名。モデル別の集計から除く。
const SYNTHETIC_MODEL: &str = "<synthetic>";

#[derive(Serialize, Deserialize, Clone, Copy, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub input: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    pub output: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.cache_creation + self.cache_read + self.output
    }

    fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.cache_creation += other.cache_creation;
        self.cache_read += other.cache_read;
        self.output += other.output;
    }

    /// 同じ応答の行どうしで、項目ごとに大きい方を採る。書き込み途中の値が
    /// 先の行に残り、出力トークンだけ小さいことがあるため。
    fn max_with(&mut self, other: &Tokens) {
        self.input = self.input.max(other.input);
        self.cache_creation = self.cache_creation.max(other.cache_creation);
        self.cache_read = self.cache_read.max(other.cache_read);
        self.output = self.output.max(other.output);
    }
}

/// 応答1回分。台帳を小さく保つため、JSONでは配列として書く。
/// (日時[秒], モデル, セッション番号, 実際の量, アプリ式の量)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct ResponseRecord(i64, String, u32, Tokens, Tokens);

/// 発言1つ分。(日時[秒], セッション番号, 履歴に出てきた行数)
/// 同じ発言は、会話の圧縮や再開のたびに同じファイル・別のファイルへ書き直される。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct MessageRecord(i64, u32, u32);

/// 端末1台分の台帳。`data/claude-stats/<ホスト名>.json`に置く。
#[derive(Serialize, Deserialize, Default)]
struct Ledger {
    version: u32,
    host: String,
    /// 最後に取り込んだ時刻(ミリ秒)。
    updated_at: u64,
    /// 履歴ファイル(projectsからの相対パス)ごとの、読み終えた位置(バイト)。
    cursors: BTreeMap<String, u64>,
    /// セッションIDの表。応答・発言からは番号で指す。
    sessions: Vec<String>,
    responses: HashMap<String, ResponseRecord>,
    messages: HashMap<String, MessageRecord>,
    /// サーバーへ送る準備ができているか(Ver7.0.1)。サーバー接続で初めて取り込むとき、
    /// これが無ければ、台帳の全記録を`dirty_*`にして(過去の分の引き継ぎ)、trueにする。
    /// falseに戻すと、全部を再送する。
    #[serde(default)]
    sync_initialized: bool,
    /// サーバーへまだ送っていない(または送ったあとに変わった)応答ID・発言ID。
    #[serde(default)]
    dirty_responses: BTreeSet<String>,
    #[serde(default)]
    dirty_messages: BTreeSet<String>,
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

impl Ledger {
    fn session_index(&mut self, session_id: &str, lookup: &mut HashMap<String, u32>) -> u32 {
        if let Some(&i) = lookup.get(session_id) {
            return i;
        }
        let i = self.sessions.len() as u32;
        self.sessions.push(session_id.to_string());
        lookup.insert(session_id.to_string(), i);
        i
    }
}

// 履歴の1行のうち、集計に使う項目だけ。中身(content)は読み飛ばす。
#[derive(Deserialize)]
struct HistoryLine {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "sessionId", default)]
    session_id: Option<String>,
    #[serde(rename = "isSidechain", default)]
    is_sidechain: bool,
    #[serde(default)]
    message: Option<HistoryMessage>,
}

#[derive(Deserialize)]
struct HistoryMessage {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<HistoryUsage>,
}

#[derive(Deserialize)]
struct HistoryUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

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

/// Claude Codeの履歴の置き場所。`CLAUDE_CONFIG_DIR`があればその下、無ければ
/// ホームの`.claude`の下。
pub fn default_projects_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("projects"));
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    Some(PathBuf::from(home).join(".claude").join("projects"))
}

/// 台帳のファイル名に使えるよう、ホスト名の英数字・`-`・`_`・`.`以外を`_`にする。
pub fn ledger_file_name(host: &str) -> String {
    let safe: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' })
        .collect();
    format!("{safe}.json")
}

/// `projects_dir`の履歴のうち、前回から増えた分を、このPCの台帳へ取り込む。
/// 台帳が壊れていて読めない場合は、上書きで取り込み済みの分を失わないよう、
/// 何も書かずにエラーを返す。
pub fn refresh(
    projects_dir: &Path,
    ledger_dir: &Path,
    host: &str,
    now_ms: u64,
) -> Result<RefreshOutcome, String> {
    refresh_with(projects_dir, ledger_dir, host, now_ms, false)
}

/// `refresh`と同じ。`for_server`がtrueのときは、サーバーへ送る準備をする(詩織Ver7.0.1)。
/// 台帳がまだ送る準備をしていなければ、いまある全記録を送る対象にし(過去の分の引き継ぎ)、
/// 以後、新しく入った・変わった記録を送る対象に足していく。
pub fn refresh_with(
    projects_dir: &Path,
    ledger_dir: &Path,
    host: &str,
    now_ms: u64,
    for_server: bool,
) -> Result<RefreshOutcome, String> {
    let ledger_path = ledger_dir.join(ledger_file_name(host));
    let mut ledger = read_ledger(&ledger_path)?.unwrap_or_else(|| Ledger {
        version: LEDGER_VERSION,
        ..Ledger::default()
    });
    ledger.host = host.to_string();
    if for_server && !ledger.sync_initialized {
        ledger.dirty_responses = ledger.responses.keys().cloned().collect();
        ledger.dirty_messages = ledger.messages.keys().cloned().collect();
        ledger.sync_initialized = true;
    }

    let mut outcome = RefreshOutcome {
        host: host.to_string(),
        ..RefreshOutcome::default()
    };
    let mut session_lookup: HashMap<String, u32> = ledger
        .sessions
        .iter()
        .enumerate()
        .map(|(i, s)| (s.clone(), i as u32))
        .collect();

    let mut files = Vec::new();
    collect_jsonl(projects_dir, &mut files);
    files.sort();
    for path in files {
        let key = path
            .strip_prefix(projects_dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(len) = fs::metadata(&path).map(|m| m.len()) else {
            continue;
        };
        let mut offset = ledger.cursors.get(&key).copied().unwrap_or(0);
        // 縮んだファイルは書き直されたものとして最初から読む。応答・発言はIDで
        // 重複を除くので実際の量は変わらないが、アプリ式の量は二重になりうる。
        if len < offset {
            offset = 0;
        }
        if len == offset {
            continue;
        }
        let Ok(chunk) = read_from(&path, offset) else {
            continue;
        };
        // 書き込み途中の最後の行は、次回に回す。
        let Some(last_newline) = chunk.iter().rposition(|&b| b == b'\n') else {
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
                Ok(parsed) => ingest_line(&mut ledger, &mut session_lookup, parsed, &mut outcome),
                Err(_) => outcome.skipped_lines += 1,
            }
        }
        ledger.cursors.insert(key, offset + complete.len() as u64);
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

fn ingest_line(
    ledger: &mut Ledger,
    session_lookup: &mut HashMap<String, u32>,
    line: HistoryLine,
    outcome: &mut RefreshOutcome,
) {
    let (Some(timestamp), Some(session_id)) = (line.timestamp.as_deref(), line.session_id.as_deref())
    else {
        return;
    };
    let Ok(at) = DateTime::parse_from_rfc3339(timestamp).map(|t| t.timestamp()) else {
        return;
    };
    let session = ledger.session_index(session_id, session_lookup);

    // メッセージ数はアプリのパネルと同じく、サブエージェント分を除いたユーザーと
    // Claudeの発言(ツールの結果を返す行もuserとして含む)を数える。
    let is_message = matches!(line.kind.as_str(), "user" | "assistant") && !line.is_sidechain;
    let track = ledger.sync_initialized;
    if let (true, Some(uuid)) = (is_message, line.uuid) {
        if track {
            ledger.dirty_messages.insert(uuid.clone());
        }
        match ledger.messages.get_mut(&uuid) {
            Some(record) => record.2 += 1,
            None => {
                ledger.messages.insert(uuid, MessageRecord(at, session, 1));
                outcome.new_messages += 1;
            }
        }
    }

    let Some(message) = line.message else {
        return;
    };
    let (Some(id), Some(usage)) = (message.id, message.usage) else {
        return;
    };
    let tokens = Tokens {
        input: usage.input_tokens,
        cache_creation: usage.cache_creation_input_tokens,
        cache_read: usage.cache_read_input_tokens,
        output: usage.output_tokens,
    };
    if track {
        ledger.dirty_responses.insert(id.clone());
    }
    match ledger.responses.get_mut(&id) {
        Some(record) => {
            record.0 = record.0.min(at);
            record.3.max_with(&tokens);
            record.4.add(&tokens);
        }
        None => {
            let model = message.model.unwrap_or_default();
            ledger
                .responses
                .insert(id, ResponseRecord(at, model, session, tokens, tokens));
            outcome.new_responses += 1;
        }
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

// ---- サーバーへ送る差分(詩織Ver7.0.1) ----
//
// 端末は、履歴を読んで台帳を作る(上の`refresh_with`)。サーバーへは、台帳のうち
// まだ送っていない・変わった記録だけを、差分として送る。サーバーは、IDで上書きして
// 足す(同じ差分を2回受け取っても結果は同じ)。台帳の全体を送らないのは、台帳が
// 数MBになり、窓口の本文の上限(4MB)に近いため。

/// 差分の形式の版。
pub const DELTA_VERSION: u32 = 1;
/// 差分1回に入れる記録の最大数。1件が約150バイトなので、約1.5MBに収まる。
pub const MAX_DELTA_RECORDS: usize = 10_000;

/// 応答1回分。(応答ID, 日時[秒], モデル, セッションID, 実際の量, アプリ式の量)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DeltaResponse(pub String, pub i64, pub String, pub String, pub Tokens, pub Tokens);

/// 発言1つ分。(発言ID, 日時[秒], セッションID, 履歴に出てきた行数)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DeltaMessage(pub String, pub i64, pub String, pub u32);

/// サーバーへ送る差分。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Delta {
    pub version: u32,
    pub responses: Vec<DeltaResponse>,
    pub messages: Vec<DeltaMessage>,
}

impl Delta {
    pub fn len(&self) -> usize {
        self.responses.len() + self.messages.len()
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

/// この端末の台帳から、まだ送っていない記録を、最大`max_records`件取り出す。
/// 送る準備ができていない台帳(`refresh_with`を`for_server`で動かしていない)や、
/// 台帳が無いときは、空の差分を返す。台帳は変えない。
pub fn take_delta(ledger_dir: &Path, host: &str, max_records: usize) -> Result<Delta, String> {
    let path = ledger_dir.join(ledger_file_name(host));
    let mut delta = Delta {
        version: DELTA_VERSION,
        ..Delta::default()
    };
    let Some(ledger) = read_ledger(&path)? else {
        return Ok(delta);
    };
    if !ledger.sync_initialized {
        return Ok(delta);
    }
    let session_of = |i: u32| ledger.sessions.get(i as usize).cloned().unwrap_or_default();
    for id in &ledger.dirty_responses {
        if delta.len() >= max_records {
            return Ok(delta);
        }
        if let Some(ResponseRecord(at, model, session, actual, app)) = ledger.responses.get(id) {
            delta
                .responses
                .push(DeltaResponse(id.clone(), *at, model.clone(), session_of(*session), *actual, *app));
        }
    }
    for id in &ledger.dirty_messages {
        if delta.len() >= max_records {
            return Ok(delta);
        }
        if let Some(MessageRecord(at, session, lines)) = ledger.messages.get(id) {
            delta
                .messages
                .push(DeltaMessage(id.clone(), *at, session_of(*session), *lines));
        }
    }
    Ok(delta)
}

/// 差分をサーバーへ送れたあとに呼び、送った記録を「送る対象」から外す。
/// 送っている間に記録が変わっていたもの(値が送ったものと違うもの)は、外さずに残す。
pub fn mark_sent(ledger_dir: &Path, host: &str, delta: &Delta) -> Result<(), String> {
    let path = ledger_dir.join(ledger_file_name(host));
    let Some(mut ledger) = read_ledger(&path)? else {
        return Ok(());
    };
    for DeltaResponse(id, at, model, session, actual, app) in &delta.responses {
        let same = ledger.responses.get(id).is_some_and(|r| {
            let current_session = ledger.sessions.get(r.2 as usize).map(String::as_str).unwrap_or("");
            r.0 == *at && r.1 == *model && current_session == session && r.3 == *actual && r.4 == *app
        });
        if same {
            ledger.dirty_responses.remove(id);
        }
    }
    for DeltaMessage(id, at, session, lines) in &delta.messages {
        let same = ledger.messages.get(id).is_some_and(|m| {
            let current_session = ledger.sessions.get(m.1 as usize).map(String::as_str).unwrap_or("");
            m.0 == *at && current_session == session && m.2 == *lines
        });
        if same {
            ledger.dirty_messages.remove(id);
        }
    }
    write_ledger(&path, &ledger)
}

/// サーバー側。端末から届いた差分を、その端末の台帳へ、IDで上書きして足す。
/// 台帳が壊れていて読めないときは、何も書かずにエラーを返す。
pub fn apply_delta(ledger_dir: &Path, host: &str, delta: &Delta, now_ms: u64) -> Result<ApplyOutcome, String> {
    if delta.version != DELTA_VERSION {
        return Err(format!("差分の版が合いません(届いた版: {}、扱える版: {DELTA_VERSION})", delta.version));
    }
    let path = ledger_dir.join(ledger_file_name(host));
    let mut ledger = read_ledger(&path)?.unwrap_or_else(|| Ledger {
        version: LEDGER_VERSION,
        ..Ledger::default()
    });
    ledger.host = host.to_string();
    let mut lookup: HashMap<String, u32> = ledger
        .sessions
        .iter()
        .enumerate()
        .map(|(i, s)| (s.clone(), i as u32))
        .collect();
    for DeltaResponse(id, at, model, session, actual, app) in &delta.responses {
        let s = ledger.session_index(session, &mut lookup);
        ledger
            .responses
            .insert(id.clone(), ResponseRecord(*at, model.clone(), s, *actual, *app));
    }
    for DeltaMessage(id, at, session, lines) in &delta.messages {
        let s = ledger.session_index(session, &mut lookup);
        ledger.messages.insert(id.clone(), MessageRecord(*at, s, *lines));
    }
    ledger.version = LEDGER_VERSION;
    ledger.updated_at = now_ms;
    write_ledger(&path, &ledger)?;
    Ok(ApplyOutcome {
        responses: delta.responses.len(),
        messages: delta.messages.len(),
    })
}

/// 端末側。まだ送っていない記録を、`max_records`件ずつ`send`で送りきる(1回ごとに、送れた分を送る対象から外す)。
/// `send`が失敗したら、そこで止めてエラーを返す。送れていない記録は、送る対象に残る(次回に送り直す)。
/// 返すのは、送れた記録の数。
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
    pub actual: u64,
    pub app: u64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelTokens {
    pub model: String,
    pub actual: u64,
    pub app: u64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeStats {
    /// 台帳のある端末(絞り込みの選択肢)。
    pub hosts: Vec<HostInfo>,
    /// 期間の初日(ローカル日付)。「すべて」のときは最初の記録の日。記録が無ければ今日。
    pub since: String,
    pub until: String,
    pub sessions: usize,
    pub messages: usize,
    /// アプリ式のメッセージ数(履歴の行ごとに数えた値)。
    pub messages_app: u64,
    pub active_days: usize,
    pub actual: Tokens,
    pub app: Tokens,
    /// 期間内で記録のある日だけ、日付順。
    pub daily: Vec<DailyTokens>,
    /// 実際の量の多い順。
    pub models: Vec<ModelTokens>,
    /// 読めなかった台帳など。
    pub warnings: Vec<String>,
}

/// `ledger_dir`の全端末の台帳を合算する。`host`を指定するとその端末だけ。
/// 日付の区切りは`tz`(通常はこのPCのローカル時刻)で決める。
pub fn summarize<Tz: TimeZone>(
    ledger_dir: &Path,
    range: StatsRange,
    host: Option<&str>,
    now_ms: u64,
    tz: &Tz,
) -> ClaudeStats {
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
            match fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| serde_json::from_str::<Ledger>(&t).map_err(|e| e.to_string()))
            {
                Ok(ledger) => ledgers.push(ledger),
                Err(e) => warnings.push(format!(
                    "{}を読めませんでした: {e}",
                    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                )),
            }
        }
    }

    let hosts = ledgers
        .iter()
        .map(|l| HostInfo {
            host: l.host.clone(),
            updated_at: l.updated_at,
        })
        .collect();

    let local_date = |secs: i64| -> NaiveDate {
        tz.timestamp_opt(secs, 0)
            .single()
            .map(|t| t.date_naive())
            .unwrap_or(NaiveDate::MIN)
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
    let mut messages_app = 0u64;
    let mut sessions = HashSet::new();
    let mut days = HashSet::new();
    let mut first_day: Option<NaiveDate> = None;
    let mut actual = Tokens::default();
    let mut app = Tokens::default();
    let mut daily: BTreeMap<NaiveDate, (u64, u64)> = BTreeMap::new();
    let mut models: HashMap<String, (u64, u64)> = HashMap::new();

    for ledger in ledgers.iter().filter(|l| host.is_none_or(|h| l.host == h)) {
        let session_of = |i: u32| ledger.sessions.get(i as usize).map(String::as_str).unwrap_or("");
        for (uuid, MessageRecord(at, session, lines)) in &ledger.messages {
            let date = local_date(*at);
            if !in_range(date) || !seen_messages.insert(uuid.as_str()) {
                continue;
            }
            messages_app += u64::from(*lines);
            sessions.insert(session_of(*session));
            days.insert(date);
            first_day = Some(first_day.map_or(date, |d| d.min(date)));
        }
        for (id, ResponseRecord(at, model, _, act, app_tokens)) in &ledger.responses {
            let date = local_date(*at);
            if !in_range(date) || !seen_responses.insert(id.as_str()) {
                continue;
            }
            actual.add(act);
            app.add(app_tokens);
            let day = daily.entry(date).or_default();
            day.0 += act.total();
            day.1 += app_tokens.total();
            if model != SYNTHETIC_MODEL && !model.is_empty() {
                let m = models.entry(model.clone()).or_default();
                m.0 += act.total();
                m.1 += app_tokens.total();
            }
            first_day = Some(first_day.map_or(date, |d| d.min(date)));
        }
    }

    let since = since_limit.or(first_day).unwrap_or(today);
    let mut models: Vec<ModelTokens> = models
        .into_iter()
        .map(|(model, (actual, app))| ModelTokens { model, actual, app })
        .collect();
    models.sort_by(|a, b| b.actual.cmp(&a.actual).then(a.model.cmp(&b.model)));

    ClaudeStats {
        hosts,
        since: since.to_string(),
        until: today.to_string(),
        sessions: sessions.len(),
        messages: seen_messages.len(),
        messages_app,
        active_days: days.len(),
        actual,
        app,
        daily: daily
            .into_iter()
            .map(|(date, (actual, app))| DailyTokens {
                date: date.to_string(),
                actual,
                app,
            })
            .collect(),
        models,
        warnings,
    }
}

/// 表示用。このPCのローカル時刻で集計する。
pub fn summarize_local(ledger_dir: &Path, range: StatsRange, host: Option<&str>, now_ms: u64) -> ClaudeStats {
    summarize(ledger_dir, range, host, now_ms, &Local)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    // 2026-10-07 12:00:00 JST
    const NOW_MS: u64 = 1_791_342_000_000;

    fn jst() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).unwrap()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("shiori-claude-stats-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn user(uuid: &str, session: &str, ts: &str) -> String {
        format!(
            r#"{{"type":"user","uuid":"{uuid}","sessionId":"{session}","timestamp":"{ts}","isSidechain":false,"message":{{"role":"user","content":"秘密の質問"}}}}"#
        )
    }

    fn assistant(uuid: &str, session: &str, ts: &str, id: &str, model: &str, out: u64, sidechain: bool) -> String {
        format!(
            r#"{{"type":"assistant","uuid":"{uuid}","sessionId":"{session}","timestamp":"{ts}","isSidechain":{sidechain},"message":{{"id":"{id}","model":"{model}","content":[{{"type":"text","text":"秘密の答え"}}],"usage":{{"input_tokens":1,"cache_creation_input_tokens":10,"cache_read_input_tokens":100,"output_tokens":{out}}}}}}}"#
        )
    }

    fn append(path: &Path, lines: &[String]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    fn summary(ledgers: &Path, range: StatsRange, host: Option<&str>) -> ClaudeStats {
        summarize(ledgers, range, host, NOW_MS, &jst())
    }

    #[test]
    fn 同じ応答の重複行は実際の量では1回だけ数える() {
        let root = temp_dir("dup");
        let (projects, ledgers) = (root.join("projects"), root.join("ledgers"));
        let ts = "2026-10-07T01:00:00Z";
        append(
            &projects.join("p/s1.jsonl"),
            &[
                user("u1", "s1", ts),
                assistant("a1", "s1", ts, "msg1", "claude-opus-5-5", 5, false),
                assistant("a2", "s1", ts, "msg1", "claude-opus-5-5", 5, false),
                assistant("a3", "s1", ts, "msg1", "claude-opus-5-5", 5, false),
            ],
        );
        refresh(&projects, &ledgers, "pc1", NOW_MS).unwrap();
        let s = summary(&ledgers, StatsRange::All, None);
        assert_eq!(s.actual, Tokens { input: 1, cache_creation: 10, cache_read: 100, output: 5 });
        assert_eq!(s.app.total(), 3 * 116);
        assert_eq!((s.sessions, s.messages, s.active_days), (1, 4, 1));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 行ごとに出力が違う応答は最大値を採る() {
        let root = temp_dir("max");
        let (projects, ledgers) = (root.join("projects"), root.join("ledgers"));
        let ts = "2026-10-07T01:00:00Z";
        append(
            &projects.join("p/s1.jsonl"),
            &[
                assistant("a1", "s1", ts, "msg1", "m", 4, false),
                assistant("a2", "s1", ts, "msg1", "m", 85, false),
            ],
        );
        refresh(&projects, &ledgers, "pc1", NOW_MS).unwrap();
        let s = summary(&ledgers, StatsRange::All, None);
        assert_eq!(s.actual.output, 85);
        assert_eq!(s.app.output, 89);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 別ファイルへの写しは二重に数えない() {
        let root = temp_dir("copy");
        let (projects, ledgers) = (root.join("projects"), root.join("ledgers"));
        let ts = "2026-10-07T01:00:00Z";
        let original = [user("u1", "s1", ts), assistant("a1", "s1", ts, "msg1", "m", 5, false)];
        append(&projects.join("p/s1.jsonl"), &original);
        // 再開したセッションのファイルには、前の会話が元のIDのまま写される。
        let mut resumed = original.to_vec();
        resumed.push(user("u2", "s2", ts));
        append(&projects.join("p/s2.jsonl"), &resumed);
        refresh(&projects, &ledgers, "pc1", NOW_MS).unwrap();
        let s = summary(&ledgers, StatsRange::All, None);
        assert_eq!(s.actual.total(), 116);
        assert_eq!(s.app.total(), 2 * 116);
        assert_eq!((s.sessions, s.messages, s.messages_app), (2, 3, 5));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn サブエージェントの発言はメッセージに数えずトークンは数える() {
        let root = temp_dir("side");
        let (projects, ledgers) = (root.join("projects"), root.join("ledgers"));
        let ts = "2026-10-07T01:00:00Z";
        append(
            &projects.join("p/s1/subagents/agent-1.jsonl"),
            &[assistant("a1", "s1", ts, "msg1", "m", 5, true)],
        );
        refresh(&projects, &ledgers, "pc1", NOW_MS).unwrap();
        let s = summary(&ledgers, StatsRange::All, None);
        assert_eq!(s.messages, 0);
        assert_eq!(s.actual.total(), 116);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 繰り返し取り込んでも増えた分だけを足す() {
        let root = temp_dir("incr");
        let (projects, ledgers) = (root.join("projects"), root.join("ledgers"));
        let file = projects.join("p/s1.jsonl");
        append(&file, &[assistant("a1", "s1", "2026-10-07T01:00:00Z", "msg1", "m", 5, false)]);
        refresh(&projects, &ledgers, "pc1", NOW_MS).unwrap();
        let second = refresh(&projects, &ledgers, "pc1", NOW_MS).unwrap();
        assert_eq!((second.files_read, second.new_responses), (0, 0));

        append(&file, &[assistant("a2", "s1", "2026-10-07T02:00:00Z", "msg2", "m", 5, false)]);
        let third = refresh(&projects, &ledgers, "pc1", NOW_MS).unwrap();
        assert_eq!(third.new_responses, 1);
        let s = summary(&ledgers, StatsRange::All, None);
        assert_eq!((s.actual.total(), s.app.total()), (232, 232));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 書き込み途中の最後の行は次回に回す() {
        let root = temp_dir("partial");
        let (projects, ledgers) = (root.join("projects"), root.join("ledgers"));
        let file = projects.join("p/s1.jsonl");
        let line = assistant("a1", "s1", "2026-10-07T01:00:00Z", "msg1", "m", 5, false);
        let (head, tail) = line.split_at(40);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, head).unwrap();
        refresh(&projects, &ledgers, "pc1", NOW_MS).unwrap();
        assert_eq!(summary(&ledgers, StatsRange::All, None).actual.total(), 0);

        let mut f = fs::OpenOptions::new().append(true).open(&file).unwrap();
        writeln!(f, "{tail}").unwrap();
        refresh(&projects, &ledgers, "pc1", NOW_MS).unwrap();
        assert_eq!(summary(&ledgers, StatsRange::All, None).actual.total(), 116);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 履歴が消えても台帳の分は残る() {
        let root = temp_dir("deleted");
        let (projects, ledgers) = (root.join("projects"), root.join("ledgers"));
        let file = projects.join("p/s1.jsonl");
        append(&file, &[assistant("a1", "s1", "2026-10-07T01:00:00Z", "msg1", "m", 5, false)]);
        refresh(&projects, &ledgers, "pc1", NOW_MS).unwrap();
        fs::remove_file(&file).unwrap();
        refresh(&projects, &ledgers, "pc1", NOW_MS).unwrap();
        assert_eq!(summary(&ledgers, StatsRange::All, None).actual.total(), 116);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 壊れた行は飛ばし_中身は台帳に入らない() {
        let root = temp_dir("broken");
        let (projects, ledgers) = (root.join("projects"), root.join("ledgers"));
        let ts = "2026-10-07T01:00:00Z";
        append(
            &projects.join("p/s1.jsonl"),
            &["{not json".to_string(), user("u1", "s1", ts), assistant("a1", "s1", ts, "msg1", "m", 5, false)],
        );
        let outcome = refresh(&projects, &ledgers, "pc1", NOW_MS).unwrap();
        assert_eq!(outcome.skipped_lines, 1);
        let text = fs::read_to_string(ledgers.join("pc1.json")).unwrap();
        assert!(!text.contains("秘密"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 壊れた台帳は上書きせずエラーにする() {
        let root = temp_dir("badledger");
        let (projects, ledgers) = (root.join("projects"), root.join("ledgers"));
        fs::create_dir_all(&ledgers).unwrap();
        fs::write(ledgers.join("pc1.json"), "{broken").unwrap();
        assert!(refresh(&projects, &ledgers, "pc1", NOW_MS).is_err());
        assert_eq!(fs::read_to_string(ledgers.join("pc1.json")).unwrap(), "{broken");
        let s = summary(&ledgers, StatsRange::All, None);
        assert_eq!(s.warnings.len(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 端末を合算し_端末で絞り込める() {
        let root = temp_dir("hosts");
        let ledgers = root.join("ledgers");
        let ts = "2026-10-07T01:00:00Z";
        let (win, mac) = (root.join("win"), root.join("mac"));
        append(&win.join("p/s1.jsonl"), &[assistant("a1", "s1", ts, "msg1", "opus", 5, false)]);
        append(&mac.join("p/s2.jsonl"), &[assistant("a2", "s2", ts, "msg2", "sonnet", 5, false)]);
        refresh(&win, &ledgers, "win-pc", NOW_MS).unwrap();
        refresh(&mac, &ledgers, "mac book", NOW_MS).unwrap();
        assert!(ledgers.join("mac_book.json").is_file());

        let all = summary(&ledgers, StatsRange::All, None);
        assert_eq!((all.hosts.len(), all.sessions, all.actual.total()), (2, 2, 232));
        assert_eq!(all.models.len(), 2);
        let mac_only = summary(&ledgers, StatsRange::All, Some("mac book"));
        assert_eq!((mac_only.sessions, mac_only.models[0].model.as_str()), (1, "sonnet"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 期間で絞り込み_日付はローカル時刻で区切る() {
        let root = temp_dir("range");
        let (projects, ledgers) = (root.join("projects"), root.join("ledgers"));
        append(
            &projects.join("p/s1.jsonl"),
            &[
                // UTCでは10/06だが、JSTでは10/07の朝。
                assistant("a1", "s1", "2026-10-06T23:30:00Z", "msg1", "m", 5, false),
                assistant("a2", "s1", "2026-09-20T01:00:00Z", "msg2", "m", 5, false),
                assistant("a3", "s1", "2026-08-01T01:00:00Z", "msg3", "m", 5, false),
                assistant("a4", "s1", "2026-08-01T01:00:00Z", "msg4", "<synthetic>", 0, false),
            ],
        );
        refresh(&projects, &ledgers, "pc1", NOW_MS).unwrap();
        let all = summary(&ledgers, StatsRange::All, None);
        assert_eq!((all.active_days, all.since.as_str(), all.until.as_str()), (3, "2026-08-01", "2026-10-07"));
        assert_eq!(all.models.len(), 1);
        let d30 = summary(&ledgers, StatsRange::Days30, None);
        assert_eq!((d30.active_days, d30.since.as_str()), (2, "2026-09-08"));
        let d7 = summary(&ledgers, StatsRange::Days7, None);
        assert_eq!(d7.daily.iter().map(|d| d.date.as_str()).collect::<Vec<_>>(), vec!["2026-10-07"]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 台帳が無ければ空で返す() {
        let s = summary(Path::new("/nonexistent-shiori-stats"), StatsRange::All, None);
        assert_eq!((s.sessions, s.messages, s.actual.total()), (0, 0, 0));
        assert!(s.warnings.is_empty());
    }

    /// 実物の履歴を一時フォルダの台帳へ取り込み、数字を表示する(手動確認用)。
    /// `cargo test real_history -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_history() {
        let projects = default_projects_dir().unwrap();
        let ledgers = temp_dir("real");
        let started = std::time::Instant::now();
        let outcome = refresh(&projects, &ledgers, "real", NOW_MS).unwrap();
        let first = started.elapsed();
        let started = std::time::Instant::now();
        refresh(&projects, &ledgers, "real", NOW_MS).unwrap();
        let second = started.elapsed();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        let s = summarize_local(&ledgers, StatsRange::All, None, now);
        let size = fs::metadata(ledgers.join("real.json")).unwrap().len();
        println!("{outcome:?}");
        println!("初回 {first:?} / 2回目 {second:?} / 台帳 {size} バイト");
        println!(
            "セッション {} メッセージ {}(アプリ式 {}) アクティブ日数 {} 実際 {} アプリ式 {}",
            s.sessions,
            s.messages,
            s.messages_app,
            s.active_days,
            s.actual.total(),
            s.app.total()
        );
        println!("{:?}", s.models);
        fs::remove_dir_all(&ledgers).unwrap();
    }

    // ---- サーバーへ送る差分(Ver7.0.1) ----

    /// 端末の台帳の、送っていない記録を、`max`件ずつサーバーの台帳へ送りきる(アプリの送信と同じ流れ)。
    /// 送った差分の数を返す。
    fn send_all(client: &Path, server: &Path, host: &str, max: usize) -> Vec<Delta> {
        let mut sent = Vec::new();
        loop {
            let delta = take_delta(client, host, max).unwrap();
            if delta.is_empty() {
                return sent;
            }
            apply_delta(server, host, &delta, NOW_MS).unwrap();
            mark_sent(client, host, &delta).unwrap();
            sent.push(delta);
        }
    }

    fn history() -> Vec<String> {
        let ts = "2026-10-07T01:00:00Z";
        vec![
            user("u1", "s1", ts),
            assistant("a1", "s1", ts, "msg1", "claude-opus-5-5", 5, false),
            user("u2", "s2", ts),
            assistant("a2", "s2", ts, "msg2", "claude-sonnet-5-5", 7, false),
        ]
    }

    #[test]
    fn 送る準備をすると既存の全記録が差分になる_過去の引き継ぎ() {
        let root = temp_dir("sync-init");
        let (projects, client) = (root.join("projects"), root.join("client"));
        append(&projects.join("p/s1.jsonl"), &history());
        // サーバー接続の前に、手元だけで取り込んだ台帳(Ver7.0までの台帳)。
        refresh(&projects, &client, "pc1", NOW_MS).unwrap();
        assert!(take_delta(&client, "pc1", 100).unwrap().is_empty(), "準備前は送るものが無い");
        // サーバー接続で初めて取り込むと、過去の分も含めて、全部が送る対象になる。
        refresh_with(&projects, &client, "pc1", NOW_MS, true).unwrap();
        let delta = take_delta(&client, "pc1", 100).unwrap();
        assert_eq!((delta.responses.len(), delta.messages.len()), (2, 4));
        assert_eq!(delta.version, DELTA_VERSION);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 台帳が無いときや準備前は空の差分() {
        let root = temp_dir("sync-none");
        assert!(take_delta(&root.join("none"), "pc1", 100).unwrap().is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 差分を取り込んだサーバーの集計は端末の台帳の集計と同じ() {
        let root = temp_dir("sync-same");
        let (projects, client, server) = (root.join("projects"), root.join("client"), root.join("server"));
        append(&projects.join("p/s1.jsonl"), &history());
        refresh_with(&projects, &client, "pc1", NOW_MS, true).unwrap();
        send_all(&client, &server, "pc1", 100);
        for range in [StatsRange::All, StatsRange::Days30, StatsRange::Days7] {
            let a = summary(&client, range, None);
            let b = summary(&server, range, None);
            assert_eq!(serde_json::to_string(&a.daily).unwrap(), serde_json::to_string(&b.daily).unwrap());
            assert_eq!(serde_json::to_string(&a.models).unwrap(), serde_json::to_string(&b.models).unwrap());
            assert_eq!((a.sessions, a.messages, a.messages_app), (b.sessions, b.messages, b.messages_app));
            assert_eq!((a.actual, a.app), (b.actual, b.app));
        }
        assert!(take_delta(&client, "pc1", 100).unwrap().is_empty(), "送ったあとは送るものが無い");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 同じ差分を2回取り込んでも結果は同じ() {
        let root = temp_dir("sync-idem");
        let (projects, client, server) = (root.join("projects"), root.join("client"), root.join("server"));
        append(&projects.join("p/s1.jsonl"), &history());
        refresh_with(&projects, &client, "pc1", NOW_MS, true).unwrap();
        let delta = take_delta(&client, "pc1", 100).unwrap();
        apply_delta(&server, "pc1", &delta, NOW_MS).unwrap();
        // 台帳の並び順(HashMap)は取り込みのたびに変わりうるので、文字列ではなくJSONの値で比べる。
        let read = || serde_json::from_str::<serde_json::Value>(&fs::read_to_string(server.join("pc1.json")).unwrap()).unwrap();
        let once = read();
        apply_delta(&server, "pc1", &delta, NOW_MS).unwrap();
        assert_eq!(once, read());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 後から行が増えた応答は最新の値で上書きされ_変わらない記録は送り直さない() {
        let root = temp_dir("sync-update");
        let (projects, client, server) = (root.join("projects"), root.join("client"), root.join("server"));
        let file = projects.join("p/s1.jsonl");
        append(&file, &history());
        refresh_with(&projects, &client, "pc1", NOW_MS, true).unwrap();
        send_all(&client, &server, "pc1", 100);
        // 応答msg1の行が増え(出力が大きい値になり)、新しい発言も入る。
        let ts = "2026-10-07T01:00:00Z";
        append(
            &file,
            &[assistant("a1b", "s1", ts, "msg1", "claude-opus-5-5", 90, false), user("u3", "s1", ts)],
        );
        refresh_with(&projects, &client, "pc1", NOW_MS, true).unwrap();
        let sent = send_all(&client, &server, "pc1", 100);
        assert_eq!(sent.len(), 1);
        let ids: Vec<&str> = sent[0].responses.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(ids, vec!["msg1"], "変わった応答だけを送る");
        let mids: Vec<&str> = sent[0].messages.iter().map(|m| m.0.as_str()).collect();
        assert_eq!(mids, vec!["a1b", "u3"], "新しい発言だけを送る(変わらない発言は送らない)");
        assert_eq!(summary(&server, StatsRange::All, None).actual.output, 90 + 7);
        assert_eq!(
            serde_json::to_string(&summary(&client, StatsRange::All, None).daily).unwrap(),
            serde_json::to_string(&summary(&server, StatsRange::All, None).daily).unwrap()
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 送っている間に変わった記録は送った扱いにしない() {
        let root = temp_dir("sync-race");
        let (projects, client, server) = (root.join("projects"), root.join("client"), root.join("server"));
        let file = projects.join("p/s1.jsonl");
        append(&file, &history());
        refresh_with(&projects, &client, "pc1", NOW_MS, true).unwrap();
        let delta = take_delta(&client, "pc1", 100).unwrap();
        // 差分を作ったあと、送り終える前に、同じ応答の行が増える。
        let ts = "2026-10-07T01:00:00Z";
        append(&file, &[assistant("a1b", "s1", ts, "msg1", "claude-opus-5-5", 90, false)]);
        refresh_with(&projects, &client, "pc1", NOW_MS, true).unwrap();
        apply_delta(&server, "pc1", &delta, NOW_MS).unwrap();
        mark_sent(&client, "pc1", &delta).unwrap();
        let rest = take_delta(&client, "pc1", 100).unwrap();
        let ids: Vec<&str> = rest.responses.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(ids, vec!["msg1"], "変わったmsg1は残る");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 差分は指定の件数に分けて送り_全部そろえば同じ集計になる() {
        let root = temp_dir("sync-chunk");
        let (projects, client, server) = (root.join("projects"), root.join("client"), root.join("server"));
        append(&projects.join("p/s1.jsonl"), &history());
        refresh_with(&projects, &client, "pc1", NOW_MS, true).unwrap();
        let sent = send_all(&client, &server, "pc1", 2);
        assert_eq!(sent.len(), 3, "6件(応答2+発言4)を2件ずつ");
        assert!(sent.iter().all(|d| d.len() <= 2));
        let (a, b) = (summary(&client, StatsRange::All, None), summary(&server, StatsRange::All, None));
        assert_eq!((a.sessions, a.messages, a.messages_app, a.actual), (b.sessions, b.messages, b.messages_app, b.actual));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 送信に失敗したら送る対象が残り_次回に送り直せる() {
        let root = temp_dir("sync-retry");
        let (projects, client, server) = (root.join("projects"), root.join("client"), root.join("server"));
        append(&projects.join("p/s1.jsonl"), &history());
        refresh_with(&projects, &client, "pc1", NOW_MS, true).unwrap();
        // 送ろうとして失敗した(mark_sentを呼ばない)。
        let _ = take_delta(&client, "pc1", 100).unwrap();
        assert_eq!(take_delta(&client, "pc1", 100).unwrap().len(), 6);
        send_all(&client, &server, "pc1", 100);
        assert_eq!(summary(&server, StatsRange::All, None).messages, 4);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 全端末の台帳をサーバーで合算し_端末をまたぐ重複は1回だけ数える() {
        let root = temp_dir("sync-hosts");
        let (server, c1, c2) = (root.join("server"), root.join("c1"), root.join("c2"));
        let (p1, p2) = (root.join("p1"), root.join("p2"));
        append(&p1.join("p/s1.jsonl"), &history());
        // 2台目には、同じ会話の写し(同じID)と、自分だけの発言がある。
        let mut second = history();
        second.push(user("u9", "s9", "2026-10-07T02:00:00Z"));
        append(&p2.join("p/s1.jsonl"), &second);
        refresh_with(&p1, &c1, "pc1", NOW_MS, true).unwrap();
        refresh_with(&p2, &c2, "pc2", NOW_MS, true).unwrap();
        send_all(&c1, &server, "pc1", 100);
        send_all(&c2, &server, "pc2", 100);
        let s = summary(&server, StatsRange::All, None);
        assert_eq!(s.hosts.len(), 2);
        assert_eq!(s.messages, 5);
        assert_eq!(s.actual.output, 5 + 7);
        assert_eq!(summary(&server, StatsRange::All, Some("pc2")).messages, 5);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 壊れた台帳は上書きせず_版が違う差分は断る() {
        let root = temp_dir("sync-bad");
        let (projects, client, server) = (root.join("projects"), root.join("client"), root.join("server"));
        append(&projects.join("p/s1.jsonl"), &history());
        refresh_with(&projects, &client, "pc1", NOW_MS, true).unwrap();
        let delta = take_delta(&client, "pc1", 100).unwrap();
        fs::create_dir_all(&server).unwrap();
        fs::write(server.join("pc1.json"), "壊れている").unwrap();
        assert!(apply_delta(&server, "pc1", &delta, NOW_MS).unwrap_err().contains("壊れて"));
        assert_eq!(fs::read_to_string(server.join("pc1.json")).unwrap(), "壊れている", "書き換えない");
        let old = Delta { version: DELTA_VERSION + 1, ..Delta::default() };
        assert!(apply_delta(&root.join("other"), "pc1", &old, NOW_MS).unwrap_err().contains("版"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn push_pendingは送りきり_失敗したら残りを送る対象に残す() {
        let root = temp_dir("sync-push");
        let (projects, client, server) = (root.join("projects"), root.join("client"), root.join("server"));
        append(&projects.join("p/s1.jsonl"), &history());
        refresh_with(&projects, &client, "pc1", NOW_MS, true).unwrap();
        // 2件ずつ送り、2回目で失敗する。
        let mut calls = 0;
        let r = push_pending(&client, "pc1", 2, |d| {
            calls += 1;
            if calls == 2 {
                return Err("つながらない".into());
            }
            apply_delta(&server, "pc1", d, NOW_MS).map(|_| ())
        });
        assert_eq!(r.unwrap_err(), "つながらない");
        assert_eq!(take_delta(&client, "pc1", 100).unwrap().len(), 4, "6件のうち、送れた2件だけが外れる");
        // 直ったあと、残りだけを送りきる。
        let n = push_pending(&client, "pc1", 2, |d| apply_delta(&server, "pc1", d, NOW_MS).map(|_| ())).unwrap();
        assert_eq!(n, 4);
        assert!(take_delta(&client, "pc1", 100).unwrap().is_empty());
        assert_eq!(summary(&server, StatsRange::All, None).messages, 4);
        // 送るものが無ければ、sendを呼ばない。
        assert_eq!(push_pending(&client, "pc1", 2, |_| Err("呼ばれない".into())).unwrap(), 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn 期間の名前は窓口の名前と合う() {
        assert_eq!(
            [StatsRange::All, StatsRange::Days30, StatsRange::Days7].map(|r| r.as_query()),
            ["all", "days30", "days7"]
        );
    }
}
