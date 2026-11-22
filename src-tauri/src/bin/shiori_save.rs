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
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

use library_store::fsx;
use library_store::notes::{self, split_frontmatter, Frontmatter};
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

// memory・素材の経路が使う、台帳の操作(library_store::registry)への薄い呼び出し。
// これらの経路をlibrary_store::notesへ移したら、なくなる。
fn ensure_project_registered(root: &Path, id: &str) -> Result<()> {
    library_store::registry::ensure_project(root, id)?;
    Ok(())
}

fn normalize_tag(root: &Path, raw: &str, newly_pending: &mut Vec<String>) -> Result<String> {
    Ok(library_store::registry::normalize_tag(root, raw, newly_pending)?)
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
    let is_asset_meta = source.extension().and_then(|e| e.to_str()) == Some(ASSET_META_EXT);
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
        let mut newly_pending_tags = Vec::new();
        fm.tags = fm
            .tags
            .iter()
            .map(|raw| normalize_tag(&root, raw, &mut newly_pending_tags))
            .collect::<Result<Vec<_>>>()?;
        return save_asset(&root, &source, fm, &body, copy_mode, newly_pending_tags);
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
    if fm.kind.as_deref() == Some(KNOWLEDGE_TYPE) {
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

// 一時ファイルに書いてからリネームで置き換える。書き込み途中で止まっても、
// 保存先に中途半端なファイルが残らない。
fn write_atomically(dest: &Path, content: &str) -> Result<()> {
    let file_name = dest
        .file_name()
        .ok_or_else(|| anyhow!("ファイル名の取得に失敗"))?
        .to_string_lossy();
    let tmp = dest.with_file_name(format!(".{file_name}.tmp"));
    if let Err(e) = std::fs::write(&tmp, content) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow!(e).context(format!("{}への書き込みに失敗", tmp.display())));
    }
    if let Err(e) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow!(e).context(format!("{}への置き換えに失敗", dest.display())));
    }
    Ok(())
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

// 棚の記事のtype。library/の4種(VALID_TYPES)とは別物で、library/には置かない。
const KNOWLEDGE_TYPE: &str = "knowledge";
// 棚の下の管理用フォルダ(索引の対象外)。下書きと、上書き前の旧版を置く。
const SHELF_SYSTEM_DIR: &str = "_system";
const SHELF_DRAFTS_DIR: &str = "drafts";
const SHELF_HISTORY_DIR: &str = "history";
const DRAFT_STATUS: &str = "draft";
// 棚の記事に必須の項目。sourceは読む側(indexing.py・shelf.rs)が1行しか読まないため、
// 保存時にSOURCE_SEPARATORで1行へそろえる。
const SHELF_REQUIRED_FIELDS: &[&str] = &["topic", "learned", "source"];
const SOURCE_SEPARATOR: &str = " / ";

// YAMLのスカラー値を文字列として取り出す(learnedが日付・数値として読まれた場合も文字列にする)。
fn yaml_text(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn shelf_field(fm: &Frontmatter, key: &str) -> String {
    fm.extra
        .get(key)
        .and_then(yaml_text)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

// sourceのリスト・複数行の文字列を、空の要素を除いて「 / 」区切りの1行にする。
fn single_line_source(value: &serde_yaml::Value) -> String {
    let parts: Vec<String> = match value {
        serde_yaml::Value::Sequence(items) => items.iter().filter_map(yaml_text).collect(),
        other => yaml_text(other)
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default(),
    };
    parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(SOURCE_SEPARATOR)
}

// 棚に置けるか検証する。memoryと同じく、不備はinboxへ逃がさずエラーにする
// (棚は、ふらるが承認した記事だけを置く場所のため)。
fn validate_shelf(fm: &Frontmatter) -> Result<()> {
    if fm.title.as_deref().map(str::trim).unwrap_or("").is_empty() {
        bail!("titleが空です");
    }
    if fm.tags.is_empty() {
        bail!("tagsが空です");
    }
    for key in SHELF_REQUIRED_FIELDS {
        if shelf_field(fm, key).is_empty() {
            bail!("{key}が空です(知識の棚の記事には{SHELF_REQUIRED_FIELDS:?}が必要です)");
        }
    }
    Ok(())
}

// 棚に置くファイル名。棚の一覧(shelf.rs)は隠しファイルを読まないため、`.`始まりは拒否する。
fn shelf_file_name(source: &Path) -> Result<String> {
    let name = source
        .file_name()
        .ok_or_else(|| anyhow!("ファイル名の取得に失敗"))?
        .to_string_lossy()
        .to_string();
    if !name.ends_with(".md") {
        bail!("知識の棚の記事は.mdにしてください: {name}");
    }
    if name.starts_with('.') {
        bail!(".で始まる名前は棚の一覧に出ないため使えません: {name}");
    }
    Ok(name)
}

// 退避名に付ける旧版の日付。learnedが無ければdateの日付部分、それも無ければunknown。
fn shelf_history_label(old_fm: &Frontmatter) -> String {
    let learned = shelf_field(old_fm, "learned");
    let label = if !learned.is_empty() {
        learned
    } else {
        old_fm
            .date
            .as_deref()
            .map(|d| d.chars().take(10).collect::<String>())
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    };
    label.replace([':', '/', '\\'], "-")
}

// 退避先で既存のファイルと衝突しない名前を返す({名前}-{日付}.md、{名前}-{日付}-2.md…)。
// 2026-10-21に、退避先の同名を確かめずに上書きして仮データを消したことの再発防止。
fn shelf_history_destination(history_dir: &Path, stem: &str, label: &str) -> PathBuf {
    let mut candidate = history_dir.join(format!("{stem}-{label}.md"));
    let mut n = 2;
    while candidate.exists() {
        candidate = history_dir.join(format!("{stem}-{label}-{n}.md"));
        n += 1;
    }
    candidate
}

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

// 知識の棚へ保存する。status: draftは_system/drafts/へ(索引に入れない、同名は上書き)、
// それ以外は棚の直下へ置いて即時に索引へ入れる。正式な記事の上書きでは、旧版を
// _system/history/へ連番付きで退避してから置き換え、作成日時(date)は旧版から引き継ぐ。
fn save_shelf(source: &Path, mut fm: Frontmatter, body: &str) -> Result<()> {
    let shelf = shelf_root();
    if !shelf.is_dir() {
        bail!("知識の棚(knowledge/)が見つかりません: {}", shelf.display());
    }
    if let Some(value) = fm.extra.get("source") {
        let line = single_line_source(value);
        fm.extra.insert("source".to_string(), serde_yaml::Value::String(line));
    }
    validate_shelf(&fm)?;
    let file_name = shelf_file_name(source)?;
    let is_draft = fm.status.trim() == DRAFT_STATUS;

    let dest_dir = if is_draft {
        shelf.join(SHELF_SYSTEM_DIR).join(SHELF_DRAFTS_DIR)
    } else {
        shelf.clone()
    };
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("{}の作成に失敗", dest_dir.display()))?;
    let dest = dest_dir.join(&file_name);
    // 保存先そのものを渡されると、旧版を退避できず、最後の元ファイルの削除で記事が消える。
    if is_same_file(source, &dest) {
        bail!(
            "保存先の記事そのものは渡せません。別の場所(下書きなら_system/drafts/)に書いてから渡してください: {}",
            dest.display()
        );
    }

    let mut history_file = None;
    if !is_draft && dest.is_file() {
        let old_text = std::fs::read_to_string(&dest)
            .with_context(|| format!("{}の読み込みに失敗", dest.display()))?;
        let (old_fm, _) = split_frontmatter(&old_text)
            .with_context(|| format!("既存の記事のfrontmatterを読めません: {}", dest.display()))?;
        fm.date = old_fm.date.clone().or(fm.date);
        let history_dir = shelf.join(SHELF_SYSTEM_DIR).join(SHELF_HISTORY_DIR);
        std::fs::create_dir_all(&history_dir)
            .with_context(|| format!("{}の作成に失敗", history_dir.display()))?;
        let stem = file_name.strip_suffix(".md").unwrap_or(&file_name);
        let path = shelf_history_destination(&history_dir, stem, &shelf_history_label(&old_fm));
        // 退避に失敗したら上書きしない。
        std::fs::write(&path, &old_text)
            .with_context(|| format!("旧版の退避に失敗: {}", path.display()))?;
        history_file = Some(path);
    }

    let frontmatter_yaml = serde_yaml::to_string(&fm).context("frontmatterのYAML化に失敗")?;
    write_atomically(&dest, &format!("---\n{frontmatter_yaml}---\n{body}"))?;
    std::fs::remove_file(source)
        .with_context(|| format!("移動元{}の削除に失敗", source.display()))?;

    if !is_draft {
        reindex_shelf_saved_file(&file_name);
    }

    let result = serde_json::json!({
        "destination": dest.display().to_string(),
        "draft": is_draft,
        "history_file": history_file.map(|p| p.display().to_string()),
    });
    print_result("棚", &result)?;
    Ok(())
}

// ---- 素材の取り込み(詩織Ver3.5) ----

// 素材のサイドカーの拡張子。「X.zip」に対し「X.zip.meta」を置く。
const ASSET_META_EXT: &str = "meta";

// 素材の実体名として拒否する、名前から明らかな秘密情報(中身は検査しない)。
// libraryはMCP経由で外部AIにも読めるため、秘密鍵・環境変数ファイルを置かせない。
fn secret_name_reason(file_name: &str) -> Option<&'static str> {
    let lower = file_name.to_ascii_lowercase();
    if lower.starts_with("id_") && !lower.ends_with(".pub") {
        return Some("SSH秘密鍵(id_*)");
    }
    if lower.ends_with(".pem") || lower.ends_with(".ppk") || lower.ends_with(".key") {
        return Some("秘密鍵・証明書(.pem/.ppk/.key)");
    }
    if lower == ".env" || lower.starts_with(".env.") {
        return Some("環境変数ファイル(.env)");
    }
    if lower == "key.txt" {
        return Some("鍵ファイル(key.txt)");
    }
    None
}

// 素材のtype/projectから保存先の`files/`を決める。mdと違い不備はinboxへ逃がさず
// エラーにする(素材だけがinboxに置かれ、実体と説明の対応が崩れるのを防ぐ)。
fn decide_asset_destination(root: &Path, fm: &Frontmatter) -> Result<PathBuf> {
    if fm.title.as_deref().map(str::trim).unwrap_or("").is_empty() {
        bail!("titleが空です");
    }
    if fm.tags.is_empty() {
        bail!("tagsが空です");
    }
    let project = fm.project.as_deref().map(str::trim).filter(|p| !p.is_empty());
    match fm.kind.as_deref() {
        Some("project") | Some("area") => {
            let p = project.ok_or_else(|| {
                anyhow!("typeがproject/areaの素材にはprojectが必要です(保存先が決まりません)")
            })?;
            ensure_project_registered(root, p)?;
            let top = if fm.kind.as_deref() == Some("project") { "10-projects" } else { "20-areas" };
            Ok(root.join(top).join(p).join("files"))
        }
        Some("resource") => Ok(root.join("30-resources").join("files")),
        Some("profile") => bail!("type: profileには素材を置けません"),
        other => bail!("typeが不正です(値: {other:?}, 素材で許可: project/area/resource)"),
    }
}

fn sha256_of(path: &Path) -> Result<String> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("{}を開けません", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).with_context(|| format!("{}の読み込みに失敗", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

// 保存先で素材・.metaのどちらとも衝突しない名前の組を返す。衝突時は実体と
// .metaに同じ連番を付ける(X.zip → X-2.zip と X-2.zip.meta)。
fn unique_asset_destination(dir: &Path, asset_name: &str) -> (PathBuf, PathBuf) {
    let (stem, ext) = match asset_name.rfind('.') {
        Some(i) if i > 0 => (&asset_name[..i], &asset_name[i..]),
        _ => (asset_name, ""),
    };
    let mut n = 1;
    loop {
        let name = if n == 1 { asset_name.to_string() } else { format!("{stem}-{n}{ext}") };
        let asset = dir.join(&name);
        let meta = dir.join(format!("{name}.{ASSET_META_EXT}"));
        if !asset.exists() && !meta.exists() {
            return (asset, meta);
        }
        n += 1;
    }
}

fn save_asset(
    root: &Path,
    meta_source: &Path,
    mut fm: Frontmatter,
    body: &str,
    copy_mode: bool,
    newly_pending_tags: Vec<String>,
) -> Result<()> {
    // 素材は.metaと同じフォルダに、.metaを除いた同名で置かれている前提。
    let asset_source = meta_source.with_extension("");
    if !asset_source.is_file() {
        bail!("素材が見つかりません(.metaの隣に同名で置いてください): {}", asset_source.display());
    }
    let asset_name = asset_source
        .file_name()
        .ok_or_else(|| anyhow!("素材のファイル名の取得に失敗"))?
        .to_string_lossy()
        .to_string();
    if let Some(reason) = secret_name_reason(&asset_name) {
        bail!("秘密情報の可能性があるため取り込めません({reason}): {asset_name}");
    }

    let dest_dir = decide_asset_destination(root, &fm)?;
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("{}の作成に失敗", dest_dir.display()))?;
    let (asset_dest, meta_dest) = unique_asset_destination(&dest_dir, &asset_name);

    // 移動は「コピー→SHA-256照合→一致したら原本を削除」の順で行う。別ドライブ間
    // でも安全に動かすためで、不一致や途中の失敗では原本を残し、作った分を片付ける。
    let source_hash = sha256_of(&asset_source)?;
    let size = std::fs::metadata(&asset_source)?.len();
    let cleanup = |paths: &[&Path]| {
        for p in paths {
            let _ = std::fs::remove_file(p);
        }
    };
    std::fs::copy(&asset_source, &asset_dest)
        .with_context(|| format!("素材のコピーに失敗: {}", asset_dest.display()))?;
    let dest_hash = match sha256_of(&asset_dest) {
        Ok(h) => h,
        Err(e) => {
            cleanup(&[&asset_dest]);
            return Err(e);
        }
    };
    if dest_hash != source_hash {
        cleanup(&[&asset_dest]);
        bail!("コピー後のSHA-256が一致しません。原本は残しました: {}", asset_source.display());
    }

    // sha256・サイズ・dateはCLIが決定的に書く(人・AIが書いた値は上書きする)。
    fm.date = Some(chrono::Local::now().to_rfc3339());
    fm.extra.insert("sha256".to_string(), serde_yaml::Value::String(source_hash));
    fm.extra.insert("size".to_string(), serde_yaml::Value::Number(size.into()));
    let frontmatter_yaml = serde_yaml::to_string(&fm).context("frontmatterのYAML化に失敗")?;
    let new_content = format!("---\n{frontmatter_yaml}---\n{body}");
    if let Err(e) = std::fs::write(&meta_dest, new_content) {
        cleanup(&[&asset_dest, &meta_dest]);
        return Err(anyhow!(e).context(format!("{}への書き込みに失敗", meta_dest.display())));
    }

    if !copy_mode {
        std::fs::remove_file(&asset_source)
            .with_context(|| format!("移動元{}の削除に失敗", asset_source.display()))?;
        std::fs::remove_file(meta_source)
            .with_context(|| format!("移動元{}の削除に失敗", meta_source.display()))?;
    }

    reindex_saved_file(root, &meta_dest);

    let result = serde_json::json!({
        "destination": meta_dest.display().to_string(),
        "asset_destination": asset_dest.display().to_string(),
        "inbox_reason": serde_json::Value::Null,
        "newly_pending_tags": newly_pending_tags,
    });
    print_result("素材", &result)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 秘密情報の名前は拒否し公開鍵は通す() {
        for name in ["id_ed25519", "id_rsa", "server.pem", "putty.PPK", "a.key", ".env", ".env.local", "key.txt"] {
            assert!(secret_name_reason(name).is_some(), "{name}は拒否されるべき");
        }
        for name in ["id_ed25519.pub", "report.pdf", "environment.zip", "monkey.txt", "pc-backup.zip"] {
            assert!(secret_name_reason(name).is_none(), "{name}は通すべき");
        }
    }

    #[test]
    fn 同名があれば実体とmetaに同じ連番を付ける() {
        let dir = std::env::temp_dir().join(format!("shiori-save-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (asset, meta) = unique_asset_destination(&dir, "a.tar.gz");
        assert_eq!(asset.file_name().unwrap(), "a.tar.gz");
        assert_eq!(meta.file_name().unwrap(), "a.tar.gz.meta");
        // 実体が無くても.metaだけ残っていれば衝突として扱う。
        std::fs::write(&meta, "x").unwrap();
        let (asset2, meta2) = unique_asset_destination(&dir, "a.tar.gz");
        assert_eq!(asset2.file_name().unwrap(), "a.tar-2.gz");
        assert_eq!(meta2.file_name().unwrap(), "a.tar-2.gz.meta");
        std::fs::remove_dir_all(&dir).unwrap();
    }

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

    fn shelf_fm(yaml: &str) -> Frontmatter {
        split_frontmatter(&format!("---\n{yaml}\n---\n本文\n")).unwrap().0
    }

    #[test]
    fn 棚のsourceはリストも複数行も1行にそろえる() {
        let fm = shelf_fm("source:\n- https://a.example/x\n- ''\n- 'Keisler, Foundations'");
        assert_eq!(
            single_line_source(&fm.extra["source"]),
            "https://a.example/x / Keisler, Foundations"
        );
        let fm = shelf_fm("source: |\n  教科書A\n\n  https://b.example\n");
        assert_eq!(single_line_source(&fm.extra["source"]), "教科書A / https://b.example");
        let fm = shelf_fm("source: 'そのまま 1行'");
        assert_eq!(single_line_source(&fm.extra["source"]), "そのまま 1行");
    }

    #[test]
    fn 長いsourceも書き出しで折り返さない() {
        let long = vec!["https://example.com/a very long title with spaces"; 6].join(SOURCE_SEPARATOR);
        let mut fm = shelf_fm("title: t");
        fm.extra.insert("source".to_string(), serde_yaml::Value::String(long.clone()));
        let yaml = serde_yaml::to_string(&fm).unwrap();
        let line = yaml.lines().find(|l| l.starts_with("source:")).unwrap();
        assert!(line.contains("example.com/a very long title with spaces / https"));
        assert!(!yaml.lines().any(|l| l.starts_with("  ")), "折り返しの継続行がある: {yaml}");
    }

    #[test]
    fn 棚の必須項目が欠けたら拒否する() {
        let ok = "title: t\ntype: knowledge\ntags:\n- 数学\ntopic: 数学\nlearned: 2026-10-23\nsource: 教科書";
        assert!(validate_shelf(&shelf_fm(ok)).is_ok());
        for missing in ["topic", "learned", "source"] {
            let yaml: String = ok
                .lines()
                .filter(|l| !l.starts_with(&format!("{missing}:")))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(validate_shelf(&shelf_fm(&yaml)).is_err(), "{missing}が無いのに通った");
        }
        assert!(validate_shelf(&shelf_fm(&ok.replace("title: t", "title: ''"))).is_err());
        assert!(validate_shelf(&shelf_fm(&ok.replace("tags:\n- 数学", "tags: []"))).is_err());
    }

    #[test]
    fn 棚の退避名はlearnedかdateの日付を使う() {
        assert_eq!(shelf_history_label(&shelf_fm("learned: '2026-10-21'")), "2026-10-21");
        assert_eq!(
            shelf_history_label(&shelf_fm("date: 2026-10-17T15:00:55+09:00")),
            "2026-10-17"
        );
        assert_eq!(shelf_history_label(&shelf_fm("title: t")), "unknown");
    }

    #[test]
    fn 棚の退避先に同名があれば連番を付ける() {
        let dir = std::env::temp_dir().join(format!("shiori-save-shelf-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let first = shelf_history_destination(&dir, "math-hyperreal", "2026-10-17");
        assert_eq!(first.file_name().unwrap(), "math-hyperreal-2026-10-17.md");
        std::fs::write(&first, "既存").unwrap();
        let second = shelf_history_destination(&dir, "math-hyperreal", "2026-10-17");
        assert_eq!(second.file_name().unwrap(), "math-hyperreal-2026-10-17-2.md");
        std::fs::write(&second, "既存").unwrap();
        let third = shelf_history_destination(&dir, "math-hyperreal", "2026-10-17");
        assert_eq!(third.file_name().unwrap(), "math-hyperreal-2026-10-17-3.md");
        // 既存の退避は書き換えない。
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "既存");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn 棚のファイル名はmdで隠しファイルでないこと() {
        assert_eq!(shelf_file_name(Path::new("x/math-a.md")).unwrap(), "math-a.md");
        assert!(shelf_file_name(Path::new("x/math-a.txt")).is_err());
        assert!(shelf_file_name(Path::new("x/.math-a.md")).is_err());
    }

}
