//! 子プロセス(埋め込みの llama-server と shiori-rag)の見張り。
//! - すでに動いていて健康なポートがあれば、起動せずにそのまま使う(窓口だけを再起動しても、子は残る)
//! - 落ちたら5秒おきに起こし直す。RAGは、埋め込みが健康になってから起こす

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct Paths {
    /// config.json・bin/<os>/・models/ を持つ実行環境
    pub runtime: PathBuf,
    pub library: PathBuf,
    /// 索引・ログ・トークンなど、窓口の状態の置き場
    pub data: PathBuf,
    pub embed_port: u16,
    pub rag_port: u16,
}

#[derive(Default)]
pub struct Health {
    pub embed: AtomicBool,
    pub rag: AtomicBool,
}

pub fn healthy(port: u16) -> bool {
    ureq::get(&format!("http://127.0.0.1:{port}/health"))
        .timeout(Duration::from_secs(2))
        .call()
        .is_ok()
}

fn os_dir() -> &'static str {
    if cfg!(target_os = "macos") { "mac" } else if cfg!(windows) { "win" } else { "linux" }
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

fn log_file(data: &Path, name: &str) -> Stdio {
    let dir = data.join("logs");
    let _ = std::fs::create_dir_all(&dir);
    match OpenOptions::new().create(true).append(true).open(dir.join(format!("{name}.log"))) {
        Ok(f) => Stdio::from(f),
        Err(_) => Stdio::null(),
    }
}

#[cfg(windows)]
fn hide(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
fn hide(_cmd: &mut Command) {}

fn spawn_embed(p: &Paths) -> std::io::Result<Child> {
    let bin = p.runtime.join("bin").join(os_dir());
    let model_dir = p.runtime.join("models").join("embed");
    let sep = if cfg!(windows) { ';' } else { ':' };
    let path = format!(
        "{}{sep}{}",
        bin.join("libs-llama").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let ctx = "2048";
    let mut cmd = Command::new(bin.join(exe("llama-server")));
    cmd.current_dir(&model_dir)
        .env("PATH", path)
        .args(["--model", "nomic-embed-text.gguf", "--port", &p.embed_port.to_string()])
        .args(["--host", "127.0.0.1", "--n-gpu-layers", "0", "--embedding"])
        .args(["--ubatch-size", ctx, "--ctx-size", ctx, "--parallel", "1"])
        .stdin(Stdio::null())
        .stdout(log_file(&p.data, "embed"))
        .stderr(log_file(&p.data, "embed"));
    hide(&mut cmd);
    cmd.spawn()
}

fn spawn_rag(p: &Paths) -> std::io::Result<Child> {
    let bin = p.runtime.join("bin").join(os_dir());
    let shelf = p.data.join("knowledge");
    let _ = std::fs::create_dir_all(&shelf);
    let mut cmd = Command::new(bin.join(exe("shiori-rag")));
    cmd.arg("serve")
        .args(["--root", &p.runtime.to_string_lossy()])
        .args(["--port", &p.rag_port.to_string()])
        .args(["--library", &p.library.to_string_lossy()])
        .args(["--shelf", &shelf.to_string_lossy()])
        .args(["--index-dir", &p.data.join("rag-index").to_string_lossy()])
        .args(["--log-dir", &p.data.join("logs").to_string_lossy()])
        .stdin(Stdio::null())
        .stdout(log_file(&p.data, "rag"))
        .stderr(log_file(&p.data, "rag"));
    hide(&mut cmd);
    cmd.spawn()
}

fn alive(child: &mut Option<Child>) -> bool {
    match child {
        Some(c) => matches!(c.try_wait(), Ok(None)),
        None => false,
    }
}

/// 見張りのスレッドを始める。窓口を止めても子は止めない(次の起動で、そのまま使う)
pub fn supervise(p: Paths, health: Arc<Health>) {
    std::thread::spawn(move || {
        let mut embed: Option<Child> = None;
        let mut rag: Option<Child> = None;
        loop {
            let embed_ok = healthy(p.embed_port);
            health.embed.store(embed_ok, Ordering::Relaxed);
            if !embed_ok && !alive(&mut embed) {
                eprintln!("[見張り] 埋め込みを起動する");
                embed = spawn_embed(&p).map_err(|e| eprintln!("[見張り] 埋め込みを起動できない: {e}")).ok();
            }
            let rag_ok = healthy(p.rag_port);
            health.rag.store(rag_ok, Ordering::Relaxed);
            // RAGは起動時に索引を同期するため、埋め込みが健康になってから起こす。
            // 同期の途中(ポートを開く前)は、プロセスが生きている間は起こし直さない
            if embed_ok && !rag_ok && !alive(&mut rag) {
                eprintln!("[見張り] RAGを起動する");
                rag = spawn_rag(&p).map_err(|e| eprintln!("[見張り] RAGを起動できない: {e}")).ok();
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}
