//! 図書館の記事(RAGのメタデータ"source")から、実ファイルを探す。
//!
//! sourceは通常、ファイル名だけ(library全体で一意という前提、services/rag/indexing.py)。
//! ただし共通記憶MD(kind: memory、詩織Ver4.1)は、どのプロジェクトも同じ`memory.md`という
//! 名前になるため、sourceを`{project}/memory.md`にしている(`source_name_for`)。この場合は、
//! source_category(`20-areas`など)からの相対パスそのものとして扱う。services/rag/app.pyの
//! `_resolve_source_path`と同じ考え方。

use std::path::{Component, Path, PathBuf};

// dir配下を再帰的に探索し、ファイル名が一致する最初のファイルを返す(詩織Ver3.1、journal廃止・
// project/area配下への階層深化に伴い追加)。project/areaの記事はsource_category直下からさらに
// project名/kindの2階層深くなるため、直接のjoinでは見つけられない。
fn find_file_by_name(dir: &Path, file_name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file_by_name(&path, file_name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(file_name) {
            return Some(path);
        }
    }
    None
}

/// search_root(library/{source_category}/)から、sourceに対応する実ファイルを返す。見つからなければNone。
/// sourceが「/」を含むときは相対パスとして扱い、`..`・絶対パス・ドライブ指定は受け付けない。
pub fn resolve_source_file(search_root: &Path, source: &str) -> Option<PathBuf> {
    if source.contains('/') || source.contains('\\') {
        let relative = Path::new(source);
        if relative.components().any(|c| !matches!(c, Component::Normal(_))) {
            return None;
        }
        let path = search_root.join(relative);
        return path.is_file().then_some(path);
    }
    find_file_by_name(search_root, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("shiori-source-path-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let area = base.join("20-areas");
        for (dir, file, text) in [
            ("shiori", "memory.md", "詩織のmemory"),
            ("tanker", "memory.md", "tankerのmemory"),
            ("shiori/journal", "a.md", "日記"),
        ] {
            std::fs::create_dir_all(area.join(dir)).unwrap();
            std::fs::write(area.join(dir).join(file), text).unwrap();
        }
        std::fs::write(base.join("secret.md"), "秘密").unwrap();
        base
    }

    #[test]
    fn ファイル名だけのsourceは階層の下からでも見つかる() {
        let base = fixture("name");
        let found = resolve_source_file(&base.join("20-areas"), "a.md").unwrap();
        assert_eq!(std::fs::read_to_string(found).unwrap(), "日記");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 同名のmemoryはproject付きのsourceでそれぞれ別のファイルに解決される() {
        let base = fixture("memory");
        let root = base.join("20-areas");
        let shiori = resolve_source_file(&root, "shiori/memory.md").unwrap();
        let tanker = resolve_source_file(&root, "tanker/memory.md").unwrap();
        assert_eq!(std::fs::read_to_string(shiori).unwrap(), "詩織のmemory");
        assert_eq!(std::fs::read_to_string(tanker).unwrap(), "tankerのmemory");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn 相対パスの外へ出る指定と存在しないsourceは見つからない() {
        let base = fixture("bad");
        let root = base.join("20-areas");
        for bad in [
            "../secret.md",
            "shiori/../tanker/memory.md",
            "shiori/../../secret.md",
            "/etc/passwd",
            "C:/secret.md",
            "shiori/missing.md",
            "missing/memory.md",
            "nothing.md",
        ] {
            assert!(resolve_source_file(&root, bad).is_none(), "{bad}は見つかってはいけない");
        }
        std::fs::remove_dir_all(&base).unwrap();
    }
}
