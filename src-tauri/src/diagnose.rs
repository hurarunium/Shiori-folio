//! 診断の内容(詩織Ver6.0の段階6)。
//!
//! 外部のAIに貼り付けて渡すための、1つの文章を作る。窓を使わないCLI(`shiori-doctor`)と、
//! 本体の診断の画面の両方から呼ぶ。詩織が正常に動いていなくても作れるよう、本体の状態
//! (Tauriの管理する状態)には頼らず、SSDのファイルとポートだけを見る。
//!
//! 守ること:
//! - 何も書き換えない(承認の記録が壊れていても脇へ移さない。直すのは起動時の自動修復の役目)
//! - 記事の本文と会話の中身は入れない(ログと同じ決まり。ログの行は1行200文字までに切る)
//!
//! 企画書は plan-shiori-ver6-s6-doctor-20261024(詩織のlibrary)。

use crate::hub::{self, Status};
use crate::shared_daemon;
use crate::tasks;
use chrono::Local;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// 自動修復の行に付ける部品名(段階6の「4」。起動時に直したものを、この部品名でログに書く)。
pub const REPAIR_PART: &str = "FIX";
/// 自己確認の「自動修復」が試しのルートで直したときの部品名。診断はこの行を「直した」に入れない。
pub const REPAIR_REHEARSAL_PART: &str = "FIX-SELFTEST";

/// ログから拾う行の上限と、1行の長さの上限。
const LOG_LINES: usize = 10;
const LINE_CHARS: usize = 200;
/// 問題のある記事を名前で並べる上限(多いときは「ほか何件」にする)。
const ARTICLE_LINES: usize = 20;
/// frontmatterの `status` の値(library の `_system/CLAUDE.md`)。
const STATUSES: &[&str] = &["draft", "new", "outdated", "deprecated", "disputed"];

/// 1つの確かめの結果。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Ok,
    Ng,
    /// 無くても動く(音声やLLMなど)か、ふらるに見てほしいもの
    Note,
}

impl Mark {
    fn label(self) -> &'static str {
        match self {
            Mark::Ok => "[OK]",
            Mark::Ng => "[NG]",
            Mark::Note => "[--]",
        }
    }
}

struct Out {
    text: String,
    ng: usize,
}

impl Out {
    fn section(&mut self, title: &str) {
        let _ = write!(self.text, "\n## {title}\n");
    }

    fn item(&mut self, mark: Mark, line: impl AsRef<str>) {
        if mark == Mark::Ng {
            self.ng += 1;
        }
        let _ = writeln!(self.text, "- {} {}", mark.label(), line.as_ref());
    }

    fn plain(&mut self, line: impl AsRef<str>) {
        let _ = writeln!(self.text, "- {}", line.as_ref());
    }

    /// 1つ上の行に付く、字下げした行。
    fn sub(&mut self, line: impl AsRef<str>) {
        let _ = writeln!(self.text, "  - {}", line.as_ref());
    }
}

/// 診断の結果。`ng`は `[NG]` の数(CLIの終了コードと、画面の見出しに使う)。
pub struct Report {
    pub text: String,
    pub ng: usize,
}

/// `root`(`portable/`)を見て、診断の文章を作る。`tool`は呼び出し元の名前(`shiori-doctor` など)。
pub fn build(root: &Path, tool: &str) -> Report {
    let mut out = Out { text: String::new(), ng: 0 };
    let _ = writeln!(
        out.text,
        "# 詩織の診断({tool} {})  {}",
        env!("CARGO_PKG_VERSION"),
        Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    out.text.push_str("外部のAIに貼り付けて渡すための文です。記事の本文と会話の中身は入っていません。\n");

    environment(&mut out, root);
    let config = files(&mut out, root);
    services(&mut out, root, config.as_ref());
    index(&mut out, root);
    extensions(&mut out, root);
    to_check(&mut out, root);
    logs(&mut out, root);

    let _ = write!(out.text, "\n(NGは{}件)\n", out.ng);
    Report { text: out.text, ng: out.ng }
}

fn os_dir() -> &'static str {
    if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(windows) {
        "win"
    } else {
        "linux"
    }
}

fn environment(out: &mut Out, root: &Path) {
    out.section("環境");
    let os = sysinfo::System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string());
    out.plain(format!("OS: {os}(CPU: {})", std::env::consts::ARCH));
    if cfg!(target_os = "macos") && std::env::consts::ARCH != "aarch64" {
        out.item(Mark::Ng, "このMacはApple シリコンではない(詩織のMac用のプログラムはApple シリコンだけ)");
    }
    out.plain(format!("SSDの場所: {}", root.display()));
    if let Ok(exe) = std::env::current_exe() {
        out.plain(format!("実行ファイル: {}", exe.display()));
    }
}

/// SSDの中に、動かすのに要るものがそろっているか。読めた `config.json` を返す。
fn files(out: &mut Out, root: &Path) -> Option<serde_json::Value> {
    out.section("SSDの中");
    let config = match std::fs::read_to_string(root.join("config.json")) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => {
                out.item(Mark::Ok, "config.json(JSONとして読める)");
                Some(v)
            }
            Err(e) => {
                out.item(Mark::Ng, format!("config.json がJSONとして読めない({e})"));
                None
            }
        },
        Err(e) => {
            out.item(Mark::Ng, format!("config.json を読めない({e})"));
            None
        }
    };

    // モデル。埋め込みとリランカーは検索に要る。LLM・音声は無くても検索は動く
    let model = |key: &str| {
        config.as_ref().and_then(|c| c[key]["modelPath"].as_str()).map(|p| root.join(p.trim_start_matches("./")))
    };
    let check = |out: &mut Out, what: &str, path: Option<PathBuf>, required: bool| match path {
        Some(p) if p.is_file() => out.item(Mark::Ok, format!("{what}: {}", rel(root, &p))),
        Some(p) => out.item(if required { Mark::Ng } else { Mark::Note }, format!("{what}が無い: {}", rel(root, &p))),
        None => out.item(if required { Mark::Ng } else { Mark::Note }, format!("{what}の場所が config.json に無い")),
    };
    check(out, "埋め込みのモデル", model("embedding"), true);
    let rerank = root.join("models").join("rerank");
    for f in ["model.safetensors", "tokenizer.json", "config.json"] {
        check(out, "リランカーのモデル", Some(rerank.join(f)), true);
    }
    let external = config.as_ref().and_then(|c| c["mode"].as_str()) == Some("external");
    check(out, if external { "LLMのモデル(外部AIモードなので使わない)" } else { "LLMのモデル" }, model("llm"), !external);
    check(out, "音声認識のモデル", model("stt"), false);
    check(out, "読み上げのモデル", model("tts"), false);

    // 実行ファイル。本体は、この診断の隣(開発の版の bin/<os>/ver6-dev/ など)を先に見て、無ければ bin/<os>/ を見る
    let bin = root.join("bin").join(os_dir());
    let exe = |name: &str| bin.join(crate::exe_name(name));
    let main_in = |dir: &Path| {
        if cfg!(target_os = "macos") {
            dir.join("shiori-folio.app").join("Contents").join("MacOS").join("shiori-folio")
        } else {
            dir.join(crate::exe_name("shiori-folio"))
        }
    };
    // 本体の診断の画面から呼んだときは、いま動いている実行ファイルが本体そのもの
    let exe_now = std::env::current_exe().ok();
    let beside = exe_now
        .clone()
        .filter(|e| e.file_stem().is_some_and(|s| s == "shiori-folio"))
        .or_else(|| exe_now.and_then(|e| e.parent().map(main_in)))
        .filter(|p| p.is_file());
    check(out, "本体", Some(beside.unwrap_or_else(|| main_in(&bin))), true);
    check(out, "埋め込みのサーバー", Some(exe("llama-server")), true);
    match crate::rust_rag_exe() {
        Some(p) => out.item(Mark::Ok, format!("RAG(Rust版): {}", rel(root, &p))),
        None if bin.join("rag-venv").is_dir() => out.item(Mark::Ok, "RAG(Python版、rag-venv)"),
        None => out.item(Mark::Ng, "RAGが無い(shiori-rag も rag-venv も無い)"),
    }
    check(out, "MCPサーバー", Some(exe("mcp_server")), false);
    check(out, "音声認識のサーバー", Some(exe("whisper-server")), false);
    check(out, "読み上げ", Some(exe("piper-plus-cli")), false);

    #[cfg(windows)]
    webview2(out);

    config
}

/// WebView2(Windowsで本体の画面を出す部品)の版を、レジストリから読む。
#[cfg(windows)]
fn webview2(out: &mut Out) {
    const KEY: &str = "Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let keys = [
        format!("HKLM\\SOFTWARE\\WOW6432Node\\{KEY}"),
        format!("HKLM\\SOFTWARE\\{KEY}"),
        format!("HKCU\\Software\\{KEY}"),
    ];
    for key in &keys {
        let Ok(o) = std::process::Command::new("reg").args(["query", key, "/v", "pv"]).output() else { continue };
        let text = String::from_utf8_lossy(&o.stdout);
        if let Some(version) = text.split_whitespace().last().filter(|v| o.status.success() && *v != "0.0.0.0") {
            out.item(Mark::Ok, format!("WebView2: {version}"));
            return;
        }
    }
    out.item(Mark::Ng, "WebView2が見つからない(本体の画面が出ない原因になる)");
}

fn services(out: &mut Out, root: &Path, config: Option<&serde_json::Value>) {
    out.section("サービス");
    let port = |key: &str, default: u16| {
        config.and_then(|c| c[key]["port"].as_u64()).and_then(|p| u16::try_from(p).ok()).unwrap_or(default)
    };
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    for (name, key, default, lock) in [
        ("埋め込み", "embedding", 8082, ".shiori-embed.lock"),
        ("RAG", "rag", 8083, ".shiori-rag.lock"),
    ] {
        let port = port(key, default);
        let healthy = ureq::get(&format!("http://127.0.0.1:{port}/health")).timeout(Duration::from_secs(2)).call().is_ok();
        let pid = shared_daemon::read_daemon_pid(root, lock);
        let alive = shared_daemon::daemon_pid_if_alive(&sys, root, lock).is_some();
        let pid_text = match (pid, alive) {
            (Some(p), true) => format!("記録のPID {p}は動いている"),
            (Some(p), false) => format!("記録のPID {p}は動いていない(古いロック)"),
            (None, _) => "ロックの記録は無い".to_string(),
        };
        // 詩織が起動していなければ止まっているのがふつうなので、NGにはしない
        let mark = if healthy { Mark::Ok } else { Mark::Note };
        let state = if healthy { "返事がある" } else { "返事が無い(詩織が起動していなければふつう)" };
        out.item(mark, format!("{name}(ポート{port}): {state}。{pid_text}"));
    }
}

fn index(out: &mut Out, root: &Path) {
    out.section("索引");
    let dir = crate::rag_vectordb_dir(root);
    if !dir.is_dir() {
        out.item(Mark::Note, format!("索引のフォルダが無い: {}(次の起動で作り直す)", rel(root, &dir)));
        return;
    }
    let count = |d: PathBuf| std::fs::read_dir(d).map(|e| e.flatten().filter(|f| !is_mac_meta(&f.path())).count()).ok();
    match count(dir.join("chunks")) {
        Some(n) => out.item(Mark::Ok, format!("{}: library {n}件", rel(root, &dir))),
        None => out.item(Mark::Ok, format!("{}(Python版の形)", rel(root, &dir))),
    }
    if let Some(n) = count(dir.join("shelf").join("chunks")) {
        out.sub(format!("棚: {n}件"));
    }
    if let Some(t) = modified(&dir.join(".index_state.json")) {
        out.sub(format!("最後の同期: {t}"));
    }
}

fn extensions(out: &mut Out, root: &Path) {
    out.section("拡張機能");
    let (list, broken) = hub::inspect(root);
    match broken {
        None => out.item(Mark::Ok, "承認の記録(data/extensions/_approvals.json)"),
        Some(detail) => out.item(Mark::Ng, format!("承認の記録が読めない({detail})")),
    }
    if list.is_empty() {
        out.plain("拡張機能は無い");
    }
    for ext in list {
        let kind = ext.manifest.as_ref().map(|m| format!("{:?}", m.kind).to_lowercase()).unwrap_or_else(|| "-".into());
        match ext.status {
            Status::Ready => out.item(Mark::Ok, format!("{}({kind}): 使える", ext.folder)),
            Status::NeedsApproval { first_time, added } => out.item(
                Mark::Note,
                format!(
                    "{}({kind}): 承認待ち({}。使うもの: {})",
                    ext.folder,
                    if first_time { "初めて" } else { "使うものが増えた" },
                    added.join("・")
                ),
            ),
            Status::Invalid { code, detail } => {
                out.item(Mark::Ng, format!("{}: {code} {}({})", ext.folder, code.message, cut(&detail)))
            }
        }
    }
}

/// ふらるが書いたものと会話の記録。詩織は直さず、見つけて報告するだけ。
fn to_check(out: &mut Out, root: &Path) {
    out.section("要確認(詩織は直さない)");
    let db = root.join("data").join("shiori.db");
    match std::fs::read(&db).ok().map(|b| b.starts_with(b"SQLite format 3\0")) {
        Some(true) => out.item(Mark::Ok, "会話の記録(data/shiori.db)"),
        Some(false) => out.item(Mark::Ng, "会話の記録(data/shiori.db)がSQLiteの形ではない"),
        None => out.item(Mark::Note, "会話の記録(data/shiori.db)が無い(初めての起動ならふつう)"),
    }
    for (what, dir, types) in [
        ("library", root.join("library"), &["project", "area", "resource", "profile"][..]),
        ("棚", root.join("knowledge"), &["knowledge"][..]),
    ] {
        let Some(files) = md_files(&dir) else {
            out.item(Mark::Ng, format!("{what}のフォルダを読めない: {}", rel(root, &dir)));
            continue;
        };
        let problems: Vec<(PathBuf, String)> =
            files.iter().filter_map(|f| article_problem(f, types).map(|why| (f.clone(), why))).collect();
        if problems.is_empty() {
            out.item(Mark::Ok, format!("{what}: 記事{}件(読めない記事・frontmatterの崩れは無い)", files.len()));
            continue;
        }
        out.item(Mark::Note, format!("{what}: 記事{}件のうち{}件に問題がある", files.len(), problems.len()));
        for (f, why) in problems.iter().take(ARTICLE_LINES) {
            out.sub(format!("{}: {why}", rel(root, f)));
        }
        if problems.len() > ARTICLE_LINES {
            out.sub(format!("ほか{}件", problems.len() - ARTICLE_LINES));
        }
    }
    task_files(out, root);
    let warnings = crate::rag_vectordb_dir(root).join(".extract_warnings.json");
    if let Some(n) = std::fs::read_to_string(warnings)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .map(|v| v.as_object().map(|o| o.len()).or_else(|| v.as_array().map(|a| a.len())).unwrap_or(0))
        .filter(|n| *n > 0)
    {
        out.item(Mark::Note, format!("素材の抽出の警告: {n}件(索引の .extract_warnings.json)"));
    }
}

/// タスク(`tasks/`、詩織Ver6.2)が読めるか、件数、崩れたmd。概要と詳細は入れず、idだけを出す。
fn task_files(out: &mut Out, root: &Path) {
    let dir = root.join("tasks");
    if !dir.exists() {
        out.item(Mark::Note, "タスクのフォルダ(tasks/)はまだ無い(初めてならふつう)");
        return;
    }
    if let Err(e) = std::fs::read_dir(&dir) {
        out.item(Mark::Ng, format!("タスクのフォルダを読めない: {} ({e})", rel(root, &dir)));
        return;
    }
    let active = tasks::list(&dir, false);
    let archived = tasks::list(&dir, true);
    let counts = format!("未完了{}件・アーカイブ{}件", active.len(), archived.len());
    let broken: Vec<&tasks::Task> = active.iter().chain(&archived).filter(|t| t.broken).collect();
    if broken.is_empty() {
        out.item(Mark::Ok, format!("タスク: {counts}(形の崩れたmdは無い)"));
        return;
    }
    out.item(Mark::Note, format!("タスク: {counts}のうち{}件のmdの形が崩れている", broken.len()));
    for task in broken.iter().take(ARTICLE_LINES) {
        out.sub(if task.archived { format!("{}(アーカイブ)", task.id) } else { task.id.clone() });
    }
    if broken.len() > ARTICLE_LINES {
        out.sub(format!("ほか{}件", broken.len() - ARTICLE_LINES));
    }
}

fn logs(out: &mut Out, root: &Path) {
    out.section("ログ");
    let dir = root.join("data").join("logs");
    for (what, prefix) in [("本体", "shiori-"), ("RAG", "rag-")] {
        let Some(path) = newest(&dir, prefix) else {
            out.plain(format!("{what}のログは無い(本体が一度も起動していない手がかり)"));
            continue;
        };
        out.plain(format!("{what}の直近のログ: {}", rel(root, &path)));
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let errors: Vec<&str> = lines.iter().copied().filter(|l| l.contains(" エラー [")).collect();
        let repairs: Vec<&str> =
            lines.iter().copied().filter(|l| l.contains(&format!("[{REPAIR_PART}]"))).collect();
        if errors.is_empty() {
            out.sub("エラーの行は無い");
        }
        for l in errors.iter().rev().take(LOG_LINES).rev() {
            out.sub(cut(l));
        }
        for l in repairs.iter().rev().take(LOG_LINES).rev() {
            out.sub(format!("直した: {}", cut(l)));
        }
    }
}

/// 診断の文章を `data/logs/diagnose-日時.txt` に書く。書いた場所を返す。
pub fn save(root: &Path, report: &Report) -> std::io::Result<PathBuf> {
    let dir = root.join("data").join("logs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("diagnose-{}.txt", Local::now().format("%Y%m%d-%H%M%S")));
    std::fs::write(&path, &report.text)?;
    Ok(path)
}

fn newest(dir: &Path, prefix: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix)))
        .filter(|p| p.extension().is_some_and(|e| e == "log"))
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH))
}

/// `dir` の下の記事(`.md`)を全部、名前の順で返す。いちばん上の `_system/`(決まりの書類など)は除く。
fn md_files(dir: &Path) -> Option<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    std::fs::read_dir(dir).ok()?;
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if is_mac_meta(&p) || (d == dir && p.file_name().is_some_and(|n| n == "_system")) {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "md") {
                files.push(p);
            }
        }
    }
    files.sort();
    Some(files)
}

/// 記事の問題(読めない、frontmatterの崩れ)を1つ返す。問題が無ければ `None`。
/// 決まりは library の `_system/CLAUDE.md` の「保存する前に」。本文は読むだけで、診断には入れない。
fn article_problem(path: &Path, types: &[&str]) -> Option<String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return Some(format!("読めない({e})")),
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return Some("UTF-8として読めない(文字コード)".into());
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let Some(rest) = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n")) else {
        return Some("frontmatterが無い(先頭が---ではない)".into());
    };
    let Some(end) = rest.find("\n---") else {
        return Some("frontmatterが閉じていない(終わりの---が無い)".into());
    };
    let fm = match serde_yaml::from_str::<serde_yaml::Value>(&rest[..end]) {
        Ok(v) if v.is_mapping() => v,
        Ok(_) => return Some("frontmatterが「項目: 値」の形ではない".into()),
        Err(e) => return Some(format!("frontmatterのYAMLが読めない({})", cut(&e.to_string()))),
    };
    let field = |k: &str| fm.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|v| !v.is_empty());
    if field("title").is_none() {
        return Some("titleが無い".into());
    }
    let Some(ty) = field("type") else {
        return Some("typeが無い".into());
    };
    if !types.contains(&ty) {
        return Some(format!("typeの値が決まりの外({ty})"));
    }
    if matches!(ty, "project" | "area") && field("kind").is_none() {
        return Some(format!("typeが{ty}なのにkindが無い"));
    }
    if let Some(status) = field("status").filter(|s| !STATUSES.contains(s)) {
        return Some(format!("statusの値が決まりの外({status})"));
    }
    None
}

/// Macがexfatに作る `._` のファイル。
fn is_mac_meta(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("._"))
}

fn modified(p: &Path) -> Option<String> {
    let t = p.metadata().ok()?.modified().ok()?;
    Some(chrono::DateTime::<Local>::from(t).format("%Y-%m-%d %H:%M").to_string())
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).display().to_string().replace('\\', "/")
}

fn cut(s: &str) -> String {
    let mut c: String = s.chars().take(LINE_CHARS).collect();
    if s.chars().count() > LINE_CHARS {
        c.push('…');
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 空のフォルダでも文章ができ_要るものが無いとNGになる() {
        let dir = std::env::temp_dir().join(format!("shiori-diagnose-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = build(&dir, "test");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(r.text.contains("## SSDの中"));
        assert!(r.text.contains("[NG] config.json を読めない"));
        assert!(r.ng > 0);
    }

    #[test]
    fn 崩れた記事を見つける() {
        let dir = std::env::temp_dir().join(format!("shiori-diagnose-articles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let types = ["project", "area", "resource", "profile"];
        let check = |name: &str, body: &[u8]| {
            let p = dir.join(name);
            std::fs::write(&p, body).unwrap();
            article_problem(&p, &types)
        };
        assert_eq!(check("ok.md", "---\ntitle: よい\ntype: resource\nstatus: new\n---\n本文".as_bytes()), None);
        assert!(check("sjis.md", &[0x82, 0xa0, 0x0a]).unwrap().contains("UTF-8"));
        assert!(check("none.md", "本文だけ".as_bytes()).unwrap().contains("frontmatterが無い"));
        assert!(check("open.md", "---\ntitle: a\n本文".as_bytes()).unwrap().contains("閉じていない"));
        assert!(check("yaml.md", "---\ntitle: [a\n---\n".as_bytes()).unwrap().contains("YAML"));
        assert!(check("notitle.md", "---\ntype: resource\n---\n".as_bytes()).unwrap().contains("title"));
        assert!(check("badtype.md", "---\ntitle: a\ntype: memo\n---\n".as_bytes()).unwrap().contains("type"));
        assert!(check("nokind.md", "---\ntitle: a\ntype: area\n---\n".as_bytes()).unwrap().contains("kind"));
        assert!(check("status.md", "---\ntitle: a\ntype: resource\nstatus: old\n---\n".as_bytes()).unwrap().contains("status"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn いちばん上の_systemは記事として数えない() {
        let dir = std::env::temp_dir().join(format!("shiori-diagnose-system-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("_system")).unwrap();
        std::fs::create_dir_all(dir.join("20-areas").join("_system")).unwrap();
        std::fs::write(dir.join("_system").join("CLAUDE.md"), "決まり").unwrap();
        std::fs::write(dir.join("20-areas").join("_system").join("a.md"), "x").unwrap();
        let files = md_files(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(files.len(), 1, "いちばん上の_systemだけを除く");
    }

    #[test]
    fn タスクの件数と崩れたmdを報告し_本文は入れない() {
        let dir = std::env::temp_dir().join(format!("shiori-diagnose-tasks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = || {
            let mut out = Out { text: String::new(), ng: 0 };
            task_files(&mut out, &dir);
            out.text
        };
        assert!(run().contains("まだ無い"));

        let tasks_dir = dir.join("tasks");
        let done = tasks::add(&tasks_dir, &tasks::NewTask { summary: "秘密の概要", ..Default::default() }).unwrap();
        tasks::add(&tasks_dir, &tasks::NewTask { summary: "秘密の概要", detail: "秘密の詳細", ..Default::default() }).unwrap();
        tasks::complete(&tasks_dir, &done.id).unwrap();
        let text = run();
        assert!(text.contains("[OK] タスク: 未完了1件・アーカイブ1件"), "{text}");

        std::fs::write(tasks_dir.join("broken.md"), "秘密の本文").unwrap();
        let text = run();
        assert!(text.contains("[--] タスク: 未完了2件・アーカイブ1件のうち1件"), "{text}");
        assert!(text.contains("broken"), "{text}");
        assert!(!text.contains("秘密"), "概要と本文は入れない: {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 長い行は切る() {
        let long = "あ".repeat(300);
        assert_eq!(cut(&long).chars().count(), LINE_CHARS + 1);
    }
}
