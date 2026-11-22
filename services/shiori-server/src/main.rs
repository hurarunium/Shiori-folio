//! shiori-server: 詩織Ver7.0のサーバー。
//!
//! 使い方:
//!   shiori-server serve --config <server.json> [--bind <IP:ポート>] [--no-children]
//!   shiori-server token add <端末名> --data <DIR>       (トークンを1回だけ表示する)
//!   shiori-server token revoke <端末名> --data <DIR>
//!   shiori-server token list --data <DIR>
//!   shiori-server backup-verify <zip> --dest <空のDIR> [--against-library <DIR>] [--against-tasks <DIR>]
//!
//! 待ち受けは設定の `bind` だけ。0.0.0.0 などの全部のアドレスは拒否する。窓口の一覧は `src/api.rs`。

use std::collections::HashMap;
use std::path::PathBuf;

use shiori_server::{auth, backup, Config};

fn options(args: &[String]) -> Result<(Vec<String>, HashMap<String, String>), String> {
    let mut pos = Vec::new();
    let mut map = HashMap::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(k) = a.strip_prefix("--") {
            if k == "no-children" {
                map.insert(k.to_string(), "1".into());
            } else {
                let v = it.next().ok_or_else(|| format!("--{k} の値が無い"))?;
                map.insert(k.to_string(), v.clone());
            }
        } else {
            pos.push(a.clone());
        }
    }
    Ok((pos, map))
}

fn need<'a>(o: &'a HashMap<String, String>, k: &str) -> Result<&'a String, String> {
    o.get(k).ok_or_else(|| format!("--{k} が必要"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match run(&args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    };
    std::process::exit(code);
}

fn run(args: &[String]) -> Result<(), String> {
    let (pos, o) = options(args.get(1..).unwrap_or(&[]))?;
    match args.first().map(String::as_str) {
        Some("serve") => {
            let mut cfg = Config::load(&PathBuf::from(need(&o, "config")?))?;
            if let Some(b) = o.get("bind") {
                cfg.bind = b.clone();
            }
            if o.contains_key("no-children") {
                cfg.no_children = true;
            }
            let running = shiori_server::start(cfg)?;
            eprintln!("待ち受け: http://{}(子の起動: {})", running.addr, if running.app.cfg.no_children { "しない" } else { "する" });
            running.wait();
            Ok(())
        }
        Some("token") => {
            let data = PathBuf::from(need(&o, "data")?);
            let tokens = data.join("tokens.json");
            match pos.first().map(String::as_str) {
                Some("add") => {
                    let name = pos.get(1).ok_or("端末名が必要")?;
                    std::fs::create_dir_all(&data).map_err(|e| e.to_string())?;
                    let token = auth::add(&tokens, name)?;
                    println!("端末 {name} のトークン(この1回だけ表示。控えてください):\n{token}");
                    Ok(())
                }
                Some("revoke") => {
                    let name = pos.get(1).ok_or("端末名が必要")?;
                    auth::revoke(&tokens, name)?;
                    println!("{name} を失効した(すぐに効く)");
                    Ok(())
                }
                Some("list") => {
                    for (name, revoked) in auth::list(&tokens) {
                        println!("{name}\t{}", if revoked { "失効" } else { "有効" });
                    }
                    Ok(())
                }
                _ => Err("使い方: shiori-server token add|revoke|list …".into()),
            }
        }
        Some("backup-verify") => backup_verify(&pos, &o),
        _ => Err("使い方: shiori-server serve|token|backup-verify …(ファイル先頭の説明を参照)".into()),
    }
}

/// zipを照らし、行き先へ復元し、manifestと一致するか調べる。元のフォルダが渡されれば、それとも照らす。
fn backup_verify(pos: &[String], o: &HashMap<String, String>) -> Result<(), String> {
    let zip = PathBuf::from(pos.first().ok_or("zipのパスが必要")?);
    let dest = PathBuf::from(need(o, "dest")?);
    let n = backup::verify_zip(&zip)?;
    println!("zipの中身はmanifestと一致した({n} ファイル)");
    let n = backup::restore(&zip, &dest)?;
    println!("復元した中身はmanifestと一致した({n} ファイル): {}", dest.display());
    for (key, sub) in [("against-library", "library"), ("against-tasks", "tasks")] {
        if let Some(orig) = o.get(key) {
            let diff = backup::diff_dirs(&dest.join(sub), &PathBuf::from(orig))?;
            if diff.is_empty() {
                println!("{sub}: 元のフォルダと一致した");
            } else {
                return Err(format!("{sub}: 元のフォルダと違う: {}", diff.join(" / ")));
            }
        }
    }
    Ok(())
}
