//! タスクリスト(詩織Ver6.2、設計書 design-shiori-tasklist-20261105)の読み書き。
//!
//! タスクは`tasks/`に1件1つのmdで置き、フォルダは実際のフォルダ(2段まで)で表す。
//! 本体の窓口とCLI(`shiori-task`)が同じ関数を呼ぶ。tauriに依存しない。
//!
//! - 呼び出し側が渡すのはidだけ。一覧の中から探して、そのパスを使う(`../`は一致しない)。
//! - 書き込みのあとは毎回`INDEX.md`を書き直す(AIが作業開始時に読む)。
//! - 名前の比較はNFCにそろえ、大文字・小文字を区別しない(exfatはMacでNFD、WindowsでNFCを返す)。
//! - ログにはidと操作だけを書く。概要と詳細は書かない。

use std::cmp::Ordering as CmpOrdering;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::UNIX_EPOCH;

use chrono::{Local, NaiveDate};
use serde::Serialize;
use serde_yaml::{Mapping, Value};
use unicode_normalization::UnicodeNormalization;

use crate::shiori_log;

const LOG_PART: &str = "TASK";
const ARCHIVE_DIR: &str = "_archive";
const INDEX_FILE: &str = "INDEX.md";
const MAX_FOLDER_DEPTH: usize = 2;
const DATE_FORMAT: &str = "%Y-%m-%d";
// Windowsの予約名。拡張子が付いていても使えない。
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

static ID_COUNTER: AtomicU32 = AtomicU32::new(0);

/// 1件のタスク。`folder`は`Unity/Tanker`の形で、未分類は空文字列。
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub summary: String,
    pub folder: String,
    pub created: String,
    pub due: Option<String>,
    pub done: Option<String>,
    pub detail: String,
    pub mtime: f64,
    /// frontmatterが崩れている印。概要にはファイル名が入る
    pub broken: bool,
    pub archived: bool,
    #[serde(skip)]
    path: PathBuf,
}

/// 追加の中身。`folder`が空なら未分類に置く。
#[derive(Debug, Default)]
pub struct NewTask<'a> {
    pub summary: &'a str,
    pub folder: &'a str,
    pub due: Option<&'a str>,
    pub detail: &'a str,
}

/// 変更の中身。`None`の項目は変えない。`due`の`Some(None)`は締切を消す。
#[derive(Debug, Default)]
pub struct TaskEdit<'a> {
    pub summary: Option<&'a str>,
    pub due: Option<Option<&'a str>>,
    pub detail: Option<&'a str>,
}

fn nfc(s: &str) -> String {
    s.nfc().collect()
}

// 名前を比べるときの形。
fn name_key(s: &str) -> String {
    nfc(s).to_lowercase()
}

fn archive_root(root: &Path) -> PathBuf {
    root.join(ARCHIVE_DIR)
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), DATE_FORMAT).ok()
}

fn require_date(s: &str, label: &str) -> Result<String, String> {
    parse_date(s)
        .map(|d| d.format(DATE_FORMAT).to_string())
        .ok_or_else(|| format!("{label}はYYYY-MM-DDの形で書いてください: {s}"))
}

fn require_summary(s: &str) -> Result<String, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("概要が空です".to_string());
    }
    if s.contains(['\r', '\n']) {
        return Err("概要は1行で書いてください".to_string());
    }
    Ok(s.to_string())
}

// ---- mdの読み書き ----

// frontmatterと本文に分ける。形が崩れていればNone。
fn parse_md(text: &str) -> Option<(Mapping, String)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text.strip_prefix("---")?;
    let rest = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n'))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            let front: Mapping = serde_yaml::from_str(&rest[..offset]).ok()?;
            let body = rest[offset + line.len()..].trim().to_string();
            return Some((front, body));
        }
        offset += line.len();
    }
    None
}

fn render_md(front: &Mapping, body: &str) -> Result<String, String> {
    let yaml = serde_yaml::to_string(front).map_err(|e| format!("frontmatterの書き出しに失敗: {e}"))?;
    let body = body.trim();
    Ok(if body.is_empty() { format!("---\n{yaml}---\n") } else { format!("---\n{yaml}---\n\n{body}\n") })
}

fn scalar(front: &Mapping, key: &str) -> Option<String> {
    match front.get(key)? {
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn set_text(front: &mut Mapping, key: &str, value: &str) {
    front.insert(Value::String(key.to_string()), Value::String(value.to_string()));
}

// 一時ファイルに書いて置き換える(途中で落ちても元のファイルを壊さない)。
fn write_atomically(dest: &Path, content: &str) -> Result<(), String> {
    let name = dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = dest.with_file_name(format!(".{name}.tmp"));
    if let Err(e) = std::fs::write(&tmp, content) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}への書き込みに失敗: {e}", tmp.display()));
    }
    if let Err(e) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}への置き換えに失敗: {e}", dest.display()));
    }
    Ok(())
}

fn file_mtime(path: &Path) -> f64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// 読めないmdも、形が崩れている印を付けて一覧に出す(黙って隠さない)。
fn read_task(path: &Path, folder: &str, archived: bool) -> Task {
    let id = path.file_stem().map(|s| nfc(&s.to_string_lossy())).unwrap_or_default();
    let file_name = path.file_name().map(|s| nfc(&s.to_string_lossy())).unwrap_or_default();
    let mtime = file_mtime(path);
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let parsed = parse_md(&text).and_then(|(front, body)| {
        let summary = scalar(&front, "summary")?;
        let created = parse_date(&scalar(&front, "created")?)?;
        Some((front, body, summary, created))
    });
    let Some((front, detail, summary, created)) = parsed else {
        shiori_log::warn(LOG_PART, "形が崩れたmd", &[("id", &id)]);
        return Task {
            id,
            summary: file_name,
            folder: folder.to_string(),
            created: String::new(),
            due: None,
            done: None,
            detail: text,
            mtime,
            broken: true,
            archived,
            path: path.to_path_buf(),
        };
    };
    let date_field = |key: &str| -> Option<String> {
        let raw = scalar(&front, key)?;
        let date = parse_date(&raw);
        if date.is_none() {
            shiori_log::warn(LOG_PART, "日付として読めない", &[("id", &id), ("項目", &key)]);
        }
        date.map(|d| d.format(DATE_FORMAT).to_string())
    };
    let due = date_field("due");
    let done = date_field("done");
    Task {
        summary,
        folder: folder.to_string(),
        created: created.format(DATE_FORMAT).to_string(),
        due,
        done,
        detail,
        mtime,
        broken: false,
        archived,
        path: path.to_path_buf(),
        id,
    }
}

// frontmatterだけを変える。知らない項目と本文はそのまま残す。
fn update_front(path: &Path, change: impl FnOnce(&mut Mapping)) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("読み込みに失敗: {e}"))?;
    let (mut front, body) = parse_md(&text).ok_or_else(|| format!("形が崩れています: {}", path.display()))?;
    change(&mut front);
    write_atomically(path, &render_md(&front, &body)?)
}

// ---- フォルダを読む ----

fn entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<(String, PathBuf)> = read
        .filter_map(|e| e.ok())
        .map(|e| (nfc(&e.file_name().to_string_lossy()), e.path()))
        .collect();
    items.sort();
    items
}

// `.`と`_`で始まるフォルダは読まない(`_archive`と、Macの隠しファイル)。
fn child_dirs(dir: &Path) -> Vec<(String, PathBuf)> {
    entries(dir)
        .into_iter()
        .filter(|(name, path)| !name.starts_with(['.', '_']) && path.is_dir())
        .collect()
}

fn md_files(dir: &Path) -> Vec<PathBuf> {
    entries(dir)
        .into_iter()
        .filter(|(name, path)| !name.starts_with('.') && path.is_file() && name.to_lowercase().ends_with(".md"))
        .map(|(_, path)| path)
        .collect()
}

// `base`の下のフォルダを2段まで返す。3段目より深いものは読まず、ログに警告する。
fn folder_dirs(base: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    for (first, first_path) in child_dirs(base) {
        found.push((first.clone(), first_path.clone()));
        for (second, second_path) in child_dirs(&first_path) {
            let name = format!("{first}/{second}");
            for (deep, _) in child_dirs(&second_path) {
                let deep_name = format!("{name}/{deep}");
                shiori_log::warn(LOG_PART, "3段目より深いフォルダは読まない", &[("フォルダ", &deep_name)]);
            }
            found.push((name, second_path));
        }
    }
    found
}

fn find_folder(base: &Path, folder: &str) -> Option<(String, PathBuf)> {
    let key = name_key(folder);
    folder_dirs(base).into_iter().find(|(name, _)| name_key(name) == key)
}

// ---- 名前の決まり ----

fn validate_segment(name: &str) -> Result<(), String> {
    let refuse = |why: &str| Err(format!("フォルダ名「{name}」は使えません({why})"));
    if name.is_empty() {
        return refuse("空です");
    }
    if name.chars().any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()) {
        return refuse("使えない文字が入っています");
    }
    if name.starts_with(['.', '_']) {
        return refuse("先頭に.と_は使えません");
    }
    if name.ends_with([' ', '　', '.']) {
        return refuse("末尾に空白と.は使えません");
    }
    let stem = name.split('.').next().unwrap_or(name).to_uppercase();
    if RESERVED_NAMES.contains(&stem.as_str()) {
        return refuse("Windowsの予約名です");
    }
    Ok(())
}

// `Unity/Tanker`を段に分けて確かめる。比較と保存のため、NFCにそろえて返す。
fn parse_folder(folder: &str) -> Result<Vec<String>, String> {
    let folder = nfc(folder);
    let segments: Vec<String> = folder.split('/').map(str::to_string).collect();
    if segments.len() > MAX_FOLDER_DEPTH {
        return Err(format!("フォルダは{MAX_FOLDER_DEPTH}段までです: {folder}"));
    }
    for segment in &segments {
        validate_segment(segment)?;
    }
    Ok(segments)
}

// あるフォルダを探す。空文字列は未分類(`root`そのもの)。
fn existing_folder(root: &Path, folder: &str) -> Result<(String, PathBuf), String> {
    if folder.is_empty() {
        return Ok((String::new(), root.to_path_buf()));
    }
    let name = parse_folder(folder)?.join("/");
    find_folder(root, &name).ok_or_else(|| format!("フォルダがありません: {name}(先にフォルダを作ってください)"))
}

// 段をたどり、無いものは作る。名前の形が違う同じフォルダは、あるものを使う。
fn ensure_folder(base: &Path, segments: &[String]) -> Result<PathBuf, String> {
    let mut current = base.to_path_buf();
    std::fs::create_dir_all(&current).map_err(|e| format!("フォルダを作れません: {e}"))?;
    for segment in segments {
        let key = name_key(segment);
        current = match child_dirs(&current).into_iter().find(|(name, _)| name_key(name) == key) {
            Some((_, path)) => path,
            None => {
                let path = current.join(segment);
                std::fs::create_dir(&path).map_err(|e| format!("フォルダを作れません: {e}"))?;
                path
            }
        };
    }
    Ok(current)
}

fn folder_segments(folder: &str) -> Vec<String> {
    if folder.is_empty() {
        Vec::new()
    } else {
        folder.split('/').map(str::to_string).collect()
    }
}

// ---- 読む ----

fn sort_by_due(tasks: &mut [Task]) {
    tasks.sort_by(|a, b| {
        let by_due = match (&a.due, &b.due) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => CmpOrdering::Less,
            (None, Some(_)) => CmpOrdering::Greater,
            (None, None) => CmpOrdering::Equal,
        };
        by_due.then_with(|| a.id.cmp(&b.id))
    });
}

/// 全件を締切の近い順(締切なしは最後)で返す。`archived`ならアーカイブの側。
pub fn list(root: &Path, archived: bool) -> Vec<Task> {
    let archive = archive_root(root);
    let base: &Path = if archived { &archive } else { root };
    let mut tasks: Vec<Task> = md_files(base)
        .into_iter()
        .filter(|p| archived || !p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(INDEX_FILE)))
        .map(|p| read_task(&p, "", archived))
        .collect();
    for (folder, dir) in folder_dirs(base) {
        tasks.extend(md_files(&dir).into_iter().map(|p| read_task(&p, &folder, archived)));
    }
    sort_by_due(&mut tasks);
    tasks
}

/// 実際のフォルダの一覧(空のフォルダも出す)。アーカイブの側は入れない。
pub fn folders(root: &Path) -> Vec<String> {
    folder_dirs(root).into_iter().map(|(name, _)| name).collect()
}

fn find(root: &Path, id: &str, archived: bool) -> Result<Task, String> {
    let id = nfc(id);
    list(root, archived)
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("タスクが見つかりません: {id}"))
}

// ---- `INDEX.md` ----

fn render_index(tasks: &[Task]) -> String {
    let mut text = String::from("# タスクの一覧(自動生成。直接編集しない)\n\n");
    if tasks.is_empty() {
        text.push_str("タスクはありません。\n");
    }
    for task in tasks {
        let due = task.due.as_ref().map(|d| format!("締切 {d}")).unwrap_or_else(|| "締切なし".to_string());
        let folder = if task.folder.is_empty() { "未分類" } else { &task.folder };
        let mark = if task.broken { "(形が崩れています)" } else { "" };
        text.push_str(&format!("- {} | {due} | {folder} | {}{mark}\n", task.id, task.summary));
    }
    text
}

/// `INDEX.md`を書く。未完了のタスクを締切の近い順に並べ、アーカイブは入れない。
pub fn write_index(root: &Path) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("{}を作れません: {e}", root.display()))?;
    write_atomically(&root.join(INDEX_FILE), &render_index(&list(root, false)))
}

// ---- タスクの操作 ----

fn new_id(root: &Path) -> String {
    let taken: Vec<String> = list(root, false).into_iter().chain(list(root, true)).map(|t| t.id).collect();
    loop {
        let now = Local::now();
        let seed = now.timestamp_subsec_nanos() ^ std::process::id().rotate_left(12);
        let serial = ID_COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B1);
        let id = format!("task-{}-{:03x}", now.format("%Y%m%d-%H%M%S"), (seed ^ serial) & 0xfff);
        if !taken.contains(&id) {
            return id;
        }
    }
}

/// タスクを追加する。フォルダは先に作っておく(打ち間違いで増えないように)。
pub fn add(root: &Path, new: &NewTask) -> Result<Task, String> {
    let summary = require_summary(new.summary)?;
    let due = new.due.map(|d| require_date(d, "締切日")).transpose()?;
    let (folder, dir) = existing_folder(root, new.folder)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}を作れません: {e}", dir.display()))?;
    let mut front = Mapping::new();
    set_text(&mut front, "summary", &summary);
    set_text(&mut front, "created", &Local::now().format(DATE_FORMAT).to_string());
    if let Some(due) = &due {
        set_text(&mut front, "due", due);
    }
    let id = new_id(root);
    let path = dir.join(format!("{id}.md"));
    write_atomically(&path, &render_md(&front, new.detail)?)?;
    write_index(root)?;
    shiori_log::info(LOG_PART, "追加した", &[("id", &id)]);
    Ok(read_task(&path, &folder, false))
}

/// 概要・締切・詳細を変える。`expected_mtime`は画面が読んだときのmtimeで、変わっていれば保存しない。
pub fn edit(root: &Path, id: &str, change: &TaskEdit, expected_mtime: Option<f64>) -> Result<Task, String> {
    let task = find(root, id, false)?;
    if task.broken {
        return Err(format!("形が崩れているので変更できません。ファイルを直してください: {}", task.path.display()));
    }
    if expected_mtime.is_some_and(|m| m != task.mtime) {
        return Err("ほかで変更されました。読み直してください".to_string());
    }
    let summary = change.summary.map(require_summary).transpose()?;
    let due = match change.due {
        Some(Some(d)) => Some(Some(require_date(d, "締切日")?)),
        Some(None) => Some(None),
        None => None,
    };
    let text = std::fs::read_to_string(&task.path).map_err(|e| format!("読み込みに失敗: {e}"))?;
    let (mut front, body) = parse_md(&text).ok_or_else(|| format!("形が崩れています: {}", task.path.display()))?;
    if let Some(summary) = &summary {
        set_text(&mut front, "summary", summary);
    }
    match &due {
        Some(Some(d)) => set_text(&mut front, "due", d),
        Some(None) => {
            front.remove("due");
        }
        None => {}
    }
    write_atomically(&task.path, &render_md(&front, change.detail.unwrap_or(&body))?)?;
    write_index(root)?;
    shiori_log::info(LOG_PART, "変更した", &[("id", &task.id)]);
    Ok(read_task(&task.path, &task.folder, false))
}

// 上書きしないように確かめてから動かす。
fn rename_file(from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() {
        return Err(format!("同じ名前のファイルがあります: {}", to.display()));
    }
    std::fs::rename(from, to).map_err(|e| format!("{}を動かせません: {e}", from.display()))
}

fn file_name_of(path: &Path) -> Result<&std::ffi::OsStr, String> {
    path.file_name().ok_or_else(|| format!("ファイル名を読めません: {}", path.display()))
}

/// フォルダを移す。`folder`が空なら未分類へ。
pub fn move_to(root: &Path, id: &str, folder: &str) -> Result<Task, String> {
    let task = find(root, id, false)?;
    let (folder, dir) = existing_folder(root, folder)?;
    if name_key(&folder) == name_key(&task.folder) {
        return Ok(task);
    }
    let dest = dir.join(file_name_of(&task.path)?);
    rename_file(&task.path, &dest)?;
    write_index(root)?;
    shiori_log::info(LOG_PART, "移した", &[("id", &task.id)]);
    Ok(read_task(&dest, &folder, false))
}

/// 完了にしてアーカイブへ送る。元のフォルダの形を保つ。
pub fn complete(root: &Path, id: &str) -> Result<Task, String> {
    let task = find(root, id, false)?;
    let dir = ensure_folder(&archive_root(root), &folder_segments(&task.folder))?;
    let dest = dir.join(file_name_of(&task.path)?);
    rename_file(&task.path, &dest)?;
    // 崩れたmdは書き直さず、そのまま送る。
    if !task.broken {
        let today = Local::now().format(DATE_FORMAT).to_string();
        update_front(&dest, |front| set_text(front, "done", &today))?;
    }
    write_index(root)?;
    shiori_log::info(LOG_PART, "完了にした", &[("id", &task.id)]);
    Ok(read_task(&dest, &task.folder, true))
}

/// アーカイブから一覧へ戻す。フォルダが消えていたら作り直す。
pub fn restore(root: &Path, id: &str) -> Result<Task, String> {
    let task = find(root, id, true)?;
    let dir = ensure_folder(root, &folder_segments(&task.folder))?;
    let dest = dir.join(file_name_of(&task.path)?);
    rename_file(&task.path, &dest)?;
    if !task.broken {
        update_front(&dest, |front| {
            front.remove("done");
        })?;
    }
    write_index(root)?;
    shiori_log::info(LOG_PART, "一覧へ戻した", &[("id", &task.id)]);
    Ok(read_task(&dest, &task.folder, false))
}

/// タスクのファイルを消す(一覧とアーカイブのどちらからでも)。戻せない。
pub fn delete(root: &Path, id: &str) -> Result<(), String> {
    let task = find(root, id, false).or_else(|_| find(root, id, true))?;
    std::fs::remove_file(&task.path).map_err(|e| format!("消せません: {e}"))?;
    write_index(root)?;
    shiori_log::info(LOG_PART, "削除した", &[("id", &task.id)]);
    Ok(())
}

// ---- フォルダの操作 ----

/// フォルダを作る。`Unity/Tanker`なら、無ければ`Unity`も作る。
pub fn folder_add(root: &Path, folder: &str) -> Result<String, String> {
    let segments = parse_folder(folder)?;
    let name = segments.join("/");
    if let Some((existing, _)) = find_folder(root, &name) {
        return Err(format!("同じ名前のフォルダがあります: {existing}"));
    }
    ensure_folder(root, &segments)?;
    write_index(root)?;
    shiori_log::info(LOG_PART, "フォルダを作った", &[("フォルダ", &name)]);
    Ok(name)
}

// `src`の中身を、すでにある`dest`へ移す。アーカイブに同じ名前のフォルダが残っているときに使う。
fn merge_into(src: &Path, dest: &Path) -> Result<(), String> {
    for (name, path) in entries(src) {
        if name.starts_with('.') {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let key = name_key(&name);
        let same = entries(dest).into_iter().find(|(other, _)| name_key(other) == key);
        match same {
            Some((_, other)) if path.is_dir() && other.is_dir() => merge_into(&path, &other)?,
            Some((_, other)) => return Err(format!("同じ名前のものがあります: {}", other.display())),
            None => {
                let target = dest.join(file_name_of(&path)?);
                std::fs::rename(&path, &target).map_err(|e| format!("{}を動かせません: {e}", path.display()))?;
            }
        }
    }
    std::fs::remove_dir(src).map_err(|e| format!("{}を片付けられません: {e}", src.display()))
}

// `base`の中のフォルダの名前を変える(無ければ何もしない)。
fn rename_folder_in(base: &Path, from: &str, to: &[String]) -> Result<(), String> {
    let Some((from_name, from_path)) = find_folder(base, from) else {
        return Ok(());
    };
    let leaf = to.last().ok_or_else(|| "フォルダ名が空です".to_string())?;
    // 大文字・小文字だけの変更は、同じ場所で名前を変える(Windowsとexfatでは同じものに見えるため)。
    if name_key(&from_name) == name_key(&to.join("/")) {
        let parent = from_path.parent().ok_or_else(|| "親のフォルダを読めません".to_string())?;
        return std::fs::rename(&from_path, parent.join(leaf)).map_err(|e| format!("名前を変えられません: {e}"));
    }
    if let Some((_, existing)) = find_folder(base, &to.join("/")) {
        return merge_into(&from_path, &existing);
    }
    let parent = ensure_folder(base, &to[..to.len() - 1])?;
    std::fs::rename(&from_path, parent.join(leaf)).map_err(|e| format!("名前を変えられません: {e}"))
}

/// フォルダの名前を変える。アーカイブの側も一緒に変える。
pub fn folder_rename(root: &Path, from: &str, to: &str) -> Result<String, String> {
    if from.is_empty() {
        return Err("名前を変えるフォルダが空です".to_string());
    }
    let (from_name, from_path) = existing_folder(root, from)?;
    let segments = parse_folder(to)?;
    let to_name = segments.join("/");
    if from_name == to_name {
        return Ok(to_name);
    }
    let same_folder = name_key(&from_name) == name_key(&to_name);
    if !same_folder && find_folder(root, &to_name).is_some() {
        return Err(format!("同じ名前のフォルダがあります: {to_name}"));
    }
    if name_key(&to_name).starts_with(&format!("{}/", name_key(&from_name))) {
        return Err(format!("フォルダを自分の中へは移せません: {to_name}"));
    }
    let archive = archive_root(root);
    let archive_has_children = find_folder(&archive, &from_name).is_some_and(|(_, p)| !child_dirs(&p).is_empty());
    if segments.len() == MAX_FOLDER_DEPTH && (!child_dirs(&from_path).is_empty() || archive_has_children) {
        return Err(format!("中にフォルダがあるので、2段目へは移せません(3段になります): {from_name}"));
    }
    rename_folder_in(root, &from_name, &segments)?;
    rename_folder_in(&archive, &from_name, &segments)?;
    write_index(root)?;
    shiori_log::info(LOG_PART, "フォルダの名前を変えた", &[("元", &from_name), ("先", &to_name)]);
    Ok(to_name)
}

/// 空のフォルダを消す。アーカイブの側は残し、戻すときに作り直す。
pub fn folder_remove(root: &Path, folder: &str) -> Result<(), String> {
    if folder.is_empty() {
        return Err("消すフォルダが空です".to_string());
    }
    let (name, path) = existing_folder(root, folder)?;
    let inside = format!("{name}/");
    let remaining = list(root, false).iter().filter(|t| t.folder == name || t.folder.starts_with(&inside)).count();
    if remaining > 0 {
        return Err(format!("未完了のタスクが{remaining}件残っているので消せません: {name}"));
    }
    // `._`などの隠しファイルだけなら空と見なす。
    let contents = entries(&path);
    if contents.iter().any(|(n, _)| !n.starts_with('.')) {
        return Err(format!("中にフォルダやファイルが残っているので消せません: {name}"));
    }
    for (_, hidden) in contents {
        std::fs::remove_file(&hidden).map_err(|e| format!("{}を消せません: {e}", hidden.display()))?;
    }
    std::fs::remove_dir(&path).map_err(|e| format!("消せません: {e}"))?;
    write_index(root)?;
    shiori_log::info(LOG_PART, "フォルダを消した", &[("フォルダ", &name)]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shiori-tasks-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn new_task(summary: &str) -> NewTask<'_> {
        NewTask { summary, ..Default::default() }
    }

    fn index_of(root: &Path) -> String {
        std::fs::read_to_string(root.join(INDEX_FILE)).unwrap()
    }

    #[test]
    fn add_writes_md_and_index() {
        let root = temp_root("add");
        folder_add(&root, "Unity/Tanker").unwrap();
        let task = add(
            &root,
            &NewTask { summary: "砲塔を直す", folder: "Unity/Tanker", due: Some("2026-11-09"), detail: "照準が速い" },
        )
        .unwrap();
        assert!(task.id.starts_with("task-"));
        assert_eq!(task.folder, "Unity/Tanker");
        assert_eq!(task.due.as_deref(), Some("2026-11-09"));
        assert_eq!(task.detail, "照準が速い");
        assert!(root.join("Unity").join("Tanker").join(format!("{}.md", task.id)).is_file());
        let line = format!("- {} | 締切 2026-11-09 | Unity/Tanker | 砲塔を直す", task.id);
        assert!(index_of(&root).contains(&line), "{}", index_of(&root));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn add_refuses_bad_input() {
        let root = temp_root("add-bad");
        assert!(add(&root, &new_task("  ")).is_err());
        assert!(add(&root, &new_task("1行目\n2行目")).is_err());
        assert!(add(&root, &NewTask { summary: "x", due: Some("来週"), ..Default::default() }).is_err());
        assert!(add(&root, &NewTask { summary: "x", due: Some("2026-02-30"), ..Default::default() }).is_err());
        assert!(add(&root, &NewTask { summary: "x", folder: "無い", ..Default::default() }).is_err());
        assert!(list(&root, false).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn summary_with_yaml_characters_round_trips() {
        let root = temp_root("yaml");
        let summary = "直す: \"引用\" #番号 'a' - [x] {y}";
        let task = add(&root, &new_task(summary)).unwrap();
        assert!(!task.broken);
        assert_eq!(task.summary, summary);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn ids_do_not_collide_within_one_second() {
        let root = temp_root("ids");
        for _ in 0..30 {
            add(&root, &new_task("x")).unwrap();
        }
        assert_eq!(list(&root, false).len(), 30);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn edit_changes_only_what_is_given() {
        let root = temp_root("edit");
        let task = add(&root, &NewTask { summary: "元", due: Some("2026-11-09"), detail: "本文", ..Default::default() }).unwrap();
        // 手で足した知らない項目は、変更で消さない
        let text = std::fs::read_to_string(&task.path).unwrap().replacen("---\n", "---\nmemo: 残す\n", 1);
        std::fs::write(&task.path, text).unwrap();

        let edited = edit(&root, &task.id, &TaskEdit { summary: Some("新"), ..Default::default() }, None).unwrap();
        assert_eq!((edited.summary.as_str(), edited.due.as_deref(), edited.detail.as_str()), ("新", Some("2026-11-09"), "本文"));
        assert!(std::fs::read_to_string(&task.path).unwrap().contains("memo: 残す"));

        let edited = edit(&root, &task.id, &TaskEdit { due: Some(None), ..Default::default() }, None).unwrap();
        assert_eq!(edited.due, None);
        let edited = edit(&root, &task.id, &TaskEdit { detail: Some("次"), ..Default::default() }, None).unwrap();
        assert_eq!(edited.detail, "次");
        assert!(edit(&root, &task.id, &TaskEdit { due: Some(Some("x")), ..Default::default() }, None).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn edit_detects_conflict() {
        let root = temp_root("conflict");
        let task = add(&root, &new_task("元")).unwrap();
        let change = TaskEdit { summary: Some("新"), ..Default::default() };
        assert!(edit(&root, &task.id, &change, Some(task.mtime + 10.0)).unwrap_err().contains("読み直して"));
        assert_eq!(list(&root, false)[0].summary, "元");
        edit(&root, &task.id, &change, Some(task.mtime)).unwrap();
        assert_eq!(list(&root, false)[0].summary, "新");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn move_between_folders_and_to_uncategorized() {
        let root = temp_root("move");
        folder_add(&root, "Unity").unwrap();
        let task = add(&root, &new_task("x")).unwrap();
        let moved = move_to(&root, &task.id, "unity").unwrap();
        assert_eq!(moved.folder, "Unity", "大文字・小文字が違っても、あるフォルダを使う");
        assert!(root.join("Unity").join(format!("{}.md", task.id)).is_file());
        assert!(index_of(&root).contains("| Unity |"));
        let moved = move_to(&root, &task.id, "").unwrap();
        assert_eq!(moved.folder, "");
        assert!(index_of(&root).contains("| 未分類 |"));
        assert!(move_to(&root, &task.id, "無い").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn complete_and_restore_keep_the_folder() {
        let root = temp_root("archive");
        folder_add(&root, "Unity/Tanker").unwrap();
        let task = add(&root, &NewTask { summary: "x", folder: "Unity/Tanker", ..Default::default() }).unwrap();

        let done = complete(&root, &task.id).unwrap();
        assert!(done.archived);
        assert!(done.done.is_some());
        assert!(root.join(ARCHIVE_DIR).join("Unity").join("Tanker").join(format!("{}.md", task.id)).is_file());
        assert!(list(&root, false).is_empty());
        assert_eq!(list(&root, true).len(), 1);
        assert!(!index_of(&root).contains(&task.id), "アーカイブはINDEXに入れない");
        assert!(complete(&root, &task.id).is_err());

        // フォルダが消えていても、戻すときに作り直す
        folder_remove(&root, "Unity/Tanker").unwrap();
        folder_remove(&root, "Unity").unwrap();
        let back = restore(&root, &task.id).unwrap();
        assert_eq!(back.folder, "Unity/Tanker");
        assert_eq!(back.done, None);
        assert!(!back.archived);
        assert!(index_of(&root).contains(&task.id));
        assert!(list(&root, true).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn delete_works_on_both_sides() {
        let root = temp_root("delete");
        let active = add(&root, &new_task("a")).unwrap();
        let archived = add(&root, &new_task("b")).unwrap();
        complete(&root, &archived.id).unwrap();
        delete(&root, &active.id).unwrap();
        delete(&root, &archived.id).unwrap();
        assert!(list(&root, false).is_empty() && list(&root, true).is_empty());
        assert!(delete(&root, &active.id).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn ids_cannot_reach_outside() {
        let root = temp_root("outside");
        std::fs::write(root.join("secret.txt"), "x").unwrap();
        for id in ["../secret", "..\\secret", "secret.txt", ""] {
            assert!(delete(&root, id).is_err(), "{id}");
        }
        assert!(root.join("secret.txt").is_file());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn folder_names_follow_the_rules() {
        let root = temp_root("names");
        for bad in [
            "", "a/b/c", "a\\b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b", "..", ".hidden", "_archive", "末尾 ",
            "末尾.", "CON", "con", "NUL.txt", "lpt1", "a//b", "/a",
        ] {
            assert!(folder_add(&root, bad).is_err(), "{bad}");
        }
        assert!(folder_add(&root, "COMET").is_ok());
        assert!(folder_add(&root, "Unity/Tanker").is_ok());
        assert_eq!(folders(&root), vec!["COMET", "Unity", "Unity/Tanker"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn same_name_in_another_form_is_refused() {
        let root = temp_root("same");
        folder_add(&root, "Unity").unwrap();
        assert!(folder_add(&root, "unity").is_err());
        folder_add(&root, "\u{30b2}\u{30fc}\u{30e0}").unwrap(); // ゲーム(NFC)
        assert!(folder_add(&root, "\u{30b1}\u{3099}\u{30fc}\u{30e0}").is_err()); // ゲーム(NFD)
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn nfd_folder_on_disk_is_used_as_it_is() {
        // Macがexfatから返す形を、実際のフォルダ名で再現する
        let root = temp_root("nfd");
        let nfd = "\u{30b1}\u{3099}\u{30fc}\u{30e0}";
        let nfc_name = "\u{30b2}\u{30fc}\u{30e0}";
        std::fs::create_dir(root.join(nfd)).unwrap();
        assert_eq!(folders(&root), vec![nfc_name]);
        let task = add(&root, &NewTask { summary: "x", folder: nfc_name, ..Default::default() }).unwrap();
        assert_eq!(task.folder, nfc_name);
        assert!(task.path.parent().unwrap().ends_with(nfd), "見つけたパスをそのまま使う");
        let done = complete(&root, &task.id).unwrap();
        assert_eq!(done.folder, nfc_name);
        assert_eq!(restore(&root, &task.id).unwrap().folder, nfc_name);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn folder_rename_changes_the_archive_too() {
        let root = temp_root("rename");
        folder_add(&root, "Unity/Tanker").unwrap();
        let active = add(&root, &NewTask { summary: "a", folder: "Unity/Tanker", ..Default::default() }).unwrap();
        let archived = add(&root, &NewTask { summary: "b", folder: "Unity/Tanker", ..Default::default() }).unwrap();
        complete(&root, &archived.id).unwrap();

        folder_rename(&root, "Unity", "Games").unwrap();
        assert_eq!(find(&root, &active.id, false).unwrap().folder, "Games/Tanker");
        assert_eq!(find(&root, &archived.id, true).unwrap().folder, "Games/Tanker");
        assert!(index_of(&root).contains("Games/Tanker"));

        folder_rename(&root, "Games/Tanker", "Games/Tank").unwrap();
        assert_eq!(find(&root, &archived.id, true).unwrap().folder, "Games/Tank");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn folder_rename_handles_case_only_change() {
        let root = temp_root("rename-case");
        folder_add(&root, "unity").unwrap();
        let task = add(&root, &NewTask { summary: "a", folder: "unity", ..Default::default() }).unwrap();
        folder_rename(&root, "unity", "Unity").unwrap();
        assert_eq!(folders(&root), vec!["Unity"]);
        assert_eq!(find(&root, &task.id, false).unwrap().folder, "Unity");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn folder_rename_refuses_what_would_break() {
        let root = temp_root("rename-bad");
        folder_add(&root, "A/B").unwrap();
        folder_add(&root, "C").unwrap();
        assert!(folder_rename(&root, "A", "C").is_err(), "同じ名前");
        assert!(folder_rename(&root, "A", "C/A").is_err(), "3段になる");
        assert!(folder_rename(&root, "C", "C/D").is_err(), "自分の中");
        assert!(folder_rename(&root, "無い", "E").is_err());
        assert!(folder_rename(&root, "C", "_x").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn folder_rename_merges_into_a_leftover_archive_folder() {
        let root = temp_root("rename-merge");
        folder_add(&root, "Old").unwrap();
        let old = add(&root, &NewTask { summary: "old", folder: "Old", ..Default::default() }).unwrap();
        complete(&root, &old.id).unwrap();
        folder_remove(&root, "Old").unwrap();

        folder_add(&root, "New").unwrap();
        let new = add(&root, &NewTask { summary: "new", folder: "New", ..Default::default() }).unwrap();
        complete(&root, &new.id).unwrap();
        folder_rename(&root, "New", "Old").unwrap();

        let archived = list(&root, true);
        assert_eq!(archived.len(), 2);
        assert!(archived.iter().all(|t| t.folder == "Old"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn folder_remove_refuses_while_something_remains() {
        let root = temp_root("remove");
        folder_add(&root, "A/B").unwrap();
        let task = add(&root, &NewTask { summary: "x", folder: "A/B", ..Default::default() }).unwrap();
        assert!(folder_remove(&root, "A/B").unwrap_err().contains("1件"));
        assert!(folder_remove(&root, "A").unwrap_err().contains("1件"));
        complete(&root, &task.id).unwrap();
        assert!(folder_remove(&root, "A").unwrap_err().contains("残っている"), "空でない(中にフォルダ)");
        // Macが作る隠しファイルだけなら、空と見なす
        std::fs::write(root.join("A").join("B").join("._x"), "x").unwrap();
        folder_remove(&root, "A/B").unwrap();
        folder_remove(&root, "A").unwrap();
        assert!(folders(&root).is_empty());
        assert_eq!(list(&root, true)[0].folder, "A/B", "アーカイブの側は残す");
        assert!(folder_remove(&root, "無い").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn index_is_sorted_by_due_with_none_last() {
        let root = temp_root("index");
        let none = add(&root, &new_task("なし")).unwrap();
        let later = add(&root, &NewTask { summary: "後", due: Some("2026-12-01"), ..Default::default() }).unwrap();
        let soon = add(&root, &NewTask { summary: "先", due: Some("2026-11-09"), ..Default::default() }).unwrap();
        let ids: Vec<String> = list(&root, false).into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![soon.id.clone(), later.id.clone(), none.id.clone()]);
        let index = index_of(&root);
        let at = |id: &str| index.find(id).unwrap();
        assert!(at(&soon.id) < at(&later.id) && at(&later.id) < at(&none.id));
        assert!(index.contains(&format!("- {} | 締切なし | 未分類 | なし", none.id)));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn index_file_is_not_a_task() {
        let root = temp_root("index-self");
        write_index(&root).unwrap();
        assert!(list(&root, false).is_empty());
        assert!(index_of(&root).contains("タスクはありません"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn broken_md_is_listed_with_a_mark() {
        let root = temp_root("broken");
        for (name, text) in [
            ("no-front.md", "本文だけ"),
            ("unclosed.md", "---\nsummary: x\n"),
            ("bad-yaml.md", "---\nsummary: [\n---\n"),
            ("no-summary.md", "---\ncreated: 2026-11-05\n---\n"),
            ("no-created.md", "---\nsummary: x\n---\n"),
        ] {
            std::fs::write(root.join(name), text).unwrap();
        }
        let tasks = list(&root, false);
        assert_eq!(tasks.len(), 5);
        assert!(tasks.iter().all(|t| t.broken && t.summary.ends_with(".md")));
        write_index(&root).unwrap();
        assert!(index_of(&root).contains("no-front.md(形が崩れています)"));

        // 中身を壊す変更は断り、移動と完了と削除はできる
        assert!(edit(&root, "no-front", &TaskEdit { summary: Some("新"), ..Default::default() }, None).is_err());
        let done = complete(&root, "no-front").unwrap();
        assert!(done.broken && done.archived);
        assert_eq!(std::fs::read_to_string(&done.path).unwrap(), "本文だけ");
        assert_eq!(restore(&root, "no-front").unwrap().detail, "本文だけ");
        delete(&root, "no-front").unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unreadable_due_counts_as_no_due_and_stays_listed() {
        let root = temp_root("due");
        std::fs::write(root.join("a.md"), "---\nsummary: x\ncreated: 2026-11-05\ndue: 来週\n---\n").unwrap();
        std::fs::write(root.join("b.md"), "---\nsummary: y\ncreated: 2026-11-05\ndue: 20261109\n---\n").unwrap();
        let tasks = list(&root, false);
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().all(|t| t.due.is_none() && !t.broken));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn crlf_and_bom_are_read() {
        let root = temp_root("crlf");
        std::fs::write(root.join("a.md"), "\u{feff}---\r\nsummary: x\r\ncreated: 2026-11-05\r\n---\r\n\r\n本文\r\n").unwrap();
        let task = &list(&root, false)[0];
        assert!(!task.broken);
        assert_eq!(task.detail, "本文");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn hidden_files_and_deep_folders_are_not_read() {
        let root = temp_root("skip");
        let good = "---\nsummary: x\ncreated: 2026-11-05\n---\n";
        std::fs::create_dir_all(root.join("A").join("B").join("C")).unwrap();
        std::fs::write(root.join("._a.md"), "binary").unwrap();
        std::fs::write(root.join(".a.md.tmp"), good).unwrap();
        std::fs::write(root.join("memo.txt"), good).unwrap();
        std::fs::write(root.join("A").join("B").join("C").join("deep.md"), good).unwrap();
        std::fs::write(root.join("A").join("B").join("ok.md"), good).unwrap();
        let tasks = list(&root, false);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "ok");
        assert_eq!(folders(&root), vec!["A", "A/B"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_root_is_empty() {
        let parent = temp_root("missing");
        let root = parent.join("無い");
        assert!(list(&root, false).is_empty());
        assert!(folders(&root).is_empty());
        std::fs::remove_dir_all(&parent).unwrap();
    }
}
