//! タスクリスト(詩織Ver6.2)のCLI。AIがタスクを操作するときの入口で、画面と同じ関数を呼ぶ。
//!
//! 使用法:
//!   shiori-task add --summary "…" [--folder Unity/Tanker] [--due 2026-11-09] [--detail-file f.md]
//!   shiori-task edit <id> [--summary "…"] [--due 日付|none] [--detail-file f.md]
//!   shiori-task move <id> --folder 詩織 / --no-folder(未分類へ)
//!   shiori-task done <id> / restore <id>
//!   shiori-task delete <id> --yes            (--yesが無ければ何もしない)
//!   shiori-task folder add <名前> / rename <元> <先> / remove <名前>
//!   shiori-task list [--archived]
//!
//! 結果は標準出力にJSONで返す。失敗は標準エラーに理由を出し、終了コード1で終わる。

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use shiori_folio_lib::tasks::{self, NewTask, TaskEdit};
use shiori_folio_lib::{project_root, shiori_log, tasks_root};

const VALUE_OPTIONS: &[&str] = &["--summary", "--folder", "--due", "--detail-file"];
// 未分類を`--folder ""`で表さない(Windows PowerShell 5.1は空の引数を渡さず落とす)。
const SWITCHES: &[&str] = &["--archived", "--yes", "--no-folder"];
const USAGE: &str = "使用法: shiori-task add|edit|move|done|restore|delete|folder|list(詳しくはソースの先頭を参照)";

#[derive(Default)]
struct Args {
    positional: Vec<String>,
    values: HashMap<String, String>,
    switches: Vec<String>,
}

impl Args {
    fn parse(raw: impl Iterator<Item = String>) -> Result<Self> {
        let mut args = Args::default();
        let mut raw = raw;
        while let Some(arg) = raw.next() {
            if VALUE_OPTIONS.contains(&arg.as_str()) {
                let value = raw.next().ok_or_else(|| anyhow!("{arg}には値が必要です"))?;
                args.values.insert(arg, value);
            } else if SWITCHES.contains(&arg.as_str()) {
                args.switches.push(arg);
            } else if arg.starts_with("--") {
                bail!("知らない指定です: {arg}");
            } else {
                args.positional.push(arg);
            }
        }
        Ok(args)
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|s| s == name)
    }

    // コマンド名の次にある、id(またはフォルダの名前)を取り出す。
    fn id(&self) -> Result<&str> {
        self.positional.get(1).map(String::as_str).ok_or_else(|| anyhow!("idを指定してください"))
    }

    fn detail(&self) -> Result<Option<String>> {
        self.value("--detail-file")
            .map(|path| std::fs::read_to_string(path).with_context(|| format!("{path}を読めません")))
            .transpose()
    }
}

fn run_folder(root: &Path, args: &Args) -> Result<Value> {
    let action = args.positional.get(1).map(String::as_str).unwrap_or("");
    let name = |at: usize| -> Result<&str> {
        args.positional.get(at).map(String::as_str).ok_or_else(|| anyhow!("フォルダの名前を指定してください"))
    };
    match action {
        "add" => {
            tasks::folder_add(root, name(2)?).map_err(|e| anyhow!(e))?;
        }
        "rename" => {
            tasks::folder_rename(root, name(2)?, name(3)?).map_err(|e| anyhow!(e))?;
        }
        "remove" => tasks::folder_remove(root, name(2)?).map_err(|e| anyhow!(e))?,
        _ => bail!("使用法: shiori-task folder add <名前> / rename <元> <先> / remove <名前>"),
    }
    Ok(json!({ "folders": tasks::folders(root) }))
}

fn run_command(root: &Path, command: &str, args: &Args) -> Result<Value> {
    let result = match command {
        "add" => {
            let summary = args.value("--summary").ok_or_else(|| anyhow!("--summaryが必要です"))?;
            let detail = args.detail()?.unwrap_or_default();
            let new = NewTask {
                summary,
                folder: args.value("--folder").unwrap_or(""),
                due: args.value("--due"),
                detail: &detail,
            };
            json!(tasks::add(root, &new).map_err(|e| anyhow!(e))?)
        }
        "edit" => {
            let detail = args.detail()?;
            let change = TaskEdit {
                summary: args.value("--summary"),
                due: args.value("--due").map(|d| if d == "none" { None } else { Some(d) }),
                detail: detail.as_deref(),
            };
            if change.summary.is_none() && change.due.is_none() && change.detail.is_none() {
                bail!("変更する項目(--summary、--due、--detail-file)を指定してください");
            }
            json!(tasks::edit(root, args.id()?, &change, None).map_err(|e| anyhow!(e))?)
        }
        "move" => {
            let folder = match (args.value("--folder"), args.has("--no-folder")) {
                (Some(folder), false) => folder,
                (None, true) => "",
                _ => bail!("--folderか--no-folderのどちらかを指定してください"),
            };
            json!(tasks::move_to(root, args.id()?, folder).map_err(|e| anyhow!(e))?)
        }
        "done" => json!(tasks::complete(root, args.id()?).map_err(|e| anyhow!(e))?),
        "restore" => json!(tasks::restore(root, args.id()?).map_err(|e| anyhow!(e))?),
        "delete" => {
            let id = args.id()?;
            if !args.has("--yes") {
                bail!("削除は戻せません。ふらるに確かめてから--yesを付けてください(何もしていません)");
            }
            tasks::delete(root, id).map_err(|e| anyhow!(e))?;
            json!({ "deleted": id })
        }
        "folder" => run_folder(root, args)?,
        "list" => json!({
            "tasks": tasks::list(root, args.has("--archived")),
            "folders": tasks::folders(root),
        }),
        _ => bail!("{USAGE}"),
    };
    Ok(result)
}

fn run() -> Result<()> {
    let args = Args::parse(std::env::args().skip(1))?;
    let command = args.positional.first().cloned().ok_or_else(|| anyhow!("{USAGE}"))?;
    // 概要が入るので、引数はログに書かない。
    shiori_log::debug("TASK", "始めた", &[("操作", &command)]);
    let result = run_command(&tasks_root(), &command, &args)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn main() -> Result<()> {
    let root = project_root();
    shiori_log::init(
        &root.join("data").join("logs"),
        shiori_log::LogKind::Task,
        &shiori_log::LogSettings::from_config(&root),
    );
    let outcome = run();
    if let Err(e) = &outcome {
        shiori_log::error("TASK", "失敗した", &[("理由", &format!("{e:#}"))]);
    }
    outcome
}
