//! 規約・記憶画面(詩織Ver4.1、memoryの閲覧はVer4.2)向け。詩織のシステムを決めている書類
//! (libraryの保存規約、タグ/プロジェクトの台帳、起動時のプロフィール、プロンプト、設定)と、
//! 共通記憶MD(全体用・各プロジェクトのmemory)を、読み取り専用で一覧・取得する。
//!
//! これらは索引の対象外(`library/_system/`はRAGが除外している)で、図書館の画面には
//! 出ない。画面から任意のパスを読めてしまわないよう、**読める書類はここで組み立てる
//! 許可リストだけ**にしている。呼び出し側が渡すのは書類のidだけで、idからパスを作らず、
//! 許可リストの中から一致するものを探す(`../`などを渡しても一致せず、読めない)。

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::layout::Layout;

// 1ファイルの読み取り上限。規約・設定は数十KBなので、これを超えるものは想定外として読まない。
const MAX_BYTES: u64 = 1024 * 1024;

// 画面の見出し(タブ)。画面の左から、この順に並べる。
const TAB_MEMORY: &str = "memory";
const TAB_SKILL: &str = "skill";
const TAB_RULES: &str = "rules";
const TAB_BEHAVIOR: &str = "behavior";

// 見出しの中の小見出し(sub)。出現順がそのまま画面の順になる。
const SUB_GLOBAL: &str = "全体";
const SUB_PROJECT: &str = "プロジェクト";
const SUB_AREA: &str = "領域";
const SUB_DEV: &str = "開発";
const SUB_SHELF: &str = "知識の棚";
const SUB_LIBRARY: &str = "libraryの規約";
const SUB_LEDGER: &str = "台帳";
const SUB_PROFILE: &str = "起動時のプロフィール";
const SUB_BEHAVIOR: &str = "詩織の動作";

// 「知識の棚」のスキル(それ以外のスキルは「開発」)。増えたらここに足す。
const SHELF_SKILLS: [&str; 2] = ["knowledge-learning", "knowledge-research"];

pub struct Entry {
    id: String,
    tab: &'static str,
    sub: &'static str,
    label: String,
    path: PathBuf,
    // 技術別の参照ファイルが持つ、親スキルのid。
    parent_id: Option<String>,
    // memoryだけが持つ、frontmatterの更新日時。
    updated: Option<String>,
}

impl Entry {
    fn new(id: String, tab: &'static str, sub: &'static str, label: String, path: PathBuf) -> Self {
        Entry { id, tab, sub, label, path, parent_id: None, updated: None }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemDocDto {
    pub id: String,
    // 見出し。"memory" | "skill" | "rules" | "behavior"。
    pub tab: String,
    // 見出しの中の小見出し。
    pub sub: String,
    pub parent_id: Option<String>,
    pub updated: Option<String>,
    pub label: String,
    pub file_name: String,
    // 画面での表示方法。"markdown" | "yaml" | "json" | "text"。
    pub format: String,
    pub exists: bool,
    pub size: u64,
    pub mtime: f64,
}

fn format_of(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("md") => "markdown",
        Some("yaml") | Some("yml") => "yaml",
        Some("json") => "json",
        _ => "text",
    }
}

// フォルダ直下のjson(隠しファイルとmacOSの`._`は除く)を名前順で返す。
fn json_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = read
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().and_then(|e| e.to_str()) == Some("json")
                && !p.file_name().and_then(|n| n.to_str()).unwrap_or(".").starts_with('.')
        })
        .collect();
    files.sort();
    files
}

fn file_name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

// memoryのfrontmatterにある`updated`(最終更新日時のISO8601文字列)。無い・読めないときは空文字。
// 同じ書式(+09:00)で書かれるので、文字列のまま新しい順に並べられる。
fn memory_updated(path: &Path) -> String {
    let Ok(meta) = std::fs::metadata(path) else {
        return String::new();
    };
    if meta.len() > MAX_BYTES {
        return String::new();
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return String::new();
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some(value) = line.strip_prefix("updated:") {
            return value.trim().trim_matches('"').to_string();
        }
    }
    String::new()
}

// `10-projects/{project}/memory.md`・`20-areas/{project}/memory.md`をフォルダから探し、
// updatedの新しい順で(project id, 小見出し, updated, パス)を返す。新しいプロジェクトのmemoryは、
// コードを直さず一覧に出る。idに使うproject idは、フォルダ名がそのまま入るので、`/`や`.`始まりは除く。
fn project_memories(library_root: &Path, layout: Layout) -> Vec<(String, &'static str, String, PathBuf)> {
    let mut found: Vec<(String, &'static str, String, PathBuf)> = Vec::new();
    // 置き場は、V7では `20-projects` だけ(`20-areas` は廃止)
    let tops: &[&str] = match layout {
        Layout::Legacy => &["10-projects", "20-areas"],
        Layout::V7 => &["20-projects"],
    };
    for top in tops.iter().copied() {
        let Ok(read) = std::fs::read_dir(library_root.join(top)) else {
            continue;
        };
        for dir in read.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()) {
            let name = file_name_of(&dir);
            if name.starts_with('.') || name.starts_with('_') {
                continue;
            }
            let memory = dir.join("memory.md");
            if memory.is_file() && !found.iter().any(|(n, _, _, _)| *n == name) {
                let sub = if top == "20-areas" { SUB_AREA } else { SUB_PROJECT };
                found.push((name, sub, memory_updated(&memory), memory));
            }
        }
    }
    // updatedの新しい順。同じ・無い場合はproject idの名前順で安定させる。
    found.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    found
}

// `30-resources/skills/`のスキルを、親→子の順で返す(開発→知識の棚、それぞれ名前順)。
// `親名--技術名.md`は親の参照ファイルで、親が無いときは独立した行にする。
// 隠しファイルとmacOSの`._`は除く。新しいスキルは、コードを直さず一覧に出る。
fn skill_entries(library_root: &Path) -> Vec<Entry> {
    let dir = library_root.join("30-resources").join("skills");
    let Ok(read) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut stems: Vec<String> = read
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("md"))
        .map(|p| file_name_of(&p))
        .filter(|n| !n.starts_with('.'))
        .map(|n| n.trim_end_matches(".md").to_string())
        .collect();
    stems.sort();

    let parent_of = |stem: &str| -> Option<String> {
        let (parent, _) = stem.split_once("--")?;
        stems.iter().any(|s| s == parent).then(|| parent.to_string())
    };
    let sub_of = |parent: &str| if SHELF_SKILLS.contains(&parent) { SUB_SHELF } else { SUB_DEV };

    let mut parents: Vec<&String> = stems.iter().filter(|s| parent_of(s).is_none()).collect();
    // 開発が先、知識の棚が後。同じ小見出しの中は名前順(stemsが整列済みで、この並べ替えは安定)。
    parents.sort_by_key(|s| sub_of(s) == SUB_SHELF);

    let mut list = Vec::new();
    for parent in parents {
        let sub = sub_of(parent);
        list.push(Entry::new(format!("skill/{parent}"), TAB_SKILL, sub, parent.clone(), dir.join(format!("{parent}.md"))));
        for child in stems.iter().filter(|s| parent_of(s).as_deref() == Some(parent.as_str())) {
            let suffix = child.split_once("--").map(|(_, rest)| rest).unwrap_or(child);
            let mut entry = Entry::new(format!("skill/{child}"), TAB_SKILL, sub, suffix.to_string(), dir.join(format!("{child}.md")));
            entry.parent_id = Some(format!("skill/{parent}"));
            list.push(entry);
        }
    }
    list
}

/// libraryにある書類(memory・スキル・規約・台帳・起動時のプロフィール)。置き場は `layout` で決まる。
pub fn library_entries(library_root: &Path, layout: Layout) -> Vec<Entry> {
    let system = library_root.join("_system");
    let organizing = system.join("library-organizing");
    let mut list = Vec::new();

    let global = library_root.join(layout.global_memory_rel());
    let mut entry = Entry::new("memory/global".into(), TAB_MEMORY, SUB_GLOBAL, "全体のmemory(memory-global.md)".into(), global.clone());
    entry.updated = Some(memory_updated(&global)).filter(|u| !u.is_empty());
    list.push(entry);
    for (project, sub, updated, path) in project_memories(library_root, layout) {
        let mut entry = Entry::new(format!("memory/project/{project}"), TAB_MEMORY, sub, format!("{project}のmemory"), path);
        entry.updated = Some(updated).filter(|u| !u.is_empty());
        list.push(entry);
    }

    list.extend(skill_entries(library_root));

    let mut add = |id: String, tab: &'static str, sub: &'static str, label: String, path: PathBuf| {
        list.push(Entry::new(id, tab, sub, label, path));
    };

    add("library/CLAUDE.md".into(), TAB_RULES, SUB_LIBRARY, "保存の規約(CLAUDE.md)".into(), system.join("CLAUDE.md"));
    for (file, label) in [
        ("01-reading-guide.md", "整理資料1: 読み方"),
        ("02-procedure.md", "整理資料2: 進め方"),
        ("03-criteria.md", "整理資料3: 判断基準"),
    ] {
        add(
            format!("library/library-organizing/{file}"),
            TAB_RULES, SUB_LIBRARY,
            label.into(),
            organizing.join(file),
        );
    }

    add("ledger/tags.yaml".into(), TAB_RULES, SUB_LEDGER, "タグ台帳(tags.yaml)".into(), system.join("tags.yaml"));
    add(
        "ledger/projects.yaml".into(),
        TAB_RULES, SUB_LEDGER,
        "プロジェクト台帳(projects.yaml)".into(),
        system.join("projects.yaml"),
    );

    add(
        "profile/tier1-profile.md".into(),
        TAB_BEHAVIOR, SUB_PROFILE,
        "起動時に読むプロフィール(tier1-profile.md)".into(),
        system.join("tier1-profile.md"),
    );

    list
}

/// アプリの手元にある書類(プロンプト・ツール定義・整形ルール・設定)。サーバーには無い。
pub fn local_entries(project_root: &Path) -> Vec<Entry> {
    let prompts = project_root.join("prompts");
    let mut list = Vec::new();
    let mut add = |id: String, tab: &'static str, sub: &'static str, label: String, path: PathBuf| {
        list.push(Entry::new(id, tab, sub, label, path));
    };

    add(
        "prompts/system-prompt.md".into(),
        TAB_BEHAVIOR, SUB_BEHAVIOR,
        "システムプロンプト(system-prompt.md)".into(),
        prompts.join("system-prompt.md"),
    );
    for p in json_files(&prompts.join("tools")) {
        let name = file_name_of(&p);
        add(format!("prompts/tools/{name}"), TAB_BEHAVIOR, SUB_BEHAVIOR, format!("ツール定義({name})"), p);
    }
    for p in json_files(&prompts.join("transforms")) {
        let name = file_name_of(&p);
        add(format!("prompts/transforms/{name}"), TAB_BEHAVIOR, SUB_BEHAVIOR, format!("整形ルール({name})"), p);
    }
    add("config/config.json".into(), TAB_BEHAVIOR, SUB_BEHAVIOR, "設定(config.json)".into(), project_root.join("config.json"));

    list
}

fn to_dto(e: Entry) -> SystemDocDto {
    let meta = std::fs::metadata(&e.path).ok().filter(|m| m.is_file());
    SystemDocDto {
        format: format_of(&e.path).to_string(),
        file_name: file_name_of(&e.path),
        exists: meta.is_some(),
        size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        mtime: meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0),
        id: e.id,
        tab: e.tab.to_string(),
        sub: e.sub.to_string(),
        parent_id: e.parent_id,
        updated: e.updated,
        label: e.label,
    }
}

/// libraryの書類の一覧(サーバーが返す)。
pub fn list_library(library_root: &Path, layout: Layout) -> Vec<SystemDocDto> {
    library_entries(library_root, layout).into_iter().map(to_dto).collect()
}

/// アプリの手元の書類の一覧(プロンプト・設定)。
pub fn list_local(project_root: &Path) -> Vec<SystemDocDto> {
    local_entries(project_root).into_iter().map(to_dto).collect()
}

/// 全体の一覧(libraryの書類と、手元の書類)。手元のlibraryを使う(サーバーに接続していない)ときの形。
pub fn list_with(library_root: &Path, layout: Layout, project_root: &Path) -> Vec<SystemDocDto> {
    let mut docs = list_library(library_root, layout);
    docs.extend(list_local(project_root));
    docs
}

pub fn list(library_root: &Path, project_root: &Path) -> Vec<SystemDocDto> {
    list_with(library_root, Layout::Legacy, project_root)
}

fn read_entry(entry: Option<Entry>, id: &str) -> Result<String, String> {
    let entry = entry.ok_or_else(|| format!("規約画面で読める書類ではありません: {id}"))?;
    let meta = std::fs::metadata(&entry.path)
        .ok()
        .filter(|m| m.is_file())
        .ok_or_else(|| format!("ファイルが見つかりません: {}", entry.path.display()))?;
    if meta.len() > MAX_BYTES {
        return Err(format!("大きすぎて表示できません({}バイト): {}", meta.len(), entry.path.display()));
    }
    std::fs::read_to_string(&entry.path)
        .map_err(|e| format!("読み込みに失敗({}): {e}", entry.path.display()))
}

/// libraryの書類を読む(サーバーが使う)。許可リストのidだけ。
pub fn read_library(library_root: &Path, layout: Layout, id: &str) -> Result<String, String> {
    read_entry(library_entries(library_root, layout).into_iter().find(|e| e.id == id), id)
}

/// アプリの手元の書類を読む。許可リストのidだけ。
pub fn read_local(project_root: &Path, id: &str) -> Result<String, String> {
    read_entry(local_entries(project_root).into_iter().find(|e| e.id == id), id)
}

pub fn read_with(library_root: &Path, layout: Layout, project_root: &Path, id: &str) -> Result<String, String> {
    let entry = library_entries(library_root, layout)
        .into_iter()
        .chain(local_entries(project_root))
        .find(|e| e.id == id);
    read_entry(entry, id)
}

pub fn read(library_root: &Path, project_root: &Path, id: &str) -> Result<String, String> {
    read_with(library_root, Layout::Legacy, project_root, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    // library/とportable直下(config.jsonとprompts/)を再現した一時フォルダ。
    fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("shiori-system-docs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let library = base.join("library");
        let root = base.join("portable");
        std::fs::create_dir_all(library.join("_system").join("library-organizing")).unwrap();
        std::fs::create_dir_all(root.join("prompts").join("tools")).unwrap();
        std::fs::create_dir_all(root.join("prompts").join("transforms")).unwrap();
        (base, library, root)
    }

    #[test]
    fn 許可リストはグループの順に並び不在のファイルはexistsがfalseになる() {
        let (base, library, root) = fixture("list");
        std::fs::write(library.join("_system").join("CLAUDE.md"), "# 規約").unwrap();
        std::fs::write(root.join("prompts").join("tools").join("search_knowledge.json"), "{}").unwrap();

        let docs = list(&library, &root);
        let tabs: Vec<&str> = docs.iter().map(|d| d.tab.as_str()).collect();
        let mut dedup = tabs.clone();
        dedup.dedup();
        assert_eq!(dedup, [TAB_MEMORY, TAB_RULES, TAB_BEHAVIOR]);
        // 規約の見出しには保存の規約・整理資料・台帳、詩織の動作には起動時のプロフィールなどが入る。
        let tab_of = |id: &str| docs.iter().find(|d| d.id == id).unwrap().tab.clone();
        assert_eq!(tab_of("library/CLAUDE.md"), TAB_RULES);
        assert_eq!(tab_of("ledger/projects.yaml"), TAB_RULES);
        assert_eq!(tab_of("profile/tier1-profile.md"), TAB_BEHAVIOR);
        assert_eq!(tab_of("config/config.json"), TAB_BEHAVIOR);

        let claude = docs.iter().find(|d| d.id == "library/CLAUDE.md").unwrap();
        assert!(claude.exists && claude.size > 0 && claude.format == "markdown");
        let tags = docs.iter().find(|d| d.id == "ledger/tags.yaml").unwrap();
        assert!(!tags.exists && tags.format == "yaml");
        assert!(docs.iter().any(|d| d.id == "prompts/tools/search_knowledge.json" && d.exists));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn プロンプトのjsonは名前順で並び隠しファイルと対象外の拡張子は除く() {
        let (base, library, root) = fixture("json");
        let transforms = root.join("prompts").join("transforms");
        for name in ["query-normalization.json", "person-correction.json", "._person-correction.json", ".hidden.json", "memo.txt"] {
            std::fs::write(transforms.join(name), "{}").unwrap();
        }
        let ids: Vec<String> = list(&library, &root)
            .into_iter()
            .filter(|d| d.id.starts_with("prompts/transforms/"))
            .map(|d| d.id)
            .collect();
        assert_eq!(
            ids,
            ["prompts/transforms/person-correction.json", "prompts/transforms/query-normalization.json"]
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 読めるのは許可リストのidだけでパスの指定は通らない() {
        let (base, library, root) = fixture("read");
        std::fs::write(library.join("_system").join("CLAUDE.md"), "# 規約").unwrap();
        std::fs::write(base.join("secret.txt"), "秘密").unwrap();

        assert_eq!(read(&library, &root, "library/CLAUDE.md").unwrap(), "# 規約");
        for bad in ["../secret.txt", "library/../../secret.txt", "secret.txt", "", "library/CLAUDE.md/.."] {
            assert!(read(&library, &root, bad).is_err(), "{bad}は読めてはいけない");
        }
        // 許可リストにあっても、ファイルが無ければエラー。
        assert!(read(&library, &root, "ledger/tags.yaml").is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 上限を超えるファイルは読まない() {
        let (base, library, root) = fixture("big");
        std::fs::write(root.join("config.json"), vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(read(&library, &root, "config/config.json").unwrap_err().contains("大きすぎ"));
        std::fs::remove_dir_all(&base).unwrap();
    }

    // memoryを置いたlibraryの一時フォルダ。updatedを指定して作る。
    fn write_memory(library: &Path, dir: &str, project: &str, updated: Option<&str>) {
        let folder = library.join(dir).join(project);
        std::fs::create_dir_all(&folder).unwrap();
        let fm = match updated {
            Some(u) => format!("---
title: {project}
updated: {u}
---
本文"),
            None => format!("---
title: {project}
---
本文"),
        };
        std::fs::write(folder.join("memory.md"), fm).unwrap();
    }

    #[test]
    fn memoryは全体用が先頭でプロジェクトはupdatedの新しい順に並ぶ() {
        let (base, library, root) = fixture("memory-order");
        std::fs::create_dir_all(library.join("40-profile")).unwrap();
        std::fs::write(library.join("40-profile").join("memory-global.md"), "---
updated: 2026-10-01T00:00:00+09:00
---
全体").unwrap();
        write_memory(&library, "20-areas", "old", Some("2026-10-02T00:00:00+09:00"));
        write_memory(&library, "10-projects", "new", Some("\"2026-10-10T00:00:00+09:00\""));
        write_memory(&library, "20-areas", "no-updated", None);
        // 隠し・_system風のフォルダ、memory.mdが無いフォルダは出さない。
        write_memory(&library, "20-areas", ".hidden", Some("2026-10-11T00:00:00+09:00"));
        std::fs::create_dir_all(library.join("20-areas").join("empty")).unwrap();

        let ids: Vec<String> = list(&library, &root)
            .into_iter()
            .filter(|d| d.tab == TAB_MEMORY)
            .map(|d| d.id)
            .collect();
        assert_eq!(
            ids,
            ["memory/global", "memory/project/new", "memory/project/old", "memory/project/no-updated"]
        );
        assert_eq!(read(&library, &root, "memory/project/new").unwrap(), "---
title: new
updated: \"2026-10-10T00:00:00+09:00\"
---
本文");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn memoryのidにパスを渡しても読めない() {
        let (base, library, root) = fixture("memory-path");
        write_memory(&library, "20-areas", "shiori", Some("2026-10-10T00:00:00+09:00"));
        std::fs::write(base.join("secret.txt"), "秘密").unwrap();
        for bad in ["memory/project/../../secret.txt", "memory/project/shiori/../x", "memory/project/", "memory/../secret.txt", "memory/project/missing"] {
            assert!(read(&library, &root, bad).is_err(), "{bad}は読めてはいけない");
        }
        assert!(read(&library, &root, "memory/project/shiori").is_ok());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn memoryは小見出しとupdatedを持つ() {
        let (base, library, root) = fixture("memory-sub");
        write_memory(&library, "10-projects", "proj", Some("2026-10-10T00:00:00+09:00"));
        write_memory(&library, "20-areas", "area", None);
        let docs = list(&library, &root);
        let find = |id: &str| docs.iter().find(|d| d.id == id).unwrap();
        assert_eq!(find("memory/global").sub, SUB_GLOBAL);
        assert_eq!(find("memory/project/proj").sub, SUB_PROJECT);
        assert_eq!(find("memory/project/proj").updated.as_deref(), Some("2026-10-10T00:00:00+09:00"));
        assert_eq!(find("memory/project/area").sub, SUB_AREA);
        assert_eq!(find("memory/project/area").updated, None);
        std::fs::remove_dir_all(&base).unwrap();
    }

    // skills/に空のスキルを置く。
    fn write_skills(library: &Path, names: &[&str]) {
        let dir = library.join("30-resources").join("skills");
        std::fs::create_dir_all(&dir).unwrap();
        for name in names {
            std::fs::write(dir.join(name), "# スキル").unwrap();
        }
    }

    #[test]
    fn スキルは開発と知識の棚に分かれ参照ファイルは親の下に並ぶ() {
        let (base, library, root) = fixture("skills");
        write_skills(
            &library,
            &[
                "work-start.md",
                "implementation-execution.md",
                "implementation-execution--tauri-principles.md",
                "implementation-execution--web-principles.md",
                "knowledge-learning.md",
                "knowledge-research.md",
                // macOSの`._`と隠しファイル、mdでないものは出さない。
                "._work-start.md",
                ".hidden.md",
                "memo.txt",
                // 親が無い参照ファイルは、独立した行になる。
                "orphan--detail.md",
            ],
        );
        let skills: Vec<_> = list(&library, &root).into_iter().filter(|d| d.tab == TAB_SKILL).collect();
        let rows: Vec<(&str, &str, Option<&str>)> =
            skills.iter().map(|d| (d.id.as_str(), d.sub.as_str(), d.parent_id.as_deref())).collect();
        assert_eq!(
            rows,
            [
                ("skill/implementation-execution", SUB_DEV, None),
                ("skill/implementation-execution--tauri-principles", SUB_DEV, Some("skill/implementation-execution")),
                ("skill/implementation-execution--web-principles", SUB_DEV, Some("skill/implementation-execution")),
                ("skill/orphan--detail", SUB_DEV, None),
                ("skill/work-start", SUB_DEV, None),
                ("skill/knowledge-learning", SUB_SHELF, None),
                ("skill/knowledge-research", SUB_SHELF, None),
            ]
        );
        // 親は名前、子は親の名前を除いた技術名で出る。
        assert_eq!(skills[0].label, "implementation-execution");
        assert_eq!(skills[1].label, "tauri-principles");
        assert_eq!(skills[3].label, "orphan--detail");
        assert!(skills.iter().all(|d| d.exists && d.format == "markdown"));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn スキルのidにパスを渡しても読めない() {
        let (base, library, root) = fixture("skills-path");
        write_skills(&library, &["work-start.md"]);
        std::fs::write(base.join("secret.txt"), "秘密").unwrap();
        assert_eq!(read(&library, &root, "skill/work-start").unwrap(), "# スキル");
        for bad in ["skill/../../secret.txt", "skill/work-start/..", "skill/", "skill/missing", "skill/._work-start"] {
            assert!(read(&library, &root, bad).is_err(), "{bad}は読めてはいけない");
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn v7では置き場が変わりmemoryは20_projectsと10_commonから出る() {
        let (base, library, _root) = fixture("v7");
        std::fs::create_dir_all(library.join("10-common")).unwrap();
        std::fs::write(library.join("10-common/memory-global.md"), "---
updated: 2026-11-01T00:00:00+09:00
---
全体").unwrap();
        write_memory(&library, "20-projects", "shiori", Some("2026-11-22T00:00:00+09:00"));
        // 旧い置き場にあるものは、V7では出さない
        write_memory(&library, "20-areas", "old", Some("2026-10-02T00:00:00+09:00"));
        let docs = list_library(&library, Layout::V7);
        let ids: Vec<&str> = docs.iter().filter(|d| d.tab == TAB_MEMORY).map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["memory/global", "memory/project/shiori"]);
        assert_eq!(docs[0].updated.as_deref(), Some("2026-11-01T00:00:00+09:00"));
        assert!(read_library(&library, Layout::V7, "memory/project/shiori").unwrap().contains("本文"));
        assert!(read_library(&library, Layout::V7, "memory/project/old").is_err());
        // libraryの一覧には、手元の書類(プロンプト・設定)は入らない
        assert!(docs.iter().all(|d| d.tab != TAB_BEHAVIOR || d.id.starts_with("profile/")));
        assert!(read_library(&library, Layout::V7, "config/config.json").is_err(), "サーバーは手元の設定を読まない");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 手元の書類はlibraryと別に一覧と本文を出せる() {
        let (base, _library, root) = fixture("local");
        std::fs::write(root.join("config.json"), "{}").unwrap();
        let ids: Vec<String> = list_local(&root).into_iter().map(|d| d.id).collect();
        assert!(ids.contains(&"config/config.json".to_string()));
        assert!(!ids.iter().any(|i| i.starts_with("memory/") || i.starts_with("library/")));
        assert_eq!(read_local(&root, "config/config.json").unwrap(), "{}");
        assert!(read_local(&root, "library/CLAUDE.md").is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
