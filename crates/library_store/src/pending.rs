//! 要確認(保留)の一覧と、解決(承認・保留・却下)。アプリの要確認の画面が使う(詩織Ver2.0 Phase 8)。
//! 対象は、タグ台帳・プロジェクト台帳(`status: pending`/`deferred`)と、受信箱の記事(`review_status`)。
//! 書き込みは、`registry`・`inbox` の、版つきの読み書きで行う(ほかの書き込みと重ならない)。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{fsx, inbox, registry, StoreError};

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PendingTag {
    pub canonical: String,
    pub status: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PendingProject {
    pub id: String,
    pub status: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PendingInboxItem {
    pub filename: String,
    pub title: Option<String>,
    pub reason: Option<String>,
    pub review_status: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct PendingItems {
    pub tags: Vec<PendingTag>,
    pub projects: Vec<PendingProject>,
    pub inbox: Vec<PendingInboxItem>,
}

#[derive(Deserialize)]
struct TagEntry {
    canonical: String,
    #[serde(default)]
    status: String,
}

#[derive(Deserialize)]
struct TagsFile {
    #[serde(default)]
    tags: Vec<TagEntry>,
}

#[derive(Deserialize)]
struct ProjectEntry {
    id: String,
    #[serde(default)]
    status: String,
}

#[derive(Deserialize)]
struct ProjectsFile {
    #[serde(default)]
    projects: Vec<ProjectEntry>,
}

#[derive(Deserialize, Default)]
struct InboxFields {
    title: Option<String>,
    reason: Option<String>,
    review_status: Option<String>,
}

/// 受信箱の記事のfrontmatterから、要確認の表示に要る3項目だけを読む。壊れていれば空。
fn inbox_fields(content: &str) -> InboxFields {
    let trimmed = content.trim_start_matches('\u{feff}');
    let Some(after_first) = trimmed.strip_prefix("---") else { return InboxFields::default() };
    let Some(end) = after_first.find("\n---") else { return InboxFields::default() };
    serde_yaml::from_str(&after_first[..end]).unwrap_or_default()
}

fn is_open(status: &str) -> bool {
    status == "pending" || status == "deferred"
}

/// 要確認の一覧。台帳が読めなければ、誤り(受信箱は、無ければ空)。
pub fn list(root: &Path) -> Result<PendingItems, String> {
    let read_text = |rel: &str| -> Result<String, String> {
        let (bytes, _) = fsx::read(root, rel).map_err(|e| format!("{rel}の読み込みに失敗: {e}"))?;
        String::from_utf8(bytes).map_err(|_| format!("{rel}がUTF-8ではありません"))
    };
    let tags_file: TagsFile =
        serde_yaml::from_str(&read_text(registry::TAGS_YAML)?).map_err(|e| format!("tags.yamlの解析に失敗: {e}"))?;
    let projects_file: ProjectsFile =
        serde_yaml::from_str(&read_text(registry::PROJECTS_YAML)?).map_err(|e| format!("projects.yamlの解析に失敗: {e}"))?;

    let tags = tags_file
        .tags
        .into_iter()
        .filter(|t| is_open(&t.status))
        .map(|t| PendingTag { canonical: t.canonical, status: t.status })
        .collect();
    let projects = projects_file
        .projects
        .into_iter()
        .filter(|p| is_open(&p.status))
        .map(|p| PendingProject { id: p.id, status: p.status })
        .collect();

    let mut items = Vec::new();
    for e in fsx::list_entries(root, inbox::INBOX_DIR).unwrap_or_default() {
        if !e.name.ends_with(".md") {
            continue;
        }
        let Ok(text) = read_text(&format!("{}/{}", inbox::INBOX_DIR, e.name)) else { continue };
        let f = inbox_fields(&text);
        if f.review_status.as_deref() == Some("confirmed") {
            continue;
        }
        items.push(PendingInboxItem { filename: e.name, title: f.title, reason: f.reason, review_status: f.review_status });
    }
    items.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(PendingItems { tags, projects, inbox: items })
}

/// 解決する対象の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Tag,
    Project,
    Inbox,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "tag" => Some(Kind::Tag),
            "project" => Some(Kind::Project),
            "inbox" => Some(Kind::Inbox),
            _ => None,
        }
    }
}

/// 操作(`confirm`・`defer`・`reject`)を、保存する状態にする。`reject` は、別に扱う。
fn action_to_status(action: &str) -> Result<&'static str, StoreError> {
    match action {
        "confirm" => Ok("confirmed"),
        "defer" => Ok("deferred"),
        other => Err(StoreError::Invalid(format!("未知の操作: {other}(confirm・defer・reject のどれか)"))),
    }
}

/// 解決する。`key` は、タグなら `canonical`、プロジェクトなら `id`、受信箱ならファイル名。
/// `reject` は、タグ・プロジェクトは台帳から外し、受信箱は記事を消す。
pub fn resolve(root: &Path, kind: Kind, key: &str, action: &str) -> Result<(), StoreError> {
    if action == "reject" {
        return match kind {
            Kind::Tag => registry::remove_item(root, registry::TAGS_YAML, "canonical", key),
            Kind::Project => registry::remove_item(root, registry::PROJECTS_YAML, "id", key),
            Kind::Inbox => inbox::remove(root, key),
        };
    }
    let status = action_to_status(action)?;
    match kind {
        Kind::Tag => registry::set_status(root, registry::TAGS_YAML, "canonical", key, status),
        Kind::Project => registry::set_status(root, registry::PROJECTS_YAML, "id", key, status),
        Kind::Inbox => inbox::set_review_status(root, key, status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let sys = d.path().join("_system");
        std::fs::create_dir_all(&sys).unwrap();
        std::fs::create_dir_all(d.path().join("00-inbox")).unwrap();
        std::fs::write(
            sys.join("tags.yaml"),
            "# 台帳\ntags:\n  - canonical: 詩織\n    status: confirmed\n  - canonical: 新タグ\n    status: pending\n  - canonical: 保留タグ\n    status: deferred\n",
        )
        .unwrap();
        std::fs::write(sys.join("projects.yaml"), "projects:\n  - id: shiori\n    status: confirmed\n  - id: newp\n    status: pending\n").unwrap();
        std::fs::write(d.path().join("00-inbox/a.md"), "---\ntitle: 受信A\nreason: tagsが空です\n---\n本文").unwrap();
        std::fs::write(d.path().join("00-inbox/b.md"), "---\ntitle: 受信B\nreview_status: confirmed\n---\n本文").unwrap();
        std::fs::write(d.path().join("00-inbox/c.md"), "---\ntitle: 受信C\nreview_status: deferred\n---\n本文").unwrap();
        std::fs::write(d.path().join("00-inbox/memo.txt"), "対象外").unwrap();
        d
    }

    #[test]
    fn 要確認は未着手と保留だけを出し確定済みは出さない() {
        let d = lib();
        let p = list(d.path()).unwrap();
        assert_eq!(p.tags.iter().map(|t| (t.canonical.as_str(), t.status.as_str())).collect::<Vec<_>>(), [("新タグ", "pending"), ("保留タグ", "deferred")]);
        assert_eq!(p.projects, vec![PendingProject { id: "newp".into(), status: "pending".into() }]);
        let names: Vec<_> = p.inbox.iter().map(|i| i.filename.as_str()).collect();
        assert_eq!(names, ["a.md", "c.md"], "確定済みと.md以外は出さない");
        assert_eq!(p.inbox[0].reason.as_deref(), Some("tagsが空です"));
        assert_eq!(p.inbox[1].review_status.as_deref(), Some("deferred"));
    }

    #[test]
    fn 承認と保留は状態を書き換え台帳のコメントは残る() {
        let d = lib();
        resolve(d.path(), Kind::Tag, "新タグ", "confirm").unwrap();
        resolve(d.path(), Kind::Project, "newp", "defer").unwrap();
        resolve(d.path(), Kind::Inbox, "a.md", "defer").unwrap();
        let tags = std::fs::read_to_string(d.path().join("_system/tags.yaml")).unwrap();
        assert!(tags.starts_with("# 台帳\n"), "先頭のコメントは残る");
        let p = list(d.path()).unwrap();
        assert_eq!(p.tags.len(), 1, "承認したタグは一覧から消える");
        assert_eq!(p.projects[0].status, "deferred");
        assert_eq!(p.inbox[0].review_status.as_deref(), Some("deferred"));
        resolve(d.path(), Kind::Inbox, "a.md", "confirm").unwrap();
        assert_eq!(list(d.path()).unwrap().inbox.len(), 1);
    }

    #[test]
    fn 却下は台帳から外し受信箱の記事は消す() {
        let d = lib();
        resolve(d.path(), Kind::Tag, "新タグ", "reject").unwrap();
        resolve(d.path(), Kind::Project, "newp", "reject").unwrap();
        resolve(d.path(), Kind::Inbox, "a.md", "reject").unwrap();
        let p = list(d.path()).unwrap();
        assert_eq!(p.tags.len(), 1);
        assert!(p.projects.is_empty());
        assert!(!d.path().join("00-inbox/a.md").exists());
    }

    #[test]
    fn 未知の操作と存在しない対象と受信箱の外は断る() {
        let d = lib();
        assert!(matches!(resolve(d.path(), Kind::Tag, "新タグ", "bogus"), Err(StoreError::Invalid(_))));
        assert!(resolve(d.path(), Kind::Tag, "無いタグ", "confirm").is_err());
        assert!(resolve(d.path(), Kind::Inbox, "../_system/tags.yaml", "reject").is_err());
        assert!(resolve(d.path(), Kind::Inbox, "none.md", "confirm").is_err());
        assert!(Kind::parse("x").is_none());
    }

    #[test]
    fn 台帳が読めなければ誤りにする() {
        let d = tempfile::tempdir().unwrap();
        assert!(list(d.path()).is_err());
    }
}
