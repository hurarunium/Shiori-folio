//! 手順書・規約の資料(skillの本体、`_system/` の資料)の更新。memoryと同じ考え方で、
//! 「読む → 作業用のファイルで直す → **読んだ版(`rev`)のときだけ**保存する → 旧版が退避される」。
//!
//! 直接のファイル操作ではなく、サーバーの窓口(`PUT /api/v1/doc`)を通る。範囲は、次の3か所だけ。
//!
//! | 場所 | 新規 | 上書き |
//! |---|---|---|
//! | `00-inbox/`(受信箱) | 可 | 可(版つき) |
//! | `30-resources/skills/`(skillの本体・手順書・スクリプト) | 可 | 可(版つき・旧版を退避) |
//! | `_system/CLAUDE.md`・`_system/library-organizing/`(規約・一斉整理の資料) | **不可** | 可(版つき・旧版を退避) |
//!
//! `_system/projects.yaml`・`tags.yaml`・`memory-history/`・`doc-history/` などは、台帳・退避を管理する別の窓口が持つので、ここからは書けない。

use crate::fsx::{self, Cond, StoreError};
use crate::assets::secret_name_reason;
use std::path::Path;

const SKILLS_DIR: &str = "30-resources/skills/";
const SYSTEM_DOC: &str = "_system/CLAUDE.md";
const SYSTEM_DIR: &str = "_system/library-organizing/";
const INBOX_DIR: &str = "00-inbox/";
const HISTORY_DIR: &str = "_system/doc-history";
const HISTORY_GENERATIONS: usize = 3;
/// 手順書・スクリプトとして許可する拡張子(skills・_system)
const ALLOWED_EXT: &[&str] = &["md", "sh", "json", "txt", "yaml", "yml", "ps1", "py"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Inbox,
    Skills,
    System,
}

/// 窓口から書ける場所か。どれにも入らなければ `None`(書けない)。
pub fn zone_of(rel: &str) -> Option<Zone> {
    if rel.starts_with(INBOX_DIR) && rel.len() > INBOX_DIR.len() {
        Some(Zone::Inbox)
    } else if rel.starts_with(SKILLS_DIR) && rel.len() > SKILLS_DIR.len() {
        Some(Zone::Skills)
    } else if rel == SYSTEM_DOC || (rel.starts_with(SYSTEM_DIR) && rel.len() > SYSTEM_DIR.len()) {
        Some(Zone::System)
    } else {
        None
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct DocPut {
    pub rev: String,
    /// 旧版を退避した先(libraryからの相対)。新規・受信箱のときはNone。
    pub history_rel: Option<String>,
    pub created: bool,
}

fn check_name(rel: &str) -> Result<(), StoreError> {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    if !ALLOWED_EXT.contains(&ext.as_str()) {
        return Err(StoreError::Invalid(format!(
            "手順書・規約として置けるのは、{}だけです: {name}",
            ALLOWED_EXT.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(" ")
        )));
    }
    if let Some(reason) = secret_name_reason(name) {
        return Err(StoreError::Invalid(format!("秘密情報の可能性があるため置けません({reason}): {name}")));
    }
    Ok(())
}

/// `{path の / を __ にしたもの}@{日時}.{拡張子}`。同じ path の退避は、この前置きで見分ける。
fn history_prefix(rel: &str) -> String {
    format!("{}@", rel.replace('/', "__"))
}

fn prune_history(root: &Path, rel: &str) -> Result<(), StoreError> {
    let prefix = history_prefix(rel);
    let mut names: Vec<String> = fsx::list_names(root, HISTORY_DIR)?.into_iter().filter(|n| n.starts_with(&prefix)).collect();
    names.sort_unstable_by(|a, b| b.cmp(a));
    for old in names.iter().skip(HISTORY_GENERATIONS) {
        fsx::remove(root, &format!("{HISTORY_DIR}/{old}"), None)?;
    }
    Ok(())
}

/// 書く。`expect_rev` は、上書きのとき(読んだ版)。無ければ新規(あれば断る)。
/// `stamp` は、退避の名前に使う日時(`20261122T153000`)。
pub fn put(root: &Path, rel: &str, content: &[u8], expect_rev: Option<&str>, stamp: &str) -> Result<DocPut, StoreError> {
    let zone = zone_of(rel).ok_or_else(|| StoreError::Invalid(format!("ここには書けません: {rel}")))?;
    if zone != Zone::Inbox {
        check_name(rel)?;
    }
    if zone == Zone::System && expect_rev.is_none() {
        return Err(StoreError::Invalid("規約・一斉整理の資料は、新規には作れません(読んだ版を `expect_rev` に渡して、上書きしてください)".into()));
    }

    // 受信箱・新規: 退避なし
    let Some(expect) = expect_rev else {
        let w = fsx::write(root, rel, content, Cond::CreateNew)?;
        return Ok(DocPut { rev: w.rev, history_rel: None, created: true });
    };

    // 受信箱の上書き: 退避なし
    if zone == Zone::Inbox {
        let w = fsx::write(root, rel, content, Cond::Overwrite { expect_rev: expect.to_string() })?;
        return Ok(DocPut { rev: w.rev, history_rel: None, created: false });
    }

    // skills・_system の上書き: 読んだ版のときだけ。旧版を退避してから置き換える(退避に失敗したら上書きしない)
    let (old, old_rev) = fsx::read(root, rel)?;
    if old_rev != expect {
        return Err(StoreError::Conflict { current_rev: old_rev });
    }
    let ext = rel.rsplit_once('.').map(|(_, e)| format!(".{e}")).unwrap_or_default();
    let prefix = history_prefix(rel);
    let mut n = 1;
    let history_rel = loop {
        let name = if n == 1 { format!("{prefix}{stamp}{ext}") } else { format!("{prefix}{stamp}-{n}{ext}") };
        let h = format!("{HISTORY_DIR}/{name}");
        match fsx::write(root, &h, &old, Cond::CreateNew) {
            Ok(_) => break h,
            Err(StoreError::Exists(_)) => n += 1,
            Err(e) => return Err(StoreError::Io(format!("旧版の退避に失敗: {e}"))),
        }
    };
    match fsx::write(root, rel, content, Cond::Overwrite { expect_rev: expect.to_string() }) {
        Ok(w) => {
            let _ = prune_history(root, rel);
            Ok(DocPut { rev: w.rev, history_rel: Some(history_rel), created: false })
        }
        Err(e) => {
            // 書けなかったときは、いま作った退避を取り消す(負けた側が、余分な退避を残さない)
            let _ = fsx::remove(root, &history_rel, None);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsx::rev_of;
    use std::path::PathBuf;

    struct Lib(PathBuf);
    impl Lib {
        fn new(tag: &str) -> Lib {
            let p = std::env::temp_dir().join(format!("library_store-docs-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Lib(p)
        }
        fn put_raw(&self, rel: &str, text: &str) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
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

    #[test]
    fn 書ける場所は3か所だけ() {
        assert_eq!(zone_of("00-inbox/a.md"), Some(Zone::Inbox));
        assert_eq!(zone_of("30-resources/skills/work-start.md"), Some(Zone::Skills));
        assert_eq!(zone_of("_system/CLAUDE.md"), Some(Zone::System));
        assert_eq!(zone_of("_system/library-organizing/02-procedure.md"), Some(Zone::System));
        for no in [
            "_system/projects.yaml", "_system/tags.yaml", "_system/memory-history/a.md", "_system/doc-history/a.md",
            "30-resources/a.md", "20-projects/shiori/memory.md", "10-common/memory-global.md", "40-profile/a.md",
            "00-inbox/", "30-resources/skills/", "_system/library-organizing/", "_system/CLAUDE.md.bak",
        ] {
            assert_eq!(zone_of(no), None, "{no}");
        }
    }

    #[test]
    fn skillは新規に作れ読んだ版で上書きでき旧版が退避される() {
        let l = Lib::new("skill");
        let rel = "30-resources/skills/work-start.md";
        let first = put(&l.0, rel, "# v1\n".as_bytes(), None, "20261122T100000").unwrap();
        assert!(first.created && first.history_rel.is_none());
        // 同じパスへの新規は断る
        assert!(matches!(put(&l.0, rel, b"x", None, "s"), Err(StoreError::Exists(_))));
        let second = put(&l.0, rel, "# v2\n".as_bytes(), Some(&first.rev), "20261122T110000").unwrap();
        assert_eq!(second.history_rel.as_deref(), Some("_system/doc-history/30-resources__skills__work-start.md@20261122T110000.md"), "退避の名前の日時は、置き換えた時刻");
        assert_eq!(l.read(second.history_rel.as_deref().unwrap()), "# v1\n", "旧版がそのまま退避される");
        assert_eq!(l.read(rel), "# v2\n");
        assert_eq!(second.rev, rev_of("# v2\n".as_bytes()));
    }

    #[test]
    fn 古い版では断られ中身も退避も変わらない() {
        let l = Lib::new("conflict");
        let rel = "30-resources/skills/a.md";
        let first = put(&l.0, rel, b"v1", None, "s1").unwrap();
        let second = put(&l.0, rel, b"v2", Some(&first.rev), "s2").unwrap();
        let r = put(&l.0, rel, b"v3", Some(&first.rev), "s3");
        assert_eq!(r, Err(StoreError::Conflict { current_rev: second.rev.clone() }));
        assert_eq!(l.read(rel), "v2");
        assert_eq!(fsx::list_names(&l.0, "_system/doc-history").unwrap().len(), 1, "負けた側は退避を残さない");
    }

    #[test]
    fn 退避は3世代で止まる() {
        let l = Lib::new("gen");
        let rel = "30-resources/skills/a.md";
        let mut rev = put(&l.0, rel, b"v0", None, "s0").unwrap().rev;
        for i in 1..=6 {
            rev = put(&l.0, rel, format!("v{i}").as_bytes(), Some(&rev), &format!("2026112{i}T100000")).unwrap().rev;
        }
        let mut names = fsx::list_names(&l.0, "_system/doc-history").unwrap();
        names.sort();
        assert_eq!(names.len(), 3, "{names:?}");
        assert!(names[0].contains("20261124") && names[2].contains("20261126"), "新しい3世代が残る: {names:?}");
    }

    #[test]
    fn 同じ日時でも退避は上書きされず連番になる() {
        let l = Lib::new("same-stamp");
        let rel = "30-resources/skills/a.md";
        let a = put(&l.0, rel, b"v0", None, "s").unwrap();
        let b = put(&l.0, rel, b"v1", Some(&a.rev), "same").unwrap();
        let c = put(&l.0, rel, b"v2", Some(&b.rev), "same").unwrap();
        assert_ne!(b.history_rel, c.history_rel);
        assert_eq!(l.read(b.history_rel.as_deref().unwrap()), "v0");
        assert_eq!(l.read(c.history_rel.as_deref().unwrap()), "v1");
    }

    #[test]
    fn 規約は上書きだけで新規には作れない() {
        let l = Lib::new("system");
        // 無い規約の新規は断る
        assert!(matches!(put(&l.0, "_system/CLAUDE.md", b"x", None, "s"), Err(StoreError::Invalid(_))));
        assert!(matches!(put(&l.0, "_system/library-organizing/99.md", b"x", None, "s"), Err(StoreError::Invalid(_))));
        // あるものは、読んだ版で上書きでき、退避される
        l.put_raw("_system/CLAUDE.md", "旧規約\n");
        let r = put(&l.0, "_system/CLAUDE.md", "新規約\n".as_bytes(), Some(&rev_of("旧規約\n".as_bytes())), "20261122T120000").unwrap();
        assert!(r.history_rel.is_some() && !r.created);
        assert_eq!(l.read("_system/CLAUDE.md"), "新規約\n");
        // 無いファイルの上書きは、NotFound
        assert!(matches!(put(&l.0, "_system/library-organizing/x.md", b"x", Some("0000"), "s"), Err(StoreError::NotFound(_))));
    }

    #[test]
    fn 台帳や退避の置き場には書けない() {
        let l = Lib::new("forbidden");
        for rel in ["_system/projects.yaml", "_system/tags.yaml", "_system/memory-history/a.md", "20-projects/a/memory.md", "../x.md"] {
            assert!(put(&l.0, rel, b"x", None, "s").is_err(), "{rel}");
        }
        assert!(!l.0.join("_system").exists());
    }

    #[test]
    fn 拡張子と秘密情報の名前を検査する() {
        let l = Lib::new("ext");
        for bad in ["30-resources/skills/a.exe", "30-resources/skills/a", "30-resources/skills/server.pem", "30-resources/skills/id_rsa.md.key", "30-resources/skills/.env"] {
            assert!(matches!(put(&l.0, bad, b"x", None, "s"), Err(StoreError::Invalid(_))), "{bad}");
        }
        for ok in ["30-resources/skills/a.md", "30-resources/skills/cross-review.sh", "30-resources/skills/sub/check.py", "30-resources/skills/x.json"] {
            assert!(put(&l.0, ok, b"x", None, "s").is_ok(), "{ok}");
        }
    }

    #[test]
    fn 受信箱は拡張子を問わず新規と上書きができ退避しない() {
        let l = Lib::new("inbox");
        let a = put(&l.0, "00-inbox/memo.md", b"1", None, "s").unwrap();
        let b = put(&l.0, "00-inbox/memo.md", b"2", Some(&a.rev), "s").unwrap();
        assert!(b.history_rel.is_none() && !l.0.join("_system").exists());
        assert!(matches!(put(&l.0, "00-inbox/memo.md", b"3", Some(&a.rev), "s"), Err(StoreError::Conflict { .. })));
    }
}
