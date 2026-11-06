//! 動作の記録(詩織Ver6.0の段階1)。Ver4.0の`app_log.rs`(日ごと・エラーだけ)を置き換える。
//!
//! 不具合を追うときに、エラーの前に何をして、詩織の中で何が起きたかを読めるようにする。
//! 本体・`mcp_server`・`shiori-save`が同じ部品を使い、同じフォルダに別々のファイルで書く
//! (複数のプログラムが同じファイルに同時に書く事故を避けるため)。
//!
//! - 1行の形: `時刻(ミリ秒まで) 重さ [部品] 出来事 キー=値…`
//! - 重さ: エラー・注意・情報・詳細の4段階。「詳細」は設定で切り替えたときだけ書く
//! - ファイル: 本体と`mcp_server`は起動1回ごと、`shiori-save`は1日に1つ
//! - 消す基準: 種類ごとに直近の`keep_runs`個を残し、全体が`max_total_mb`を超えたら古いものから消す
//! - 記事の本文と会話の中身は書かない(呼び出し側で、事実〈文字数など〉だけを渡す)

use chrono::{DateTime, Local};
use serde::Deserialize;
use std::fmt::Display;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// ログのファイルの種類。ファイル名の頭と、ファイルを分ける単位を決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogKind {
    /// 詩織の本体。起動1回ごと(`shiori-20261023-140512.log`)
    App,
    /// MCPサーバー。起動1回ごと(`mcp-20261023-140512.log`)
    Mcp,
    /// `shiori-save`。1日に1つ(`save-20261023.log`)
    Save,
    /// `shiori-task`。1日に1つ(`task-20261106.log`)
    Task,
}

impl LogKind {
    fn prefix(self) -> &'static str {
        match self {
            LogKind::App => "shiori-",
            LogKind::Mcp => "mcp-",
            LogKind::Save => "save-",
            LogKind::Task => "task-",
        }
    }

    fn per_run(self) -> bool {
        !matches!(self, LogKind::Save | LogKind::Task)
    }

    /// 標準エラーにも写すか。本体だけ写す(開発中に端末で読めるように)。
    /// CLIは標準エラーをAIや人が読む文(「警告: …」)に使い、MCPサーバーは標準出力が通信の専用なので、写さない。
    fn echoes(self) -> bool {
        matches!(self, LogKind::App)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Error => "エラー",
            Level::Warn => "注意",
            Level::Info => "情報",
            Level::Debug => "詳細",
        }
    }
}

/// `config.json`の`log`の項目(`detail`・`keepRuns`・`maxTotalMb`)。無ければ既定値を使う。
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct LogSettings {
    /// 「詳細」の重さも書くか
    pub detail: bool,
    /// 種類ごとに残すファイルの数
    pub keep_runs: usize,
    /// フォルダ全体の上限(MB)
    pub max_total_mb: u64,
}

impl Default for LogSettings {
    fn default() -> Self {
        Self { detail: false, keep_runs: 30, max_total_mb: 100 }
    }
}

impl LogSettings {
    /// `config.json`から`log`の項目だけを読む。CLIは本体の設定の型を持たないので、ここで読む。
    pub fn from_config(root: &Path) -> Self {
        let Ok(text) = fs::read_to_string(root.join("config.json")) else {
            return Self::default();
        };
        serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("log").cloned())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }
}

struct Sink {
    file: Option<File>,
    path: Option<PathBuf>,
    echo: bool,
}

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();
static DETAIL: AtomicBool = AtomicBool::new(false);

/// 起動時に1回呼ぶ。ファイルを開き、古いログを消し、panicも記録するようにする。
/// フォルダを作れないときも、標準エラーには出し続ける(記録のために本来の処理を止めない)。
pub fn init(dir: &Path, kind: LogKind, settings: &LogSettings) {
    DETAIL.store(settings.detail, Ordering::Relaxed);
    let now = Local::now();
    let (file, path) = match fs::create_dir_all(dir) {
        Ok(()) => {
            let (file, path) = open_file(dir, kind, now);
            // 開いたばかりのファイルは消さないように、開いてから整理する
            prune(dir, kind, settings, path.as_deref());
            (file, path)
        }
        Err(e) => {
            eprintln!("[LOG-001] ログのフォルダを作れない: {} ({e})", dir.display());
            (None, None)
        }
    };
    let _ = SINK.set(Mutex::new(Sink { file, path, echo: kind.echoes() }));
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write(Level::Error, "APP", "panic", &[("内容", &info)]);
        default_hook(info);
    }));
}

/// いま書いているファイルの場所(診断の画面などで示すため)。
pub fn current_path() -> Option<PathBuf> {
    SINK.get().and_then(|s| s.lock().ok()).and_then(|s| s.path.clone())
}

/// 「詳細」を書くかを切り替える(設定画面から)。
pub fn set_detail(on: bool) {
    DETAIL.store(on, Ordering::Relaxed);
}

pub fn detail_enabled() -> bool {
    DETAIL.load(Ordering::Relaxed)
}

/// 1行を書く。`part`は部品名(`HUB`など)か、エラーのときはエラーコード(`EXT-003`)か拡張機能の名前。
pub fn write(level: Level, part: &str, event: &str, fields: &[(&str, &dyn Display)]) {
    if level == Level::Debug && !detail_enabled() {
        return;
    }
    let line = format_line(Local::now(), level, part, event, fields);
    // 初期化の前(または失敗)は、標準エラーにだけ出す
    let Some(sink) = SINK.get() else {
        eprintln!("{line}");
        return;
    };
    let Ok(mut sink) = sink.lock() else { return };
    if sink.echo {
        eprintln!("{line}");
    }
    if let Some(file) = sink.file.as_mut() {
        let _ = file.write_all(line.as_bytes());
        let _ = file.write_all(b"\n");
    }
}

pub fn error(part: &str, event: &str, fields: &[(&str, &dyn Display)]) {
    write(Level::Error, part, event, fields);
}

pub fn warn(part: &str, event: &str, fields: &[(&str, &dyn Display)]) {
    write(Level::Warn, part, event, fields);
}

pub fn info(part: &str, event: &str, fields: &[(&str, &dyn Display)]) {
    write(Level::Info, part, event, fields);
}

pub fn debug(part: &str, event: &str, fields: &[(&str, &dyn Display)]) {
    write(Level::Debug, part, event, fields);
}

/// 1行の文字列を作る。改行は`\n`の2文字にして、1つの出来事が必ず1行に収まるようにする。
pub fn format_line(
    time: DateTime<Local>,
    level: Level,
    part: &str,
    event: &str,
    fields: &[(&str, &dyn Display)],
) -> String {
    let mut line = format!(
        "{} {} [{}] {}",
        time.format("%Y-%m-%d %H:%M:%S%.3f"),
        level.label(),
        part,
        one_line(event)
    );
    for (key, value) in fields {
        line.push(' ');
        line.push_str(key);
        line.push('=');
        line.push_str(&quote_value(&value.to_string()));
    }
    line
}

fn one_line(s: &str) -> String {
    s.replace('\r', "").replace('\n', "\\n")
}

/// 空白・`=`・`"`を含む値と空の値は、`"`で囲む(読む人とAIが、どこまでが値か分かるように)。
fn quote_value(value: &str) -> String {
    let value = one_line(value);
    if value.is_empty() || value.contains([' ', '　', '=', '"']) {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value
    }
}

fn file_name(kind: LogKind, now: DateTime<Local>) -> String {
    let stamp = if kind.per_run() { now.format("%Y%m%d-%H%M%S") } else { now.format("%Y%m%d") };
    format!("{}{stamp}.log", kind.prefix())
}

/// 起動1回ごとのファイルは新しく作る(同じ秒に2回起動したら`-2`などを付ける)。
/// 1日1つのファイルは追記する。
fn open_file(dir: &Path, kind: LogKind, now: DateTime<Local>) -> (Option<File>, Option<PathBuf>) {
    let base = file_name(kind, now);
    if !kind.per_run() {
        let path = dir.join(&base);
        return match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => (Some(f), Some(path)),
            Err(e) => {
                eprintln!("[LOG-002] ログのファイルを開けない: {} ({e})", path.display());
                (None, None)
            }
        };
    }
    let stem = base.trim_end_matches(".log");
    for n in 1..100 {
        let name = if n == 1 { base.clone() } else { format!("{stem}-{n}.log") };
        let path = dir.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => return (Some(f), Some(path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                eprintln!("[LOG-002] ログのファイルを開けない: {} ({e})", path.display());
                return (None, None);
            }
        }
    }
    (None, None)
}

/// ログのファイルなら、(種類, 並べ替えに使う名前)を返す。名前の形が違うファイルには触らない。
fn parse_log_name(name: &str) -> Option<LogKind> {
    if name.starts_with("._") || !name.ends_with(".log") {
        return None;
    }
    [LogKind::App, LogKind::Mcp, LogKind::Save, LogKind::Task].into_iter().find(|k| {
        name.strip_prefix(k.prefix())
            .map(|rest| rest.as_bytes().first().is_some_and(u8::is_ascii_digit))
            .unwrap_or(false)
    })
}

/// 消すファイルを選ぶ。`files`は(名前, 大きさ)。名前に時刻が入っているので、名前の順が古い順になる。
/// `keep`(いま書いているファイル)は消さない。
fn choose_removals(
    files: &[(String, u64)],
    kind: LogKind,
    settings: &LogSettings,
    keep: Option<&str>,
) -> Vec<String> {
    let mut remove = Vec::new();
    // 同じ種類のうち、新しい順にkeep_runs個を残す
    let mut same: Vec<&(String, u64)> =
        files.iter().filter(|(n, _)| parse_log_name(n) == Some(kind)).collect();
    same.sort_by(|a, b| b.0.cmp(&a.0));
    for (name, _) in same.iter().skip(settings.keep_runs.max(1)) {
        if Some(name.as_str()) != keep {
            remove.push(name.clone());
        }
    }
    // 全体の上限を超えたら、種類を問わず古いものから消す
    let limit = settings.max_total_mb.saturating_mul(1024 * 1024);
    let mut rest: Vec<&(String, u64)> = files
        .iter()
        .filter(|(n, _)| parse_log_name(n).is_some() && !remove.contains(n))
        .collect();
    let mut total: u64 = rest.iter().map(|(_, s)| s).sum();
    // 種類の頭を外した時刻の部分で、古い順に並べる
    rest.sort_by_key(|(n, _)| {
        let kind = parse_log_name(n).unwrap();
        n.trim_start_matches(kind.prefix()).to_string()
    });
    for (name, size) in rest {
        if total <= limit {
            break;
        }
        if Some(name.as_str()) == keep {
            continue;
        }
        remove.push(name.clone());
        total = total.saturating_sub(*size);
    }
    remove
}

fn prune(dir: &Path, kind: LogKind, settings: &LogSettings, keep: Option<&Path>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let files: Vec<(String, u64)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let size = e.metadata().ok()?.len();
            Some((name, size))
        })
        .collect();
    let keep_name = keep.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
    for name in choose_removals(&files, kind, settings, keep_name.as_deref()) {
        let _ = fs::remove_file(dir.join(name));
    }
}

/// 検索の結果の上位を、ログの1つの値にまとめる(タイトル・場所・点数まで。本文は入れない)。
/// 例: `起動の並列化〔startup-plan.md〕0.993 / ログ〔log.md〕0.951`
pub fn summarize_hits<'a>(hits: impl IntoIterator<Item = (&'a str, &'a str, f64)>, top: usize) -> String {
    hits.into_iter()
        .take(top)
        .map(|(title, source, score)| format!("{title}〔{source}〕{score:.3}"))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// `format!`と同じ書き方で、本体のエラーとして記録する(Ver4.0の`log_error!`と同じ使い方)。
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::shiori_log::write($crate::shiori_log::Level::Error, "APP", &format!($($arg)*), &[])
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32, ms: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap() + chrono::Duration::milliseconds(ms as i64)
    }

    #[test]
    fn 一行の形() {
        let t = at(2026, 10, 23, 14, 5, 12, 345);
        let line = format_line(t, Level::Info, "PET", "状態を変えた", &[("from", &"普段"), ("理由", &"無操作5分")]);
        assert_eq!(line, "2026-10-23 14:05:12.345 情報 [PET] 状態を変えた from=普段 理由=無操作5分");
    }

    #[test]
    fn 空白や改行を含む値は囲み一行に収める() {
        let t = at(2026, 1, 2, 3, 4, 5, 6);
        let line = format_line(t, Level::Error, "RAG-012", "失敗\n2行目", &[("q", &"a b"), ("e", &""), ("x", &"say \"hi\"")]);
        assert_eq!(line, "2026-01-02 03:04:05.006 エラー [RAG-012] 失敗\\n2行目 q=\"a b\" e=\"\" x=\"say \\\"hi\\\"\"");
        assert!(!line.contains('\n'));
    }

    #[test]
    fn ファイル名() {
        let t = at(2026, 10, 23, 14, 5, 12, 0);
        assert_eq!(file_name(LogKind::App, t), "shiori-20261023-140512.log");
        assert_eq!(file_name(LogKind::Mcp, t), "mcp-20261023-140512.log");
        assert_eq!(file_name(LogKind::Save, t), "save-20261023.log");
        assert_eq!(file_name(LogKind::Task, t), "task-20261023.log");
    }

    #[test]
    fn ログの名前の見分け() {
        assert_eq!(parse_log_name("shiori-20261023-140512.log"), Some(LogKind::App));
        assert_eq!(parse_log_name("mcp-20261023-140512-2.log"), Some(LogKind::Mcp));
        assert_eq!(parse_log_name("save-20261023.log"), Some(LogKind::Save));
        assert_eq!(parse_log_name("task-20261023.log"), Some(LogKind::Task));
        // Ver4.0の日ごとのファイルもshiori-の形なので、同じ種類として古い順に消える
        assert_eq!(parse_log_name("shiori-20261007.log"), Some(LogKind::App));
        assert_eq!(parse_log_name("._shiori-20261023-140512.log"), None);
        assert_eq!(parse_log_name("shiori-abc.log"), None);
        assert_eq!(parse_log_name("other.log"), None);
    }

    #[test]
    fn 直近の数だけ残す() {
        let settings = LogSettings { detail: false, keep_runs: 2, max_total_mb: 100 };
        let files = vec![
            ("shiori-20261021-100000.log".to_string(), 10),
            ("shiori-20261022-100000.log".to_string(), 10),
            ("shiori-20261023-100000.log".to_string(), 10),
            ("mcp-20261020-100000.log".to_string(), 10),
            ("note.txt".to_string(), 10),
        ];
        let removed = choose_removals(&files, LogKind::App, &settings, Some("shiori-20261023-100000.log"));
        assert_eq!(removed, vec!["shiori-20261021-100000.log".to_string()]);
    }

    #[test]
    fn 全体の上限を超えたら古いものから消す() {
        let mb = 1024 * 1024;
        let settings = LogSettings { detail: false, keep_runs: 30, max_total_mb: 2 };
        let files = vec![
            ("mcp-20261020-100000.log".to_string(), mb),
            ("shiori-20261021-100000.log".to_string(), mb),
            ("save-20261022.log".to_string(), mb),
            ("shiori-20261023-100000.log".to_string(), mb),
        ];
        let removed = choose_removals(&files, LogKind::App, &settings, Some("shiori-20261023-100000.log"));
        assert_eq!(removed, vec!["mcp-20261020-100000.log".to_string(), "shiori-20261021-100000.log".to_string()]);
    }

    #[test]
    fn 検索の結果のまとめ() {
        let hits = vec![("起動", "a.md", 0.99321), ("ログ", "b.md", 0.5), ("三つ目", "c.md", 0.1)];
        assert_eq!(summarize_hits(hits, 2), "起動〔a.md〕0.993 / ログ〔b.md〕0.500");
    }

    #[test]
    fn 設定が無ければ既定値() {
        let s: LogSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, LogSettings::default());
        let s: LogSettings = serde_json::from_str("{\"detail\": true, \"keepRuns\": 5}").unwrap();
        assert!(s.detail);
        assert_eq!(s.keep_runs, 5);
        assert_eq!(s.max_total_mb, 100);
    }
}
