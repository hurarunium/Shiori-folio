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
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use shiori_folio_lib::shiori_log;
use shiori_folio_lib::{library_root, load_search_backend_config, project_root, rag_client, shelf_root};

// 詩織Ver3.1(journal廃止・project/area配下へのkind統合、2026-09-16)。
// journalをtypeから廃止し、project/areaそれぞれの配下にjournal/resourceの
// 2種類(kindフィールド)を持たせる形に変更した。resourceは「project/area
// に紐づかない外部由来の知識」専用に純化し、ふらるさん自身についての記録は
// 新設のprofileへ独立させた。
const VALID_TYPES: &[&str] = &["project", "area", "resource", "profile"];
// type: project/areaの記事だけが持つ、記事の性質(作業ログか、そこから
// 生まれた意思決定・知見か)。type: resource/profileには存在しない
// (存在してもdecide_destinationでは参照しない)。
const VALID_KINDS: &[&str] = &["journal", "resource"];
// 詩織Ver3.2(40-profile/のサブフォルダ細分化、2026-09-21)。type: profileの
// 記事だけが持つ分類。未指定を許容する点がkindと異なり、profile-huraru.md
// のような「プロフィール全体の入り口」的な記事はcategoryなしのまま
// 40-profile/直下に置かれる。
const VALID_CATEGORIES: &[&str] = &[
    "temperament-and-thinking",
    "values",
    "life-history",
    "relationships",
    "self-image",
    "daily-and-work",
];

// 詩織Ver4.1(共通記憶MD、2026-10-10)。kind: memoryは「いま有効な状態・やること・
// 申し送り」を持つ丸ごと上書き型の記事で、通常記事のように連番を付けて積み上げず、
// 保存先のファイル名も固定する(プロジェクトにつき1ファイル)。
const MEMORY_KIND: &str = "memory";
const MEMORY_FILE_NAME: &str = "memory.md";
const MEMORY_GLOBAL_FILE_NAME: &str = "memory-global.md";
// 上書き前の旧版を退避する先(library/_system/配下は索引から除外される)。
const MEMORY_HISTORY_DIR: &str = "memory-history";
const MEMORY_HISTORY_GENERATIONS: usize = 3;
// 退避ファイル名でproject idと区別するための、全体用memoryの識別子。
const MEMORY_GLOBAL_KEY: &str = "_global";
// 上限(本文だけ。frontmatterは数えない)。超えても保存は止めず、警告で整理を促す。
// frontmatterを外すのは、`related` が記録の増えるほど伸び、整理しても減らせないため(2026-10-24、ふらる)。
const MEMORY_MAX_LINES: usize = 200;
const MEMORY_MAX_CHARS: usize = 6000;

/// memoryの本文の行数と字数が上限を超えていれば、その行数と字数を返す。
fn memory_over_limit(body: &str) -> Option<(usize, usize)> {
    let lines = body.lines().count();
    let chars = body.chars().count();
    (lines > MEMORY_MAX_LINES || chars > MEMORY_MAX_CHARS).then_some((lines, chars))
}

fn default_index() -> bool {
    true
}

fn default_status() -> String {
    "new".to_string()
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct Frontmatter {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    // 詩織Ver3.1で新設。type: project/areaの記事のみ使う(journal | resource)。
    // YAML上のフィールド名は"kind"だが、上のkind(YAML上の"type")と紛らわしい
    // ためRust側の変数名はsub_kindとした。
    #[serde(rename = "kind", skip_serializing_if = "Option::is_none")]
    sub_kind: Option<String>,
    // 詩織Ver3.2で新設。type: profileの記事のみ使う(6分類の列挙、未指定可)。
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    // 本文とは別の1〜2文の要約(検索結果表示・rerank精度向上に使う)。
    // 必須化はしない(無い場合は単に空欄のまま)。
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    // 検索対象に含めるか(services/rag/indexing.py側で参照する)。デフォルトtrue。
    #[serde(default = "default_index")]
    index: bool,
    // draft/new/outdated/deprecated/disputedの5値を想定するが、typeと違い
    // 判断を誤っても実害が小さいためCLI側でのenumバリデーションはしない。
    #[serde(default = "default_status")]
    status: String,
    // 内部的に関連する他記事へのリンク(将来のリンクグラフの元データ)。
    #[serde(default)]
    related: Vec<String>,
    // 記事の作成日時(ISO8601)。詩織Ver3.3で新設。Claude Codeが書いた値が
    // あっても信用せず、保存の都度このCLIが現在時刻で上書きする(type/kind
    // の判定と同じく、日時のような機械的に決まる値はAIの裁量に委ねず決定的に
    // 付与する。手動入力によるズレ・付け忘れの防止が目的)。
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<String>,
    // 詩織Ver4.1で新設。kind: memoryだけが持つ最終更新日時。dateと同じく
    // 保存の都度このCLIが現在時刻を書き、競合検知(--expect-updated)の基準になる。
    #[serde(skip_serializing_if = "Option::is_none")]
    updated: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    // 上記以外のfrontmatterフィールドは検証・ルーティングの対象外だが、
    // 保存時に消さず維持する。
    #[serde(flatten)]
    extra: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Debug, Deserialize)]
struct ProjectsFile {
    #[serde(default)]
    projects: Vec<ProjectEntry>,
}

#[derive(Debug, Deserialize)]
struct ProjectEntry {
    id: String,
}

#[derive(Debug, Deserialize)]
struct TagsFile {
    #[serde(default)]
    tags: Vec<TagEntry>,
}

#[derive(Debug, Deserialize)]
struct TagEntry {
    canonical: String,
    #[serde(default)]
    aliases: Vec<String>,
}

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
    let is_asset_meta = source.extension().and_then(|e| e.to_str()) == Some(ASSET_META_EXT);
    if copy_mode && !is_asset_meta {
        bail!("--copyは素材の.meta取り込み専用です");
    }

    let root = library_root();
    if !root.is_dir() {
        bail!("詩織のSSDが接続されていません(library/が見つかりません)");
    }
    let system_dir = root.join("_system");
    let projects_yaml = system_dir.join("projects.yaml");
    let tags_yaml = system_dir.join("tags.yaml");

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
            .map(|raw| normalize_tag(&tags_yaml, raw, &mut newly_pending_tags))
            .collect::<Result<Vec<_>>>()?;
        return save_asset(&root, &projects_yaml, &source, fm, &body, copy_mode, newly_pending_tags);
    }

    // 共通記憶MDは、固定の保存先への丸ごと上書き・競合検知・旧版退避という
    // 通常記事と異なる流れを持つため、専用の経路で処理する。
    if fm.sub_kind.as_deref() == Some(MEMORY_KIND) {
        return save_memory(&root, &projects_yaml, &tags_yaml, &source, fm, &body, expect_updated);
    }
    if expect_updated.is_some() {
        bail!("--expect-updatedはkind: memoryの更新専用です");
    }

    // 知識の棚の記事は、libraryの配置ルール・タグの台帳を使わず、棚へ置く専用の経路で処理する。
    if fm.kind.as_deref() == Some(KNOWLEDGE_TYPE) {
        return save_shelf(&source, fm, &body);
    }

    // タグは常に正規化する(inbox行きになる場合でも、表記ゆれの統一自体は
    // 後で見返したときに有用なため)。
    let mut newly_pending_tags = Vec::new();
    fm.tags = fm
        .tags
        .iter()
        .map(|raw| normalize_tag(&tags_yaml, raw, &mut newly_pending_tags))
        .collect::<Result<Vec<_>>>()?;

    let (dest_dir, inbox_reason) = match validate(&fm) {
        Some(reason) => {
            fm.reason = Some(reason.clone());
            (root.join("00-inbox"), Some(reason))
        }
        None => (decide_destination(&root, &fm, &projects_yaml)?, None),
    };

    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("{}の作成に失敗", dest_dir.display()))?;
    let dest_path = unique_destination(&dest_dir, &source)?;

    let frontmatter_yaml = serde_yaml::to_string(&fm).context("frontmatterのYAML化に失敗")?;
    let new_content = format!("---\n{frontmatter_yaml}---\n{body}");
    std::fs::write(&dest_path, new_content)
        .with_context(|| format!("{}への書き込みに失敗", dest_path.display()))?;
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

// frontmatterの必須項目を検証する。不備があれば理由文字列を返す
// (Noneなら正常、呼び出し側はdecide_destinationへ進む)。
fn validate(fm: &Frontmatter) -> Option<String> {
    if fm.title.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Some("titleが空です".to_string());
    }
    if fm.tags.is_empty() {
        return Some("tagsが空です".to_string());
    }
    match fm.kind.as_deref() {
        Some(k) if VALID_TYPES.contains(&k) => {
            // project/areaはkind(journal|resource)も必須。resource/profileには
            // 存在しない概念なので、指定されていても無視する(バリデーション
            // 対象外)。
            if k == "project" || k == "area" {
                match fm.sub_kind.as_deref() {
                    Some(sk) if VALID_KINDS.contains(&sk) => None,
                    other => Some(format!(
                        "kindが不正です(値: {other:?}, 許可値: {VALID_KINDS:?}。typeがproject/areaの記事には必須です)"
                    )),
                }
            } else if k == "profile" {
                // categoryは未指定を許容する(profile-huraru.md想定)。
                // 指定されている場合のみ値の妥当性を検証する。
                match fm.category.as_deref() {
                    None => None,
                    Some(c) if VALID_CATEGORIES.contains(&c) => None,
                    Some(c) => Some(format!(
                        "categoryが不正です(値: {c:?}, 許可値: {VALID_CATEGORIES:?})"
                    )),
                }
            } else {
                None
            }
        }
        other => Some(format!("typeが不正です(値: {other:?}, 許可値: {VALID_TYPES:?})")),
    }
}

// library/_system/CLAUDE.mdの「配置先の決め方」を実装する(詩織Ver3.0、
// データ管理法見直し2-2節)。typeの判定・付与自体はこのCLIの役目ではなく、
// 書き込みを行うAI(Claude Code)が自律的に行う前提(人間はtypeの値を直接
// 参照・指定しない)。このCLIは決定されたtypeを機械的にフォルダへ変換する
// だけの決定的ロジックを担う。
fn decide_destination(root: &Path, fm: &Frontmatter, projects_yaml: &Path) -> Result<PathBuf> {
    let project = fm.project.as_deref().map(str::trim).filter(|p| !p.is_empty());

    match fm.kind.as_deref().expect("validateを通過済みのためSome") {
        // 期限・ゴールのある進行中の取り組み。kind(journal|resource)で
        // さらにサブフォルダへ分ける(詩織Ver3.1、journal廃止統合)。
        "project" => match project {
            Some(p) => {
                ensure_project_registered(projects_yaml, p)?;
                let sub_kind = fm.sub_kind.as_deref().expect("validateを通過済みのためSome");
                Ok(root.join("10-projects").join(p).join(sub_kind))
            }
            None => Ok(root.join("00-inbox")),
        },

        // 終わりのない継続的関心領域。projectフィールドを識別子として流用する
        // (huraru.com運営・詩織開発等、type: projectと同じ命名空間で管理する)。
        // projectと同じくkindでjournal/resourceに分ける。
        "area" => match project {
            Some(p) => {
                ensure_project_registered(projects_yaml, p)?;
                let sub_kind = fm.sub_kind.as_deref().expect("validateを通過済みのためSome");
                Ok(root.join("20-areas").join(p).join(sub_kind))
            }
            None => Ok(root.join("00-inbox")),
        },

        // 【詩織Ver3.1で意味を変更】project/areaに紐づかない、外部から
        // 与えられた知識・参考資料専用に純化した(project/area内で生まれた
        // 意思決定・知見はkind: resourceとしてそれぞれの配下に置くようになった
        // ため)。projectフィールドが付いていても無視し、常に直下に置く。
        "resource" => Ok(root.join("30-resources")),

        // ふらるさん自身についての記録(詩織Ver3.1で新設。旧30-resources/profile/
        // の独立後継)。詩織Ver3.2でcategory(6分類)によるサブフォルダ分けに
        // 対応。category未指定の記事(profile-huraru.md等)は直下に置く。
        "profile" => match fm.category.as_deref() {
            Some(c) => Ok(root.join("40-profile").join(c)),
            None => Ok(root.join("40-profile")),
        },

        _ => unreachable!("validateで許可値のみ通過しているはず"),
    }
}

// frontmatterから取り出したYAMLのスカラー値を、コロンや日本語を含む文字列でも
// 壊れないよう安全にクォートしてシリアライズする。
fn yaml_scalar(s: &str) -> Result<String> {
    Ok(serde_yaml::to_string(s)?.trim_end().to_string())
}

// projects.yamlに指定idが存在するかを調べる。存在しなければ末尾に
// status: pendingで追記登録する。既存ファイルの先頭コメント等を保つため、
// パース→丸ごと再シリアライズはせず、追記のみで済ませる。
//
// 【Ver3.0で変更】旧30-knowledgeのdomain逆引きが不要になった(30-resourcesは
// project別サブフォルダ構成のため)ため、戻り値のdomainは廃止した。
fn ensure_project_registered(projects_yaml: &Path, id: &str) -> Result<()> {
    let text = std::fs::read_to_string(projects_yaml)
        .with_context(|| format!("{}の読み込みに失敗", projects_yaml.display()))?;
    let parsed: ProjectsFile = serde_yaml::from_str(&text)
        .with_context(|| format!("{}の解析に失敗", projects_yaml.display()))?;

    if parsed.projects.iter().any(|p| p.id == id) {
        return Ok(());
    }

    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(projects_yaml)
        .with_context(|| format!("{}への追記オープンに失敗", projects_yaml.display()))?;
    writeln!(f, "  - id: {}\n    status: pending", yaml_scalar(id)?)?;
    Ok(())
}

// tags.yamlのcanonical/aliasesと照合し、一致すればcanonical表記を返す。
// 一致しなければ末尾にstatus: pendingで自己登録し、そのままの表記を返す。
fn normalize_tag(tags_yaml: &Path, raw: &str, newly_pending: &mut Vec<String>) -> Result<String> {
    let text = std::fs::read_to_string(tags_yaml)
        .with_context(|| format!("{}の読み込みに失敗", tags_yaml.display()))?;
    let parsed: TagsFile = serde_yaml::from_str(&text)
        .with_context(|| format!("{}の解析に失敗", tags_yaml.display()))?;

    for entry in &parsed.tags {
        if entry.canonical == raw || entry.aliases.iter().any(|a| a == raw) {
            return Ok(entry.canonical.clone());
        }
    }
    // 同一実行内で同じ新規タグが複数回出てきても二重登録しない。
    if newly_pending.contains(&raw.to_string()) {
        return Ok(raw.to_string());
    }

    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(tags_yaml)
        .with_context(|| format!("{}への追記オープンに失敗", tags_yaml.display()))?;
    writeln!(
        f,
        "  - canonical: {}\n    aliases: []\n    status: pending",
        yaml_scalar(raw)?
    )?;
    newly_pending.push(raw.to_string());
    Ok(raw.to_string())
}

// 先頭`---`〜次の`---`をfrontmatterとしてパースし、(frontmatter, 本文)を返す。
fn split_frontmatter(content: &str) -> Result<(Frontmatter, String)> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let rest = content
        .strip_prefix("---\r\n")
        .or_else(|| content.strip_prefix("---\n"))
        .ok_or_else(|| anyhow!("frontmatter(先頭の---)が見つかりません"))?;
    let end = rest
        .find("\n---")
        .ok_or_else(|| anyhow!("frontmatterの終端(---)が見つかりません"))?;
    let yaml_part = &rest[..end];
    let body = rest[end + "\n---".len()..]
        .strip_prefix("\r\n")
        .or_else(|| rest[end + "\n---".len()..].strip_prefix('\n'))
        .unwrap_or(&rest[end + "\n---".len()..]);

    let fm: Frontmatter =
        serde_yaml::from_str(yaml_part).context("frontmatterのYAML解析に失敗")?;
    Ok((fm, body.to_string()))
}

// 保存先ディレクトリ内でファイル名の衝突を避ける
// (source-2.md、source-3.md...と連番を振る)。
fn unique_destination(dir: &Path, source: &Path) -> Result<PathBuf> {
    let file_name = source
        .file_name()
        .ok_or_else(|| anyhow!("ファイル名の取得に失敗"))?
        .to_string_lossy()
        .to_string();
    let stem = file_name
        .strip_suffix(".md")
        .ok_or_else(|| anyhow!("拡張子が.mdではありません: {file_name}"))?;

    let mut candidate = dir.join(&file_name);
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}-{n}.md"));
        n += 1;
    }
    Ok(candidate)
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

// ---- 共通記憶MD(詩織Ver4.1) ----

// memoryの保存先と、退避ファイル名に使う識別子(project id、全体用は"_global")を返す。
// 副作用なし(projects.yamlへの登録は、競合検知を通ってから行う)。
fn memory_target(root: &Path, fm: &Frontmatter) -> Result<(PathBuf, String)> {
    let project = fm.project.as_deref().map(str::trim).filter(|p| !p.is_empty());
    match fm.kind.as_deref() {
        Some(t @ ("project" | "area")) => {
            let p = project.ok_or_else(|| {
                anyhow!("typeがproject/areaのmemoryにはprojectが必要です(保存先が決まりません)")
            })?;
            if p.contains(['/', '\\']) || p.contains("..") {
                bail!("projectにパス区切りや..は使えません: {p}");
            }
            let top = if t == "project" { "10-projects" } else { "20-areas" };
            Ok((root.join(top).join(p).join(MEMORY_FILE_NAME), p.to_string()))
        }
        // 全体用。projectが付いていても無視する。
        Some("profile") => Ok((
            root.join("40-profile").join(MEMORY_GLOBAL_FILE_NAME),
            MEMORY_GLOBAL_KEY.to_string(),
        )),
        other => bail!("memoryのtypeが不正です(値: {other:?}, 許可値: project/area/profile)"),
    }
}

// 既存memoryの版を表す値。updatedを持たない初期の手書き版はdateで代用する。
fn memory_version(fm: &Frontmatter) -> Result<String> {
    fm.updated
        .as_deref()
        .or(fm.date.as_deref())
        .map(|v| v.trim().to_string())
        .ok_or_else(|| anyhow!("既存のmemoryにupdatedもdateもありません。手作業でfrontmatterを直してください"))
}

// 退避ファイル名。ISO8601の':'はWindowsのファイル名に使えないため'-'へ置き換える。
fn memory_history_file_name(key: &str, version: &str) -> String {
    format!("{key}-{MEMORY_KIND}-{}.md", version.replace(':', "-"))
}

// keyの退避ファイルを新しい順に並べ、MEMORY_HISTORY_GENERATIONSを超えた古いものを消す。
// 版はISO8601なので、同じ端末で作られた名前は辞書順がそのまま時系列になる。
fn prune_memory_history(history_dir: &Path, key: &str) -> Result<()> {
    let prefix = format!("{key}-{MEMORY_KIND}-");
    let mut names: Vec<String> = std::fs::read_dir(history_dir)
        .with_context(|| format!("{}の読み込みに失敗", history_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| {
            // 版(年の数字)が続くものだけを対象にし、別projectの退避を巻き込まない。
            n.strip_prefix(&prefix)
                .is_some_and(|rest| n.ends_with(".md") && rest.starts_with(|c: char| c.is_ascii_digit()))
        })
        .collect();
    names.sort_unstable_by(|a, b| b.cmp(a));
    for old in names.iter().skip(MEMORY_HISTORY_GENERATIONS) {
        std::fs::remove_file(history_dir.join(old))
            .with_context(|| format!("古い退避{old}の削除に失敗"))?;
    }
    Ok(())
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

// 共通記憶MDを保存する。通常記事と違い、不備はinboxへ逃がさずエラーにする
// (固定の保存先に置く前提のため、別の場所へ置くと「プロジェクトのmemory」でなくなる)。
//
// 既存のmemoryを更新するときは、読んだ時点のupdatedを--expect-updatedで渡す必要がある。
// 現在のupdated(またはdate)と違えば、他のセッションが先に更新したとみなして
// 何も書かずに終了する(ロックは使わない。同時保存しない運用が前提)。
fn save_memory(
    root: &Path,
    projects_yaml: &Path,
    tags_yaml: &Path,
    source: &Path,
    mut fm: Frontmatter,
    body: &str,
    expect_updated: Option<String>,
) -> Result<()> {
    if fm.title.as_deref().map(str::trim).unwrap_or("").is_empty() {
        bail!("titleが空です");
    }
    if fm.tags.is_empty() {
        bail!("tagsが空です");
    }
    let (dest, key) = memory_target(root, &fm)?;
    // 保存先のmemoryそのものを渡されると、新しい内容を書いた直後に元ファイルの削除で消える
    // (save_shelfと同じ確認。何かを書く前に断る)。
    if is_same_file(source, &dest) {
        bail!(
            "保存先のmemoryそのものは渡せません。作業用の場所へコピーして書き直してから渡してください: {}",
            dest.display()
        );
    }
    let now = chrono::Local::now().to_rfc3339();

    let old_text = if dest.is_file() {
        Some(
            std::fs::read_to_string(&dest)
                .with_context(|| format!("{}の読み込みに失敗", dest.display()))?,
        )
    } else {
        None
    };
    let old_version = match (&old_text, &expect_updated) {
        (Some(text), Some(expected)) => {
            let (old_fm, _) = split_frontmatter(text)?;
            let current = memory_version(&old_fm)?;
            if current != expected.trim() {
                bail!(
                    "競合: 渡されたupdated({})が現在のmemoryと一致しません。他のセッションが先に更新した可能性があります。何も書き込んでいません。memoryを読み直し、その内容に今回の変更を反映したうえで、読み直した時点のupdatedを--expect-updatedに渡して再度保存してください: {}",
                    expected.trim(),
                    dest.display()
                );
            }
            // dateは作成日時のまま引き継ぐ。
            fm.date = old_fm.date.or(Some(now.clone()));
            Some(current)
        }
        (Some(_), None) => bail!(
            "既存のmemoryの更新には--expect-updated <読んだ時点のupdated>が必要です。memoryを読み、そのupdatedを渡してください: {}",
            dest.display()
        ),
        (None, Some(_)) => bail!(
            "memoryがまだ存在しません。--expect-updatedを付けずに新規作成してください: {}",
            dest.display()
        ),
        (None, None) => {
            fm.date = Some(now.clone());
            None
        }
    };
    fm.updated = Some(now);

    // ここから先は書き込みを伴う。競合検知を通った後にだけ、タグとprojectを登録する。
    let mut newly_pending_tags = Vec::new();
    fm.tags = fm
        .tags
        .iter()
        .map(|raw| normalize_tag(tags_yaml, raw, &mut newly_pending_tags))
        .collect::<Result<Vec<_>>>()?;
    if matches!(fm.kind.as_deref(), Some("project" | "area")) {
        ensure_project_registered(projects_yaml, &key)?;
    }
    // 保存先(40-profile/など)がまだ無い場合に備える。
    let dest_dir = dest.parent().ok_or_else(|| anyhow!("保存先の親フォルダの取得に失敗"))?;
    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("{}の作成に失敗", dest_dir.display()))?;

    // 旧版の退避が済んでから上書きする(退避に失敗したら上書きしない)。
    let mut history_file = None;
    if let (Some(text), Some(version)) = (&old_text, &old_version) {
        let history_dir = root.join("_system").join(MEMORY_HISTORY_DIR);
        std::fs::create_dir_all(&history_dir)
            .with_context(|| format!("{}の作成に失敗", history_dir.display()))?;
        let path = history_dir.join(memory_history_file_name(&key, version));
        std::fs::write(&path, text)
            .with_context(|| format!("旧版の退避に失敗: {}", path.display()))?;
        history_file = Some(path);
    }

    let frontmatter_yaml = serde_yaml::to_string(&fm).context("frontmatterのYAML化に失敗")?;
    let new_content = format!("---\n{frontmatter_yaml}---\n{body}");
    write_atomically(&dest, &new_content)?;
    std::fs::remove_file(source)
        .with_context(|| format!("移動元{}の削除に失敗", source.display()))?;
    if history_file.is_some() {
        let history_dir = root.join("_system").join(MEMORY_HISTORY_DIR);
        if let Err(e) = prune_memory_history(&history_dir, &key) {
            eprintln!("警告: 古い退避の整理に失敗しました({e:#})。");
            shiori_log::warn("SAVE", "古い退避の整理に失敗した", &[("理由", &format!("{e:#}"))]);
        }
    }

    let mut warnings = Vec::new();
    if let Some((lines, chars)) = memory_over_limit(&body) {
        let w = format!(
            "memoryが上限を超えています(本文{lines}行・{chars}字。上限は本文{MEMORY_MAX_LINES}行・{MEMORY_MAX_CHARS}字)。完了した項目を消し、履歴が要る内容はjournal/decisionへ移して整理してください"
        );
        eprintln!("警告: {w}");
        shiori_log::warn("SAVE", "memoryが上限を超えた", &[("行", &lines), ("字", &chars)]);
        warnings.push(w);
    }

    reindex_saved_file(root, &dest);

    let result = serde_json::json!({
        "destination": dest.display().to_string(),
        "updated": fm.updated,
        "history_file": history_file.map(|p| p.display().to_string()),
        "inbox_reason": serde_json::Value::Null,
        "newly_pending_tags": newly_pending_tags,
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
fn decide_asset_destination(root: &Path, fm: &Frontmatter, projects_yaml: &Path) -> Result<PathBuf> {
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
            ensure_project_registered(projects_yaml, p)?;
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
    projects_yaml: &Path,
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

    let dest_dir = decide_asset_destination(root, &fm, projects_yaml)?;
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
    fn memoryの保存先はtypeごとに固定される() {
        let root = Path::new("lib");
        let (p, key) = memory_target(root, &fm_of("area", Some("shiori"))).unwrap();
        assert_eq!(p, root.join("20-areas").join("shiori").join("memory.md"));
        assert_eq!(key, "shiori");
        let (p, _) = memory_target(root, &fm_of("project", Some("tanker"))).unwrap();
        assert_eq!(p, root.join("10-projects").join("tanker").join("memory.md"));
        // 全体用はprojectがあっても無視する。
        let (p, key) = memory_target(root, &fm_of("profile", Some("shiori"))).unwrap();
        assert_eq!(p, root.join("40-profile").join("memory-global.md"));
        assert_eq!(key, MEMORY_GLOBAL_KEY);
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
            &root.join("projects.yaml"),
            &root.join("tags.yaml"),
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

    #[test]
    fn memoryの保存先が決まらない入力は拒否する() {
        let root = Path::new("lib");
        assert!(memory_target(root, &fm_of("area", None)).is_err());
        assert!(memory_target(root, &fm_of("resource", None)).is_err());
        assert!(memory_target(root, &fm_of("area", Some("../x"))).is_err());
        assert!(memory_target(root, &fm_of("area", Some("a/b"))).is_err());
    }

    #[test]
    fn 退避ファイル名にコロンを含めない() {
        let name = memory_history_file_name("shiori", "2026-10-10T15:31:52.928312+09:00");
        assert_eq!(name, "shiori-memory-2026-10-10T15-31-52.928312+09-00.md");
    }

    #[test]
    fn memoryの上限は本文だけで数える() {
        assert_eq!(memory_over_limit(&"あ".repeat(MEMORY_MAX_CHARS)), None);
        assert_eq!(memory_over_limit(&"あ".repeat(MEMORY_MAX_CHARS + 1)), Some((1, MEMORY_MAX_CHARS + 1)));
        assert_eq!(memory_over_limit(&"行\n".repeat(MEMORY_MAX_LINES + 1)), Some((MEMORY_MAX_LINES + 1, (MEMORY_MAX_LINES + 1) * 2)));
    }

    #[test]
    fn 退避は新しい3世代だけ残し別projectの退避は触らない() {
        let dir = std::env::temp_dir().join(format!("shiori-save-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for day in 1..=5 {
            let v = format!("2026-10-0{day}T10:00:00+09:00");
            std::fs::write(dir.join(memory_history_file_name("shiori", &v)), "x").unwrap();
        }
        let other = dir.join(memory_history_file_name("shiori-memory-x", "2026-10-01T10:00:00+09:00"));
        std::fs::write(&other, "x").unwrap();
        let tanker = dir.join(memory_history_file_name("tanker", "2026-10-01T10:00:00+09:00"));
        std::fs::write(&tanker, "x").unwrap();

        prune_memory_history(&dir, "shiori").unwrap();

        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("shiori-memory-2026"))
            .collect();
        left.sort();
        assert_eq!(left.len(), 3);
        assert!(left[0].contains("2026-10-03"), "古い2世代が消え、03〜05が残る: {left:?}");
        assert!(other.exists() && tanker.exists());
        std::fs::remove_dir_all(&dir).unwrap();
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

    #[test]
    fn 既存memoryの版はupdatedを優先しなければdateを使う() {
        let mut fm = Frontmatter::default();
        assert!(memory_version(&fm).is_err());
        fm.date = Some("D".to_string());
        assert_eq!(memory_version(&fm).unwrap(), "D");
        fm.updated = Some("U".to_string());
        assert_eq!(memory_version(&fm).unwrap(), "U");
    }
}
