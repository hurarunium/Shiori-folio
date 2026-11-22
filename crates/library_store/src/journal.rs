//! 日次の日誌(P-13・P-16)。プロジェクトごと、日付ごとに1ファイル。追記だけができる。
//!
//! - 置き場: `{プロジェクトの置き場}/journal/YYYY-MM-DD.md`(`Layout::journal_dir`)。
//! - **日の区切りは午前5時**。時計から5時間を引いた日付が、日誌の日(`day`)。
//! - 節の見出しは、ここが付ける: `## HH:MM 端末・AI`。午前0〜5時の節は日付も付ける(`## 11/22 03:30 mac・codex`)。
//! - 追記のとき、`summary` があれば、全体の日誌(`10-common/journal/YYYY-MM-DD.md`)の `## 目次` へ
//!   `- HH:MM [project] summary` を1行足す。
//! - 既存の節の書き換え・削除はできない(追記だけ)。時計は引数で渡す(午前5時の前後を試験できるように)。

use crate::fsx::{self, Cond, StoreError};
use crate::layout::{Layout, COMMON_PROJECT};
use crate::registry;
use chrono::{DateTime, Datelike, Duration, FixedOffset, Timelike};
use serde::Serialize;
use std::path::Path;

/// 日の区切り(午前5時)。
pub const DAY_START_HOUR: i64 = 5;
const TOC_HEADING: &str = "## 目次";
const MAX_RETRY: usize = 50;

pub struct AppendRequest<'a> {
    pub project: &'a str,
    /// 節の本文(見出しは含めない)。
    pub content: &'a str,
    /// 書いた端末(トークンの名前)。
    pub device: &'a str,
    /// AIの種類(任意。`claude-code`・`codex` など)。
    pub ai: Option<&'a str>,
    /// 全体の日誌の目次に出す1行(任意)。
    pub summary: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Appended {
    /// 追記した日誌(libraryからの相対)。
    pub rel: String,
    pub day: String,
    pub heading: String,
    pub rev: String,
    /// 全体の日誌の目次へ足したとき `true`。
    pub toc_added: bool,
    /// 節の追記は済んだが、目次の更新に失敗したときの理由。
    pub toc_error: Option<String>,
    /// 書き方の規則の警告(`rules::lint_journal_section`)。保存は止めない。
    pub warnings: Vec<crate::rules::Warning>,
}

/// 日誌の日(午前5時より前は、前の日)。
pub fn day_of(now: &DateTime<FixedOffset>) -> chrono::NaiveDate {
    (*now - Duration::hours(DAY_START_HOUR)).date_naive()
}

/// 節の見出し(`## ` を含まない本体)。
pub fn heading(now: &DateTime<FixedOffset>, device: &str, ai: Option<&str>) -> String {
    let who = match ai.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => format!("{device}・{a}"),
        None => device.to_string(),
    };
    let time = format!("{:02}:{:02}", now.hour(), now.minute());
    if (now.hour() as i64) < DAY_START_HOUR {
        // 日の区切りをまたいだ(午前0〜5時の)節は、日付も付ける
        format!("{:02}/{:02} {time} {who}", now.month(), now.day())
    } else {
        format!("{time} {who}")
    }
}

#[derive(Serialize)]
struct JournalFrontmatter<'a> {
    title: String,
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(rename = "kind")]
    sub_kind: &'a str,
    project: &'a str,
    tags: Vec<&'a str>,
    day: &'a str,
    date: &'a str,
    updated: &'a str,
    status: &'a str,
    index: bool,
    related: Vec<String>,
}

fn new_file(project: &str, day: &str, now: &str, body: &str) -> Result<String, StoreError> {
    let title = if project == COMMON_PROJECT { format!("全体 日誌 {day}") } else { format!("{project} 日誌 {day}") };
    let fm = JournalFrontmatter {
        title,
        kind: "project",
        sub_kind: "journal",
        project,
        tags: vec!["日誌"],
        day,
        date: now,
        updated: now,
        status: "new",
        index: true,
        related: vec![],
    };
    let yaml = serde_yaml::to_string(&fm).map_err(|e| StoreError::Invalid(format!("frontmatterのYAML化に失敗: {e}")))?;
    Ok(format!("---\n{yaml}---\n{body}"))
}

/// frontmatterの `updated:` の行を書き換える。
fn set_updated(text: &str, now: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut in_front = false;
    let mut closed = false;
    for (i, line) in text.split_inclusive('\n').enumerate() {
        let bare = line.trim_end_matches(['\r', '\n']);
        if i == 0 && bare == "---" {
            in_front = true;
        } else if in_front && !closed && bare == "---" {
            closed = true;
        } else if in_front && !closed && bare.starts_with("updated:") {
            out.push_str(&format!("updated: {now}\n"));
            continue;
        }
        out.push_str(line);
    }
    out
}

fn ensure_newline(mut s: String) -> String {
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

fn section(heading: &str, content: &str) -> String {
    format!("\n## {heading}\n\n{}\n", content.trim_end())
}

/// 読んで・足して・版つきで書く(重なったら、読み直してやり直す)。無ければ作る。
fn update_file(
    root: &Path,
    rel: &str,
    mut make: impl FnMut(Option<&str>) -> Result<String, StoreError>,
) -> Result<String, StoreError> {
    for attempt in 0..MAX_RETRY {
        let (old, cond) = match fsx::read(root, rel) {
            Ok((bytes, rev)) => {
                let text = String::from_utf8(bytes).map_err(|_| StoreError::Invalid(format!("{rel}がUTF-8ではありません")))?;
                (Some(text), Cond::Overwrite { expect_rev: rev })
            }
            Err(StoreError::NotFound(_)) => (None, Cond::CreateNew),
            Err(e) => return Err(e),
        };
        let new_text = make(old.as_deref())?;
        match fsx::write(root, rel, new_text.as_bytes(), cond) {
            Ok(w) => return Ok(w.rev),
            Err(StoreError::Conflict { .. }) | Err(StoreError::Exists(_)) => {
                std::thread::sleep(std::time::Duration::from_millis(1 + (attempt as u64 % 8)));
            }
            Err(e) => return Err(e),
        }
    }
    Err(StoreError::Io(format!("{rel}への書き込みが重なって、やり直しても書けませんでした")))
}

fn check_project(project: &str) -> Result<(), StoreError> {
    if project.trim().is_empty() {
        return Err(StoreError::Invalid("projectが空です".into()));
    }
    if project.contains(['/', '\\', ':']) || project.contains("..") || project.starts_with('.') {
        return Err(StoreError::Invalid(format!("projectに使えない文字があります: {project}")));
    }
    Ok(())
}

/// 全体の日誌の `## 目次` へ1行足した全文を返す(目次が無ければ、frontmatterの直後に作る)。
fn insert_toc_line(text: &str, line: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let toc_at = lines.iter().position(|l| l.trim_end() == TOC_HEADING);
    let mut out: Vec<String> = Vec::new();
    match toc_at {
        Some(start) => {
            // 目次の節の終わり(次の「## 」の見出し、または末尾)
            let end = lines[start + 1..]
                .iter()
                .position(|l| l.starts_with("## "))
                .map(|i| start + 1 + i)
                .unwrap_or(lines.len());
            // 節の中の、最後の中身のある行の次に足す
            let mut insert_at = end;
            while insert_at > start + 1 && lines[insert_at - 1].trim().is_empty() {
                insert_at -= 1;
            }
            out.extend(lines[..insert_at].iter().map(|s| s.to_string()));
            if insert_at == start + 1 {
                out.push(String::new());
            }
            out.push(line.to_string());
            if end < lines.len() {
                out.push(String::new());
                out.extend(lines[end..].iter().map(|s| s.to_string()));
            }
        }
        None => {
            // frontmatterの直後に目次を作る
            let mut idx = 0;
            if lines.first().map(|l| l.trim_end()) == Some("---") {
                if let Some(close) = lines[1..].iter().position(|l| l.trim_end() == "---") {
                    idx = close + 2;
                }
            }
            out.extend(lines[..idx].iter().map(|s| s.to_string()));
            out.push(TOC_HEADING.to_string());
            out.push(String::new());
            out.push(line.to_string());
            if idx < lines.len() {
                out.push(String::new());
            }
            out.extend(lines[idx..].iter().map(|s| s.to_string()));
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// 日誌へ節を追記する。`now` は呼び出し側(サーバー)の時計。
pub fn append(
    root: &Path,
    layout: Layout,
    req: &AppendRequest<'_>,
    now: &DateTime<FixedOffset>,
) -> Result<Appended, StoreError> {
    check_project(req.project)?;
    // PowerShellの `Set-Content -Encoding utf8` などが付ける、先頭のBOM(見えない文字)は、本文に残さない
    let content = req.content.trim_start_matches('\u{feff}');
    if content.trim().is_empty() {
        return Err(StoreError::Invalid("節の本文が空です".into()));
    }
    let day = day_of(now).format("%Y-%m-%d").to_string();
    let now_s = now.to_rfc3339();
    let head = heading(now, req.device, req.ai);
    let sec = section(&head, content);

    // 書き方の規則の警告。memoryとの再掲の確認のため、同じプロジェクトのmemoryの行を読む(無ければ、確認しない)
    let memory_rel = if req.project == COMMON_PROJECT {
        Some(layout.global_memory_rel().to_string())
    } else {
        layout.project_dir("project", req.project).ok().map(|d| format!("{d}/memory.md"))
    };
    let memory_lines: Vec<String> = memory_rel
        .and_then(|rel| fsx::read(root, &rel).ok())
        .and_then(|(bytes, _)| String::from_utf8(bytes).ok())
        .and_then(|text| crate::notes::split_frontmatter(&text).ok())
        .map(|(_, memory_body)| crate::rules::content_lines(&memory_body))
        .unwrap_or_default();
    let warnings = crate::rules::lint_journal_section(content, &memory_lines);

    if req.project != COMMON_PROJECT {
        registry::ensure_project(root, req.project)?;
    }
    let rel = format!("{}/{day}.md", layout.journal_dir(req.project));
    let rev = update_file(root, &rel, |old| match old {
        Some(text) => Ok(set_updated(&ensure_newline(text.to_string()), &now_s) + &sec),
        None => new_file(req.project, &day, &now_s, sec.trim_start_matches('\n')),
    })?;

    // 全体の日誌の目次(summaryがあり、全体の日誌への追記でないとき)
    let mut toc_added = false;
    let mut toc_error = None;
    let summary = req.summary.map(str::trim).filter(|s| !s.is_empty());
    if let (Some(summary), true) = (summary, req.project != COMMON_PROJECT) {
        let toc_line = format!("- {:02}:{:02} [{}] {}", now.hour(), now.minute(), req.project, summary.replace('\n', " "));
        let common_rel = format!("{}/{day}.md", layout.journal_dir(COMMON_PROJECT));
        match update_file(root, &common_rel, |old| match old {
            Some(text) => Ok(set_updated(&insert_toc_line(&ensure_newline(text.to_string()), &toc_line), &now_s)),
            None => Ok(insert_toc_line(&new_file(COMMON_PROJECT, &day, &now_s, "")?, &toc_line)),
        }) {
            Ok(_) => toc_added = true,
            Err(e) => toc_error = Some(e.to_string()),
        }
    }

    Ok(Appended { rel, day, heading: head, rev, toc_added, toc_error, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::split_frontmatter;
    use std::path::PathBuf;

    struct Lib(PathBuf);
    impl Lib {
        fn new(tag: &str) -> Lib {
            let p = std::env::temp_dir().join(format!("library_store-journal-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(p.join("_system")).unwrap();
            std::fs::write(p.join(registry::PROJECTS_YAML), "projects:\n  - id: shiori\n    status: confirmed\n  - id: common\n    status: confirmed\n").unwrap();
            std::fs::write(p.join(registry::TAGS_YAML), "tags: []\n").unwrap();
            Lib(p)
        }
        fn read(&self, rel: &str) -> String {
            std::fs::read_to_string(self.0.join(rel)).unwrap()
        }
    }
    impl Drop for Lib {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn req<'a>(project: &'a str, content: &'a str) -> AppendRequest<'a> {
        AppendRequest { project, content, device: "win", ai: Some("claude-code"), summary: None }
    }

    #[test]
    fn 日の区切りは午前5時() {
        assert_eq!(day_of(&at("2026-11-22T03:30:00+09:00")).to_string(), "2026-11-21");
        assert_eq!(day_of(&at("2026-11-22T04:59:59+09:00")).to_string(), "2026-11-21");
        assert_eq!(day_of(&at("2026-11-22T05:00:00+09:00")).to_string(), "2026-11-22");
        assert_eq!(day_of(&at("2026-11-22T23:59:00+09:00")).to_string(), "2026-11-22");
        // 月・年またぎ
        assert_eq!(day_of(&at("2026-12-01T02:00:00+09:00")).to_string(), "2026-11-30");
        assert_eq!(day_of(&at("2027-01-01T00:10:00+09:00")).to_string(), "2026-12-31");
    }

    #[test]
    fn 節の見出しは端末とaiを付け深夜は日付も付ける() {
        assert_eq!(heading(&at("2026-11-22T14:30:00+09:00"), "win", Some("claude-code")), "14:30 win・claude-code");
        assert_eq!(heading(&at("2026-11-22T14:30:00+09:00"), "mac", None), "14:30 mac");
        assert_eq!(heading(&at("2026-11-22T03:30:00+09:00"), "mac", Some("codex")), "11/22 03:30 mac・codex");
        assert_eq!(heading(&at("2026-11-22T14:30:00+09:00"), "mac", Some("  ")), "14:30 mac");
    }

    #[test]
    fn 初めての追記は日誌を作りfrontmatterを付ける() {
        let l = Lib::new("new");
        let r = append(&l.0, Layout::V7, &req("shiori", "やったこと\n"), &at("2026-11-22T14:30:00+09:00")).unwrap();
        assert_eq!(r.rel, "20-projects/shiori/journal/2026-11-22.md");
        assert_eq!((r.day.as_str(), r.heading.as_str()), ("2026-11-22", "14:30 win・claude-code"));
        let text = l.read(&r.rel);
        let (fm, body) = split_frontmatter(&text).unwrap();
        assert_eq!(fm.title.as_deref(), Some("shiori 日誌 2026-11-22"));
        assert_eq!((fm.kind.as_deref(), fm.sub_kind.as_deref(), fm.project.as_deref()), (Some("project"), Some("journal"), Some("shiori")));
        assert_eq!(fm.tags, vec!["日誌".to_string()]);
        assert_eq!(fm.status, "new");
        assert!(fm.index);
        assert_eq!(fm.extra["day"].as_str(), Some("2026-11-22"));
        assert_eq!(body, "## 14:30 win・claude-code\n\nやったこと\n");
        assert_eq!(r.rev, crate::fsx::rev_of(text.as_bytes()));
    }

    #[test]
    fn 二回目の追記は末尾に節を足しupdatedだけ変える() {
        let l = Lib::new("second");
        let first = append(&l.0, Layout::V7, &req("shiori", "朝\n"), &at("2026-11-22T09:00:00+09:00")).unwrap();
        let before = l.read(&first.rel);
        let second = append(&l.0, Layout::V7, &AppendRequest { device: "mac", ai: Some("codex"), ..req("shiori", "昼\n") }, &at("2026-11-22T12:15:00+09:00")).unwrap();
        assert_eq!(first.rel, second.rel);
        let text = l.read(&second.rel);
        assert!(text.contains("date: 2026-11-22T09:00:00+09:00"), "作成日時は変わらない: {text}");
        assert!(text.contains("updated: 2026-11-22T12:15:00+09:00"));
        assert!(text.contains("## 09:00 win・claude-code\n\n朝\n\n## 12:15 mac・codex\n\n昼\n"), "{text}");
        // 既存の節は、一字も変わらない
        let tail_of_before = before.split("---\n").nth(2).unwrap();
        assert!(text.contains(tail_of_before));
    }

    #[test]
    fn 午前5時の前後で日誌が切り替わる() {
        let l = Lib::new("boundary");
        let a = append(&l.0, Layout::V7, &req("shiori", "夜更かし\n"), &at("2026-11-22T04:59:00+09:00")).unwrap();
        let b = append(&l.0, Layout::V7, &req("shiori", "朝\n"), &at("2026-11-22T05:00:00+09:00")).unwrap();
        assert_eq!(a.rel, "20-projects/shiori/journal/2026-11-21.md");
        assert_eq!(b.rel, "20-projects/shiori/journal/2026-11-22.md");
        assert!(l.read(&a.rel).contains("## 11/22 04:59 win・claude-code"));
        assert!(l.read(&b.rel).contains("## 05:00 win・claude-code"));
    }

    #[test]
    fn summaryがあれば全体の日誌の目次へ足す() {
        let l = Lib::new("toc");
        let mut r1 = req("shiori", "a\n");
        r1.summary = Some("設計を読んだ");
        let a = append(&l.0, Layout::V7, &r1, &at("2026-11-22T09:00:00+09:00")).unwrap();
        assert!(a.toc_added && a.toc_error.is_none());
        let mut r2 = req("shiori", "b\n");
        r2.summary = Some("実装した");
        append(&l.0, Layout::V7, &r2, &at("2026-11-22T10:30:00+09:00")).unwrap();
        // summaryが無ければ足さない
        append(&l.0, Layout::V7, &req("shiori", "c\n"), &at("2026-11-22T11:00:00+09:00")).unwrap();
        // 全体の日誌への追記は、節として末尾に入り、目次には足さない
        let c = append(&l.0, Layout::V7, &AppendRequest { summary: Some("無視される"), ..req("common", "環境の話\n") }, &at("2026-11-22T12:00:00+09:00")).unwrap();
        assert!(!c.toc_added);

        let text = l.read("10-common/journal/2026-11-22.md");
        let (fm, body) = split_frontmatter(&text).unwrap();
        assert_eq!(fm.project.as_deref(), Some("common"));
        assert!(body.starts_with("## 目次\n\n- 09:00 [shiori] 設計を読んだ\n- 10:30 [shiori] 実装した\n\n## 12:00 win・claude-code\n\n環境の話\n"), "{body}");
        assert_eq!(text.matches("無視される").count(), 0);
    }

    #[test]
    fn 目次が無い既存の全体の日誌にも足せる() {
        let l = Lib::new("toc-missing");
        append(&l.0, Layout::V7, &req("common", "先に節だけ\n"), &at("2026-11-22T08:00:00+09:00")).unwrap();
        let mut r = req("shiori", "x\n");
        r.summary = Some("要約");
        append(&l.0, Layout::V7, &r, &at("2026-11-22T09:00:00+09:00")).unwrap();
        let (_, body) = split_frontmatter(&l.read("10-common/journal/2026-11-22.md")).unwrap();
        assert!(body.starts_with("## 目次\n\n- 09:00 [shiori] 要約\n\n## 08:00 win・claude-code\n\n先に節だけ\n"), "{body}");
    }

    #[test]
    fn legacyの並びでは10_projectsの下に置く() {
        let l = Lib::new("legacy");
        let r = append(&l.0, Layout::Legacy, &req("shiori", "x\n"), &at("2026-11-22T09:00:00+09:00")).unwrap();
        assert_eq!(r.rel, "10-projects/shiori/journal/2026-11-22.md");
    }

    #[test]
    fn 未登録のprojectは台帳へpendingで足す() {
        let l = Lib::new("register");
        append(&l.0, Layout::V7, &req("新作", "x\n"), &at("2026-11-22T09:00:00+09:00")).unwrap();
        assert!(l.read(registry::PROJECTS_YAML).ends_with("  - id: 新作\n    status: pending\n"));
    }

    #[test]
    fn 不正な入力は何も書かずに断る() {
        let l = Lib::new("invalid");
        let now = at("2026-11-22T09:00:00+09:00");
        for bad in ["", " ", "../x", "a/b", "a\\b", ".hidden", "a:b"] {
            assert!(matches!(append(&l.0, Layout::V7, &req(bad, "x\n"), &now), Err(StoreError::Invalid(_))), "{bad:?}");
        }
        assert!(matches!(append(&l.0, Layout::V7, &req("shiori", "  \n"), &now), Err(StoreError::Invalid(_))));
        assert!(!l.0.join("20-projects").exists() && !l.0.join("10-common").exists());
    }

    #[test]
    fn 同時に追記しても全部の節が残る() {
        let l = Lib::new("race");
        let root = std::sync::Arc::new(l.0.clone());
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let root = root.clone();
                std::thread::spawn(move || {
                    let content = format!("節{i}\n");
                    let mut r = req("shiori", &content);
                    r.device = if i % 2 == 0 { "win" } else { "mac" };
                    r.summary = Some("同時");
                    append(&root, Layout::V7, &r, &at(&format!("2026-11-22T10:0{i}:00+09:00")))
                })
            })
            .collect();
        for h in handles {
            let r = h.join().unwrap().unwrap();
            assert!(r.toc_added, "{r:?}");
        }
        let text = l.read("20-projects/shiori/journal/2026-11-22.md");
        for i in 0..8 {
            assert_eq!(text.matches(&format!("\n節{i}\n")).count(), 1, "節{i}: {text}");
        }
        let common = l.read("10-common/journal/2026-11-22.md");
        assert_eq!(common.matches("[shiori] 同時").count(), 8, "{common}");
        // frontmatterは壊れていない
        assert!(split_frontmatter(&text).is_ok() && split_frontmatter(&common).is_ok());
    }

    #[test]
    fn 先頭のbomは本文に残さない() {
        let l = Lib::new("bom");
        let r = append(&l.0, Layout::V7, &req("shiori", "\u{feff}BOMつきの本文\n"), &at("2026-11-22T09:00:00+09:00")).unwrap();
        let text = l.read(&r.rel);
        assert!(!text.contains('\u{feff}'), "{text:?}");
        assert!(text.contains("## 09:00 win・claude-code\n\nBOMつきの本文\n"));
        // BOMだけの本文は、空として断る
        assert!(matches!(append(&l.0, Layout::V7, &req("shiori", "\u{feff}\n"), &at("2026-11-22T09:00:00+09:00")), Err(StoreError::Invalid(_))));
    }


    #[test]
    fn 追記の結果に書き方の規則の警告が付き保存は止まらない() {
        let l = Lib::new("warn");
        let r = append(&l.0, Layout::V7, &req("shiori", "### 次にやること\n\n- 設計を読む\n"), &at("2026-11-22T09:00:00+09:00")).unwrap();
        let ids: Vec<&str> = r.warnings.iter().map(|w| w.id).collect();
        assert!(ids.contains(&"J1") && ids.contains(&"J2"), "{ids:?}");
        assert!(l.read(&r.rel).contains("設計を読む"), "警告があっても、書き込まれる");
        // 具体的な節は、警告なし
        let ok = "### やったこと\n\n`journal.rs` で、先頭の BOM を本文に残さないようにした(PowerShell の `Set-Content` が付けるため)。試験を1件足して、112件が通った。\n";
        assert!(append(&l.0, Layout::V7, &req("shiori", ok), &at("2026-11-22T10:00:00+09:00")).unwrap().warnings.is_empty());
    }

    #[test]
    fn memoryの内容の再掲は警告される() {
        let l = Lib::new("reprise");
        let memory = "---\ntitle: m\ntype: project\nkind: memory\nproject: shiori\ntags: [a]\n---\n## やること\n\n- Phase 2: services/shiori-proto-server の窓口を本番の形にする(子のPID引き継ぎと、1分おきの自動起動を含む)。(11/22)\n";
        std::fs::create_dir_all(l.0.join("20-projects/shiori")).unwrap();
        std::fs::write(l.0.join("20-projects/shiori/memory.md"), memory).unwrap();
        let body = "Phase 2: services/shiori-proto-server の窓口を本番の形にする。子のPID引き継ぎと1分おきの自動起動を含む。\n";
        let r = append(&l.0, Layout::V7, &req("shiori", body), &at("2026-11-22T09:00:00+09:00")).unwrap();
        assert!(r.warnings.iter().any(|w| w.id == "D4" && w.message.contains("再掲")), "{:?}", r.warnings);
    }

}
