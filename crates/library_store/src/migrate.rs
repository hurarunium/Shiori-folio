//! 移行ツール(詩織Ver7.0 Phase 5-B。P-14・P-18・P-19)。v6.2のlibraryを、V7の並びへ変換して行き先へ入れる。
//!
//! **元のlibraryは書き換えない**。行き先(新しい場所)へ、3つの段階で入れ、段階ごとに照合する。
//!
//! 1. **写し**: 元のlibraryを、そのまま行き先へ写す(大きさとSHA-256を、元と照らす)。
//! 2. **フォルダの変換(P-18・P-19)**: `10-projects`・`20-areas` → `20-projects`、アーカイブも同様、
//!    `40-profile/memory-global.md` → `10-common/memory-global.md`。`type: area` → `type: project`(この1行だけ)。
//! 3. **journalの日誌への変換(P-14)**: 旧journal(`kind: journal`)を、プロジェクトごと・日ごとの日誌に束ねる。
//!    旧記事は削除せず、`status: outdated` にして `90-archive/20-projects/{project}/` へ移す。全体の日誌の目次も作る。
//!
//! 警告(日付の無い旧journal、名前の衝突、読めないファイル)が1つでもあれば、適用しない。
//! macOSの付属ファイル(`._*`・`.DS_Store`)は、サーバーが扱えないので、写さない(件数を報告する)。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, TimeZone, Timelike};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::journal::DAY_START_HOUR;
use crate::layout::COMMON_PROJECT;

/// 日付だけ(時刻なし)の `date` を、時刻つきにするときの時差。
const DEFAULT_OFFSET_SECS: i32 = 9 * 3600;
/// 報告に並べる件数の上限(多いときは、件数だけ出す)。
const LIST_LIMIT: usize = 50;

#[derive(Debug, Default, Clone)]
pub struct Report {
    /// 写すファイルの数(除外したものは含めない)
    pub files: usize,
    /// 写さなかったファイル(`._*`・`.DS_Store`)の数
    pub excluded: usize,
    /// 段階2で、置き場が変わるファイルの数
    pub moved: usize,
    /// 段階2で、`type: area` を `type: project` に書き換える記事の数
    pub type_fixed: usize,
    /// 旧journal(`kind: journal`)の数
    pub old_journals: usize,
    /// 作る日誌の数((プロジェクト, 日)の組)
    pub journal_days: usize,
    /// 全体の日誌の数(日の数)
    pub common_days: usize,
    /// プロジェクトごとの(旧journalの数, 日誌の数)
    pub per_project: BTreeMap<String, (usize, usize)>,
    /// 旧記事 → 新しい日誌の節の対応
    pub mapping: Vec<(String, String)>,
    /// 記録の本文に、古いパス(`10-projects/`・`20-areas/`)が残る記事の数(書き換えない)
    pub old_path_mentions: usize,
    /// 補足(日付だけの旧journalなど。止めない)
    pub notes: Vec<String>,
    /// 警告。1つでもあれば、適用しない
    pub warnings: Vec<String>,
}

impl Report {
    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "【段階1 写し】{}件を写す(`._*`・`.DS_Store` の{}件は写さない)", self.files, self.excluded);
        let _ = writeln!(s, "【段階2 フォルダの変換】置き場が変わる{}件、`type: area` → `type: project` に書き換える{}件", self.moved, self.type_fixed);
        let _ = writeln!(
            s,
            "【段階3 日誌への変換】旧journal {}件 → 日誌 {}件(全体の日誌 {}日分)。旧記事は `status: outdated` にして `90-archive/20-projects/` へ移す",
            self.old_journals, self.journal_days, self.common_days
        );
        if !self.per_project.is_empty() {
            let _ = writeln!(s, "プロジェクトごと(旧journal → 日誌):");
            for (p, (old, days)) in &self.per_project {
                let _ = writeln!(s, "  {p}: {old}件 → {days}日");
            }
        }
        let _ = writeln!(s, "記録の本文に古いパス(`10-projects/`・`20-areas/`)が残る記事: {}件(書き換えない)", self.old_path_mentions);
        if !self.mapping.is_empty() {
            let _ = writeln!(s, "旧記事 → 新しい節の対応(先頭{LIST_LIMIT}件。全体は件数 {}):", self.mapping.len());
            for (old, new) in self.mapping.iter().take(LIST_LIMIT) {
                let _ = writeln!(s, "  {old} → {new}");
            }
        }
        for n in &self.notes {
            let _ = writeln!(s, "補足: {n}");
        }
        if self.warnings.is_empty() {
            let _ = writeln!(s, "警告: なし(適用できる)");
        } else {
            let _ = writeln!(s, "警告 {}件(1つでもあれば、適用しない):", self.warnings.len());
            for w in self.warnings.iter().take(LIST_LIMIT) {
                let _ = writeln!(s, "  - {w}");
            }
        }
        s
    }
}

// ---- 小さな部品 ----

fn sha_bytes(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn sha_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}を開けません: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("{}を読めません: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|x| format!("{x:02x}")).collect())
}

/// macOSの付属ファイルなど、写さないもの。
fn is_excluded(rel: &str) -> bool {
    rel.split('/').any(|c| c.starts_with("._") || c == ".DS_Store" || c == "Thumbs.db")
}

/// 配下のファイル(`/` 区切りの相対パス)と、除外したものの数。
fn walk(root: &Path) -> Result<(Vec<(String, PathBuf)>, usize), String> {
    fn rec(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>, excluded: &mut usize) -> Result<(), String> {
        let rd = std::fs::read_dir(dir).map_err(|e| format!("{}を読めません: {e}", dir.display()))?;
        for e in rd {
            let e = e.map_err(|e| format!("{}を読めません: {e}", dir.display()))?;
            let name = e.file_name().to_string_lossy().into_owned();
            let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let ft = e.file_type().map_err(|e| format!("{child}の種類を調べられません: {e}"))?;
            if ft.is_dir() {
                if name.starts_with("._") {
                    *excluded += 1;
                    continue;
                }
                rec(&e.path(), &child, out, excluded)?;
            } else if ft.is_file() {
                if is_excluded(&child) {
                    *excluded += 1;
                } else {
                    out.push((child, e.path()));
                }
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    let mut excluded = 0;
    rec(root, "", &mut out, &mut excluded)?;
    out.sort();
    Ok((out, excluded))
}

/// 段階2の置き場の変換。
pub fn map_path(rel: &str) -> String {
    for (from, to) in [
        ("90-archive/10-projects/", "90-archive/20-projects/"),
        ("90-archive/20-areas/", "90-archive/20-projects/"),
        ("10-projects/", "20-projects/"),
        ("20-areas/", "20-projects/"),
    ] {
        if let Some(rest) = rel.strip_prefix(from) {
            return format!("{to}{rest}");
        }
    }
    if rel == "40-profile/memory-global.md" {
        return "10-common/memory-global.md".to_string();
    }
    rel.to_string()
}

/// frontmatter(先頭の `---` から次の `---` まで)の範囲(行の番号 [始まり, 終わり) )。
fn front_lines(text: &str) -> Option<(usize, usize)> {
    let mut lines = text.lines();
    if lines.next().map(|l| l.trim_end()) != Some("---") {
        return None;
    }
    for (i, l) in lines.enumerate() {
        if l.trim_end() == "---" {
            return Some((1, i + 1));
        }
    }
    None
}

/// `type: area` → `type: project`(frontmatterの、この1行だけ)。無ければ `None`。
fn fix_type_area(text: &str) -> Option<String> {
    let (a, b) = front_lines(text)?;
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    for (i, line) in text.split_inclusive('\n').enumerate() {
        let bare = line.trim_end_matches(['\r', '\n']);
        if i >= a && i < b && !changed && matches!(bare.trim_end(), "type: area" | "type: \"area\"" | "type: 'area'") {
            let eol = &line[bare.len()..];
            out.push_str("type: project");
            out.push_str(eol);
            changed = true;
        } else {
            out.push_str(line);
        }
    }
    changed.then_some(out)
}

/// frontmatterの `status:` を `outdated` にする(無ければ足す)。
fn set_status_outdated(text: &str) -> String {
    let Some((a, b)) = front_lines(text) else { return text.to_string() };
    let mut out = String::with_capacity(text.len() + 20);
    let mut done = false;
    for (i, line) in text.split_inclusive('\n').enumerate() {
        let bare = line.trim_end_matches(['\r', '\n']);
        if i >= a && i < b && !done && bare.starts_with("status:") {
            out.push_str("status: outdated");
            out.push_str(&line[bare.len()..]);
            done = true;
            continue;
        }
        if i == b && !done {
            out.push_str("status: outdated\n");
            done = true;
        }
        out.push_str(line);
    }
    out
}

/// frontmatterをYAMLとして読み、本文(frontmatterの後)と一緒に返す。
fn parse_front(text: &str) -> Option<(serde_yaml::Value, String)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (a, b) = front_lines(text)?;
    let lines: Vec<&str> = text.lines().collect();
    let yaml = lines[a..b].join("\n");
    let value: serde_yaml::Value = serde_yaml::from_str(&yaml).ok()?;
    let body = lines[b + 1..].join("\n");
    Some((value, body))
}

fn yaml_str(v: Option<&serde_yaml::Value>) -> String {
    match v {
        Some(serde_yaml::Value::String(s)) => s.trim().to_string(),
        Some(serde_yaml::Value::Null) | None => String::new(),
        Some(other) => serde_yaml::to_string(other).unwrap_or_default().trim().to_string(),
    }
}

fn parse_date(raw: &str) -> Option<(DateTime<FixedOffset>, bool)> {
    let raw = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some((dt, false));
    }
    if let Ok(d) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        let dt = FixedOffset::east_opt(DEFAULT_OFFSET_SECS)?.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).single()?;
        return Some((dt, true));
    }
    None
}

/// 見出し(`#`)の段を1つ下げる(6段を超えるものは6段に丸める)。コードの囲み(```)の中は変えない。
fn demote_headings(body: &str) -> String {
    let mut out = Vec::new();
    let mut in_code = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            out.push(line.to_string());
            continue;
        }
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if !in_code && (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            if hashes < 6 {
                out.push(format!("#{line}"));
            } else {
                out.push(line.to_string());
            }
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

#[derive(Serialize)]
struct JournalFm {
    title: String,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(rename = "kind")]
    sub_kind: &'static str,
    project: String,
    tags: Vec<String>,
    day: String,
    date: String,
    updated: String,
    status: &'static str,
    index: bool,
    related: Vec<String>,
}

fn fm_text(fm: &JournalFm) -> Result<String, String> {
    let yaml = serde_yaml::to_string(fm).map_err(|e| format!("frontmatterのYAML化に失敗: {e}"))?;
    Ok(format!("---\n{yaml}---\n"))
}

// ---- 計画 ----

struct Ent {
    rel: String,
    abs: PathBuf,
    size: u64,
    /// 元の中身のSHA-256
    sha1: String,
    /// 段階2のあとの置き場と、中身(書き換えが無ければ `None`)・SHA-256
    to2: String,
    fixed: Option<String>,
    sha2: String,
    /// 段階3のあと(最終)の置き場と、SHA-256。旧journalは、アーカイブへ移る
    to3: String,
    archived: Option<String>,
    sha3: String,
}

struct Old {
    ent: usize,
    project: String,
    day: String,
    ts: DateTime<FixedOffset>,
    heading: String,
    toc_text: String,
    tags: Vec<String>,
    date_raw: String,
    stem: String,
    body: String,
}

pub struct Plan {
    ents: Vec<Ent>,
    olds: Vec<Old>,
    /// 作る日誌(置き場, 中身)
    outputs: Vec<(String, String)>,
    pub report: Report,
}

impl Plan {
    fn expected(&self, stage: u8) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        for e in &self.ents {
            match stage {
                1 => m.insert(e.rel.clone(), e.sha1.clone()),
                2 => m.insert(e.to2.clone(), e.sha2.clone()),
                _ => m.insert(e.to3.clone(), e.sha3.clone()),
            };
        }
        if stage == 3 {
            for (rel, text) in &self.outputs {
                m.insert(rel.clone(), sha_bytes(text.as_bytes()));
            }
        }
        m
    }
}

fn journal_candidate(rel: &str) -> Option<(String, String)> {
    // `{10-projects|20-areas}/{project}/journal/{name}.md`
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.len() == 4 && matches!(parts[0], "10-projects" | "20-areas") && parts[2] == "journal" && parts[3].ends_with(".md") {
        return Some((parts[1].to_string(), parts[3].trim_end_matches(".md").to_string()));
    }
    None
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 元のlibraryを読んで、計画と報告を作る(何も書かない)。
pub fn build_plan(src: &Path) -> Result<Plan, String> {
    let (files, excluded) = walk(src)?;
    let mut report = Report { excluded, files: files.len(), ..Default::default() };
    let mut ents: Vec<Ent> = Vec::new();
    let mut olds: Vec<Old> = Vec::new();

    for (rel, abs) in files {
        let size = std::fs::metadata(&abs).map_err(|e| format!("{rel}を調べられません: {e}"))?.len();
        let to2 = map_path(&rel);
        if to2 != rel {
            report.moved += 1;
        }
        let is_md = rel.ends_with(".md");
        let (sha1, fixed, sha2, text) = if is_md {
            let bytes = std::fs::read(&abs).map_err(|e| format!("{rel}を読めません: {e}"))?;
            let sha1 = sha_bytes(&bytes);
            match String::from_utf8(bytes) {
                Ok(text) => {
                    let fixed = fix_type_area(&text);
                    if fixed.is_some() {
                        report.type_fixed += 1;
                    }
                    let sha2 = fixed.as_ref().map(|f| sha_bytes(f.as_bytes())).unwrap_or_else(|| sha1.clone());
                    if !rel.starts_with("_system/") && (text.contains("10-projects/") || text.contains("20-areas/")) {
                        report.old_path_mentions += 1;
                    }
                    (sha1, fixed, sha2, Some(text))
                }
                Err(_) => {
                    report.warnings.push(format!("{rel}: UTF-8ではないので、読めません"));
                    (sha1.clone(), None, sha1, None)
                }
            }
        } else {
            let sha = sha_file(&abs)?;
            (sha.clone(), None, sha, None)
        };
        let idx = ents.len();
        ents.push(Ent {
            rel: rel.clone(),
            abs,
            size,
            sha1,
            to2: to2.clone(),
            fixed: fixed.clone(),
            sha2: sha2.clone(),
            to3: to2,
            archived: None,
            sha3: sha2,
        });

        // 旧journalか
        let (Some((project, stem)), Some(text)) = (journal_candidate(&rel), text) else { continue };
        let Some((fm, body)) = parse_front(&text) else { continue };
        if yaml_str(fm.get("kind")) != "journal" {
            continue;
        }
        let date_raw = yaml_str(fm.get("date"));
        if date_raw.is_empty() {
            report.warnings.push(format!("{rel}: date が無いので、日誌の日を決められません"));
            continue;
        }
        let Some((ts, date_only)) = parse_date(&date_raw) else {
            report.warnings.push(format!("{rel}: date が読めません({date_raw})"));
            continue;
        };
        if date_only {
            report.notes.push(format!("{rel}: date が日付だけ({date_raw})。時刻が無いので、日の区切りの5時間は引かず、その日付の 00:00 として入れる"));
        }
        let day_date = if date_only { ts.date_naive() } else { (ts - Duration::hours(DAY_START_HOUR)).date_naive() };
        let day = day_date.format("%Y-%m-%d").to_string();
        let title = {
            let t = yaml_str(fm.get("title"));
            if t.is_empty() { stem.clone() } else { one_line(&t) }
        };
        let time = format!("{:02}:{:02}", ts.hour(), ts.minute());
        let heading = if !date_only && (ts.hour() as i64) < DAY_START_HOUR {
            format!("{:02}/{:02} {time} {title}", ts.month(), ts.day())
        } else {
            format!("{time} {title}")
        };
        let summary = one_line(&yaml_str(fm.get("summary")));
        let toc_text = if summary.is_empty() { title.clone() } else { summary };
        let mut tags = Vec::new();
        if let Some(serde_yaml::Value::Sequence(seq)) = fm.get("tags") {
            for t in seq {
                let t = yaml_str(Some(t));
                if !t.is_empty() {
                    tags.push(t);
                }
            }
        }
        olds.push(Old {
            ent: idx,
            project,
            day,
            ts,
            heading,
            toc_text,
            tags,
            date_raw,
            stem,
            body: demote_headings(body.trim_matches('\n')),
        });
    }
    report.old_journals = olds.len();

    // 段階2の置き場の衝突
    let mut targets: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in &ents {
        targets.entry(e.to2.clone()).or_default().push(e.rel.clone());
    }
    for (to, from) in &targets {
        if from.len() > 1 {
            report.warnings.push(format!("名前の衝突(段階2): {} が同じ {to} になる", from.join(" と ")));
        }
    }

    // 段階3: 日誌を作る
    let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (i, o) in olds.iter().enumerate() {
        groups.entry((o.project.clone(), o.day.clone())).or_default().push(i);
    }
    let mut outputs: Vec<(String, String)> = Vec::new();
    let mut common: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for ((project, day), idxs) in &mut groups {
        idxs.sort_by(|a, b| olds[*a].ts.cmp(&olds[*b].ts).then_with(|| olds[*a].stem.cmp(&olds[*b].stem)));
        let rel = format!("20-projects/{project}/journal/{day}.md");
        let mut tags: BTreeSet<String> = BTreeSet::new();
        for i in idxs.iter() {
            tags.extend(olds[*i].tags.iter().cloned());
        }
        tags.remove("日誌");
        let mut all_tags = vec!["日誌".to_string()];
        all_tags.extend(tags);
        let first = &olds[idxs[0]];
        let last = &olds[*idxs.last().unwrap()];
        let fm = JournalFm {
            title: format!("{project} 日誌 {day}"),
            kind: "project",
            sub_kind: "journal",
            project: project.clone(),
            tags: all_tags,
            day: day.clone(),
            date: first.date_raw.clone(),
            updated: last.date_raw.clone(),
            status: "new",
            index: true,
            related: idxs.iter().map(|i| olds[*i].stem.clone()).collect(),
        };
        let mut text = fm_text(&fm)?;
        for (n, i) in idxs.iter().enumerate() {
            let o = &olds[*i];
            if n > 0 {
                text.push('\n');
            }
            let _ = write!(text, "## {}\n\n{}\n", o.heading, o.body);
        }
        for i in idxs.iter() {
            report.mapping.push((ents[olds[*i].ent].rel.clone(), format!("{rel} の「## {}」", olds[*i].heading)));
            common.entry(day.clone()).or_default().push(*i);
        }
        let e = report.per_project.entry(project.clone()).or_insert((0, 0));
        e.0 += idxs.len();
        e.1 += 1;
        outputs.push((rel, text));
    }
    report.journal_days = groups.len();
    for (day, mut idxs) in common {
        idxs.sort_by(|a, b| olds[*a].ts.cmp(&olds[*b].ts).then_with(|| olds[*a].stem.cmp(&olds[*b].stem)));
        let first = &olds[idxs[0]];
        let last = &olds[*idxs.last().unwrap()];
        let fm = JournalFm {
            title: format!("全体 日誌 {day}"),
            kind: "project",
            sub_kind: "journal",
            project: COMMON_PROJECT.to_string(),
            tags: vec!["日誌".to_string()],
            day: day.clone(),
            date: first.date_raw.clone(),
            updated: last.date_raw.clone(),
            status: "new",
            index: true,
            related: vec![],
        };
        let mut text = fm_text(&fm)?;
        text.push_str("## 目次\n\n");
        for i in &idxs {
            let o = &olds[*i];
            let _ = writeln!(text, "- {:02}:{:02} [{}] {}", o.ts.hour(), o.ts.minute(), o.project, o.toc_text);
        }
        outputs.push((format!("10-common/journal/{day}.md"), text));
        report.common_days += 1;
    }

    // 旧記事のアーカイブ先(`status: outdated`)
    for o in &olds {
        let e = &mut ents[o.ent];
        let name = e.rel.rsplit('/').next().unwrap_or("").to_string();
        e.to3 = format!("90-archive/20-projects/{}/{name}", o.project);
        let base = e.fixed.clone().unwrap_or_else(|| std::fs::read_to_string(&e.abs).unwrap_or_default());
        let arch = set_status_outdated(&base);
        e.sha3 = sha_bytes(arch.as_bytes());
        e.archived = Some(arch);
    }

    // 段階3の置き場の衝突(最終の置き場が重なる)
    let mut finals: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in &ents {
        finals.entry(e.to3.clone()).or_default().push(e.rel.clone());
    }
    for (rel, _) in &outputs {
        finals.entry(rel.clone()).or_default().push("(作る日誌)".to_string());
    }
    for (to, from) in &finals {
        if from.len() > 1 && !targets.get(to).is_some_and(|f| f.len() > 1) {
            report.warnings.push(format!("名前の衝突(段階3): {} が同じ {to} になる", from.join(" と ")));
        }
    }

    Ok(Plan { ents, olds, outputs, report })
}

// ---- 照合 ----

/// 配下の実際の内容(置き場 → SHA-256)と、期待を比べて、食い違いを返す。
fn compare(expected: &BTreeMap<String, String>, dst: &Path) -> Result<Vec<String>, String> {
    let (files, _) = walk(dst)?;
    let mut actual: BTreeMap<String, String> = BTreeMap::new();
    for (rel, abs) in files {
        actual.insert(rel, sha_file(&abs)?);
    }
    let mut problems = Vec::new();
    for (rel, sha) in expected {
        match actual.get(rel) {
            None => problems.push(format!("無い: {rel}")),
            Some(a) if a != sha => problems.push(format!("中身が違う: {rel}")),
            _ => {}
        }
    }
    for rel in actual.keys() {
        if !expected.contains_key(rel) {
            problems.push(format!("余計: {rel}"));
        }
    }
    Ok(problems)
}

/// 日誌の中身を、行き先の実際のファイルから確かめる(計画とは別の経路)。
/// すべての旧記事について、本文(見出しの段を1つ下げたもの)と見出しが、新しい日誌にそのまま含まれること。
fn check_journals(plan: &Plan, dst: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    let mut texts: BTreeMap<String, String> = BTreeMap::new();
    for o in &plan.olds {
        let ent = &plan.ents[o.ent];
        let rel = format!("20-projects/{}/journal/{}.md", o.project, o.day);
        let journal = texts.entry(rel.clone()).or_insert_with(|| std::fs::read_to_string(dst.join(&rel)).unwrap_or_default()).clone();
        let archived = std::fs::read_to_string(dst.join(&ent.to3)).unwrap_or_default();
        let Some((_, body)) = parse_front(&archived) else {
            problems.push(format!("{}: アーカイブ({})を読めない", ent.rel, ent.to3));
            continue;
        };
        if !journal.contains(&demote_headings(body.trim_matches('\n'))) {
            problems.push(format!("{}: 本文が日誌 {rel} に含まれていない", ent.rel));
        }
        if !journal.contains(&format!("\n## {}\n", o.heading)) && !journal.contains(&format!("## {}\n", o.heading)) {
            problems.push(format!("{}: 見出し「{}」が日誌 {rel} に無い", ent.rel, o.heading));
        }
        if !archived.lines().any(|l| l.trim_end() == "status: outdated") {
            problems.push(format!("{}: アーカイブが status: outdated になっていない", ent.rel));
        }
    }
    problems
}

fn verify_stage(plan: &Plan, dst: &Path, stage: u8) -> Result<(), String> {
    let mut problems = compare(&plan.expected(stage), dst)?;
    if stage == 3 {
        problems.extend(check_journals(plan, dst));
    }
    if problems.is_empty() {
        return Ok(());
    }
    let more = problems.len().saturating_sub(LIST_LIMIT);
    let mut msg = format!("段階{stage}の照合に通りません({}件):\n", problems.len());
    for p in problems.iter().take(LIST_LIMIT) {
        let _ = writeln!(msg, "  - {p}");
    }
    if more > 0 {
        let _ = writeln!(msg, "  …ほか{more}件");
    }
    Err(msg)
}

// ---- 実行 ----

fn mtime_of(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn set_mtime(path: &Path, t: Option<std::time::SystemTime>) {
    if let (Some(t), Ok(f)) = (t, std::fs::OpenOptions::new().write(true).open(path)) {
        let _ = f.set_modified(t);
    }
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}を作れません: {e}", d.display()))?;
    }
    Ok(())
}

fn remove_empty_dirs(root: &Path) {
    fn rec(dir: &Path) -> bool {
        let Ok(rd) = std::fs::read_dir(dir) else { return false };
        let mut empty = true;
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !rec(&p) {
                    empty = false;
                }
            } else {
                empty = false;
            }
        }
        if empty {
            let _ = std::fs::remove_dir(dir);
        }
        empty
    }
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                rec(&e.path());
            }
        }
    }
}

fn check_dst_empty(dst: &Path) -> Result<(), String> {
    if dst.exists() {
        let mut rd = std::fs::read_dir(dst).map_err(|e| format!("{}を読めません: {e}", dst.display()))?;
        if rd.next().is_some() {
            return Err(format!("行き先が空ではありません: {}(新しい、空の場所を指定してください)", dst.display()));
        }
    }
    Ok(())
}

/// 予行。何も書かない。行き先は、空(または無い)ことだけ確かめる。
pub fn plan(src: &Path, dst: &Path) -> Result<Report, String> {
    check_dst_empty(dst)?;
    Ok(build_plan(src)?.report)
}

/// 適用。3つの段階を順に行い、段階ごとに照合する(通らなければ、そこで止める)。行き先を捨てれば、元の状態に戻る。
pub fn apply(src: &Path, dst: &Path) -> Result<Report, String> {
    check_dst_empty(dst)?;
    let plan = build_plan(src)?;
    if !plan.report.warnings.is_empty() {
        return Err(format!("警告があるので、適用しません:\n{}", plan.report.render()));
    }

    // 段階1: 写し
    std::fs::create_dir_all(dst).map_err(|e| format!("{}を作れません: {e}", dst.display()))?;
    for e in &plan.ents {
        let to = dst.join(&e.rel);
        ensure_parent(&to)?;
        std::fs::copy(&e.abs, &to).map_err(|er| format!("{}を写せません: {er}", e.rel))?;
        set_mtime(&to, mtime_of(&e.abs));
        let _ = e.size;
    }
    verify_stage(&plan, dst, 1)?;

    // 段階2: フォルダの変換
    for e in &plan.ents {
        let from = dst.join(&e.rel);
        let to = dst.join(&e.to2);
        let t = mtime_of(&from);
        if e.to2 != e.rel {
            ensure_parent(&to)?;
            std::fs::rename(&from, &to).map_err(|er| format!("{}を移せません: {er}", e.rel))?;
        }
        if let Some(text) = &e.fixed {
            std::fs::write(&to, text.as_bytes()).map_err(|er| format!("{}を書けません: {er}", e.to2))?;
            set_mtime(&to, t);
        }
    }
    remove_empty_dirs(dst);
    verify_stage(&plan, dst, 2)?;

    // 段階3: 日誌への変換
    for (rel, text) in &plan.outputs {
        let to = dst.join(rel);
        ensure_parent(&to)?;
        std::fs::write(&to, text.as_bytes()).map_err(|er| format!("{rel}を書けません: {er}"))?;
    }
    for o in &plan.olds {
        let e = &plan.ents[o.ent];
        let from = dst.join(&e.to2);
        let to = dst.join(&e.to3);
        let t = mtime_of(&from);
        ensure_parent(&to)?;
        std::fs::rename(&from, &to).map_err(|er| format!("{}を移せません: {er}", e.to2))?;
        if let Some(text) = &e.archived {
            std::fs::write(&to, text.as_bytes()).map_err(|er| format!("{}を書けません: {er}", e.to3))?;
            set_mtime(&to, t);
        }
    }
    remove_empty_dirs(dst);
    verify_stage(&plan, dst, 3)?;
    Ok(plan.report)
}

/// 全体の照合をやり直す。問題の一覧を返す(空なら、すべて合っている)。
pub fn verify(src: &Path, dst: &Path) -> Result<Vec<String>, String> {
    let plan = build_plan(src)?;
    let mut problems = compare(&plan.expected(3), dst)?;
    problems.extend(check_journals(&plan, dst));
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn journal(title: &str, date: &str, summary: &str, tags: &str, body: &str) -> String {
        format!("---\ntitle: {title}\ntype: area\nkind: journal\ntags: {tags}\nproject: x\nsummary: {summary}\nstatus: new\ndate: {date}\n---\n{body}")
    }

    /// 境界の例を含むダミーのlibrary(v6.2の並び)。
    fn dummy() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        // 同じ日の2件(ISO・小数秒つき)と、午前3時台(前の日に入る)
        put(r, "10-projects/a/journal/j1.md", &journal("朝の作業", "2026-10-07T10:00:00.123456+09:00", "朝にやったこと", "[詩織, 運用]", "\n## 背景\n\n朝の本文。\n\n### 小さな見出し\n\n- 項目\n"));
        put(r, "10-projects/a/journal/j2.md", &journal("午後の作業", "\"2026-10-07T14:30:00+09:00\"", "", "[詩織]", "\n## 行ったこと\n\n```\n## コードの中の見出しは変えない\n```\n\n本文。\n"));
        put(r, "10-projects/a/journal/j3.md", &journal("深夜の作業", "2026-10-08T03:30:00+09:00", "深夜の要約", "[深夜]", "\n本文だけ。\n"));
        // 日付だけ
        put(r, "20-areas/b/journal/j4.md", &journal("日付だけ", "2026-10-09", "", "[]", "\n# 一番上の見出し\n\n###### 6段の見出し\n"));
        put(r, "20-areas/b/memory.md", "---\ntitle: bのmemory\ntype: area\nkind: memory\n---\n本文に 20-areas/b/ という古いパスがある。\n");
        put(r, "10-projects/a/memory.md", "---\ntitle: aのmemory\ntype: project\nkind: memory\n---\nmemory\n");
        put(r, "10-projects/a/resource/doc.md", "---\ntitle: 資料\ntype: project\n---\n資料\n");
        put(r, "40-profile/memory-global.md", "---\ntitle: 全体\nkind: memory\n---\n全体のmemory\n");
        put(r, "40-profile/self-image/x.md", "---\ntitle: 自己像\ntype: profile\n---\n自己像\n");
        put(r, "90-archive/20-areas/b/old.md", "---\ntitle: 古い\ntype: area\n---\n古い記事\n");
        put(r, "90-archive/10-projects/a/older.md", "---\ntitle: もっと古い\ntype: project\n---\n本文\n");
        put(r, "30-resources/skills/work-start.md", "# スキル\n");
        put(r, "_system/CLAUDE.md", "# 規約\n");
        put(r, "00-inbox/memo.md", "---\ntitle: メモ\n---\nメモ\n");
        std::fs::write(d.path().join("10-projects/a/files.bin"), [0u8, 159, 146, 150, 255]).unwrap();
        // macOSの付属ファイル(写さない)
        put(r, "10-projects/a/journal/._j1.md", "binary");
        put(r, "._root", "binary");
        d
    }

    fn read(root: &Path, rel: &str) -> String {
        std::fs::read_to_string(root.join(rel)).unwrap_or_else(|_| panic!("{rel}が読めない"))
    }

    #[test]
    fn 予行は何も書かず件数と対応を報告する() {
        let src = dummy();
        let dst = tempfile::tempdir().unwrap();
        let rep = plan(src.path(), &dst.path().join("out")).unwrap();
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
        assert_eq!(rep.excluded, 2, "._ の2件は写さない");
        assert_eq!((rep.old_journals, rep.journal_days, rep.common_days), (4, 2, 2), "深夜3時台は前の日に入るので、aの3件は1日にまとまる");
        assert_eq!(rep.per_project["a"], (3, 1));
        assert_eq!(rep.per_project["b"], (1, 1));
        assert_eq!(rep.type_fixed, 6, "type: area の記事: j1〜j4・bのmemory・古い記事");
        assert_eq!(rep.old_path_mentions, 1);
        assert!(rep.notes.iter().any(|n| n.contains("日付だけ")));
        assert_eq!(rep.mapping.len(), 4);
        assert!(rep.render().contains("警告: なし"));
        assert!(!dst.path().join("out").exists(), "予行は何も書かない");
    }

    #[test]
    fn 三つの段階が通りフォルダと型と日誌が決めたとおりになる() {
        let src = dummy();
        let out = tempfile::tempdir().unwrap();
        let dst = out.path().join("v7");
        apply(src.path(), &dst).unwrap();
        // 元は変わらない
        assert!(src.path().join("10-projects/a/journal/j1.md").exists());
        // 段階2
        assert!(dst.join("20-projects/a/memory.md").exists() && !dst.join("10-projects").exists() && !dst.join("20-areas").exists());
        assert!(read(&dst, "20-projects/b/memory.md").contains("type: project") && !read(&dst, "20-projects/b/memory.md").contains("type: area"));
        assert!(read(&dst, "20-projects/b/memory.md").contains("20-areas/b/"), "記録の本文の古いパスは書き換えない");
        assert!(dst.join("10-common/memory-global.md").exists() && !dst.join("40-profile/memory-global.md").exists());
        assert!(dst.join("40-profile/self-image/x.md").exists(), "memory-global以外の40-profileは動かさない");
        assert!(dst.join("90-archive/20-projects/b/old.md").exists() && dst.join("90-archive/20-projects/a/older.md").exists());
        assert_eq!(std::fs::read(dst.join("20-projects/a/files.bin")).unwrap(), vec![0u8, 159, 146, 150, 255]);
        assert!(!dst.join("._root").exists());
        // 段階3: 同じ日の2件が1つの日誌に、時刻順で
        let j = read(&dst, "20-projects/a/journal/2026-10-07.md");
        let (m, n) = (j.find("## 10:00 朝の作業").unwrap(), j.find("## 14:30 午後の作業").unwrap());
        assert!(m < n);
        assert!(j.contains("### 背景") && j.contains("#### 小さな見出し"), "見出しは1段下げる");
        assert!(j.contains("## コードの中の見出しは変えない"), "コードの囲みの中は変えない");
        assert!(j.contains("day: '2026-10-07'") || j.contains("day: 2026-10-07"));
        assert!(j.contains("related:") && j.contains("- j1") && j.contains("- j2"));
        // 午前3時台は前の日に入り、見出しに日付が付く
        let j = read(&dst, "20-projects/a/journal/2026-10-07.md");
        assert!(j.contains("## 10/08 03:30 深夜の作業"), "深夜: {j}");
        // 日付だけの記事は、その日付のまま。見出しの段は丸める
        let jb = read(&dst, "20-projects/b/journal/2026-10-09.md");
        assert!(jb.contains("## 00:00 日付だけ") && jb.contains("## 一番上の見出し") || jb.contains("### 一番上の見出し"));
        assert!(jb.contains("###### 6段の見出し"), "6段は6段のまま");
        // 全体の日誌の目次(summaryが無ければtitle)
        let c = read(&dst, "10-common/journal/2026-10-07.md");
        assert!(c.contains("## 目次") && c.contains("- 10:00 [a] 朝にやったこと") && c.contains("- 14:30 [a] 午後の作業") && c.contains("- 03:30 [a] 深夜の要約"));
        // 旧記事は削除せず outdated で退避
        let old = read(&dst, "90-archive/20-projects/a/j1.md");
        assert!(old.contains("status: outdated") && old.contains("type: project") && old.contains("朝の本文。"));
        assert!(!dst.join("20-projects/a/journal/j1.md").exists());
        // 通しの照合
        assert_eq!(verify(src.path(), &dst).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn 更新時刻は写しても保たれる() {
        let src = dummy();
        let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        set_mtime(&src.path().join("30-resources/skills/work-start.md"), Some(t));
        let out = tempfile::tempdir().unwrap();
        apply(src.path(), &out.path().join("v7")).unwrap();
        assert_eq!(mtime_of(&out.path().join("v7/30-resources/skills/work-start.md")), Some(t));
    }

    #[test]
    fn 改ざんと欠落をverifyが見つける() {
        let src = dummy();
        let out = tempfile::tempdir().unwrap();
        let dst = out.path().join("v7");
        apply(src.path(), &dst).unwrap();
        // 1ファイルの改ざん
        std::fs::write(dst.join("20-projects/a/resource/doc.md"), "改ざん").unwrap();
        let p = verify(src.path(), &dst).unwrap();
        assert!(p.iter().any(|x| x.contains("中身が違う") && x.contains("doc.md")), "{p:?}");
        std::fs::write(dst.join("20-projects/a/resource/doc.md"), read(src.path(), "10-projects/a/resource/doc.md")).unwrap();
        assert!(verify(src.path(), &dst).unwrap().is_empty());
        // 旧記事の欠落(アーカイブから消えた)
        std::fs::remove_file(dst.join("90-archive/20-projects/a/j2.md")).unwrap();
        let p = verify(src.path(), &dst).unwrap();
        assert!(p.iter().any(|x| x.contains("無い") && x.contains("j2.md")), "{p:?}");
        // 日誌の節の欠落(本文が消えた)
        let _ = std::fs::write(dst.join("90-archive/20-projects/a/j2.md"), read(src.path(), "10-projects/a/journal/j2.md"));
        let jp = dst.join("20-projects/a/journal/2026-10-07.md");
        let text = std::fs::read_to_string(&jp).unwrap().replace("午後の作業", "名前が変わった");
        std::fs::write(&jp, text).unwrap();
        let p = verify(src.path(), &dst).unwrap();
        assert!(p.iter().any(|x| x.contains("見出し") || x.contains("中身が違う")), "{p:?}");
        // 余計なファイル
        std::fs::write(dst.join("extra.txt"), "x").unwrap();
        assert!(verify(src.path(), &dst).unwrap().iter().any(|x| x.contains("余計")));
    }

    #[test]
    fn 警告があれば適用しない() {
        // dateが無い・読めない旧journal
        let src = dummy();
        put(src.path(), "10-projects/a/journal/nodate.md", "---\ntitle: 日付なし\nkind: journal\n---\n本文\n");
        put(src.path(), "10-projects/a/journal/baddate.md", "---\ntitle: 変な日付\nkind: journal\ndate: きのう\n---\n本文\n");
        let rep = plan(src.path(), &tempfile::tempdir().unwrap().path().join("o")).unwrap();
        assert_eq!(rep.warnings.len(), 2, "{:?}", rep.warnings);
        let out = tempfile::tempdir().unwrap();
        let err = apply(src.path(), &out.path().join("v7")).unwrap_err();
        assert!(err.contains("警告があるので、適用しません") && err.contains("nodate.md"), "{err}");
        assert!(!out.path().join("v7").exists(), "警告があれば、何も書かない");
    }

    #[test]
    fn 名前の衝突を見つけて止める() {
        let src = dummy();
        // 10-projects と 20-areas の同じ名前のmemory(どちらも 20-projects/c/memory.md になる)
        put(src.path(), "10-projects/c/memory.md", "---\ntitle: c1\n---\n1\n");
        put(src.path(), "20-areas/c/memory.md", "---\ntitle: c2\n---\n2\n");
        let rep = plan(src.path(), &tempfile::tempdir().unwrap().path().join("o")).unwrap();
        assert!(rep.warnings.iter().any(|w| w.contains("名前の衝突(段階2)") && w.contains("20-projects/c/memory.md")), "{:?}", rep.warnings);
        // アーカイブ先の衝突(同じ名前の旧journalが、別の置き場にある)
        let src = dummy();
        put(src.path(), "20-areas/a/journal/j1.md", &journal("同名", "2026-10-07T12:00:00+09:00", "", "[]", "\n本文\n"));
        let rep = plan(src.path(), &tempfile::tempdir().unwrap().path().join("o")).unwrap();
        assert!(rep.warnings.iter().any(|w| w.contains("衝突") && w.contains("j1.md")), "{:?}", rep.warnings);
    }

    #[test]
    fn 行き先が空でなければ断る() {
        let src = dummy();
        let out = tempfile::tempdir().unwrap();
        std::fs::write(out.path().join("x.txt"), "x").unwrap();
        assert!(apply(src.path(), out.path()).unwrap_err().contains("空ではありません"));
        assert!(plan(src.path(), out.path()).unwrap_err().contains("空ではありません"));
    }

    #[test]
    fn 中身を変える関数の境界() {
        assert_eq!(fix_type_area("---\ntype: area\nkind: x\n---\ntype: area\n").unwrap(), "---\ntype: project\nkind: x\n---\ntype: area\n", "本文の行は変えない");
        assert!(fix_type_area("---\ntype: project\n---\n").is_none());
        assert!(fix_type_area("type: area\n").is_none(), "frontmatterが無ければ変えない");
        assert_eq!(fix_type_area("---\r\ntype: area\r\n---\r\n").unwrap(), "---\r\ntype: project\r\n---\r\n", "改行はそのまま");
        assert_eq!(set_status_outdated("---\nstatus: new\n---\n本文"), "---\nstatus: outdated\n---\n本文");
        assert_eq!(set_status_outdated("---\ntitle: a\n---\n本文"), "---\ntitle: a\nstatus: outdated\n---\n本文", "無ければ足す");
        assert_eq!(map_path("90-archive/20-areas/x/a.md"), "90-archive/20-projects/x/a.md");
        assert_eq!(map_path("30-resources/a.md"), "30-resources/a.md");
        assert_eq!(demote_headings("# a\n```\n# b\n```\n###### c\n#nospace"), "## a\n```\n# b\n```\n###### c\n#nospace");
    }
}
