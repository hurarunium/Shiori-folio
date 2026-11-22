//! 知識の棚(`portable/knowledge/`)の記事の規則と保存(詩織Ver5.0)。
//! 置き場(root)はlibraryではなく棚。出典で確かめた知識の正本を置く場所で、
//! libraryの配置ルール・タグの台帳は使わない。

use crate::fsx::{self, Cond, StoreError};
use crate::notes::{split_frontmatter, Frontmatter};
use std::path::{Path, PathBuf};

/// 棚の記事のtype。library/の4種(`notes::VALID_TYPES`)とは別物で、library/には置かない。
pub const KNOWLEDGE_TYPE: &str = "knowledge";
// 棚の下の管理用フォルダ(索引の対象外)。下書きと、上書き前の旧版を置く。
const SHELF_SYSTEM_DIR: &str = "_system";
const SHELF_DRAFTS_DIR: &str = "drafts";
const SHELF_HISTORY_DIR: &str = "history";
pub const DRAFT_STATUS: &str = "draft";
// 棚の記事に必須の項目。sourceは読む側(indexing・shelf.rs)が1行しか読まないため、
// 保存時にSOURCE_SEPARATORで1行へそろえる。
const SHELF_REQUIRED_FIELDS: &[&str] = &["topic", "learned", "source"];
pub const SOURCE_SEPARATOR: &str = " / ";

/// YAMLのスカラー値を文字列として取り出す(learnedが日付・数値として読まれた場合も文字列にする)。
fn yaml_text(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn shelf_field(fm: &Frontmatter, key: &str) -> String {
    fm.extra
        .get(key)
        .and_then(yaml_text)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// sourceのリスト・複数行の文字列を、空の要素を除いて「 / 」区切りの1行にする。
pub fn single_line_source(value: &serde_yaml::Value) -> String {
    let parts: Vec<String> = match value {
        serde_yaml::Value::Sequence(items) => items.iter().filter_map(yaml_text).collect(),
        other => yaml_text(other)
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default(),
    };
    parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(SOURCE_SEPARATOR)
}

/// 棚に置けるか検証する。memoryと同じく、不備はinboxへ逃がさずエラーにする
/// (棚は、ふらるが承認した記事だけを置く場所のため)。
pub fn validate_shelf(fm: &Frontmatter) -> Result<(), StoreError> {
    if fm.title.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err(StoreError::Invalid("titleが空です".into()));
    }
    if fm.tags.is_empty() {
        return Err(StoreError::Invalid("tagsが空です".into()));
    }
    for key in SHELF_REQUIRED_FIELDS {
        if shelf_field(fm, key).is_empty() {
            return Err(StoreError::Invalid(format!(
                "{key}が空です(知識の棚の記事には{SHELF_REQUIRED_FIELDS:?}が必要です)"
            )));
        }
    }
    Ok(())
}

/// 棚に置くファイル名の検査。棚の一覧(shelf.rs)は隠しファイルを読まないため、`.`始まりは拒否する。
pub fn check_file_name(name: &str) -> Result<(), StoreError> {
    if name.contains('/') || name.contains('\\') {
        return Err(StoreError::Invalid(format!("ファイル名に区切りは使えません: {name}")));
    }
    if !name.ends_with(".md") {
        return Err(StoreError::Invalid(format!("知識の棚の記事は.mdにしてください: {name}")));
    }
    if name.starts_with('.') {
        return Err(StoreError::Invalid(format!(
            ".で始まる名前は棚の一覧に出ないため使えません: {name}"
        )));
    }
    Ok(())
}

/// 保存先(棚からの相対パス)。下書きは`_system/drafts/`、それ以外は棚の直下。
pub fn destination_rel(file_name: &str, is_draft: bool) -> String {
    if is_draft {
        format!("{SHELF_SYSTEM_DIR}/{SHELF_DRAFTS_DIR}/{file_name}")
    } else {
        file_name.to_string()
    }
}

/// 退避名に付ける旧版の日付。learnedが無ければdateの日付部分、それも無ければunknown。
pub fn shelf_history_label(old_fm: &Frontmatter) -> String {
    let learned = shelf_field(old_fm, "learned");
    let label = if !learned.is_empty() {
        learned
    } else {
        old_fm
            .date
            .as_deref()
            .map(|d| d.chars().take(10).collect::<String>())
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    };
    label.replace([':', '/', '\\'], "-")
}

#[derive(Debug, PartialEq, Eq)]
pub struct SavedShelf {
    pub path: PathBuf,
    /// 棚からの相対パス。
    pub rel: String,
    pub rev: String,
    pub draft: bool,
    /// 旧版を退避した先(棚からの相対)。新規・下書きのときはNone。
    pub history_rel: Option<String>,
}

/// 旧版を、退避先で既存のファイルと衝突しない名前({名前}-{日付}.md、{名前}-{日付}-2.md…)で書く。
/// 2026-10-21に、退避先の同名を確かめずに上書きして仮データを消したことの再発防止。
fn write_history(shelf: &Path, stem: &str, label: &str, old_text: &str) -> Result<String, StoreError> {
    let mut n = 1;
    loop {
        let name = if n == 1 { format!("{stem}-{label}.md") } else { format!("{stem}-{label}-{n}.md") };
        let rel = format!("{SHELF_SYSTEM_DIR}/{SHELF_HISTORY_DIR}/{name}");
        match fsx::write(shelf, &rel, old_text.as_bytes(), Cond::CreateNew) {
            Ok(_) => return Ok(rel),
            Err(StoreError::Exists(_)) => n += 1,
            Err(e) => return Err(StoreError::Io(format!("旧版の退避に失敗: {e}"))),
        }
    }
}

/// 知識の棚へ保存する。status: draftは`_system/drafts/`へ(索引に入れない、同名は上書き)、
/// それ以外は棚の直下へ置く。正式な記事の上書きでは、旧版を`_system/history/`へ連番付きで退避してから
/// 置き換え、作成日時(date)は旧版から引き継ぐ。索引への反映と元ファイルの削除は、呼び出し側が行う。
pub fn save_shelf(shelf: &Path, file_name: &str, mut fm: Frontmatter, body: &str) -> Result<SavedShelf, StoreError> {
    if !shelf.is_dir() {
        return Err(StoreError::NotFound(format!("知識の棚(knowledge/)が見つかりません: {}", shelf.display())));
    }
    if let Some(value) = fm.extra.get("source") {
        let line = single_line_source(value);
        fm.extra.insert("source".to_string(), serde_yaml::Value::String(line));
    }
    validate_shelf(&fm)?;
    check_file_name(file_name)?;
    let is_draft = fm.status.trim() == DRAFT_STATUS;
    let rel = destination_rel(file_name, is_draft);

    // 正式な記事の上書きだけ、旧版を読む。読んだ版のときだけ置き換える。
    let mut old_rev = None;
    let mut history_rel = None;
    if !is_draft {
        match fsx::read(shelf, &rel) {
            Ok((bytes, rev)) => {
                let old_text = String::from_utf8(bytes)
                    .map_err(|_| StoreError::Invalid(format!("既存の記事がUTF-8ではありません: {rel}")))?;
                let (old_fm, _) = split_frontmatter(&old_text)
                    .map_err(|e| StoreError::Invalid(format!("既存の記事のfrontmatterを読めません: {rel} ({e})")))?;
                fm.date = old_fm.date.clone().or(fm.date);
                let stem = file_name.strip_suffix(".md").unwrap_or(file_name);
                // 退避に失敗したら上書きしない。
                history_rel = Some(write_history(shelf, stem, &shelf_history_label(&old_fm), &old_text)?);
                old_rev = Some(rev);
            }
            Err(StoreError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
    }

    let frontmatter_yaml = serde_yaml::to_string(&fm)
        .map_err(|e| StoreError::Invalid(format!("frontmatterのYAML化に失敗: {e}")))?;
    let content = format!("---\n{frontmatter_yaml}---\n{body}");
    let cond = match (&old_rev, is_draft) {
        (Some(rev), _) => Cond::Overwrite { expect_rev: rev.clone() },
        (None, true) => Cond::Any, // 下書きは同名を上書きする
        (None, false) => Cond::CreateNew,
    };
    let written = match fsx::write(shelf, &rel, content.as_bytes(), cond) {
        Ok(w) => w,
        Err(e) => {
            // 書けなかったときは、いま作った退避(この名前は自分だけが作った)を取り消す。
            // 競合で負けた側が、勝った側と同じ旧版の退避を余分に残さないため。
            if let Some(h) = &history_rel {
                let _ = fsx::remove(shelf, h, None);
            }
            return Err(match e {
                StoreError::Conflict { .. } | StoreError::Exists(_) => StoreError::Stale(format!(
                    "競合: 保存の途中で、他のセッションが先に同じ記事を更新しました。何も上書きしていません。読み直してやり直してください: {rel}"
                )),
                other => other,
            });
        }
    };

    Ok(SavedShelf { path: written.path, rel, rev: written.rev, draft: is_draft, history_rel })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Shelf(PathBuf);
    impl Shelf {
        fn new(tag: &str) -> Shelf {
            let p = std::env::temp_dir().join(format!("library_store-shelf-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Shelf(p)
        }
        fn read(&self, rel: &str) -> String {
            std::fs::read_to_string(self.0.join(rel)).unwrap()
        }
    }
    impl Drop for Shelf {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn shelf_fm(yaml: &str) -> Frontmatter {
        split_frontmatter(&format!("---\n{yaml}\n---\n本文\n")).unwrap().0
    }

    const OK: &str = "title: t\ntype: knowledge\ntags:\n- 数学\ntopic: 数学\nlearned: 2026-10-23\nsource: 教科書";

    #[test]
    fn 棚のsourceはリストも複数行も1行にそろえる() {
        let fm = shelf_fm("source:\n- https://a.example/x\n- ''\n- 'Keisler, Foundations'");
        assert_eq!(single_line_source(&fm.extra["source"]), "https://a.example/x / Keisler, Foundations");
        let fm = shelf_fm("source: |\n  教科書A\n\n  https://b.example\n");
        assert_eq!(single_line_source(&fm.extra["source"]), "教科書A / https://b.example");
        let fm = shelf_fm("source: 'そのまま 1行'");
        assert_eq!(single_line_source(&fm.extra["source"]), "そのまま 1行");
    }

    #[test]
    fn 長いsourceも書き出しで折り返さない() {
        let long = vec!["https://example.com/a very long title with spaces"; 6].join(SOURCE_SEPARATOR);
        let mut fm = shelf_fm("title: t");
        fm.extra.insert("source".to_string(), serde_yaml::Value::String(long.clone()));
        let yaml = serde_yaml::to_string(&fm).unwrap();
        let line = yaml.lines().find(|l| l.starts_with("source:")).unwrap();
        assert!(line.contains("example.com/a very long title with spaces / https"));
        assert!(line.ends_with(&long) || line.contains(&long), "{line}");
    }

    #[test]
    fn 棚の必須項目が欠けたら拒否する() {
        assert!(validate_shelf(&shelf_fm(OK)).is_ok());
        for missing in ["topic", "learned", "source"] {
            let yaml: String = OK
                .lines()
                .filter(|l| !l.starts_with(&format!("{missing}:")))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(validate_shelf(&shelf_fm(&yaml)).is_err(), "{missing}が無いのに通った");
        }
        assert!(validate_shelf(&shelf_fm(&OK.replace("title: t", "title: ''"))).is_err());
        assert!(validate_shelf(&shelf_fm(&OK.replace("tags:\n- 数学", "tags: []"))).is_err());
    }

    #[test]
    fn 棚の退避名はlearnedかdateの日付を使う() {
        assert_eq!(shelf_history_label(&shelf_fm("learned: '2026-10-21'")), "2026-10-21");
        assert_eq!(shelf_history_label(&shelf_fm("date: 2026-10-17T15:00:55+09:00")), "2026-10-17");
        assert_eq!(shelf_history_label(&shelf_fm("title: t")), "unknown");
    }

    #[test]
    fn 棚のファイル名はmdで隠しファイルでないこと() {
        assert!(check_file_name("math-a.md").is_ok());
        for bad in ["math-a.txt", ".math-a.md", "x/math-a.md", "x\\math-a.md"] {
            assert!(check_file_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn 正式な記事を棚の直下に新規保存する() {
        let s = Shelf::new("new");
        let saved = save_shelf(&s.0, "math-a.md", shelf_fm(OK), "本文\n").unwrap();
        assert_eq!((saved.rel.as_str(), saved.draft, saved.history_rel.is_none()), ("math-a.md", false, true));
        assert!(s.read("math-a.md").ends_with("---\n本文\n"));
        // 同じ名前での2回目の新規は、上書き扱いになる(次のテスト)。ここでは棚の外に何も作らない
        assert!(!s.0.join("_system").exists());
    }

    #[test]
    fn 上書きは旧版を連番つきで退避しdateを引き継ぐ() {
        let s = Shelf::new("update");
        let first = save_shelf(&s.0, "math-a.md", shelf_fm(&format!("{OK}\ndate: 2026-10-01T00:00:00+09:00")), "旧\n").unwrap();
        let old_text = s.read(&first.rel);
        // 同じlearnedで2回・3回と上書きしても、退避は上書きされず連番になる
        let second = save_shelf(&s.0, "math-a.md", shelf_fm(&format!("{OK}\ndate: 2026-11-01T00:00:00+09:00")), "新1\n").unwrap();
        let third = save_shelf(&s.0, "math-a.md", shelf_fm(&format!("{OK}\ndate: 2026-11-02T00:00:00+09:00")), "新2\n").unwrap();

        assert_eq!(second.history_rel.as_deref(), Some("_system/history/math-a-2026-10-23.md"));
        assert_eq!(third.history_rel.as_deref(), Some("_system/history/math-a-2026-10-23-2.md"));
        assert_eq!(s.read("_system/history/math-a-2026-10-23.md"), old_text, "最初の旧版がそのまま残る");
        let text = s.read("math-a.md");
        assert!(text.contains("date: 2026-10-01T00:00:00+09:00"), "作成日時は最初のまま: {text}");
        assert!(text.ends_with("---\n新2\n"));
    }

    #[test]
    fn 下書きは専用の場所へ置き同名は上書きし退避しない() {
        let s = Shelf::new("draft");
        let y = format!("{OK}\nstatus: draft");
        let a = save_shelf(&s.0, "math-a.md", shelf_fm(&y), "1\n").unwrap();
        let b = save_shelf(&s.0, "math-a.md", shelf_fm(&y), "2\n").unwrap();
        assert_eq!((a.rel.as_str(), a.draft, b.history_rel.is_none()), ("_system/drafts/math-a.md", true, true));
        assert!(s.read(&b.rel).ends_with("---\n2\n"));
        assert!(!s.0.join("math-a.md").exists(), "棚の直下には置かない");
        assert!(!s.0.join("_system/history").exists());
    }

    #[test]
    fn 不備のある記事は何も書かずに断る() {
        let s = Shelf::new("invalid");
        assert!(save_shelf(&s.0, "a.md", shelf_fm("title: t\ntags: [a]"), "x").is_err());
        assert!(save_shelf(&s.0, "a.txt", shelf_fm(OK), "x").is_err());
        assert!(save_shelf(&s.0, ".a.md", shelf_fm(OK), "x").is_err());
        assert!(std::fs::read_dir(&s.0).unwrap().next().is_none(), "何も作られない");
        // 棚が無い
        let missing = s.0.join("無い");
        assert!(matches!(save_shelf(&missing, "a.md", shelf_fm(OK), "x"), Err(StoreError::NotFound(_))));
    }

    #[test]
    fn 壊れた既存の記事は何も変えずに断る() {
        let s = Shelf::new("broken");
        std::fs::write(s.0.join("a.md"), "frontmatterの無い記事").unwrap();
        let r = save_shelf(&s.0, "a.md", shelf_fm(OK), "新\n");
        assert!(matches!(r, Err(StoreError::Invalid(_))), "{r:?}");
        assert_eq!(s.read("a.md"), "frontmatterの無い記事");
        assert!(!s.0.join("_system").exists(), "退避も作られない");
    }

    #[test]
    fn 同じ記事を同時に更新しても片方の変更が消えない() {
        let s = Shelf::new("race");
        save_shelf(&s.0, "a.md", shelf_fm(OK), "初\n").unwrap();
        let root = std::sync::Arc::new(s.0.clone());
        let handles: Vec<_> = (0..6)
            .map(|i| {
                let root = root.clone();
                std::thread::spawn(move || save_shelf(&root, "a.md", shelf_fm(OK), &format!("本文{i}\n")))
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        // 通った更新の数だけ、旧版の退避が残る(負けた側は、退避を残さない)
        let ok = results.iter().filter(|r| r.is_ok()).count();
        assert!(ok >= 1);
        assert_eq!(fsx::list_names(&s.0, "_system/history").unwrap().len(), ok);
        assert!(results.iter().all(|r| r.is_ok() || matches!(r, Err(StoreError::Stale(_)))), "{results:?}");
    }
}
