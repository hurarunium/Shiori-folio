//! `00-inbox` の操作(メモの保存、要確認UIの確定・保留・却下)。
//! 書き込みはすべて `fsx` を通す。

use crate::fsx::{self, Cond, StoreError};
use std::path::Path;

pub const INBOX_DIR: &str = "00-inbox";

/// 同じ名前のメモがすでにあるとき、連番(-2、-3 …)を付けて探す上限。
const MAX_MEMO_SUFFIX: usize = 1000;

/// 読んでから書くまでに、別の書き込みが入ったときの、やり直しの最大回数。
const MAX_RETRY: usize = 50;

/// inbox直下のファイル名だけを許す(区切り・`..`・空は断る)。
fn rel_of(filename: &str) -> Result<String, StoreError> {
    if filename.is_empty() || filename.contains('/') || filename.contains('\\') || filename == ".." {
        return Err(StoreError::Invalid("不正なファイル名です。".into()));
    }
    Ok(format!("{INBOX_DIR}/{filename}"))
}

/// メモを `00-inbox/{stem}.md` に保存する。同じ名前があれば `{stem}-2.md`、`-3` … と連番を付ける
/// (同じ秒に2回保存しても、先のメモを上書きしない)。保存したファイル名を返す。
pub fn save_memo(root: &Path, stem: &str, markdown: &str) -> Result<String, StoreError> {
    for n in 1..=MAX_MEMO_SUFFIX {
        let name = if n == 1 { format!("{stem}.md") } else { format!("{stem}-{n}.md") };
        match fsx::write(root, &rel_of(&name)?, markdown.as_bytes(), Cond::CreateNew) {
            Ok(_) => return Ok(name),
            Err(StoreError::Exists(_)) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(StoreError::Io(format!("{stem}の名前が空きませんでした")))
}

/// frontmatter内の `review_status:` 行を書き換えた全文を返す(無ければ終端の直前に足す)。
/// frontmatter以外の本文には触れない。
pub fn set_review_status_text(text: &str, new_status: &str) -> Result<String, StoreError> {
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();

    if lines.first().map(|l| l.trim()) != Some("---") {
        return Err(StoreError::Invalid("frontmatterが見つかりませんでした。".into()));
    }
    let closing_idx = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, l)| l.trim() == "---")
        .map(|(i, _)| i)
        .ok_or_else(|| StoreError::Invalid("frontmatterの終端が見つかりませんでした。".into()))?;

    if let Some(status_idx) = (1..closing_idx).find(|&i| lines[i].starts_with("review_status:")) {
        lines[status_idx] = format!("review_status: {new_status}");
    } else {
        lines.insert(closing_idx, format!("review_status: {new_status}"));
    }

    let mut new_text = lines.join("\n");
    if text.ends_with('\n') {
        new_text.push('\n');
    }
    Ok(new_text)
}

/// inboxのファイルの `review_status` を書き換える(要確認UIの確定・保留)。
pub fn set_review_status(root: &Path, filename: &str, new_status: &str) -> Result<(), StoreError> {
    let rel = rel_of(filename)?;
    for attempt in 0..MAX_RETRY {
        let (bytes, rev) = fsx::read(root, &rel)?;
        let text = String::from_utf8(bytes).map_err(|_| StoreError::Invalid(format!("{rel}がUTF-8ではありません")))?;
        let new_text = set_review_status_text(&text, new_status)?;
        match fsx::write(root, &rel, new_text.as_bytes(), Cond::Overwrite { expect_rev: rev }) {
            Ok(_) => return Ok(()),
            Err(StoreError::Conflict { .. }) => {
                std::thread::sleep(std::time::Duration::from_millis(1 + (attempt as u64 % 8)));
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(StoreError::Io(format!("{rel}への書き込みが重なって、やり直しても書けませんでした")))
}

/// inboxのファイルを消す(要確認UIの却下)。
pub fn remove(root: &Path, filename: &str) -> Result<(), StoreError> {
    fsx::remove(root, &rel_of(filename)?, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str) -> Dir {
            let p = std::env::temp_dir().join(format!("library_store-inbox-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(p.join(INBOX_DIR)).unwrap();
            Dir(p)
        }
        fn put(&self, name: &str, text: &str) {
            std::fs::write(self.0.join(INBOX_DIR).join(name), text).unwrap();
        }
        fn get(&self, name: &str) -> String {
            std::fs::read_to_string(self.0.join(INBOX_DIR).join(name)).unwrap()
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // lib.rs(要確認UI)から移したテスト
    #[test]
    fn review_statusが無ければ足し本文は変えない() {
        let d = Dir::new("insert");
        d.put("a.md", "---\ntitle: テスト\nreason: タグが空でした\n---\n\n本文\n");
        set_review_status(&d.0, "a.md", "deferred").unwrap();
        let result = d.get("a.md");
        assert!(result.contains("review_status: deferred"));
        assert!(result.contains("本文"), "本文が保持されていない");
    }

    #[test]
    fn review_statusがあれば置き換える() {
        let d = Dir::new("replace");
        d.put("a.md", "---\ntitle: テスト\nreview_status: deferred\n---\n\n本文\n");
        set_review_status(&d.0, "a.md", "confirmed").unwrap();
        let result = d.get("a.md");
        assert!(result.contains("review_status: confirmed"));
        assert!(!result.contains("review_status: deferred"));
    }

    #[test]
    fn frontmatterが無いファイルは何も書かずに断る() {
        let d = Dir::new("nofm");
        d.put("a.md", "本文だけ\n");
        assert!(matches!(set_review_status(&d.0, "a.md", "confirmed"), Err(StoreError::Invalid(_))));
        assert_eq!(d.get("a.md"), "本文だけ\n");
    }

    #[test]
    fn ファイル名に区切りや親の指定があれば断る() {
        let d = Dir::new("name");
        for bad in ["", "..", "a/b.md", "a\\b.md", "../x.md"] {
            assert!(matches!(set_review_status(&d.0, bad, "confirmed"), Err(StoreError::Invalid(_))), "{bad:?}");
            assert!(matches!(remove(&d.0, bad), Err(StoreError::Invalid(_))), "{bad:?}");
        }
    }

    #[test]
    fn 却下はファイルを消す() {
        let d = Dir::new("remove");
        d.put("a.md", "x");
        remove(&d.0, "a.md").unwrap();
        assert!(!d.0.join(INBOX_DIR).join("a.md").exists());
        assert!(matches!(remove(&d.0, "a.md"), Err(StoreError::NotFound(_))));
    }

    #[test]
    fn メモを保存し同じ名前があれば連番を付けて上書きしない() {
        let d = Dir::new("memo");
        assert_eq!(save_memo(&d.0, "memo-100", "1つ目").unwrap(), "memo-100.md");
        assert_eq!(save_memo(&d.0, "memo-100", "2つ目").unwrap(), "memo-100-2.md");
        assert_eq!(save_memo(&d.0, "memo-100", "3つ目").unwrap(), "memo-100-3.md");
        assert_eq!(d.get("memo-100.md"), "1つ目");
        assert_eq!(d.get("memo-100-2.md"), "2つ目");
        assert_eq!(d.get("memo-100-3.md"), "3つ目");
    }

    #[test]
    fn inboxが無くてもメモを保存できる() {
        let d = Dir::new("memo-nodir");
        std::fs::remove_dir_all(d.0.join(INBOX_DIR)).unwrap();
        assert_eq!(save_memo(&d.0, "memo-1", "x").unwrap(), "memo-1.md");
    }
}
