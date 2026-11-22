//! 書き込みの口。すべての書き込みはここを通す。
//! - 置き場(root)を引数で受け、相対パスがrootの外へ出ないことを検査する。
//! - 一時ファイル→置き換えで書く(途中で止まっても本体は壊れない)。
//! - 版(rev)は中身のSHA-256。上書きは、読んだ版と今の版が違えば断る。
//! - 書き込みは、このプロセスの中で1本の列に並べる。

use sha2::{Digest, Sha256};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// 書き込みの失敗。サーバーはこれをそのまま状態コードに対応させる
/// (Conflict→409、NotFound→404、Exists→409、Invalid→422、Io→500)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// 読んだ版と今の版が違う。今の版を持つ。
    Conflict { current_rev: String },
    /// 古い版を元にした更新(memoryのupdatedの不一致など)。メッセージを持つ。
    Stale(String),
    NotFound(String),
    /// 新規のつもりが、すでにある。
    Exists(String),
    Invalid(String),
    Io(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Conflict { current_rev } => {
                write!(f, "版が一致しません(今の版: {current_rev})")
            }
            StoreError::Stale(m) => write!(f, "{m}"),
            StoreError::NotFound(p) => write!(f, "見つかりません: {p}"),
            StoreError::Exists(p) => write!(f, "すでにあります: {p}"),
            StoreError::Invalid(m) => write!(f, "{m}"),
            StoreError::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for StoreError {}

fn io_err(what: &str, path: &Path, e: std::io::Error) -> StoreError {
    StoreError::Io(format!("{what}に失敗: {} ({e})", path.display()))
}

/// 書き込みの条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cond {
    /// 無ければ作る。あれば断る(Exists)。
    CreateNew,
    /// あるものを、読んだ版のときだけ上書きする。無ければNotFound、版が違えばConflict。
    Overwrite { expect_rev: String },
    /// 条件なし(あれば置き換え、無ければ作る)。移行・内部の整理用。
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub path: PathBuf,
    /// 書いたあとの版。
    pub rev: String,
}

/// 版 = 中身のSHA-256(16進の小文字)。
pub fn rev_of(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// rootからの相対パス(区切りは`/`)を、rootの中の実パスにする。
/// 空・絶対パス・`..`・区切りの`\`・ドライブ指定・NUL・macOSの`._`ファイルは断る。
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf, StoreError> {
    if rel.is_empty() {
        return Err(StoreError::Invalid("パスが空です".into()));
    }
    if rel.contains('\\') || rel.contains('\0') || rel.contains(':') {
        return Err(StoreError::Invalid(format!("使えない文字があります: {rel}")));
    }
    let mut out = root.to_path_buf();
    let mut count = 0;
    for comp in Path::new(rel).components() {
        match comp {
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                if name.starts_with("._") {
                    return Err(StoreError::Invalid(format!("macOSの付随ファイルは扱えません: {rel}")));
                }
                out.push(&*name);
                count += 1;
            }
            _ => return Err(StoreError::Invalid(format!("rootの外や絶対パスは指定できません: {rel}"))),
        }
    }
    if count == 0 {
        return Err(StoreError::Invalid("パスが空です".into()));
    }
    Ok(out)
}

static WRITE_LOCK: Mutex<()> = Mutex::new(());
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 一時ファイル→置き換えで書く(条件の確認はしない。内部用)。
pub fn write_atomically(dest: &Path, content: &[u8]) -> Result<(), StoreError> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io_err("フォルダの作成", dir, e))?;
    }
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let seq = TMP_SEQ.fetch_add(1, Ordering::SeqCst);
    let tmp = dest.with_file_name(format!(".{name}.{}.{seq}.tmp", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, content) {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_err("一時ファイルの書き込み", &tmp, e));
    }
    if let Err(e) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_err("置き換え", dest, e));
    }
    Ok(())
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    // 前の書き込みが途中で落ちても、続けて使えるようにする
    WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// 条件を確かめて書く。書き込みはこのプロセスの中で1本の列に並ぶ。
pub fn write(root: &Path, rel: &str, content: &[u8], cond: Cond) -> Result<Written, StoreError> {
    let dest = resolve(root, rel)?;
    let _guard = lock();
    match &cond {
        Cond::CreateNew => {
            if dest.exists() {
                return Err(StoreError::Exists(rel.to_string()));
            }
        }
        Cond::Overwrite { expect_rev } => {
            let current = match std::fs::read(&dest) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(StoreError::NotFound(rel.to_string()))
                }
                Err(e) => return Err(io_err("読み込み", &dest, e)),
            };
            let current_rev = rev_of(&current);
            if &current_rev != expect_rev {
                return Err(StoreError::Conflict { current_rev });
            }
        }
        Cond::Any => {}
    }
    write_atomically(&dest, content)?;
    Ok(Written { path: dest, rev: rev_of(content) })
}

/// 読む。本文と、その版を返す。
pub fn read(root: &Path, rel: &str) -> Result<(Vec<u8>, String), StoreError> {
    let path = resolve(root, rel)?;
    match std::fs::read(&path) {
        Ok(b) => {
            let rev = rev_of(&b);
            Ok((b, rev))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(StoreError::NotFound(rel.to_string())),
        Err(e) => Err(io_err("読み込み", &path, e)),
    }
}

/// フォルダの中のファイル名の一覧(順不同)。フォルダが無ければ空。
pub fn list_names(root: &Path, rel_dir: &str) -> Result<Vec<String>, StoreError> {
    let dir = resolve(root, rel_dir)?;
    match std::fs::read_dir(&dir) {
        Ok(rd) => Ok(rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(io_err("一覧の取得", &dir, e)),
    }
}

/// フォルダの中のファイルの一覧の1件。
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    /// 最終更新(UNIX時刻の秒)
    pub modified: i64,
}

/// フォルダの中の**ファイル**の一覧(名前の降順。日付の名前なら、新しい順)。
/// 隠しファイル(`.`始まり、macOSの`._`)・一時ファイル(`.tmp`)・サブフォルダは含めない。フォルダが無ければ空。
pub fn list_entries(root: &Path, rel_dir: &str) -> Result<Vec<Entry>, StoreError> {
    let dir = resolve(root, rel_dir)?;
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err("一覧の取得", &dir, e)),
    };
    let mut out = Vec::new();
    for e in rd.filter_map(|e| e.ok()) {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.ends_with(".tmp") {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.push(Entry { name, size: meta.len(), modified });
    }
    out.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(out)
}

/// 消す。読んだ版を渡したときは、版が一致するときだけ消す。
pub fn remove(root: &Path, rel: &str, expect_rev: Option<&str>) -> Result<(), StoreError> {
    let path = resolve(root, rel)?;
    let _guard = lock();
    if let Some(expect) = expect_rev {
        let (_, current_rev) = read(root, rel)?;
        if current_rev != expect {
            return Err(StoreError::Conflict { current_rev });
        }
    }
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(StoreError::NotFound(rel.to_string())),
        Err(e) => Err(io_err("削除", &path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str) -> Dir {
            let p = std::env::temp_dir().join(format!("library_store-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn revは中身のsha256() {
        // 空文字列のSHA-256
        assert_eq!(rev_of(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_ne!(rev_of(b"a"), rev_of(b"b"));
    }

    #[test]
    fn パスはrootの外へ出られない() {
        let root = Path::new("/lib");
        for bad in ["", "..", "../x.md", "a/../../x.md", "/etc/passwd", "a\\b.md", "C:/x.md", "a/._b.md", "._x.md", "a\0b"] {
            assert!(matches!(resolve(root, bad), Err(StoreError::Invalid(_))), "通ってしまった: {bad:?}");
        }
        assert_eq!(resolve(root, "00-inbox/a.md").unwrap(), Path::new("/lib").join("00-inbox").join("a.md"));
    }

    #[test]
    fn 新規は無ければ作りあれば断る() {
        let d = Dir::new("create");
        let w = write(&d.0, "00-inbox/a.md", "あ".as_bytes(), Cond::CreateNew).unwrap();
        assert_eq!(w.rev, rev_of("あ".as_bytes()));
        assert_eq!(std::fs::read_to_string(&w.path).unwrap(), "あ");
        assert_eq!(
            write(&d.0, "00-inbox/a.md", b"x", Cond::CreateNew),
            Err(StoreError::Exists("00-inbox/a.md".into()))
        );
        assert_eq!(std::fs::read_to_string(&w.path).unwrap(), "あ");
    }

    #[test]
    fn 上書きは読んだ版のときだけ通る() {
        let d = Dir::new("overwrite");
        let first = write(&d.0, "a.md", b"1", Cond::CreateNew).unwrap();
        let second = write(&d.0, "a.md", b"2", Cond::Overwrite { expect_rev: first.rev.clone() }).unwrap();
        assert_eq!(second.rev, rev_of(b"2"));
        // 古い版では断られ、今の版が返る。中身は変わらない
        assert_eq!(
            write(&d.0, "a.md", b"3", Cond::Overwrite { expect_rev: first.rev }),
            Err(StoreError::Conflict { current_rev: second.rev.clone() })
        );
        assert_eq!(std::fs::read(&second.path).unwrap(), b"2");
        // 無いものの上書きはNotFound
        assert_eq!(
            write(&d.0, "none.md", b"x", Cond::Overwrite { expect_rev: second.rev }),
            Err(StoreError::NotFound("none.md".into()))
        );
    }

    #[test]
    fn 条件なしは作りも置き換えもする() {
        let d = Dir::new("any");
        write(&d.0, "x/y.md", b"1", Cond::Any).unwrap();
        write(&d.0, "x/y.md", b"2", Cond::Any).unwrap();
        assert_eq!(read(&d.0, "x/y.md").unwrap().0, b"2");
    }

    #[test]
    fn 書いたあとに一時ファイルが残らない() {
        let d = Dir::new("tmp");
        write(&d.0, "a.md", b"1", Cond::Any).unwrap();
        write(&d.0, "a.md", b"2", Cond::Any).unwrap();
        let names: Vec<String> = std::fs::read_dir(&d.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.md".to_string()]);
    }

    #[test]
    fn 読むと本文と版が返る() {
        let d = Dir::new("read");
        write(&d.0, "a.md", b"hello", Cond::Any).unwrap();
        let (body, rev) = read(&d.0, "a.md").unwrap();
        assert_eq!(body, b"hello");
        assert_eq!(rev, rev_of(b"hello"));
        assert_eq!(read(&d.0, "b.md").unwrap_err(), StoreError::NotFound("b.md".into()));
    }

    #[test]
    fn 削除は版が違えば断る() {
        let d = Dir::new("remove");
        let w = write(&d.0, "a.md", b"1", Cond::Any).unwrap();
        assert_eq!(
            remove(&d.0, "a.md", Some("0000")),
            Err(StoreError::Conflict { current_rev: w.rev.clone() })
        );
        assert!(w.path.exists());
        remove(&d.0, "a.md", Some(&w.rev)).unwrap();
        assert!(!w.path.exists());
        assert_eq!(remove(&d.0, "a.md", None), Err(StoreError::NotFound("a.md".into())));
    }

    #[test]
    fn 同じ記事への同時書き込みは片方だけ通る() {
        let d = Dir::new("race");
        // 書く中身("0"〜"7")と重ならない初期値にする(中身が同じなら版も同じで、正当に通ってしまうため)
        let first = write(&d.0, "a.md", b"init", Cond::CreateNew).unwrap();
        let root = std::sync::Arc::new(d.0.clone());
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let root = root.clone();
                let rev = first.rev.clone();
                std::thread::spawn(move || {
                    write(&root, "a.md", format!("{i}").as_bytes(), Cond::Overwrite { expect_rev: rev })
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|r| matches!(r, Err(StoreError::Conflict { .. }))).count(), 7);
    }

    #[test]
    fn 一覧は名前の降順で隠しファイルとフォルダを含めない() {
        let d = Dir::new("list");
        std::fs::create_dir_all(d.0.join("j")).unwrap();
        // 書き込みの口は `._` を断るので、隠しファイルは、直接作る
        for n in ["2026-11-20.md", "2026-11-22.md", "2026-11-21.md", ".hidden.md", "._a.md", "x.md.tmp"] {
            std::fs::write(d.0.join("j").join(n), b"x").unwrap();
        }
        std::fs::create_dir_all(d.0.join("j/sub")).unwrap();
        let names: Vec<String> = list_entries(&d.0, "j").unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["2026-11-22.md", "2026-11-21.md", "2026-11-20.md"]);
        assert_eq!(list_entries(&d.0, "j").unwrap()[0].size, 1);
        assert!(list_entries(&d.0, "無い").unwrap().is_empty(), "フォルダが無ければ空");
        assert!(matches!(list_entries(&d.0, "../x"), Err(StoreError::Invalid(_))));
    }

}
