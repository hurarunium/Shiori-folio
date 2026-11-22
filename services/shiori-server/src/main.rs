//! shiori-server: 詩織Ver7.0のサーバー。
//!
//! 使い方:
//!   shiori-server serve --config <server.json> [--bind <IP:ポート>] [--no-children]
//!   shiori-server token add <端末名> --data <DIR>       (トークンを1回だけ表示する)
//!   shiori-server token revoke <端末名> --data <DIR>
//!   shiori-server token list --data <DIR>
//!   shiori-server restore <zip> --dest <空のDIR> [--against-library <DIR>] [--against-tasks <DIR>]   (backup-verify・verify も同じ)
//!   (serve は、毎日05:00(日誌の1日の区切りと同じ)に世代を作り、毎日7・毎週4を残して整理する。止まっていた分は起動時に追いつく)
//!   shiori-server backup-verify <zip> --dest <空のDIR> [--against-library <DIR>] [--against-tasks <DIR>]
//!   shiori-server migrate-plan <元のlibrary> <行き先>     (予行。何も書かない。報告を出す)
//!   shiori-server migrate-apply <元のlibrary> <行き先>    (3つの段階を、照合しながら適用する。行き先は空の場所)
//!   shiori-server migrate-verify <元のlibrary> <行き先>   (全体の照合をやり直す)
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
            // 毎日05:00の世代(止まっていた分は、起動時に追いつく)
            backup::spawn_schedule(
                running.app.roots.clone(),
                running.app.backups.clone(),
                running.app.clock.clone(),
            );
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
        Some("backup-verify" | "restore" | "verify") => backup_verify(&pos, &o),
        Some(cmd @ ("migrate-plan" | "migrate-apply" | "migrate-verify")) => migrate(cmd, &pos),
        _ => Err("使い方: shiori-server serve|token|backup-verify|migrate-plan|migrate-apply|migrate-verify …(ファイル先頭の説明を参照)".into()),
    }
}

/// 移行ツール(詩織Ver7.0 Phase 5-B)。元のlibraryは書き換えない。
fn migrate(cmd: &str, pos: &[String]) -> Result<(), String> {
    let src = PathBuf::from(pos.first().ok_or("元のlibraryのパスが必要")?);
    let dst = PathBuf::from(pos.get(1).ok_or("行き先のパスが必要")?);
    match cmd {
        "migrate-plan" => {
            let report = library_store::migrate::plan(&src, &dst)?;
            print!("{}", report.render());
            if report.warnings.is_empty() { Ok(()) } else { Err("警告があります(このままでは、適用できません)".into()) }
        }
        "migrate-apply" => {
            let report = library_store::migrate::apply(&src, &dst)?;
            print!("{}", report.render());
            println!("適用して、3つの段階の照合に通りました: {}", dst.display());
            Ok(())
        }
        _ => {
            let problems = library_store::migrate::verify(&src, &dst)?;
            if problems.is_empty() {
                println!("照合に通りました: すべて合っています");
                Ok(())
            } else {
                for p in problems.iter().take(100) {
                    println!("  - {p}");
                }
                Err(format!("照合に通りません({}件)", problems.len()))
            }
        }
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
