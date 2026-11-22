//! 記事の読み書き。パスの検証、版(rev)の計算、expect_revによる衝突検知を持つ。
//!
//! 論理パスの決まり:
//! - ふつうのパスは library/ からの相対(例: `20-areas/x/memory.md`)
//! - `@tasks/` で始まるパスは tasks/ からの相対
//! 書き込めるのは、library の `00-inbox/` と、`@tasks/` 配下の `.md` だけ。

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

pub const MAX_CONTENT: usize = 1024 * 1024;

#[derive(Debug, PartialEq)]
pub enum StoreError {
    BadPath,
    Forbidden,
    NotFound,
    NotUtf8,
    TooLarge,
    /// expect_rev が現在の版と合わない。current は現在の版(ファイルが無ければNone)
    Conflict { current: Option<String> },
    Io(String),
}

pub struct Store {
    pub library: PathBuf,
    pub tasks: PathBuf,
    /// 読んで比べて書くまでを1つずつにする(同じ記事への同時の書き込みで、片方を必ず409にする)
    lock: Mutex<()>,
}

#[derive(Clone, Copy, PartialEq)]
enum Area {
    Library,
    Tasks,
}

/// 内容の版。SHA-256の先頭16桁
pub fn rev_of(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn split(path: &str) -> Result<(Area, PathBuf), StoreError> {
    if path.is_empty() || path.len() > 300 || path.contains('\\') || path.contains(':') || path.contains('\0') || path.contains("//") || path.ends_with('/') {
        return Err(StoreError::BadPath);
    }
    let (area, rest) = match path.strip_prefix("@tasks/") {
        Some(r) => (Area::Tasks, r),
        None => (Area::Library, path),
    };
    if rest.is_empty() || rest.starts_with('/') {
        return Err(StoreError::BadPath);
    }
    let rel = PathBuf::from(rest);
    for c in rel.components() {
        match c {
            Component::Normal(s) if !s.is_empty() => {}
            _ => return Err(StoreError::BadPath),
        }
    }
    Ok((area, rel))
}

/// 既にある一番近い親をたどって、基準のフォルダの外(シンボリックリンクなど)に出ないか確かめる
fn inside(base: &Path, target: &Path) -> bool {
    let Ok(base) = base.canonicalize() else { return false };
    let mut p = target.to_path_buf();
    loop {
        if p.exists() {
            return p.canonicalize().map(|c| c.starts_with(&base)).unwrap_or(false);
        }
        if !p.pop() {
            return false;
        }
    }
}

impl Store {
    pub fn new(library: PathBuf, tasks: PathBuf) -> Self {
        Store { library, tasks, lock: Mutex::new(()) }
    }

    fn base(&self, area: Area) -> &Path {
        match area {
            Area::Library => &self.library,
            Area::Tasks => &self.tasks,
        }
    }

    /// 記事を読む。(本文, rev)
    pub fn read(&self, path: &str) -> Result<(String, String), StoreError> {
        let (area, rel) = split(path)?;
        let full = self.base(area).join(&rel);
        if !inside(self.base(area), &full) {
            return Err(StoreError::BadPath);
        }
        let bytes = fs::read(&full).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => StoreError::NotFound,
            _ => StoreError::Io(e.to_string()),
        })?;
        let rev = rev_of(&bytes);
        let text = String::from_utf8(bytes).map_err(|_| StoreError::NotUtf8)?;
        Ok((text, rev))
    }

    /// 記事を書く。expect_rev は、読んだときの rev か、新しく作るときの "new"。返り値は新しい rev
    pub fn write(&self, path: &str, content: &str, expect_rev: &str) -> Result<String, StoreError> {
        if content.len() > MAX_CONTENT {
            return Err(StoreError::TooLarge);
        }
        let (area, rel) = split(path)?;
        let allowed = match area {
            Area::Tasks => true,
            Area::Library => rel.starts_with("00-inbox"),
        };
        let ext_ok = rel.extension().and_then(|e| e.to_str()) == Some("md");
        if !allowed || !ext_ok {
            return Err(StoreError::Forbidden);
        }
        let full = self.base(area).join(&rel);
        if !inside(self.base(area), &full) {
            return Err(StoreError::BadPath);
        }
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let current = match fs::read(&full) {
            Ok(b) => Some(rev_of(&b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(StoreError::Io(e.to_string())),
        };
        let matches = match (&current, expect_rev) {
            (None, "new") => true,
            (Some(c), r) if r != "new" => c == r,
            _ => false,
        };
        if !matches {
            return Err(StoreError::Conflict { current });
        }
        let dir = full.parent().ok_or(StoreError::BadPath)?;
        fs::create_dir_all(dir).map_err(|e| StoreError::Io(e.to_string()))?;
        if !inside(self.base(area), &full) {
            return Err(StoreError::BadPath);
        }
        // 同じフォルダの一時ファイルに書いて、置き換える(途中で止まっても中途半端な記事を残さない)
        let tmp = dir.join(format!(".tmp-{}-{}", std::process::id(), rev_of(content.as_bytes())));
        let result = (|| -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
            fs::rename(&tmp, &full)
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(&tmp);
            return Err(StoreError::Io(e.to_string()));
        }
        Ok(rev_of(content.as_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("library/00-inbox")).unwrap();
        fs::create_dir_all(d.path().join("library/20-areas")).unwrap();
        fs::create_dir_all(d.path().join("tasks")).unwrap();
        let s = Store::new(d.path().join("library"), d.path().join("tasks"));
        (d, s)
    }

    #[test]
    fn 新規作成と上書きのrev() {
        let (_d, s) = store();
        let r1 = s.write("00-inbox/a.md", "一つ目", "new").unwrap();
        assert_eq!(s.read("00-inbox/a.md").unwrap(), ("一つ目".to_string(), r1.clone()));
        let r2 = s.write("00-inbox/a.md", "二つ目", &r1).unwrap();
        assert_ne!(r1, r2);
    }

    #[test]
    fn 古いrevは衝突になる() {
        let (_d, s) = store();
        let r1 = s.write("00-inbox/a.md", "一つ目", "new").unwrap();
        s.write("00-inbox/a.md", "二つ目", &r1).unwrap();
        // 同じ r1 を持ったもう1人は、拒否される
        match s.write("00-inbox/a.md", "三つ目", &r1) {
            Err(StoreError::Conflict { current: Some(_) }) => {}
            other => panic!("{other:?}"),
        }
        // すでにあるのに new は衝突
        assert!(matches!(s.write("00-inbox/a.md", "x", "new"), Err(StoreError::Conflict { .. })));
        // 無いのに rev 指定は衝突
        assert!(matches!(s.write("00-inbox/zz.md", "x", "abcd"), Err(StoreError::Conflict { current: None })));
    }

    #[test]
    fn 書き込める場所の制限() {
        let (_d, s) = store();
        assert_eq!(s.write("20-areas/a.md", "x", "new"), Err(StoreError::Forbidden));
        assert_eq!(s.write("00-inbox/a.txt", "x", "new"), Err(StoreError::Forbidden));
        assert!(s.write("@tasks/t.md", "x", "new").is_ok());
    }

    #[test]
    fn 危ないパスの拒否() {
        let (_d, s) = store();
        for p in ["../x.md", "00-inbox/../../x.md", "/etc/x.md", "C:/x.md", "00-inbox\\a.md", "", "@tasks/", "00-inbox//a.md"] {
            assert_eq!(s.read(p), Err(StoreError::BadPath), "{p}");
            assert_eq!(s.write(p, "x", "new"), Err(StoreError::BadPath), "{p}");
        }
    }

    #[test]
    fn 同時の書き込みは片方だけ成功する() {
        let (_d, s) = store();
        let s = std::sync::Arc::new(s);
        let r0 = s.write("00-inbox/a.md", "元", "new").unwrap();
        let hs: Vec<_> = (0..8)
            .map(|i| {
                let s = s.clone();
                let r0 = r0.clone();
                std::thread::spawn(move || s.write("00-inbox/a.md", &format!("書き手{i}"), &r0).is_ok())
            })
            .collect();
        let ok = hs.into_iter().map(|h| h.join().unwrap()).filter(|x| *x).count();
        assert_eq!(ok, 1);
    }

    #[test]
    fn 別の記事は同時に両方成功する() {
        let (_d, s) = store();
        let s = std::sync::Arc::new(s);
        let hs: Vec<_> = (0..8)
            .map(|i| {
                let s = s.clone();
                std::thread::spawn(move || s.write(&format!("00-inbox/n{i}.md"), "x", "new").is_ok())
            })
            .collect();
        assert!(hs.into_iter().all(|h| h.join().unwrap()));
    }
}
