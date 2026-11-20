//! 世代のバックアップ(試作): library と tasks を1つのzipにまとめ、manifest.json(ファイルごとの
//! 大きさとSHA-256、作った日時、版)を入れる。作ったあとに開き直して照らす。復元と照合もここ。
//! zipは、WindowsでもMacでも、そのまま開ける(通常のdeflate)。

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MANIFEST: &str = "manifest.json";

#[derive(Serialize, Deserialize, Clone)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub created: String,
    pub files: Vec<Entry>,
}

/// 世代を作った結果
pub struct Generation {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub files: usize,
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// root の下のファイルを全部、(論理パス, 実際のパス) で集める。論理パスは prefix/相対パス(区切りは /)
fn collect(root: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
        let rd = fs::read_dir(dir).map_err(|e| format!("読めない {}: {e}", dir.display()))?;
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            // Macがexfatに作る `._` のファイルは、中身ではない
            if name.starts_with("._") {
                continue;
            }
            let child = format!("{rel}/{name}");
            let p = ent.path();
            let ft = ent.file_type().map_err(|e| e.to_string())?;
            if ft.is_dir() {
                walk(&p, &child, out)?;
            } else if ft.is_file() {
                out.push((child, p));
            }
        }
        Ok(())
    }
    if root.is_dir() {
        walk(root, prefix, out)?;
    }
    Ok(())
}

/// 世代を1つ作る。sources は (zipの中の先頭の名前, 元のフォルダ)
pub fn create(sources: &[(&str, &Path)], out_dir: &Path) -> Result<Generation, String> {
    fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut name = format!("gen-{stamp}.zip");
    let mut n = 1;
    while out_dir.join(&name).exists() {
        name = format!("gen-{stamp}-{n}.zip");
        n += 1;
    }
    let tmp = out_dir.join(format!("{name}.tmp"));

    let mut files = Vec::new();
    for (prefix, root) in sources {
        collect(root, prefix, &mut files)?;
    }
    files.sort();

    let mut entries = Vec::new();
    {
        let f = File::create(&tmp).map_err(|e| e.to_string())?;
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (logical, real) in &files {
            let bytes = fs::read(real).map_err(|e| format!("読めない {}: {e}", real.display()))?;
            zw.start_file(logical.as_str(), opts).map_err(|e| e.to_string())?;
            zw.write_all(&bytes).map_err(|e| e.to_string())?;
            entries.push(Entry { path: logical.clone(), size: bytes.len() as u64, sha256: hex_sha256(&bytes) });
        }
        let manifest = Manifest { version: 1, created: chrono::Local::now().to_rfc3339(), files: entries.clone() };
        zw.start_file(MANIFEST, opts).map_err(|e| e.to_string())?;
        zw.write_all(serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?.as_bytes())
            .map_err(|e| e.to_string())?;
        zw.finish().map_err(|e| e.to_string())?;
    }

    // 作ったあとに、開き直して照らす。合わなければ捨てる
    if let Err(e) = verify_zip(&tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("作った世代を照らしたら合わなかった: {e}"));
    }
    let bytes = fs::read(&tmp).map_err(|e| e.to_string())?;
    let sha = hex_sha256(&bytes);
    let dest = out_dir.join(&name);
    fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    fs::write(out_dir.join(format!("{name}.sha256")), &sha).map_err(|e| e.to_string())?;
    Ok(Generation { name, size: bytes.len() as u64, sha256: sha, files: entries.len() })
}

fn read_manifest<R: Read + std::io::Seek>(ar: &mut zip::ZipArchive<R>) -> Result<Manifest, String> {
    let mut s = String::new();
    ar.by_name(MANIFEST).map_err(|_| "manifest.json が無い".to_string())?.read_to_string(&mut s).map_err(|e| e.to_string())?;
    serde_json::from_str(&s).map_err(|e| format!("manifest.json が読めない: {e}"))
}

/// zipを開き直して、中身がmanifestと(大きさ・SHA-256・ファイルの集まりとして)一致するか調べる。ファイル数を返す
pub fn verify_zip(zip_path: &Path) -> Result<usize, String> {
    let f = File::open(zip_path).map_err(|e| e.to_string())?;
    let mut ar = zip::ZipArchive::new(f).map_err(|e| format!("zipを開けない: {e}"))?;
    let manifest = read_manifest(&mut ar)?;
    let want: BTreeMap<&str, &Entry> = manifest.files.iter().map(|e| (e.path.as_str(), e)).collect();
    let mut seen = 0;
    for i in 0..ar.len() {
        let mut ent = ar.by_index(i).map_err(|e| e.to_string())?;
        let name = ent.name().map(|c| c.to_string()).map_err(|e| format!("ファイル名を読めない: {e}"))?;
        if ent.is_dir() || name == MANIFEST {
            continue;
        }
        let mut buf = Vec::new();
        ent.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        let Some(w) = want.get(name.as_str()) else { return Err(format!("manifestに無いファイルがある: {name}")) };
        if w.size != buf.len() as u64 || w.sha256 != hex_sha256(&buf) {
            return Err(format!("中身がmanifestと違う: {name}"));
        }
        seen += 1;
    }
    if seen != manifest.files.len() {
        return Err(format!("manifestにあるのにzipに無いファイルがある({} 件)", manifest.files.len() - seen));
    }
    Ok(seen)
}

/// zipを行き先に展開し、manifestと照らす。行き先は、無いか、空であること。ファイル数を返す
pub fn restore(zip_path: &Path, dest: &Path) -> Result<usize, String> {
    if dest.exists() && fs::read_dir(dest).map_err(|e| e.to_string())?.next().is_some() {
        return Err(format!("行き先が空ではない: {}", dest.display()));
    }
    let f = File::open(zip_path).map_err(|e| e.to_string())?;
    let mut ar = zip::ZipArchive::new(f).map_err(|e| format!("zipを開けない: {e}"))?;
    let manifest = read_manifest(&mut ar)?;
    for i in 0..ar.len() {
        let mut ent = ar.by_index(i).map_err(|e| e.to_string())?;
        let name = ent.name().map(|c| c.to_string()).map_err(|e| format!("ファイル名を読めない: {e}"))?;
        if ent.is_dir() || name == MANIFEST {
            continue;
        }
        // 「..」や絶対パスで、行き先の外へ書かないようにする
        let rel = ent.enclosed_name().ok_or_else(|| format!("危ないパス: {name}"))?;
        let target = dest.join(rel);
        if let Some(p) = target.parent() {
            fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut out = File::create(&target).map_err(|e| e.to_string())?;
        std::io::copy(&mut ent, &mut out).map_err(|e| e.to_string())?;
    }
    let problems = compare_with_manifest(&manifest, dest)?;
    if !problems.is_empty() {
        return Err(format!("復元した中身がmanifestと違う: {}", problems.join(" / ")));
    }
    Ok(manifest.files.len())
}

/// 行き先のフォルダが、manifestと一致するか(足りない・違う・余分なファイルを返す)
fn compare_with_manifest(manifest: &Manifest, dir: &Path) -> Result<Vec<String>, String> {
    let mut problems = Vec::new();
    let mut have = Vec::new();
    for top in top_names(manifest) {
        collect(&dir.join(&top), &top, &mut have)?;
    }
    let have: BTreeMap<String, PathBuf> = have.into_iter().collect();
    for e in &manifest.files {
        match have.get(&e.path) {
            None => problems.push(format!("無い: {}", e.path)),
            Some(p) => {
                let b = fs::read(p).map_err(|er| er.to_string())?;
                if b.len() as u64 != e.size || hex_sha256(&b) != e.sha256 {
                    problems.push(format!("違う: {}", e.path));
                }
            }
        }
    }
    let want: std::collections::BTreeSet<&str> = manifest.files.iter().map(|e| e.path.as_str()).collect();
    for k in have.keys() {
        if !want.contains(k.as_str()) {
            problems.push(format!("余分: {k}"));
        }
    }
    Ok(problems)
}

fn top_names(manifest: &Manifest) -> Vec<String> {
    let mut v: Vec<String> = manifest.files.iter().filter_map(|e| e.path.split('/').next().map(String::from)).collect();
    v.sort();
    v.dedup();
    v
}

/// 2つのフォルダの中身(相対パス・大きさ・SHA-256)が一致するか。違いの一覧を返す(空なら一致)
pub fn diff_dirs(a: &Path, b: &Path) -> Result<Vec<String>, String> {
    let mut fa = Vec::new();
    let mut fb = Vec::new();
    collect(a, "", &mut fa)?;
    collect(b, "", &mut fb)?;
    let ma: BTreeMap<String, PathBuf> = fa.into_iter().collect();
    let mb: BTreeMap<String, PathBuf> = fb.into_iter().collect();
    let mut out = Vec::new();
    for (k, p) in &ma {
        match mb.get(k) {
            None => out.push(format!("片方に無い: {k}")),
            Some(q) => {
                if hex_sha256(&fs::read(p).map_err(|e| e.to_string())?) != hex_sha256(&fs::read(q).map_err(|e| e.to_string())?) {
                    out.push(format!("違う: {k}"));
                }
            }
        }
    }
    for k in mb.keys() {
        if !ma.contains_key(k) {
            out.push(format!("片方に無い: {k}"));
        }
    }
    Ok(out)
}

/// 世代の一覧(名前の順)。(名前, 大きさ, zip全体のSHA-256)
pub fn list(dir: &Path) -> Vec<(String, u64, String)> {
    let mut v = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            if is_generation_name(&name) {
                let size = ent.metadata().map(|m| m.len()).unwrap_or(0);
                let sha = fs::read_to_string(dir.join(format!("{name}.sha256"))).unwrap_or_default();
                v.push((name, size, sha.trim().to_string()));
            }
        }
    }
    v.sort();
    v
}

/// gen-YYYYMMDD-HHMMSS(-n).zip の形だけを、世代の名前として認める(取り出すときに、別のパスを指せないように)
pub fn is_generation_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("gen-").and_then(|r| r.strip_suffix(".zip")) else { return false };
    let mut parts = rest.split('-');
    let ok = |s: Option<&str>, n: usize| s.is_some_and(|s| s.len() == n && s.bytes().all(|b| b.is_ascii_digit()));
    if !(ok(parts.next(), 8) && ok(parts.next(), 6)) {
        return false;
    }
    match parts.next() {
        None => true,
        Some(s) => !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && parts.next().is_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(root: &Path) {
        fs::create_dir_all(root.join("lib/00-inbox")).unwrap();
        fs::create_dir_all(root.join("tasks")).unwrap();
        fs::write(root.join("lib/00-inbox/a.md"), "あいう").unwrap();
        fs::write(root.join("lib/b.md"), "hello").unwrap();
        fs::write(root.join("tasks/t.md"), "task").unwrap();
    }

    #[test]
    fn 作って照らして復元して一致する() {
        let d = tempfile::tempdir().unwrap();
        sample(d.path());
        let g = create(&[("library", &d.path().join("lib")), ("tasks", &d.path().join("tasks"))], &d.path().join("gens")).unwrap();
        assert_eq!(g.files, 3);
        let zip = d.path().join("gens").join(&g.name);
        assert_eq!(verify_zip(&zip).unwrap(), 3);
        let dest = d.path().join("restored");
        assert_eq!(restore(&zip, &dest).unwrap(), 3);
        assert!(diff_dirs(&dest.join("library"), &d.path().join("lib")).unwrap().is_empty());
        assert!(diff_dirs(&dest.join("tasks"), &d.path().join("tasks")).unwrap().is_empty());
    }

    #[test]
    fn 行き先が空でなければ復元しない() {
        let d = tempfile::tempdir().unwrap();
        sample(d.path());
        let g = create(&[("library", &d.path().join("lib"))], &d.path().join("gens")).unwrap();
        let zip = d.path().join("gens").join(&g.name);
        assert!(restore(&zip, &d.path().join("lib")).is_err());
    }

    #[test]
    fn 世代の名前の形() {
        assert!(is_generation_name("gen-20261120-030000.zip"));
        assert!(is_generation_name("gen-20261120-030000-2.zip"));
        assert!(!is_generation_name("gen-20261120-030000.zip.sha256"));
        assert!(!is_generation_name("../gen-20261120-030000.zip"));
        assert!(!is_generation_name("gen-2026-1.zip"));
    }

    #[test]
    fn 壊れたzipは照合で落ちる() {
        let d = tempfile::tempdir().unwrap();
        sample(d.path());
        let g = create(&[("library", &d.path().join("lib"))], &d.path().join("gens")).unwrap();
        let zip = d.path().join("gens").join(&g.name);
        let mut b = fs::read(&zip).unwrap();
        let mid = b.len() / 3;
        b[mid] ^= 0xff;
        fs::write(&zip, b).unwrap();
        assert!(verify_zip(&zip).is_err());
    }
}
