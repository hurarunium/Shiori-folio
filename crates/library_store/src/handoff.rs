//! ホームの「申し送り」を、共通記憶MD(kind: memory、詩織Ver4.1)から作るための解析と、サーバー(V7)が使う、
//! libraryから選ぶ処理(`latest`)。
//!
//! 以前は、最も新しいjournalの「申し送り」の節を拾っていた。journalは「その日の記録」で、
//! 片付いた後も残り、プロジェクトもまたいで選ばれる。memoryの「やること」「申し送り・待ち」は、
//! 完了したら削除される「いま有効な分」の正本なので、ホームはそちらを出す。
//! 手元のlibraryを使うアプリは、RAGの一覧から選ぶ(アプリ側)。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{fsx, layout::Layout};

/// memoryの「やること」と「申し送り・待ち」の項目(飾りを外した1行ずつ)。
#[derive(Debug, Default, PartialEq)]
pub struct MemorySections {
    pub todos: Vec<String>,
    pub notes: Vec<String>,
}

/// ホームの申し送り(詩織Ver3.9。Ver4.1でmemoryの「やること」「申し送り・待ち」を優先)。
/// memoryから作ったとき(`from_memory`)は`todos`・`notes`を、journalから作ったときは`items`を使う。
#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub title: String,
    pub project: String,
    pub date: String,
    pub source: String,
    pub source_category: String,
    pub items: Vec<String>,
    pub from_memory: bool,
    pub todos: Vec<String>,
    pub notes: Vec<String>,
}

/// library/からの相対パスが、プロジェクト単位のmemory(`10-projects/{project}/memory.md`・
/// `20-areas/{project}/memory.md`・V7の`20-projects/{project}/memory.md`)か。全体用のmemoryは含めない。
pub fn is_project_memory_path(relative_path: &str) -> bool {
    let normalized = relative_path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').collect();
    parts.len() == 3 && matches!(parts[0], "10-projects" | "20-areas" | "20-projects") && parts[2] == "memory.md"
}

enum Section {
    Todos,
    Notes,
}

// 太字・コード・リンクの飾りと、チェックボックス(`[ ] `・`[x] `)を外した1行にする。
fn clean_item(raw: &str) -> String {
    let text = raw.replace("**", "").replace('`', "").replace("[[", "").replace("]]", "");
    let text = text.trim();
    for prefix in ["[ ] ", "[x] ", "[X] "] {
        if let Some(rest) = text.strip_prefix(prefix) {
            return rest.trim().to_string();
        }
    }
    text.to_string()
}

/// 「やること」の見出しの節と、「申し送り」を含む見出しの節から、先頭階層の箇条書きを取り出す。
/// 入れ子の項目(字下げした行)は、親の補足なので拾わない。
pub fn memory_sections(content: &str) -> MemorySections {
    let mut sections = MemorySections::default();
    let mut current: Option<Section> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let title = trimmed.trim_start_matches('#').trim();
            current = if title == "やること" {
                Some(Section::Todos)
            } else if title.contains("申し送り") {
                Some(Section::Notes)
            } else {
                None
            };
            continue;
        }
        let Some(section) = &current else { continue };
        let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) else {
            continue;
        };
        let item = clean_item(item);
        if item.is_empty() {
            continue;
        }
        match section {
            Section::Todos => sections.todos.push(item),
            Section::Notes => sections.notes.push(item),
        }
    }
    sections
}

/// 「申し送り」を含む見出しの節から、箇条書きの各項目を飾りを外した1行ずつにして返す(journal用)。
/// 入れ子の項目は親にまとめず、そのまま1行にする。
pub fn journal_items(content: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut in_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            in_section = trimmed.contains("申し送り");
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            items.push(item.replace("**", "").replace('`', "").replace("[[", "").replace("]]", ""));
        }
    }
    items
}

struct Front {
    title: String,
    project: String,
    /// 新しさの比べ(`updated`、無ければ`date`)。同じ形のISO8601なので、文字列の比較で足りる
    stamp: String,
}

fn yaml_text(v: Option<&serde_yaml::Value>) -> String {
    match v {
        Some(serde_yaml::Value::String(s)) => s.trim().to_string(),
        Some(serde_yaml::Value::Null) | None => String::new(),
        Some(other) => serde_yaml::to_string(other).unwrap_or_default().trim().to_string(),
    }
}

fn front_of(content: &str) -> Front {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let fm: Option<serde_yaml::Value> = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
        .and_then(|rest| rest.find("\n---").map(|end| &rest[..end]))
        .and_then(|yaml| serde_yaml::from_str(yaml).ok());
    let get = |k: &str| fm.as_ref().and_then(|v| v.get(k));
    let updated = yaml_text(get("updated"));
    let stamp = if updated.is_empty() { yaml_text(get("date")) } else { updated };
    Front { title: yaml_text(get("title")), project: yaml_text(get("project")), stamp }
}

fn read_text(root: &Path, rel: &str) -> Option<String> {
    let (bytes, _) = fsx::read(root, rel).ok()?;
    String::from_utf8(bytes).ok()
}

/// ホームの申し送りを、libraryから選ぶ(サーバー用)。各プロジェクトのmemory(`updated`が最も新しいもの)の
/// 「やること」「申し送り・待ち」を優先し、無ければ、各プロジェクトの最新の日誌から「申し送り」の節を拾う。
pub fn latest(root: &Path, layout: Layout) -> Option<Handoff> {
    let tops: &[&str] = match layout {
        Layout::Legacy => &["10-projects", "20-areas"],
        Layout::V7 => &["20-projects"],
    };
    let mut memory: Option<(String, Handoff)> = None;
    let mut journal: Option<Handoff> = None;
    for top in tops.iter().copied() {
        let mut names = fsx::list_names(root, top).unwrap_or_default();
        names.sort();
        for name in names {
            if name.starts_with(['.', '_']) {
                continue;
            }
            let memory_rel = format!("{top}/{name}/memory.md");
            if let Some(content) = read_text(root, &memory_rel) {
                let sections = memory_sections(&content);
                if !(sections.todos.is_empty() && sections.notes.is_empty()) {
                    let f = front_of(&content);
                    let h = Handoff {
                        title: f.title,
                        project: if f.project.is_empty() { name.clone() } else { f.project },
                        date: f.stamp.chars().take(10).collect(),
                        source: format!("{name}/memory.md"),
                        source_category: top.to_string(),
                        from_memory: true,
                        todos: sections.todos,
                        notes: sections.notes,
                        ..Default::default()
                    };
                    if memory.as_ref().is_none_or(|(s, _)| f.stamp > *s) {
                        memory = Some((f.stamp, h));
                    }
                }
            }
            // 日誌(新しい日から順に、申し送りを持つ最初のもの)
            let jdir = format!("{top}/{name}/journal");
            let mut files: Vec<_> =
                fsx::list_entries(root, &jdir).unwrap_or_default().into_iter().filter(|e| e.name.ends_with(".md")).collect();
            files.sort_by(|a, b| b.name.cmp(&a.name));
            for f in files.into_iter().take(5) {
                let Some(content) = read_text(root, &format!("{jdir}/{}", f.name)) else { continue };
                let items = journal_items(&content);
                if items.is_empty() {
                    continue;
                }
                let date = f.name.trim_end_matches(".md").to_string();
                if journal.as_ref().is_none_or(|j| date > j.date) {
                    journal = Some(Handoff {
                        title: front_of(&content).title,
                        project: name.clone(),
                        date,
                        source: format!("{name}/journal/{}", f.name),
                        source_category: top.to_string(),
                        items,
                        ..Default::default()
                    });
                }
                break;
            }
        }
    }
    memory.map(|(_, h)| h).or(journal)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMORY: &str = "---\ntitle: 詩織の共通記憶\nkind: memory\n---\n\n## 概要\n\n- これは概要なので拾わない\n\n## 現在の状態\n\n- 状態も拾わない\n\n## やること\n\n- [ ] Macで **ビルド** する。(10/10)\n- [x] 済んだもの\n  - 入れ子は拾わない\n- 通常の箇条書き\n\n## 申し送り・待ち\n\n- `tags.yaml` に [[設計]] が無い。(10/10)\n- \n\n## その他\n\n- これも拾わない\n";

    #[test]
    fn やることと申し送りの先頭階層の項目だけを飾りとチェックボックスを外して取り出す() {
        let s = memory_sections(MEMORY);
        assert_eq!(s.todos, ["Macで ビルド する。(10/10)", "済んだもの", "通常の箇条書き"]);
        assert_eq!(s.notes, ["tags.yaml に 設計 が無い。(10/10)"]);
    }

    #[test]
    fn 該当する見出しが無ければ空になる() {
        assert_eq!(memory_sections("## 概要\n\n- だけ\n"), MemorySections::default());
        assert_eq!(memory_sections(""), MemorySections::default());
    }

    #[test]
    fn プロジェクト単位のmemoryのパスだけを判定する() {
        for ok in ["20-areas/shiori/memory.md", "10-projects/tanker/memory.md", "20-areas\\shiori\\memory.md", "20-projects/shiori/memory.md"] {
            assert!(is_project_memory_path(ok), "{ok}は対象");
        }
        for ng in [
            "40-profile/memory-global.md",
            "10-common/memory-global.md",
            "20-areas/shiori/journal/memory.md",
            "90-archive/20-areas/shiori/memory.md",
            "20-areas/shiori/memory.md.bak",
            "30-resources/memory.md",
            "memory.md",
        ] {
            assert!(!is_project_memory_path(ng), "{ng}は対象外");
        }
    }

    fn put(root: &std::path::Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn v7ではupdatedが最も新しいmemoryのやることを出す() {
        let d = tempfile::tempdir().unwrap();
        let mem = |t: &str, u: &str, todo: &str| format!("---\ntitle: {t}\nupdated: {u}\n---\n## やること\n\n- {todo}\n");
        put(d.path(), "20-projects/a/memory.md", &mem("A", "2026-11-20T10:00:00+09:00", "古い"));
        put(d.path(), "20-projects/b/memory.md", &mem("B", "2026-11-22T10:00:00+09:00", "新しい"));
        put(d.path(), "20-projects/c/memory.md", "---\ntitle: C\nupdated: 2026-11-23T10:00:00+09:00\n---\n## 概要\n\n- やることが無い\n");
        put(d.path(), "10-projects/old/memory.md", &mem("旧", "2026-11-30T10:00:00+09:00", "旧い置き場"));
        let h = latest(d.path(), Layout::V7).unwrap();
        assert_eq!((h.project.as_str(), h.todos.as_slice(), h.from_memory), ("b", &["新しい".to_string()][..], true));
        assert_eq!((h.source.as_str(), h.source_category.as_str(), h.date.as_str()), ("b/memory.md", "20-projects", "2026-11-22"));
    }

    #[test]
    fn memoryに該当が無ければ最新の日誌の申し送りを出す() {
        let d = tempfile::tempdir().unwrap();
        put(d.path(), "20-projects/a/memory.md", "---\ntitle: A\n---\n## 概要\n\n- なし\n");
        put(d.path(), "20-projects/a/journal/2026-11-20.md", "---\ntitle: 20日\n---\n## 申し送り\n\n- 古い\n");
        put(d.path(), "20-projects/a/journal/2026-11-21.md", "---\ntitle: 21日\n---\n## 09:00 端末\n\n- 申し送りではない\n");
        put(d.path(), "20-projects/b/journal/2026-11-22.md", "---\ntitle: 22日\n---\n## 申し送り\n\n- **新しい** 項目\n");
        let h = latest(d.path(), Layout::V7).unwrap();
        assert_eq!((h.project.as_str(), h.date.as_str(), h.items.as_slice()), ("b", "2026-11-22", &["新しい 項目".to_string()][..]));
        assert!(!h.from_memory);
        assert_eq!(h.source, "b/journal/2026-11-22.md");
    }

    #[test]
    fn 何も無ければnone() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(latest(d.path(), Layout::V7), None);
    }
}
