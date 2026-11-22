//! 監査ログ。`data/audit/YYYY-MM.jsonl`。1行に1件のJSON(時刻・端末・クライアント・接続元・操作・パス・結果・かかった時間)。
//! 本文は残さない。書き込み・削除・保存・追記・トークンの操作ごとに残る(読み取りも、操作として残す)。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct Audit {
    dir: PathBuf,
    lock: Mutex<()>,
}

impl Audit {
    pub fn open(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("監査ログの置き場を作れない {}: {e}", dir.display()))?;
        Ok(Audit { dir: dir.to_path_buf(), lock: Mutex::new(()) })
    }

    pub fn record(&self, device: &str, client: &str, remote: &str, method: &str, url: &str, status: u16, ms: u64) {
        let now = chrono::Local::now();
        let line = serde_json::json!({
            "ts": now.to_rfc3339(),
            "device": device, "client": client, "remote": remote, "method": method, "url": url,
            "status": status, "ms": ms,
        });
        let path = self.dir.join(format!("{}.jsonl", now.format("%Y-%m")));
        let _guard = self.lock.lock().unwrap_or_else(|p| p.into_inner());
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 月ごとのファイルに1行ずつ残り本文は残らない() {
        let d = tempfile::tempdir().unwrap();
        let a = Audit::open(&d.path().join("audit")).unwrap();
        a.record("win", "cli", "127.0.0.1", "POST", "/api/v1/save", 200, 12);
        a.record("mac", "app", "127.0.0.1", "GET", "/api/v1/doc?path=library/a.md", 404, 3);
        let names: Vec<_> = std::fs::read_dir(d.path().join("audit")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), 1);
        assert!(names[0].ends_with(".jsonl") && names[0].len() == "2026-11.jsonl".len());
        let text = std::fs::read_to_string(d.path().join("audit").join(&names[0])).unwrap();
        assert_eq!(text.lines().count(), 2);
        let v: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!((v["device"].as_str(), v["client"].as_str(), v["status"].as_u64()), (Some("win"), Some("cli"), Some(200)));
    }
}
