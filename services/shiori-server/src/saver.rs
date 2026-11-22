//! 保存の振り分け。`shiori-save` が手元でやっていた判断(素材・memory・棚・記事)を、サーバーで行う。
//! 規則と書き込みの中身は、すべて `library_store`。ここは、どの経路に入れるかを決めて、結果をJSONにする。

use chrono::{DateTime, FixedOffset};
use library_store::journal::AppendRequest;
use library_store::layout::COMMON_PROJECT;
use library_store::{assets, journal, notes, shelf, Layout, StoreError};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct Roots {
    pub library: PathBuf,
    pub knowledge: PathBuf,
    pub tasks: PathBuf,
}

pub struct SaveReq {
    pub file_name: String,
    /// frontmatter+本文(素材のときは、`.meta` の中身)
    pub content: String,
    pub expect_updated: Option<String>,
    /// 先に送った素材の実体の識別子(SHA-256)。素材の取り込みのときだけ。
    pub asset_upload: Option<String>,
}

/// 保存のあとに、索引へ入れ直す対象。
#[derive(Debug, PartialEq, Eq)]
pub enum Reindex {
    /// libraryからの相対パス
    Library(String),
    /// 棚の中のファイル名(棚からの相対パス)
    Shelf(String),
}

/// 保存する。`device` は、書いた端末(トークンの名前)。memoryの整理で外した項目を、日誌へ記録するときの見出しに使う。
pub fn save(
    roots: &Roots,
    layout: Layout,
    uploads: &Path,
    req: &SaveReq,
    device: &str,
    now: &DateTime<FixedOffset>,
) -> Result<(Value, Vec<Reindex>), StoreError> {
    let now_s = now.to_rfc3339();
    let (mut fm, body) = notes::split_frontmatter(&req.content)?;
    // dateは常にサーバーの時刻で上書きする(AIが書いた値は信用しない)
    fm.date = Some(now_s.clone());

    // 素材: `.meta` と、先に送った実体
    if let Some(sha) = &req.asset_upload {
        let asset_name = req
            .file_name
            .strip_suffix(&format!(".{}", assets::ASSET_META_EXT))
            .ok_or_else(|| StoreError::Invalid("素材の取り込みは、.metaのファイル名で送ってください".into()))?;
        if !sha.chars().all(|c| c.is_ascii_hexdigit()) || sha.len() != 64 {
            return Err(StoreError::Invalid("asset_uploadがSHA-256ではありません".into()));
        }
        if asset_name.contains(['/', '\\']) || asset_name.is_empty() {
            return Err(StoreError::Invalid("素材の名前が正しくありません".into()));
        }
        let dir = uploads.join(sha);
        let source = dir.join(asset_name);
        if !source.is_file() {
            return Err(StoreError::NotFound("先に素材の実体を送ってください(upload)".into()));
        }
        let saved = assets::save_asset_with(layout, &roots.library, &source, fm, &body, &now_s)?;
        let _ = std::fs::remove_dir_all(&dir);
        let body = json!({
            "kind": "素材",
            "destination": format!("library/{}", saved.meta_rel),
            "asset_destination": format!("library/{}", saved.asset_rel),
            "inbox_reason": Value::Null,
            "newly_pending_tags": saved.newly_pending_tags,
            "sha256": saved.sha256,
            "size": saved.size,
        });
        return Ok((body, vec![Reindex::Library(saved.meta_rel)]));
    }
    if req.file_name.ends_with(&format!(".{}", assets::ASSET_META_EXT)) {
        return Err(StoreError::Invalid("素材の取り込みには、実体を先に送ってください(upload)".into()));
    }

    // 共通記憶MD
    if fm.sub_kind.as_deref() == Some(notes::MEMORY_KIND) {
        let saved = notes::save_memory_with(layout, &roots.library, fm, &body, req.expect_updated.as_deref(), &now_s)?;
        // 書き方の規則の警告(V7)。v6.2の並びでは、これまでどおりの上限の警告(文字列)
        let mut warnings: Vec<Value> = saved.warnings.iter().map(|w| serde_json::to_value(w).unwrap_or(Value::Null)).collect();
        if let Some((lines, chars)) = saved.over_limit {
            warnings.push(json!(notes::over_limit_warning(lines, chars)));
        }
        let mut reindex = vec![Reindex::Library(saved.rel.clone())];

        // 外された項目は、そのプロジェクトの日誌へ、節として記録する(整理で、残したい情報が失われないように)
        let mut recorded = Value::Null;
        if !saved.removed_items.is_empty() {
            let project = if saved.project == notes::MEMORY_GLOBAL_KEY { COMMON_PROJECT } else { saved.project.as_str() };
            let content = format!("### memoryから外した項目\n\n{}\n", saved.removed_items.join("\n"));
            let append = AppendRequest { project, content: &content, device, ai: Some("memory整理"), summary: None };
            recorded = match journal::append(&roots.library, layout, &append, now) {
                Ok(a) => {
                    reindex.push(Reindex::Library(a.rel.clone()));
                    json!({ "path": format!("library/{}", a.rel), "heading": a.heading })
                }
                // 記録に失敗しても、memoryの保存は済んでいる。失敗は知らせる
                Err(e) => json!({ "error": e.to_string() }),
            };
        }
        let body = json!({
            "kind": "memory",
            "destination": format!("library/{}", saved.rel),
            "updated": saved.updated,
            "history_file": saved.history_rel.as_ref().map(|h| format!("library/{h}")),
            "inbox_reason": Value::Null,
            "newly_pending_tags": saved.newly_pending_tags,
            "warnings": warnings,
            "size": { "lines": saved.size.0, "chars": saved.size.1 },
            "removed_items": saved.removed_items.len(),
            "removed_recorded_to": recorded,
            "rev": saved.rev,
        });
        return Ok((body, reindex));
    }
    if req.expect_updated.is_some() {
        return Err(StoreError::Invalid("expect_updatedはkind: memoryの更新専用です".into()));
    }

    // 知識の棚
    if fm.kind.as_deref() == Some(shelf::KNOWLEDGE_TYPE) {
        let saved = shelf::save_shelf(&roots.knowledge, &req.file_name, fm, &body)?;
        let reindex = if saved.draft { Vec::new() } else { vec![Reindex::Shelf(saved.rel.clone())] };
        let body = json!({
            "kind": "棚",
            "destination": format!("knowledge/{}", saved.rel),
            "draft": saved.draft,
            "history_file": saved.history_rel.as_ref().map(|h| format!("knowledge/{h}")),
            "rev": saved.rev,
        });
        return Ok((body, reindex));
    }

    // 記事。日誌は、V7では追記だけ(1作業1ファイルの新規作成は、日付の名前の重複で連番がつく事故になる)
    if layout == Layout::V7 && fm.sub_kind.as_deref() == Some("journal") {
        return Err(StoreError::Invalid("日誌は shiori-save --journal で追記してください(日付ごとの1ファイルに、節として足されます)".into()));
    }
    let saved = notes::save_article_with(layout, &roots.library, &req.file_name, fm, &body)?;
    let body = json!({
        "kind": "記事",
        "destination": format!("library/{}", saved.rel),
        "inbox_reason": saved.inbox_reason,
        "newly_pending_tags": saved.newly_pending_tags,
        "rev": saved.rev,
    });
    Ok((body, vec![Reindex::Library(saved.rel)]))
}
