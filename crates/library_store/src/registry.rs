//! `_system/projects.yaml`・`_system/tags.yaml`(台帳)の読み書き。
//!
//! 台帳は先頭のコメントを人間が書いているため、パースして丸ごと書き直さない。
//! 読むときだけパースし、書くときは対象のブロックをテキスト上で探して、
//! 該当の行だけを書き換える(削除はブロックごと除く、登録は末尾へ足す)。
//! 書くときは「読む → 変える → 版つきで置き換える」。読んでから書くまでに
//! 別の書き込みが入ったら、読み直してやり直す。

use crate::fsx::{self, Cond, StoreError};
use serde::Deserialize;
use std::path::Path;

pub const PROJECTS_YAML: &str = "_system/projects.yaml";
pub const TAGS_YAML: &str = "_system/tags.yaml";

/// 読んでから書くまでに、別の書き込みが入ったときの、やり直しの最大回数。
const MAX_RETRY: usize = 50;

#[derive(Debug, Deserialize)]
struct ProjectsFile {
    #[serde(default)]
    projects: Vec<ProjectEntry>,
}

#[derive(Debug, Deserialize)]
struct ProjectEntry {
    id: String,
}

#[derive(Debug, Deserialize)]
struct TagsFile {
    #[serde(default)]
    tags: Vec<TagEntry>,
}

#[derive(Debug, Deserialize)]
struct TagEntry {
    canonical: String,
    #[serde(default)]
    aliases: Vec<String>,
}

// ---- テキストの操作(ファイルに触れない) ----

/// frontmatterから取り出した値を、コロンや日本語を含んでも壊れないよう、YAMLのスカラーにする。
fn yaml_scalar(s: &str) -> Result<String, StoreError> {
    serde_yaml::to_string(s)
        .map(|t| t.trim_end().to_string())
        .map_err(|e| StoreError::Invalid(format!("YAMLにできない値です: {e}")))
}

/// 台帳の、`  - {key_field}: {key_value}` で始まるブロックの行範囲([開始行, 終了行))。
/// 終了行は、次の `  - ` 行、または末尾。
fn find_list_item_range(lines: &[String], key_field: &str, key_value: &str) -> Option<(usize, usize)> {
    let marker = format!("- {key_field}:");
    let mut start = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(&marker) {
            let value = rest.trim().trim_matches('"').trim_matches('\'');
            if value == key_value {
                start = Some(i);
                break;
            }
        }
    }
    let start = start?;
    let mut end = lines.len();
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim_start().starts_with("- ") && line.starts_with("  -") {
            end = i;
            break;
        }
    }
    Some((start, end))
}

fn not_found(key_field: &str, key_value: &str) -> StoreError {
    StoreError::NotFound(format!("{key_field}: {key_value} のエントリ"))
}

/// 対象のエントリの `status:` 行だけを書き換えた全文を返す。
pub fn set_status_text(text: &str, key_field: &str, key_value: &str, new_status: &str) -> Result<String, StoreError> {
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    let (start, end) = find_list_item_range(&lines, key_field, key_value).ok_or_else(|| not_found(key_field, key_value))?;

    let status_line_idx = (start..end)
        .find(|&i| lines[i].trim_start().starts_with("status:"))
        .ok_or_else(|| StoreError::Invalid("statusフィールドが見つかりませんでした。".into()))?;
    let indent_len = lines[status_line_idx].len() - lines[status_line_idx].trim_start().len();
    lines[status_line_idx] = format!("{}status: {new_status}", " ".repeat(indent_len));

    let mut new_text = lines.join("\n");
    if text.ends_with('\n') {
        new_text.push('\n');
    }
    Ok(new_text)
}

/// 対象のエントリをブロックごと除いた全文を返す。
pub fn remove_item_text(text: &str, key_field: &str, key_value: &str) -> Result<String, StoreError> {
    let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    let (start, end) = find_list_item_range(&lines, key_field, key_value).ok_or_else(|| not_found(key_field, key_value))?;

    let mut remaining: Vec<String> = lines[..start].to_vec();
    remaining.extend_from_slice(&lines[end..]);

    let mut new_text = remaining.join("\n");
    if !remaining.is_empty() && text.ends_with('\n') {
        new_text.push('\n');
    }
    Ok(new_text)
}

/// 末尾にブロックを足した全文を返す。末尾が改行で終わっていなくても、前の行とつながらない。
fn append_block(text: &str, block: &str) -> String {
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(block);
    out
}

fn parse<T: for<'de> Deserialize<'de>>(text: &str, what: &str) -> Result<T, StoreError> {
    serde_yaml::from_str(text).map_err(|e| StoreError::Invalid(format!("{what}の解析に失敗: {e}")))
}

/// projects.yamlの全文から、登録されているidを登録順に返す。
pub fn project_ids(text: &str) -> Result<Vec<String>, StoreError> {
    let parsed: ProjectsFile = parse(text, "projects.yaml")?;
    Ok(parsed.projects.into_iter().map(|p| p.id).collect())
}

// ---- ファイルの操作 ----

fn read_text(root: &Path, rel: &str) -> Result<(String, String), StoreError> {
    let (bytes, rev) = fsx::read(root, rel)?;
    let text = String::from_utf8(bytes).map_err(|_| StoreError::Invalid(format!("{rel}がUTF-8ではありません")))?;
    Ok((text, rev))
}

/// 台帳を読み、`change` で新しい全文を作り、版つきで置き換える。
/// `change` が `None` を返したときは、何も書かない(戻り値は `false`)。
/// 読んでから書くまでに別の書き込みが入ったら、読み直してやり直す。
fn update(
    root: &Path,
    rel: &str,
    mut change: impl FnMut(&str) -> Result<Option<String>, StoreError>,
) -> Result<bool, StoreError> {
    for attempt in 0..MAX_RETRY {
        let (text, rev) = read_text(root, rel)?;
        let Some(new_text) = change(&text)? else {
            return Ok(false);
        };
        match fsx::write(root, rel, new_text.as_bytes(), Cond::Overwrite { expect_rev: rev }) {
            Ok(_) => return Ok(true),
            Err(StoreError::Conflict { .. }) => {
                // 別の書き込みと重なった。少し待って、読み直す
                std::thread::sleep(std::time::Duration::from_millis(1 + (attempt as u64 % 8)));
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(StoreError::Io(format!("{rel}への書き込みが重なって、やり直しても書けませんでした")))
}

/// 対象のエントリの `status:` を書き換える(要確認UIの確定・保留)。
pub fn set_status(root: &Path, rel: &str, key_field: &str, key_value: &str, new_status: &str) -> Result<(), StoreError> {
    update(root, rel, |t| set_status_text(t, key_field, key_value, new_status).map(Some)).map(|_| ())
}

/// 対象のエントリをブロックごと除く(要確認UIの却下)。
pub fn remove_item(root: &Path, rel: &str, key_field: &str, key_value: &str) -> Result<(), StoreError> {
    update(root, rel, |t| remove_item_text(t, key_field, key_value).map(Some)).map(|_| ())
}

/// projects.yamlに指定idが無ければ、末尾に `status: pending` で登録する。
/// 登録したときだけ `true`。すでにあれば何も書かない。
pub fn ensure_project(root: &Path, id: &str) -> Result<bool, StoreError> {
    let block = format!("  - id: {}\n    status: pending\n", yaml_scalar(id)?);
    update(root, PROJECTS_YAML, |t| {
        if project_ids(t)?.iter().any(|p| p == id) {
            return Ok(None);
        }
        Ok(Some(append_block(t, &block)))
    })
}

/// tags.yamlのcanonical・aliasesと照合し、一致すればcanonical表記を返す。
/// 一致しなければ末尾に `status: pending` で登録し、そのままの表記を返す。
/// `newly_pending` は、同じ実行の中で同じ新規タグを二重に登録しないための記録。
pub fn normalize_tag(root: &Path, raw: &str, newly_pending: &mut Vec<String>) -> Result<String, StoreError> {
    let (text, _) = read_text(root, TAGS_YAML)?;
    let parsed: TagsFile = parse(&text, "tags.yaml")?;
    for entry in &parsed.tags {
        if entry.canonical == raw || entry.aliases.iter().any(|a| a == raw) {
            return Ok(entry.canonical.clone());
        }
    }
    if newly_pending.iter().any(|t| t == raw) {
        return Ok(raw.to_string());
    }

    let block = format!("  - canonical: {}\n    aliases: []\n    status: pending\n", yaml_scalar(raw)?);
    update(root, TAGS_YAML, |t| {
        // 読み直した時点で、別の書き込みがすでに登録していたら、足さない
        let parsed: TagsFile = parse(t, "tags.yaml")?;
        if parsed.tags.iter().any(|e| e.canonical == raw || e.aliases.iter().any(|a| a == raw)) {
            return Ok(None);
        }
        Ok(Some(append_block(t, &block)))
    })?;
    newly_pending.push(raw.to_string());
    Ok(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str, tags: &str, projects: &str) -> Dir {
            let p = std::env::temp_dir().join(format!("library_store-registry-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(p.join("_system")).unwrap();
            std::fs::write(p.join(TAGS_YAML), tags).unwrap();
            std::fs::write(p.join(PROJECTS_YAML), projects).unwrap();
            Dir(p)
        }
        fn tags(&self) -> String {
            std::fs::read_to_string(self.0.join(TAGS_YAML)).unwrap()
        }
        fn projects(&self) -> String {
            std::fs::read_to_string(self.0.join(PROJECTS_YAML)).unwrap()
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const SAMPLE_TAGS: &str = "# ヘッダーコメント1行目\n# ヘッダーコメント2行目\ntags:\n  - canonical: 詩織\n    aliases: [shiori]\n    status: confirmed\n  - canonical: 新タグ\n    aliases: []\n    status: pending\n";
    const SAMPLE_PROJECTS: &str = "# プロジェクト一覧\nprojects:\n  - id: shiori\n    status: confirmed\n";

    // lib.rs(要確認UI)から移したテスト
    #[test]
    fn 状態の書き換えは対象のエントリだけを変える() {
        let d = Dir::new("set", SAMPLE_TAGS, SAMPLE_PROJECTS);
        set_status(&d.0, TAGS_YAML, "canonical", "新タグ", "confirmed").unwrap();
        let result = d.tags();
        assert!(result.contains("# ヘッダーコメント1行目"), "先頭コメントが保持されていない");
        assert!(result.contains("canonical: 詩織\n    aliases: [shiori]\n    status: confirmed"), "既存エントリが変化した");
        assert!(result.contains("canonical: 新タグ\n    aliases: []\n    status: confirmed"), "対象エントリが書き換わっていない");
    }

    #[test]
    fn 削除は対象のエントリだけを除く() {
        let d = Dir::new("remove", SAMPLE_TAGS, SAMPLE_PROJECTS);
        remove_item(&d.0, TAGS_YAML, "canonical", "新タグ").unwrap();
        let result = d.tags();
        assert!(result.contains("# ヘッダーコメント1行目"), "先頭コメントが保持されていない");
        assert!(result.contains("canonical: 詩織"), "既存エントリが消えた");
        assert!(!result.contains("新タグ"), "対象エントリが削除されていない");
    }

    #[test]
    fn 対象が無ければ何も書かずに断る() {
        let d = Dir::new("missing", SAMPLE_TAGS, SAMPLE_PROJECTS);
        assert!(matches!(
            set_status(&d.0, TAGS_YAML, "canonical", "無いタグ", "confirmed"),
            Err(StoreError::NotFound(_))
        ));
        assert_eq!(d.tags(), SAMPLE_TAGS);
    }

    #[test]
    fn 引用符つきの値も見つかる() {
        let text = "projects:\n  - id: \"a:b\"\n    status: pending\n";
        let out = set_status_text(text, "id", "a:b", "confirmed").unwrap();
        assert_eq!(out, "projects:\n  - id: \"a:b\"\n    status: confirmed\n");
    }

    // shiori_save.rsの追記を移したもの
    #[test]
    fn 未登録のprojectは末尾にpendingで足す() {
        let d = Dir::new("proj-add", SAMPLE_TAGS, SAMPLE_PROJECTS);
        assert!(ensure_project(&d.0, "新作").unwrap());
        let result = d.projects();
        assert!(result.starts_with(SAMPLE_PROJECTS), "既存の内容が変わった");
        assert!(result.ends_with("  - id: 新作\n    status: pending\n"), "{result}");
        assert_eq!(project_ids(&result).unwrap(), vec!["shiori".to_string(), "新作".to_string()]);
    }

    #[test]
    fn 登録済みのprojectは何も書かない() {
        let d = Dir::new("proj-exists", SAMPLE_TAGS, SAMPLE_PROJECTS);
        assert!(!ensure_project(&d.0, "shiori").unwrap());
        assert_eq!(d.projects(), SAMPLE_PROJECTS);
    }

    #[test]
    fn 末尾に改行が無い台帳でも前の行とつながらない() {
        let d = Dir::new("proj-nonl", SAMPLE_TAGS, "projects:\n  - id: shiori\n    status: confirmed");
        ensure_project(&d.0, "x").unwrap();
        assert_eq!(project_ids(&d.projects()).unwrap(), vec!["shiori".to_string(), "x".to_string()]);
    }

    #[test]
    fn 日本語やコロンを含むidも壊れずに登録できる() {
        let d = Dir::new("proj-quote", SAMPLE_TAGS, SAMPLE_PROJECTS);
        ensure_project(&d.0, "a: b # c").unwrap();
        assert!(project_ids(&d.projects()).unwrap().contains(&"a: b # c".to_string()));
    }

    #[test]
    fn タグは別名ならcanonicalに直し未登録なら足す() {
        let d = Dir::new("tag", SAMPLE_TAGS, SAMPLE_PROJECTS);
        let mut pending = Vec::new();
        assert_eq!(normalize_tag(&d.0, "shiori", &mut pending).unwrap(), "詩織");
        assert_eq!(normalize_tag(&d.0, "詩織", &mut pending).unwrap(), "詩織");
        assert_eq!(d.tags(), SAMPLE_TAGS);
        assert!(pending.is_empty());

        assert_eq!(normalize_tag(&d.0, "未知", &mut pending).unwrap(), "未知");
        assert_eq!(pending, vec!["未知".to_string()]);
        // 同じ実行で、もう一度同じタグが来ても、二重に登録しない
        assert_eq!(normalize_tag(&d.0, "未知", &mut pending).unwrap(), "未知");
        assert_eq!(d.tags().matches("canonical: 未知").count(), 1);
        assert!(d.tags().ends_with("  - canonical: 未知\n    aliases: []\n    status: pending\n"));
    }

    #[test]
    fn 同時に別々のタグを登録しても全部残る() {
        let d = Dir::new("tags-race", SAMPLE_TAGS, SAMPLE_PROJECTS);
        let root = std::sync::Arc::new(d.0.clone());
        let handles: Vec<_> = (0..12)
            .map(|i| {
                let root = root.clone();
                std::thread::spawn(move || {
                    let mut pending = Vec::new();
                    normalize_tag(&root, &format!("並行{i}"), &mut pending)
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let result = d.tags();
        for i in 0..12 {
            assert_eq!(result.matches(&format!("canonical: 並行{i}
")).count(), 1, "並行{i}: {result}");
        }
        assert!(result.starts_with("# ヘッダーコメント1行目"), "コメントも残る");
    }

}
