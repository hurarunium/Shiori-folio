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

/// 世代の名前から、作った日時(ローカル)を読む。
pub fn generation_time(name: &str) -> Option<chrono::NaiveDateTime> {
    if !is_generation_name(name) {
        return None;
    }
    let rest = name.strip_prefix("gen-")?.strip_suffix(".zip")?;
    let mut it = rest.split('-');
    let (d, t) = (it.next()?, it.next()?);
    chrono::NaiveDateTime::parse_from_str(&format!("{d}{t}"), "%Y%m%d%H%M%S").ok()
}

/// 毎日の世代を残す数と、毎週の世代を残す数
/// 1日の区切り(日誌と同じ午前5時)
pub const DAY_START_HOUR: u32 = 5;
pub const KEEP_DAILY: usize = 7;
pub const KEEP_WEEKLY: usize = 4;

/// 整理で消す世代の名前を返す(消さない=残すのは、直近 KEEP_DAILY 日の各日の最新と、
/// それより古いものの各週の最新を直近 KEEP_WEEKLY 週)。名前の形でない物は、触らない。
pub fn prune_targets(names: &[String]) -> Vec<String> {
    use chrono::Datelike;
    let mut gens: Vec<(chrono::NaiveDateTime, &String)> =
        names.iter().filter_map(|n| generation_time(n).map(|t| (t, n))).collect();
    gens.sort();
    let mut keep: std::collections::BTreeSet<&String> = Default::default();
    // 毎日: 日ごとの最新を、新しい日から KEEP_DAILY 日分
    let mut by_day: BTreeMap<chrono::NaiveDate, &String> = BTreeMap::new();
    for (t, n) in &gens {
        by_day.insert(t.date(), n);
    }
    let mut daily_days = Vec::new();
    for (d, n) in by_day.iter().rev().take(KEEP_DAILY) {
        keep.insert(*n);
        daily_days.push(*d);
    }
    // 毎週: 毎日の分に入らなかった日のうち、週ごとの最新を、新しい週から KEEP_WEEKLY 週分
    let mut by_week: BTreeMap<(i32, u32), &String> = BTreeMap::new();
    for (d, n) in &by_day {
        if daily_days.contains(d) {
            continue;
        }
        let w = d.iso_week();
        by_week.insert((w.year(), w.week()), n);
    }
    for (_, n) in by_week.iter().rev().take(KEEP_WEEKLY) {
        keep.insert(*n);
    }
    gens.iter().map(|(_, n)| (*n).clone()).filter(|n| !keep.contains(n)).collect()
}

/// 整理を実行して、消した世代の名前を返す。
pub fn prune(dir: &Path) -> Vec<String> {
    let names: Vec<String> = list(dir).into_iter().map(|(n, _, _)| n).collect();
    let mut gone = Vec::new();
    for n in prune_targets(&names) {
        if fs::remove_file(dir.join(&n)).is_ok() {
            let _ = fs::remove_file(dir.join(format!("{n}.sha256")));
            gone.push(n);
        }
    }
    gone
}

/// いま世代を作る時か。毎日 05:00(日誌の1日の区切りと同じ)を境にして、直近の 05:00 より後に作った世代が無ければ作る
/// (サーバーが 05:00 に止まっていても、起動したときに追いつく)。
pub fn is_due(now: chrono::NaiveDateTime, latest: Option<chrono::NaiveDateTime>) -> bool {
    let three = chrono::NaiveTime::from_hms_opt(DAY_START_HOUR, 0, 0).unwrap();
    let today = now.date().and_time(three);
    let boundary = if now >= today { today } else { today - chrono::Duration::days(1) };
    latest.map_or(true, |l| l < boundary)
}

/// 毎日の世代を作る見張り(`serve` で動かす)。作ったあと、整理する。失敗は標準エラーに出して、次の周で再び試す。
pub fn spawn_schedule(
    roots: crate::saver::Roots,
    dir: PathBuf,
    clock: crate::Clock,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || loop {
        let now = clock().naive_local();
        let latest = list(&dir).into_iter().filter_map(|(n, _, _)| generation_time(&n)).max();
        if is_due(now, latest) {
            match create(&[("library", &roots.library), ("knowledge", &roots.knowledge), ("tasks", &roots.tasks)], &dir) {
                Ok(g) => {
                    eprintln!("毎日の世代を作った: {}({}ファイル)", g.name, g.files);
                    for n in prune(&dir) {
                        eprintln!("古い世代を整理した: {n}");
                    }
                }
                Err(e) => eprintln!("毎日の世代を作れなかった(次の周で再び試す): {e}"),
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(300));
    })
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

    fn n(s: &str) -> String {
        format!("gen-{s}.zip")
    }

    #[test]
    fn 整理は毎日7と毎週4を残す() {
        // 2026-11-01(日)から30日、毎日05:00の世代
        let mut names = Vec::new();
        let start = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        for i in 0..50 {
            let d = start + chrono::Duration::days(i);
            names.push(n(&format!("{}-030000", d.format("%Y%m%d"))));
        }
        // 同じ日の2つめ(手動)
        names.push(n("20261119-120000"));
        let gone = prune_targets(&names);
        let kept: Vec<&String> = names.iter().filter(|x| !gone.contains(x)).collect();
        // 最新は 2026-11-19(10/1+49日)。毎日7(11/13〜11/19)+毎週4
        assert!(kept.contains(&&n("20261119-120000")), "同じ日は最新だけ");
        assert!(!kept.contains(&&n("20261119-030000")));
        for d in 13..=18 {
            assert!(kept.contains(&&n(&format!("202611{d}-030000"))), "{d}");
        }
        assert_eq!(kept.len(), 7 + 4);
        // 形でない名前は触らない
        assert!(prune_targets(&["notes.zip".to_string()]).is_empty());
    }

    #[test]
    fn 世代の時が来たか() {
        let dt = |s: &str| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap();
        assert!(is_due(dt("2026-11-22 10:00:00"), None));
        // 今日の05:00の後に作ってあれば、不要
        assert!(!is_due(dt("2026-11-22 10:00:00"), Some(dt("2026-11-22 05:00:05"))));
        // 05:00の前なら、昨日の05:00が境
        assert!(!is_due(dt("2026-11-22 04:00:00"), Some(dt("2026-11-21 05:00:05"))));
        assert!(is_due(dt("2026-11-22 04:00:00"), Some(dt("2026-11-21 04:00:00"))));
        // 起動したら、前の日の世代しか無い(追いつく)
        assert!(is_due(dt("2026-11-22 08:00:00"), Some(dt("2026-11-21 05:00:05"))));
    }

    #[test]
    fn 名前から日時を読む() {
        assert_eq!(generation_time("gen-20261122-030000-2.zip").unwrap().to_string(), "2026-11-22 03:00:00");
        assert!(generation_time("x.zip").is_none());
    }
}
