//! 知識の棚タブ(Ver5.0候補)向け。`knowledge/`直下の記事を、読み取り専用で一覧・取得する。
//!
//! 一覧と本文はRAGを経由せず、ファイルを直接読む(RAGが止まっていても棚は読める)。
//! 検索だけはRAG(`/search_shelf`)を使う。画面から任意のパスを読めてしまわないよう、
//! 読めるのは**一覧に出るファイルだけ**。呼び出し側が渡すのはファイル名だけで、
//! 一覧の中から一致するものを探す(`../`などを渡しても一致せず、読めない)。

use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

// 1記事の読み取り上限。棚の記事は短い想定なので、超えるものは想定外として読まない。
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShelfDocDto {
    pub name: String,
    pub title: String,
    pub topic: String,
    pub learned: String,
    pub summary: String,
    pub source: String,
    pub mtime: f64,
}

// frontmatter(先頭の---〜---)から`key: value`の1行を取り出す。値の前後のクォートは外す。
fn frontmatter_value(text: &str, key: &str) -> String {
    let Some(rest) = text.strip_prefix("---") else {
        return String::new();
    };
    let rest = rest.trim_start_matches(['\r', '\n']);
    let prefix = format!("{key}:");
    for line in rest.lines() {
        if line.trim_end() == "---" {
            break;
        }
        if let Some(value) = line.strip_prefix(&prefix) {
            let value = value.trim();
            let unquoted = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            return unquoted.to_string();
        }
    }
    String::new()
}

fn is_shelf_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or(".");
    path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") && !name.starts_with('.')
}

// 棚の記事を、学んだ日の新しい順(同じ日は名前順)で返す。フォルダが無ければ空。
pub fn list(shelf: &Path) -> Vec<ShelfDocDto> {
    let Ok(read) = std::fs::read_dir(shelf) else {
        return Vec::new();
    };
    let mut docs: Vec<ShelfDocDto> = read
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| is_shelf_file(p))
        .filter_map(|p| {
            let meta = std::fs::metadata(&p).ok()?;
            if meta.len() > MAX_BYTES {
                return None;
            }
            let text = std::fs::read_to_string(&p).ok()?;
            let name = p.file_name()?.to_string_lossy().to_string();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            let title = frontmatter_value(&text, "title");
            Some(ShelfDocDto {
                title: if title.is_empty() { name.clone() } else { title },
                name,
                topic: frontmatter_value(&text, "topic"),
                learned: frontmatter_value(&text, "learned"),
                summary: frontmatter_value(&text, "summary"),
                source: frontmatter_value(&text, "source"),
                mtime,
            })
        })
        .collect();
    docs.sort_by(|a, b| b.learned.cmp(&a.learned).then_with(|| a.name.cmp(&b.name)));
    docs
}

// 一覧に出るファイル名だけ読める。名前からパスを作らず、一覧の中から探す。
pub fn read(shelf: &Path, name: &str) -> Result<String, String> {
    let docs = list(shelf);
    let doc = docs
        .iter()
        .find(|d| d.name == name)
        .ok_or_else(|| format!("棚に見つかりません: {name}"))?;
    std::fs::read_to_string(shelf.join(&doc.name)).map_err(|e| format!("読み込みに失敗: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "---\ntitle: '超実数: 無限小'\ntopic: 数学\nlearned: '2026-10-17'\nsource: \"AIとの対話\"\n---\n\n本文\n";

    fn temp_shelf(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("shiori-shelf-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn frontmatter_values_are_unquoted() {
        assert_eq!(frontmatter_value(NOTE, "title"), "超実数: 無限小");
        assert_eq!(frontmatter_value(NOTE, "learned"), "2026-10-17");
        assert_eq!(frontmatter_value(NOTE, "source"), "AIとの対話");
        assert_eq!(frontmatter_value(NOTE, "missing"), "");
        // 本文側の行は拾わない。
        assert_eq!(frontmatter_value("---\na: 1\n---\ntopic: x\n", "topic"), "");
    }

    #[test]
    fn list_skips_hidden_and_non_md_and_sorts_newest_first() {
        let dir = temp_shelf("list");
        std::fs::write(dir.join("a.md"), NOTE).unwrap();
        std::fs::write(dir.join("b.md"), NOTE.replace("2026-10-17", "2026-10-20")).unwrap();
        std::fs::write(dir.join("._a.md"), "binary").unwrap();
        std::fs::write(dir.join("memo.txt"), "x").unwrap();
        let names: Vec<_> = list(&dir).into_iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["b.md", "a.md"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_only_allows_listed_files() {
        let dir = temp_shelf("read");
        std::fs::write(dir.join("a.md"), NOTE).unwrap();
        assert!(read(&dir, "a.md").unwrap().contains("本文"));
        assert!(read(&dir, "../a.md").is_err());
        assert!(read(&dir, "missing.md").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_shelf_is_empty() {
        assert!(list(Path::new("/nonexistent-shelf-dir")).is_empty());
    }
}
