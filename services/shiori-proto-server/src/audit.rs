//! 監査ログ。1行に1件のJSON(時刻・端末・接続元・操作・パス・結果・かかった時間)。本文は残さない。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

pub struct Audit {
    file: Mutex<File>,
}

impl Audit {
    pub fn open(path: &Path) -> Result<Self, String> {
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("監査ログを開けない {}: {e}", path.display()))?;
        Ok(Audit { file: Mutex::new(f) })
    }

    pub fn record(&self, device: &str, remote: &str, method: &str, url: &str, status: u16, ms: u64) {
        let line = serde_json::json!({
            "ts": chrono::Local::now().to_rfc3339(),
            "device": device, "remote": remote, "method": method, "url": url,
            "status": status, "ms": ms,
        });
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(f, "{line}");
        }
    }
}
