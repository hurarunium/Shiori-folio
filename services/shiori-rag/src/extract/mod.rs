//! 素材(`.meta` の実体)の文の抽出(`services/rag/extraction/` と同じ守り)。
//!
//! 流れ: 拡張子の確認 → 大きさの確認 → キャッシュの確認(素材のsha256) → 別プロセスで抽出 →
//! 秘密情報の検査 → 結果(または理由つきの失敗)を返す。
//! 抽出は、このプログラム自身を `shiori-rag extract <素材> <結果のJSON>` で別プロセスとして起動して
//! 行う。壊れた・悪意のある素材で抽出が止まらない・落ちる場合も、サーバーは巻き込まれない
//! (時間の上限で親が止める)。結果を標準出力ではなくファイルで受け取るのは、PDFの部品が
//! 標準出力に何かを書いても、結果が混ざらないようにするため。

pub mod formats;
pub mod limits;
pub mod secrets;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 結果の形が変わったら上げる(古いキャッシュを使わないため)。
const CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractResult {
    pub ok: bool,
    pub sections: Vec<SectionOut>,
    /// 失敗または飛ばした理由。okでも、文が無いとき("テキストなし")は理由を持つ
    pub reason: String,
    /// 利用者に警告として見せるべきか(秘密情報の検出など)
    pub warn: bool,
    pub truncated: bool,
    /// 使ったキャッシュのファイル名(キャッシュに無い・残さないときはNone)。キャッシュの掃除に使う
    #[serde(skip)]
    pub cache_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionOut {
    pub label: String,
    pub text: String,
}

impl ExtractResult {
    fn fail(reason: impl Into<String>) -> Self {
        Self { ok: false, reason: reason.into(), ..Default::default() }
    }
}

/// 抽出プロセスが書く結果。
#[derive(Serialize, Deserialize)]
struct WorkerOutput {
    ok: bool,
    #[serde(default)]
    sections: Vec<SectionOut>,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    detail: String,
}

/// 抽出の入口(サーバーから呼ぶ)。失敗も結果として返し、外へは出さない。
pub fn extract_asset(path: &Path, cache_dir: &Path) -> ExtractResult {
    if !formats::supported(path) {
        return ExtractResult::fail("未対応の形式");
    }
    match std::fs::metadata(path) {
        Ok(m) if m.len() > limits::MAX_FILE_BYTES => return ExtractResult::fail("ファイルが大きすぎる"),
        Ok(_) => {}
        Err(_) => return ExtractResult::fail("抽出に失敗(OSError)"),
    }
    let Ok(digest) = sha256_file(path) else { return ExtractResult::fail("抽出に失敗(OSError)") };
    let cache_name = format!("{digest}-v{CACHE_VERSION}.json");
    let cache_file = cache_dir.join(&cache_name);
    if let Ok(text) = std::fs::read_to_string(&cache_file)
        && let Ok(mut cached) = serde_json::from_str::<ExtractResult>(&text)
    {
        cached.cache_name = Some(cache_name);
        return cached;
    }
    let mut result = run_worker(path);
    // 一時的な失敗(タイムアウト)はキャッシュしない
    if result.ok || result.reason != "タイムアウト" {
        let _ = std::fs::create_dir_all(cache_dir);
        if let Ok(json) = serde_json::to_string(&result)
            && std::fs::write(&cache_file, json).is_ok()
        {
            result.cache_name = Some(cache_name);
        }
    }
    result
}

/// 使っていないキャッシュ(`used` に無い `.json`)を消す。消した数を返す。
/// 素材を差し替えた・消したときの古い結果と、`CACHE_VERSION` を上げる前の結果が残り続けないようにする。
pub fn prune_cache(cache_dir: &Path, used: &std::collections::HashSet<String>) -> usize {
    let Ok(entries) = std::fs::read_dir(cache_dir) else { return 0 };
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".json") && !name.starts_with("._") && !used.contains(&name) && std::fs::remove_file(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn run_worker(path: &Path) -> ExtractResult {
    let Ok(exe) = std::env::current_exe() else { return ExtractResult::fail("抽出に失敗(OSError)") };
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let out: PathBuf = std::env::temp_dir().join(format!("shiori-rag-extract-{}-{n}.json", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let mut cmd = Command::new(exe);
    cmd.arg("extract").arg(path).arg(&out).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // コンソールの窓を出さない(CREATE_NO_WINDOW)
        cmd.creation_flags(0x0800_0000);
    }
    let Ok(mut child) = cmd.spawn() else { return ExtractResult::fail("抽出に失敗(OSError)") };
    let start = Instant::now();
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) if start.elapsed() > Duration::from_secs(limits::TIMEOUT_SECONDS) => {
                let _ = child.kill();
                let _ = child.wait();
                break true;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break false,
        }
    };
    let result = if timed_out { ExtractResult::fail("タイムアウト") } else { read_worker_output(&out) };
    let _ = std::fs::remove_file(&out);
    result
}

fn read_worker_output(out: &Path) -> ExtractResult {
    match std::fs::metadata(out) {
        Ok(m) if m.len() > limits::WORKER_OUTPUT_MAX_BYTES => return ExtractResult::fail("抽出結果が大きすぎる"),
        Ok(_) => {}
        Err(_) => return ExtractResult::fail("抽出プロセスが異常終了"),
    }
    let Some(data) = std::fs::read_to_string(out).ok().and_then(|t| serde_json::from_str::<WorkerOutput>(&t).ok())
    else {
        return ExtractResult::fail("抽出プロセスが異常終了");
    };
    if !data.ok {
        return ExtractResult::fail(if data.reason.is_empty() { "抽出に失敗".into() } else { data.reason });
    }
    if data.sections.is_empty() {
        return ExtractResult { ok: true, reason: "テキストなし".into(), ..Default::default() };
    }
    let all: Vec<&str> = data.sections.iter().map(|s| s.text.as_str()).collect();
    if let Some(secret) = secrets::find_secret(&all.join("\n")) {
        // 秘密情報らしきものが含まれる素材は、索引に入れない(中身の断片も記録に残さない)
        return ExtractResult { ok: false, reason: format!("秘密情報らしき文字列を検出({secret})"), warn: true, ..Default::default() };
    }
    ExtractResult { ok: true, sections: data.sections, truncated: data.truncated, ..Default::default() }
}

/// 抽出プロセスの本体(`shiori-rag extract <素材> <結果のJSON>`)。
pub fn worker_main(asset: &Path, out: &Path) -> i32 {
    let result = std::panic::catch_unwind(|| work(asset));
    let output = match result {
        Ok(Ok((sections, truncated))) => WorkerOutput { ok: true, sections, truncated, reason: String::new(), detail: String::new() },
        Ok(Err(e)) => WorkerOutput {
            ok: false,
            sections: vec![],
            truncated: false,
            detail: e.detail.chars().take(200).collect(),
            reason: e.reason,
        },
        Err(_) => WorkerOutput {
            ok: false,
            sections: vec![],
            truncated: false,
            reason: "抽出に失敗".into(),
            detail: "panic".into(),
        },
    };
    match serde_json::to_vec(&output).map(|json| std::fs::write(out, json)) {
        Ok(Ok(())) => 0,
        _ => 1,
    }
}

fn work(asset: &Path) -> Result<(Vec<SectionOut>, bool), formats::ExtractionError> {
    let ext = formats::ext_of(asset);
    let extractor = formats::extractor_for(&ext).ok_or_else(|| formats::ExtractionError::with("未対応の形式", &ext))?;
    let size = std::fs::metadata(asset).map_err(|e| formats::ExtractionError::with("抽出に失敗", e))?.len();
    if size > limits::MAX_FILE_BYTES {
        return Err(formats::ExtractionError::new("ファイルが大きすぎる"));
    }
    let mut total = 0usize;
    let mut out = Vec::new();
    let mut truncated = false;
    for (label, text) in extractor(asset)? {
        let mut text = crate::text::py_strip(&text).to_string();
        if text.is_empty() {
            continue;
        }
        let len = text.chars().count();
        if total + len > limits::MAX_CHARS {
            text = text.chars().take(limits::MAX_CHARS - total).collect();
            truncated = true;
        }
        total += text.chars().count();
        out.push(SectionOut { label, text });
        if truncated {
            break;
        }
    }
    Ok((out, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 使っていないキャッシュだけを消す() {
        let d = tempfile::tempdir().unwrap();
        for name in ["keep-v1.json", "old-v1.json", "keep-v0.json", "._keep-v1.json", "memo.txt"] {
            std::fs::write(d.path().join(name), "{}").unwrap();
        }
        let used: std::collections::HashSet<String> = ["keep-v1.json".to_string()].into();
        assert_eq!(prune_cache(d.path(), &used), 2);
        let mut left: Vec<String> =
            std::fs::read_dir(d.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        // 使っているもの、Macの付属ファイル、キャッシュ以外のファイルは残す
        assert_eq!(left, vec!["._keep-v1.json", "keep-v1.json", "memo.txt"]);
        // フォルダが無くても落ちない
        assert_eq!(prune_cache(&d.path().join("none"), &used), 0);
    }
}
