//! 素材(zip・png・pdf等)の取り込み(詩織Ver3.5)。
//!
//! 素材は、同名に`.meta`を付けたサイドカー(frontmatter+本文)とペアで置く。素材は`files/`へ取り込む
//! (コピー→SHA-256照合→一致したら完了)。mdと違い、不備があっても00-inbox/へ逃がさずエラーにする
//! (素材だけがinboxに置かれると、実体と説明の対応が崩れるため)。
//! 原本の削除は、呼び出し側が行う(コピーだけにしたいときがあるため)。

use crate::fsx::{self, Cond, StoreError};
use crate::notes::Frontmatter;
use crate::registry;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// 素材のサイドカーの拡張子。「X.zip」に対し「X.zip.meta」を置く。
pub const ASSET_META_EXT: &str = "meta";

/// 素材の実体名として拒否する、名前から明らかな秘密情報(中身は検査しない)。
/// libraryはMCP経由で外部AIにも読めるため、秘密鍵・環境変数ファイルを置かせない。
pub fn secret_name_reason(file_name: &str) -> Option<&'static str> {
    let lower = file_name.to_ascii_lowercase();
    if lower.starts_with("id_") && !lower.ends_with(".pub") {
        return Some("SSH秘密鍵(id_*)");
    }
    if lower.ends_with(".pem") || lower.ends_with(".ppk") || lower.ends_with(".key") {
        return Some("秘密鍵・証明書(.pem/.ppk/.key)");
    }
    if lower == ".env" || lower.starts_with(".env.") {
        return Some("環境変数ファイル(.env)");
    }
    if lower == "key.txt" {
        return Some("鍵ファイル(key.txt)");
    }
    None
}

/// 素材のtype/projectから、保存先の`files/`(libraryからの相対)を決める。
/// project/areaのprojectが未登録なら、`projects.yaml`へ`pending`で登録する。
pub fn decide_asset_destination(root: &Path, fm: &Frontmatter) -> Result<String, StoreError> {
    if fm.title.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err(StoreError::Invalid("titleが空です".into()));
    }
    if fm.tags.is_empty() {
        return Err(StoreError::Invalid("tagsが空です".into()));
    }
    let project = fm.project.as_deref().map(str::trim).filter(|p| !p.is_empty());
    match fm.kind.as_deref() {
        Some(t @ ("project" | "area")) => {
            let p = project.ok_or_else(|| {
                StoreError::Invalid("typeがproject/areaの素材にはprojectが必要です(保存先が決まりません)".into())
            })?;
            registry::ensure_project(root, p)?;
            let top = if t == "project" { "10-projects" } else { "20-areas" };
            Ok(format!("{top}/{p}/files"))
        }
        Some("resource") => Ok("30-resources/files".to_string()),
        Some("profile") => Err(StoreError::Invalid("type: profileには素材を置けません".into())),
        other => Err(StoreError::Invalid(format!(
            "typeが不正です(値: {other:?}, 素材で許可: project/area/resource)"
        ))),
    }
}

/// ファイルのSHA-256(16進の小文字)。大きなファイルも、1MBずつ読んで求める。
pub fn sha256_of(path: &Path) -> Result<String, StoreError> {
    let io = |what: &str, e: std::io::Error| StoreError::Io(format!("{what}: {} ({e})", path.display()));
    let mut file = std::fs::File::open(path).map_err(|e| io("開けません", e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| io("読み込みに失敗", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// 実体と`.meta`に付ける名前の組(連番つき)。衝突時は、実体と`.meta`に同じ連番を付ける
/// (X.zip → X-2.zip と X-2.zip.meta)。
pub fn asset_names(asset_name: &str, n: usize) -> (String, String) {
    let (stem, ext) = match asset_name.rfind('.') {
        Some(i) if i > 0 => (&asset_name[..i], &asset_name[i..]),
        _ => (asset_name, ""),
    };
    let name = if n <= 1 { asset_name.to_string() } else { format!("{stem}-{n}{ext}") };
    let meta = format!("{name}.{ASSET_META_EXT}");
    (name, meta)
}

#[derive(Debug, PartialEq, Eq)]
pub struct SavedAsset {
    pub asset_path: PathBuf,
    pub meta_path: PathBuf,
    pub asset_rel: String,
    pub meta_rel: String,
    pub sha256: String,
    pub size: u64,
    pub newly_pending_tags: Vec<String>,
}

/// 素材を取り込む: 秘密情報の名前の拒否 → タグの正規化 → 保存先の決定 → 実体のコピー(SHA-256で照合)
/// → `.meta`の書き込み。不一致や途中の失敗では、作った分を片付ける(原本には触れない)。
/// sha256・size・dateは、ここが決定的に書く(人・AIが書いた値は上書きする)。`now`は呼び出し側の現在時刻。
pub fn save_asset(
    root: &Path,
    asset_source: &Path,
    mut fm: Frontmatter,
    body: &str,
    now: &str,
) -> Result<SavedAsset, StoreError> {
    let asset_name = asset_source
        .file_name()
        .ok_or_else(|| StoreError::Invalid("素材のファイル名の取得に失敗".into()))?
        .to_string_lossy()
        .to_string();
    if let Some(reason) = secret_name_reason(&asset_name) {
        return Err(StoreError::Invalid(format!(
            "秘密情報の可能性があるため取り込めません({reason}): {asset_name}"
        )));
    }

    let mut newly_pending_tags = Vec::new();
    let mut tags = Vec::with_capacity(fm.tags.len());
    for raw in &fm.tags {
        tags.push(registry::normalize_tag(root, raw, &mut newly_pending_tags)?);
    }
    fm.tags = tags;

    let dest_dir = decide_asset_destination(root, &fm)?;

    let source_hash = sha256_of(asset_source)?;
    let size = std::fs::metadata(asset_source)
        .map_err(|e| StoreError::Io(format!("素材の大きさを取れません: {} ({e})", asset_source.display())))?
        .len();

    fm.date = Some(now.to_string());
    fm.extra.insert("sha256".to_string(), serde_yaml::Value::String(source_hash.clone()));
    fm.extra.insert("size".to_string(), serde_yaml::Value::Number(size.into()));
    let frontmatter_yaml = serde_yaml::to_string(&fm)
        .map_err(|e| StoreError::Invalid(format!("frontmatterのYAML化に失敗: {e}")))?;
    let meta_content = format!("---\n{frontmatter_yaml}---\n{body}");

    let mut n = 1;
    loop {
        let (name, meta_name) = asset_names(&asset_name, n);
        let asset_rel = format!("{dest_dir}/{name}");
        let meta_rel = format!("{dest_dir}/{meta_name}");
        let asset_path = fsx::resolve(root, &asset_rel)?;
        let meta_path = fsx::resolve(root, &meta_rel)?;

        // 実体が無くても、.metaだけ残っていれば衝突として扱う。
        if meta_path.exists() {
            n += 1;
            continue;
        }
        if let Some(dir) = asset_path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| StoreError::Io(format!("フォルダの作成に失敗: {} ({e})", dir.display())))?;
        }
        // 名前を「新規作成」で確保してから書く(同時に同じ名前を取り込んでも、互いに上書きしない)。
        let mut dest_file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&asset_path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                n += 1;
                continue;
            }
            Err(e) => {
                return Err(StoreError::Io(format!("素材のコピーに失敗: {} ({e})", asset_path.display())));
            }
        };
        let cleanup = |paths: &[&Path]| {
            for p in paths {
                let _ = std::fs::remove_file(p);
            }
        };

        // 移動は「コピー→SHA-256照合→一致したら完了」の順で行う。別ドライブ間でも安全に動かすためで、
        // 不一致や途中の失敗では、作った分を片付ける。
        let copied = std::fs::File::open(asset_source)
            .and_then(|mut src| std::io::copy(&mut src, &mut dest_file));
        drop(dest_file);
        if let Err(e) = copied {
            cleanup(&[&asset_path]);
            return Err(StoreError::Io(format!("素材のコピーに失敗: {} ({e})", asset_path.display())));
        }
        let dest_hash = match sha256_of(&asset_path) {
            Ok(h) => h,
            Err(e) => {
                cleanup(&[&asset_path]);
                return Err(e);
            }
        };
        if dest_hash != source_hash {
            cleanup(&[&asset_path]);
            return Err(StoreError::Io(format!(
                "コピー後のSHA-256が一致しません。原本は残しました: {}",
                asset_source.display()
            )));
        }

        match fsx::write(root, &meta_rel, meta_content.as_bytes(), Cond::CreateNew) {
            Ok(_) => {
                return Ok(SavedAsset {
                    asset_path,
                    meta_path,
                    asset_rel,
                    meta_rel,
                    sha256: source_hash,
                    size,
                    newly_pending_tags,
                });
            }
            // 確保したあとに、同じ名前の.metaが別に作られていた: 自分の実体を片付けて、次の番号へ
            Err(StoreError::Exists(_)) => {
                cleanup(&[&asset_path]);
                n += 1;
            }
            Err(e) => {
                cleanup(&[&asset_path, &meta_path]);
                return Err(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::split_frontmatter;

    struct Lib(PathBuf);
    impl Lib {
        fn new(tag: &str) -> Lib {
            let p = std::env::temp_dir().join(format!("library_store-assets-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(p.join("_system")).unwrap();
            std::fs::write(p.join(registry::PROJECTS_YAML), "# 一覧\nprojects:\n  - id: shiori\n    status: confirmed\n").unwrap();
            std::fs::write(
                p.join(registry::TAGS_YAML),
                "# タグ\ntags:\n  - canonical: 詩織\n    aliases: [shiori]\n    status: confirmed\n",
            )
            .unwrap();
            Lib(p)
        }
        /// 原本を作業用の場所に置く
        fn source(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let p = self.0.join("_work").join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes).unwrap();
            p
        }
    }
    impl Drop for Lib {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fm(yaml: &str) -> Frontmatter {
        split_frontmatter(&format!("---\n{yaml}\n---\n本文\n")).unwrap().0
    }

    const RES: &str = "title: 資料\ntype: resource\ntags: [shiori, 新タグ]";

    #[test]
    fn 秘密情報の名前は拒否し公開鍵は通す() {
        for name in ["id_ed25519", "id_rsa", "server.pem", "putty.PPK", "a.key", ".env", ".env.local", "key.txt"] {
            assert!(secret_name_reason(name).is_some(), "{name}は拒否されるべき");
        }
        for name in ["id_ed25519.pub", "report.pdf", "environment.zip", "monkey.txt", "pc-backup.zip"] {
            assert!(secret_name_reason(name).is_none(), "{name}は通すべき");
        }
    }

    #[test]
    fn 実体とmetaには同じ連番を付ける() {
        assert_eq!(asset_names("a.tar.gz", 1), ("a.tar.gz".to_string(), "a.tar.gz.meta".to_string()));
        assert_eq!(asset_names("a.tar.gz", 2), ("a.tar-2.gz".to_string(), "a.tar-2.gz.meta".to_string()));
        assert_eq!(asset_names("README", 3), ("README-3".to_string(), "README-3.meta".to_string()));
        assert_eq!(asset_names(".gitignore", 2), (".gitignore-2".to_string(), ".gitignore-2.meta".to_string()));
    }

    #[test]
    fn 素材を取り込みmetaにsha256とsizeを書く() {
        let l = Lib::new("save");
        let src = l.source("a.zip", b"PK-bytes");
        let saved = save_asset(&l.0, &src, fm(RES), "説明\n", "2026-11-22T10:00:00+09:00").unwrap();
        assert_eq!((saved.asset_rel.as_str(), saved.meta_rel.as_str()), ("30-resources/files/a.zip", "30-resources/files/a.zip.meta"));
        assert_eq!(std::fs::read(&saved.asset_path).unwrap(), b"PK-bytes");
        assert_eq!(saved.size, 8);
        assert_eq!(saved.sha256, crate::fsx::rev_of(b"PK-bytes"));
        let meta = std::fs::read_to_string(&saved.meta_path).unwrap();
        assert!(meta.contains(&format!("sha256: {}", saved.sha256)) && meta.contains("size: 8"), "{meta}");
        assert!(meta.contains("date: 2026-11-22T10:00:00+09:00"));
        assert!(meta.contains("- 詩織") && meta.ends_with("---\n説明\n"), "タグが正規化される: {meta}");
        assert_eq!(saved.newly_pending_tags, vec!["新タグ".to_string()]);
        assert!(src.exists(), "原本には触れない");
    }

    #[test]
    fn 人やaiが書いたsha256とsizeは上書きする() {
        let l = Lib::new("overwrite-meta");
        let src = l.source("a.zip", b"abc");
        let saved = save_asset(&l.0, &src, fm(&format!("{RES}\nsha256: でたらめ\nsize: 999")), "x\n", "n").unwrap();
        let meta = std::fs::read_to_string(&saved.meta_path).unwrap();
        assert!(!meta.contains("でたらめ") && !meta.contains("999") && meta.contains("size: 3"), "{meta}");
    }

    #[test]
    fn 同名の素材は実体とmetaに同じ連番を付けて上書きしない() {
        let l = Lib::new("dup");
        let a = save_asset(&l.0, &l.source("a.zip", b"1"), fm(RES), "x\n", "n").unwrap();
        let b = save_asset(&l.0, &l.source("a.zip", b"2"), fm(RES), "x\n", "n").unwrap();
        // 実体が無く.metaだけ残っている名前も、衝突として扱う
        std::fs::remove_file(&b.asset_path).unwrap();
        let c = save_asset(&l.0, &l.source("a.zip", b"3"), fm(RES), "x\n", "n").unwrap();
        assert_eq!(a.asset_rel, "30-resources/files/a.zip");
        assert_eq!(b.asset_rel, "30-resources/files/a-2.zip");
        assert_eq!(c.asset_rel, "30-resources/files/a-3.zip");
        assert_eq!(std::fs::read(&a.asset_path).unwrap(), b"1");
        assert!(c.meta_path.exists());
    }

    #[test]
    fn 秘密情報の名前や不備のある素材は何も書かずに断る() {
        let l = Lib::new("reject");
        let before = std::fs::read_to_string(l.0.join(registry::TAGS_YAML)).unwrap();
        assert!(save_asset(&l.0, &l.source("server.pem", b"x"), fm(RES), "x", "n").is_err());
        // 秘密情報の名前は、タグを台帳へ登録する前に断る
        assert_eq!(std::fs::read_to_string(l.0.join(registry::TAGS_YAML)).unwrap(), before);
        assert!(save_asset(&l.0, &l.source("a.zip", b"x"), fm("title: ' '\ntype: resource\ntags: [a]"), "x", "n").is_err());
        assert!(save_asset(&l.0, &l.source("a.zip", b"x"), fm("title: t\ntype: resource"), "x", "n").is_err());
        assert!(save_asset(&l.0, &l.source("a.zip", b"x"), fm("title: t\ntype: profile\ntags: [a]"), "x", "n").is_err());
        assert!(save_asset(&l.0, &l.source("a.zip", b"x"), fm("title: t\ntype: area\ntags: [a]"), "x", "n").is_err());
        assert!(save_asset(&l.0, &l.source("a.zip", b"x"), fm("title: t\ntype: でたらめ\ntags: [a]"), "x", "n").is_err());
        assert!(!l.0.join("30-resources").exists(), "何も作られない");
    }

    #[test]
    fn projectやareaの素材はprojectの下に置きprojectを台帳へ登録する() {
        let l = Lib::new("project");
        let saved = save_asset(&l.0, &l.source("a.png", b"png"), fm("title: t\ntype: area\nproject: 新作\ntags: [詩織]"), "x\n", "n").unwrap();
        assert_eq!(saved.asset_rel, "20-areas/新作/files/a.png");
        let yaml = std::fs::read_to_string(l.0.join(registry::PROJECTS_YAML)).unwrap();
        assert!(yaml.ends_with("  - id: 新作\n    status: pending\n"), "{yaml}");
        let escape = save_asset(&l.0, &l.source("b.png", b"png"), fm("title: t\ntype: project\nproject: '../x'\ntags: [詩織]"), "x\n", "n");
        assert!(matches!(escape, Err(StoreError::Invalid(_))), "{escape:?}");
    }

    #[test]
    fn 同時に同じ名前の素材を取り込んでも互いに上書きしない() {
        let l = Lib::new("race");
        let root = std::sync::Arc::new(l.0.clone());
        let sources: Vec<PathBuf> = (0..6).map(|i| l.source(&format!("s{i}/a.zip"), format!("中身{i}").as_bytes())).collect();
        let handles: Vec<_> = sources
            .into_iter()
            .map(|src| {
                let root = root.clone();
                std::thread::spawn(move || save_asset(&root, &src, fm(RES), "x\n", "n"))
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let mut names: Vec<String> = results.iter().map(|r| r.as_ref().unwrap().asset_rel.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 6, "名前が重ならない: {names:?}");
        let mut contents: Vec<Vec<u8>> = results.iter().map(|r| std::fs::read(&r.as_ref().unwrap().asset_path).unwrap()).collect();
        contents.sort();
        contents.dedup();
        assert_eq!(contents.len(), 6, "6つとも別の中身のまま残る");
    }
}
