//! 詩織Ver2.0(セカンドブレイン化)の保存CLI(設計指示書v3、7章)。
//!
//! Claude Codeはlibrary/へ直接ファイル操作で書き込めるが、配置先フォルダの
//! 判定・タグ/プロジェクトの正規化はLLMの裁量に任せず、このCLIが決定的な
//! ロジックとして担う(library/_system/CLAUDE.mdの配置ルールをそのまま実装)。
//!
//! 使用法: `shiori-save <mdファイルのパス>`
//!         `shiori-save [--copy] <素材>.meta`(素材ファイルの取り込み、詩織Ver3.5)
//!         `shiori-save [--expect-updated <updated>] <memory.md>`(共通記憶MD、詩織Ver4.1)
//!         `shiori-save <type: knowledgeの記事>`(知識の棚、詩織Ver5.0候補。保存先はlibrary/ではなくknowledge/)
//! 対象ファイルにはfrontmatter(title/type/tags/project/author)が付与済みで
//! あることを前提とする。実行後、対象ファイルはlibrary/配下の決定先へ
//! 移動される(コピーではなく移動、元の場所には残らない)。frontmatterが
//! 不備な場合も保存自体は諦めず、00-inbox/へreasonフィールド付きで置く。
//!
//! 素材(zip・png・pdf等)は、同名に`.meta`を付けたサイドカー(frontmatter+本文)
//! とペアで置き、`.meta`を指定して取り込む。素材は`files/`へ移動(コピー→SHA-256
//! 照合→原本削除)する。mdと違い、不備があっても00-inbox/へ逃がさずエラーにする
//! (素材だけがinboxに置かれると、実体と説明の対応が崩れるため)。

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};

use library_store::assets;
use library_store::fsx;
use library_store::notes::{self, split_frontmatter, Frontmatter};
use library_store::shelf;
use shiori_folio_lib::shiori_log;
use shiori_folio_lib::{library_root, load_search_backend_config, project_root, rag_client, shelf_root};

fn main() -> Result<()> {
    let root = project_root();
    shiori_log::init(
        &root.join("data").join("logs"),
        shiori_log::LogKind::Save,
        &shiori_log::LogSettings::from_config(&root),
    );
    let args: Vec<String> = std::env::args().skip(1).collect();
    shiori_log::info("SAVE", "始めた", &[("引数", &args.join(" "))]);
    let outcome = run();
    if let Err(e) = &outcome {
        shiori_log::error("SAVE", "失敗した", &[("理由", &format!("{e:#}"))]);
    }
    outcome
}

/// 保存の結果を標準出力にJSONで出し、ログにも残す。
fn print_result(route: &str, result: &serde_json::Value) -> Result<()> {
    let dest = result.get("destination").and_then(|v| v.as_str()).unwrap_or("");
    shiori_log::info("SAVE", "保存した", &[("種類", &route), ("先", &dest)]);
    println!("{}", serde_json::to_string_pretty(result)?);
    Ok(())
}

fn run() -> Result<()> {
    let mut copy_mode = false;
    let mut expect_updated = None;
    let mut source_arg = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--copy" {
            copy_mode = true;
        } else if arg == "--expect-updated" {
            expect_updated = Some(
                args.next()
                    .ok_or_else(|| anyhow!("--expect-updatedには、読んだ時点のupdatedの値が必要です"))?,
            );
        } else if source_arg.is_none() {
            source_arg = Some(arg);
        } else {
            bail!("引数が多すぎます: {arg}");
        }
    }
    let source = source_arg
        .ok_or_else(|| anyhow!("使用法: shiori-save [--copy] <mdファイル または 素材.meta のパス>"))?;
    let source = PathBuf::from(source);
    if !source.is_file() {
        bail!("ファイルが見つかりません: {}", source.display());
    }
    let is_asset_meta = source.extension().and_then(|e| e.to_str()) == Some(assets::ASSET_META_EXT);
    if copy_mode && !is_asset_meta {
        bail!("--copyは素材の.meta取り込み専用です");
    }

    let root = library_root();
    if !root.is_dir() {
        bail!("詩織のSSDが接続されていません(library/が見つかりません)");
    }

    let content = std::fs::read_to_string(&source)
        .with_context(|| format!("{}の読み込みに失敗", source.display()))?;
    let (mut fm, body) = split_frontmatter(&content)?;

    // dateは常にCLI実行時刻で上書きする(Claude Codeが書いた値があっても
    // 信用しない。理由はFrontmatter構造体のコメント参照)。
    fm.date = Some(chrono::Local::now().to_rfc3339());

    if is_asset_meta {
        return save_asset(&root, &source, fm, &body, copy_mode);
    }

    // 共通記憶MDは、固定の保存先への丸ごと上書き・競合検知・旧版退避という
    // 通常記事と異なる流れを持つため、専用の経路で処理する。
    if fm.sub_kind.as_deref() == Some(notes::MEMORY_KIND) {
        return save_memory(&root, &source, fm, &body, expect_updated);
    }
    if expect_updated.is_some() {
        bail!("--expect-updatedはkind: memoryの更新専用です");
    }

    // 知識の棚の記事は、libraryの配置ルール・タグの台帳を使わず、棚へ置く専用の経路で処理する。
    if fm.kind.as_deref() == Some(shelf::KNOWLEDGE_TYPE) {
        return save_shelf(&source, fm, &body);
    }

    // 記事の規則(タグの正規化・検証・配置先の決定・同名の連番)と書き込みは、library_store::notes。
    let file_name = source
        .file_name()
        .ok_or_else(|| anyhow!("ファイル名の取得に失敗"))?
        .to_string_lossy()
        .to_string();
    let saved = notes::save_article(&root, &file_name, fm, &body)?;
    let dest_path = saved.path;
    let inbox_reason = saved.inbox_reason;
    let newly_pending_tags = saved.newly_pending_tags;
    std::fs::remove_file(&source)
        .with_context(|| format!("移動元{}の削除に失敗", source.display()))?;

    reindex_saved_file(&root, &dest_path);

    let result = serde_json::json!({
        "destination": dest_path.display().to_string(),
        "inbox_reason": inbox_reason,
        "newly_pending_tags": newly_pending_tags,
    });
    print_result("記事", &result)?;
    Ok(())
}

// 書き込み時フック(詩織Ver3.0、データ管理法見直し2-5節)。RAGサーバーが
// すでに起動していれば即座にこのファイルだけre-indexし、検索の都度の
// 全件スキャンを待たず保存直後から検索対象にする。未起動時はここで
// 起動を試みない(起動待ちでCLIの応答を遅らせないため)。次回のRAGサーバー
// 起動時に行われる全件mtimeスキャンが安全網として拾う。
fn reindex_saved_file(root: &Path, dest_path: &Path) {
    let Ok(relative) = dest_path.strip_prefix(root) else {
        return;
    };
    let relative_str = relative.to_string_lossy();
    match load_search_backend_config(&project_root()) {
        Ok(backend) if rag_client::is_running(backend.rag_port) => {
            if let Err(e) = rag_client::reindex_file(backend.rag_port, &relative_str) {
                eprintln!(
                    "警告: 即時re-indexに失敗しました({e})。次回のRAGサーバー起動時に反映されます。"
                );
                shiori_log::warn("SAVE", "即時の索引に失敗した", &[("理由", &e)]);
            }
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("警告: 検索バックエンド設定の読み込みに失敗しました({e})。");
            shiori_log::warn("SAVE", "検索の設定を読めない", &[("理由", &e)]);
        }
    }
}

// 共通記憶MDを保存する。規則(保存先・競合検知・旧版の退避・上限の警告)と書き込みは
// library_store::notes::save_memory。ここは、元ファイルとの同一確認、元ファイルの削除、
// 索引への反映、結果の出力だけを担う。
fn save_memory(
    root: &Path,
    source: &Path,
    fm: Frontmatter,
    body: &str,
    expect_updated: Option<String>,
) -> Result<()> {
    // 保存先のmemoryそのものを渡されると、新しい内容を書いた直後に元ファイルの削除で消える
    // (save_shelfと同じ確認。何かを書く前に断る)。
    let (rel, _) = notes::memory_target(&fm)?;
    let dest = fsx::resolve(root, &rel)?;
    if is_same_file(source, &dest) {
        bail!(
            "保存先のmemoryそのものは渡せません。作業用の場所へコピーして書き直してから渡してください: {}",
            dest.display()
        );
    }
    let now = chrono::Local::now().to_rfc3339();
    let saved = notes::save_memory(root, fm, body, expect_updated.as_deref(), &now)?;

    std::fs::remove_file(source)
        .with_context(|| format!("移動元{}の削除に失敗", source.display()))?;
    if let Some(e) = &saved.prune_error {
        eprintln!("警告: 古い退避の整理に失敗しました({e})。");
        shiori_log::warn("SAVE", "古い退避の整理に失敗した", &[("理由", e)]);
    }

    let mut warnings = Vec::new();
    if let Some((lines, chars)) = saved.over_limit {
        let w = notes::over_limit_warning(lines, chars);
        eprintln!("警告: {w}");
        shiori_log::warn("SAVE", "memoryが上限を超えた", &[("行", &lines), ("字", &chars)]);
        warnings.push(w);
    }

    reindex_saved_file(root, &saved.path);

    let history_file = match &saved.history_rel {
        Some(h) => Some(fsx::resolve(root, h)?.display().to_string()),
        None => None,
    };
    let result = serde_json::json!({
        "destination": saved.path.display().to_string(),
        "updated": saved.updated,
        "history_file": history_file,
        "inbox_reason": serde_json::Value::Null,
        "newly_pending_tags": saved.newly_pending_tags,
        "warnings": warnings,
    });
    print_result("memory", &result)?;
    Ok(())
}

// ---- 知識の棚(詩織Ver5.0候補) ----

// 同じファイルを指しているか(棚の記事そのものを渡された場合の検出用)。
fn is_same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

// RAGが起動中なら、棚の記事1件だけを即時に索引へ入れる(reindex_saved_fileの棚版)。
fn reindex_shelf_saved_file(file_name: &str) {
    match load_search_backend_config(&project_root()) {
        Ok(backend) if rag_client::is_running(backend.rag_port) => {
            if let Err(e) = rag_client::reindex_shelf_file(backend.rag_port, file_name) {
                eprintln!(
                    "警告: 棚の即時re-indexに失敗しました({e})。次回のRAGサーバー起動時に反映されます。"
                );
                shiori_log::warn("SAVE", "棚の即時の索引に失敗した", &[("理由", &e)]);
            }
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("警告: 検索バックエンド設定の読み込みに失敗しました({e})。");
            shiori_log::warn("SAVE", "検索の設定を読めない", &[("理由", &e)]);
        }
    }
}

// 知識の棚へ保存する。規則(検証・下書きの扱い・旧版の連番つき退避・dateの引き継ぎ)と書き込みは
// library_store::shelf::save_shelf。ここは、元ファイルとの同一確認、元ファイルの削除、
// 索引への反映、結果の出力だけを担う。
fn save_shelf(source: &Path, fm: Frontmatter, body: &str) -> Result<()> {
    let shelf = shelf_root();
    if !shelf.is_dir() {
        bail!("知識の棚(knowledge/)が見つかりません: {}", shelf.display());
    }
    let file_name = source
        .file_name()
        .ok_or_else(|| anyhow!("ファイル名の取得に失敗"))?
        .to_string_lossy()
        .to_string();
    shelf::check_file_name(&file_name)?;
    let is_draft = fm.status.trim() == shelf::DRAFT_STATUS;
    // 保存先そのものを渡されると、旧版を退避できず、最後の元ファイルの削除で記事が消える。
    let dest = fsx::resolve(&shelf, &shelf::destination_rel(&file_name, is_draft))?;
    if is_same_file(source, &dest) {
        bail!(
            "保存先の記事そのものは渡せません。別の場所(下書きなら_system/drafts/)に書いてから渡してください: {}",
            dest.display()
        );
    }
    let saved = shelf::save_shelf(&shelf, &file_name, fm, body)?;
    std::fs::remove_file(source)
        .with_context(|| format!("移動元{}の削除に失敗", source.display()))?;

    if !saved.draft {
        reindex_shelf_saved_file(&file_name);
    }

    let history_file = match &saved.history_rel {
        Some(h) => Some(fsx::resolve(&shelf, h)?.display().to_string()),
        None => None,
    };
    let result = serde_json::json!({
        "destination": saved.path.display().to_string(),
        "draft": saved.draft,
        "history_file": history_file,
    });
    print_result("棚", &result)?;
    Ok(())
}

// ---- 素材の取り込み(詩織Ver3.5) ----

// 素材を取り込む。規則(秘密情報の名前の拒否・保存先の決定・SHA-256照合つきのコピー・.metaの書き込み)は
// library_store::assets::save_asset。ここは、.metaの隣の実体の確認、原本の削除(--copyでなければ)、
// 索引への反映、結果の出力だけを担う。
fn save_asset(
    root: &Path,
    meta_source: &Path,
    fm: Frontmatter,
    body: &str,
    copy_mode: bool,
) -> Result<()> {
    // 素材は.metaと同じフォルダに、.metaを除いた同名で置かれている前提。
    let asset_source = meta_source.with_extension("");
    if !asset_source.is_file() {
        bail!("素材が見つかりません(.metaの隣に同名で置いてください): {}", asset_source.display());
    }
    let now = chrono::Local::now().to_rfc3339();
    let saved = assets::save_asset(root, &asset_source, fm, body, &now)?;

    if !copy_mode {
        std::fs::remove_file(&asset_source)
            .with_context(|| format!("移動元{}の削除に失敗", asset_source.display()))?;
        std::fs::remove_file(meta_source)
            .with_context(|| format!("移動元{}の削除に失敗", meta_source.display()))?;
    }

    reindex_saved_file(root, &saved.meta_path);

    let result = serde_json::json!({
        "destination": saved.meta_path.display().to_string(),
        "asset_destination": saved.asset_path.display().to_string(),
        "inbox_reason": serde_json::Value::Null,
        "newly_pending_tags": saved.newly_pending_tags,
    });
    print_result("素材", &result)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm_of(kind: &str, project: Option<&str>) -> Frontmatter {
        Frontmatter {
            kind: Some(kind.to_string()),
            project: project.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn 保存先のmemoryそのものを渡すと何も変えずに断る() {
        let root = std::env::temp_dir().join(format!("shiori-save-memself-{}", std::process::id()));
        let dest = root.join("20-areas").join("shiori").join("memory.md");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let original = "---\ntitle: t\nkind: memory\nupdated: u1\n---\n本文\n";
        std::fs::write(&dest, original).unwrap();
        let fm = Frontmatter {
            title: Some("t".to_string()),
            tags: vec!["詩織".to_string()],
            ..fm_of("area", Some("shiori"))
        };
        let result = save_memory(
            &root,
            &dest,
            fm,
            "新しい本文\n",
            Some("u1".to_string()),
        );
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), original);
        assert!(!root.join("_system").exists(), "旧版の退避も行わない");
        std::fs::remove_dir_all(&root).unwrap();
    }

}
