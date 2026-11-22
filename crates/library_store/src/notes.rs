//! 記事(frontmatter+本文のmd)の規則と保存。
//!
//! `library/_system/CLAUDE.md` の「配置先の決め方」を実装する(詩織Ver3.0、データ管理法見直し2-2節)。
//! typeの判定・付与はAI(Claude Codeなど)の役目で、ここは決まったtypeを機械的にフォルダへ変換するだけの、
//! 決定的な規則を担う。アプリ・CLI・サーバーが、同じ規則で保存するために、ここに置く。

use crate::fsx::{self, Cond, StoreError};
use crate::layout::Layout;
use crate::registry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// 詩織Ver3.1(journal廃止・project/area配下へのkind統合、2026-09-16)。
// journalをtypeから廃止し、project/areaそれぞれの配下にjournal/resourceの
// 2種類(kindフィールド)を持たせる形に変更した。resourceは「project/area
// に紐づかない外部由来の知識」専用に純化し、ふらるさん自身についての記録は
// 新設のprofileへ独立させた。
pub const VALID_TYPES: &[&str] = &["project", "area", "resource", "profile"];
// type: project/areaの記事だけが持つ、記事の性質(作業ログか、そこから
// 生まれた意思決定・知見か)。type: resource/profileには存在しない
// (存在してもdecide_destinationでは参照しない)。
pub const VALID_KINDS: &[&str] = &["journal", "resource"];
// 詩織Ver3.2(40-profile/のサブフォルダ細分化、2026-09-21)。type: profileの
// 記事だけが持つ分類。未指定を許容する点がkindと異なり、profile-huraru.md
// のような「プロフィール全体の入り口」的な記事はcategoryなしのまま
// 40-profile/直下に置かれる。
pub const VALID_CATEGORIES: &[&str] = &[
    "temperament-and-thinking",
    "values",
    "life-history",
    "relationships",
    "self-image",
    "daily-and-work",
];

pub const INBOX_DIR: &str = "00-inbox";

fn default_index() -> bool {
    true
}

fn default_status() -> String {
    "new".to_string()
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Frontmatter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    // 詩織Ver3.1で新設。type: project/areaの記事のみ使う(journal | resource)。
    // YAML上のフィールド名は"kind"だが、上のkind(YAML上の"type")と紛らわしい
    // ためRust側の変数名はsub_kindとした。
    #[serde(rename = "kind", skip_serializing_if = "Option::is_none")]
    pub sub_kind: Option<String>,
    // 詩織Ver3.2で新設。type: profileの記事のみ使う(6分類の列挙、未指定可)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    // 本文とは別の1〜2文の要約(検索結果表示・rerank精度向上に使う)。
    // 必須化はしない(無い場合は単に空欄のまま)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    // 検索対象に含めるか(services/rag/indexing.py側で参照する)。デフォルトtrue。
    #[serde(default = "default_index")]
    pub index: bool,
    // draft/new/outdated/deprecated/disputedの5値を想定するが、typeと違い
    // 判断を誤っても実害が小さいためenumバリデーションはしない。
    #[serde(default = "default_status")]
    pub status: String,
    // 内部的に関連する他記事へのリンク(将来のリンクグラフの元データ)。
    #[serde(default)]
    pub related: Vec<String>,
    // 記事の作成日時(ISO8601)。詩織Ver3.3で新設。AIが書いた値があっても
    // 信用せず、保存の都度、呼び出し側が現在時刻で上書きする(日時のような
    // 機械的に決まる値はAIの裁量に委ねず決定的に付与する。手動入力によるズレ・
    // 付け忘れの防止が目的)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    // 詩織Ver4.1で新設。kind: memoryだけが持つ最終更新日時。dateと同じく
    // 保存の都度現在時刻を書き、競合検知(--expect-updated)の基準になる。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    // 上記以外のfrontmatterフィールドは検証・ルーティングの対象外だが、
    // 保存時に消さず維持する。
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

/// 先頭`---`〜次の`---`をfrontmatterとしてパースし、(frontmatter, 本文)を返す。
pub fn split_frontmatter(content: &str) -> Result<(Frontmatter, String), StoreError> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let rest = content
        .strip_prefix("---\r\n")
        .or_else(|| content.strip_prefix("---\n"))
        .ok_or_else(|| StoreError::Invalid("frontmatter(先頭の---)が見つかりません".into()))?;
    let end = rest
        .find("\n---")
        .ok_or_else(|| StoreError::Invalid("frontmatterの終端(---)が見つかりません".into()))?;
    let yaml_part = &rest[..end];
    let after = &rest[end + "\n---".len()..];
    let body = after
        .strip_prefix("\r\n")
        .or_else(|| after.strip_prefix('\n'))
        .unwrap_or(after);

    let fm: Frontmatter = serde_yaml::from_str(yaml_part)
        .map_err(|e| StoreError::Invalid(format!("frontmatterのYAML解析に失敗: {e}")))?;
    Ok((fm, body.to_string()))
}

/// frontmatterの必須項目を検証する。不備があれば理由の文を返す(Noneなら正常)。
/// 不備のある記事は、保存を諦めず、理由つきで00-inboxへ置く。
pub fn validate(fm: &Frontmatter) -> Option<String> {
    validate_with(Layout::Legacy, fm)
}

/// 置き場の並び(`Layout`)に応じて検証する。
pub fn validate_with(layout: Layout, fm: &Frontmatter) -> Option<String> {
    if fm.title.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Some("titleが空です".to_string());
    }
    if fm.tags.is_empty() {
        return Some("tagsが空です".to_string());
    }
    match fm.kind.as_deref() {
        Some(k) if layout.valid_types().contains(&k) => {
            // project/areaはkind(journal|resource)も必須。resource/profileには
            // 存在しない概念なので、指定されていても無視する(バリデーション対象外)。
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
                    Some(c) => Some(format!("categoryが不正です(値: {c:?}, 許可値: {VALID_CATEGORIES:?})")),
                }
            } else {
                None
            }
        }
        other => Some(format!("typeが不正です(値: {other:?}, 許可値: {:?})", layout.valid_types())),
    }
}

/// 配置先のフォルダ(libraryからの相対。区切りは`/`)を決める。`validate`を通った記事だけを渡す。
/// project/areaで、projectが未登録なら、`projects.yaml` へ `pending` で登録する。
pub fn decide_destination(root: &Path, fm: &Frontmatter) -> Result<String, StoreError> {
    decide_destination_with(Layout::Legacy, root, fm)
}

/// `decide_destination` の、置き場の並びを指定する版。
pub fn decide_destination_with(layout: Layout, root: &Path, fm: &Frontmatter) -> Result<String, StoreError> {
    let project = fm.project.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let kind = fm
        .kind
        .as_deref()
        .ok_or_else(|| StoreError::Invalid("typeがありません(validateを通していません)".into()))?;
    let sub_kind = || {
        fm.sub_kind
            .as_deref()
            .ok_or_else(|| StoreError::Invalid("kindがありません(validateを通していません)".into()))
    };

    match kind {
        // 期限・ゴールのある進行中の取り組み。kind(journal|resource)でさらにサブフォルダへ分ける。
        "project" => match project {
            Some(p) => {
                registry::ensure_project(root, p)?;
                Ok(format!("{}/{}", layout.project_dir("project", p)?, sub_kind()?))
            }
            None => Ok(INBOX_DIR.to_string()),
        },

        // 終わりのない継続的関心領域。projectフィールドを識別子として流用する
        // (huraru.com運営・詩織開発等、type: projectと同じ命名空間で管理する)。
        "area" => match project {
            Some(p) => {
                registry::ensure_project(root, p)?;
                Ok(format!("{}/{}", layout.project_dir("area", p)?, sub_kind()?))
            }
            None => Ok(INBOX_DIR.to_string()),
        },

        // 【詩織Ver3.1で意味を変更】project/areaに紐づかない、外部から与えられた
        // 知識・参考資料専用。projectフィールドが付いていても無視し、常に直下に置く。
        "resource" => Ok("30-resources".to_string()),

        // ふらるさん自身についての記録(詩織Ver3.1で新設)。詩織Ver3.2でcategory(6分類)による
        // サブフォルダ分けに対応。category未指定の記事は直下に置く。
        "profile" => match fm.category.as_deref() {
            Some(c) => Ok(format!("40-profile/{c}")),
            None => Ok("40-profile".to_string()),
        },

        other => Err(StoreError::Invalid(format!("typeが不正です: {other}"))),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct SavedArticle {
    pub path: PathBuf,
    /// libraryからの相対パス(区切りは`/`)。
    pub rel: String,
    /// 書いた中身の版。
    pub rev: String,
    /// 不備があって00-inboxへ置いたときの理由。
    pub inbox_reason: Option<String>,
    /// この保存で、新しく `pending` で台帳に入ったタグ。
    pub newly_pending_tags: Vec<String>,
}

/// 記事を保存する: タグの正規化 → 検証 → 配置先の決定 → 書き込み(同名があれば連番)。
/// `file_name` は元のファイル名(`.md`)。`fm.date` は、呼び出し側が現在時刻で上書きしておく。
/// 元のファイルの削除と、索引への反映は、呼び出し側が行う。
pub fn save_article(root: &Path, file_name: &str, fm: Frontmatter, body: &str) -> Result<SavedArticle, StoreError> {
    save_article_with(Layout::Legacy, root, file_name, fm, body)
}

/// `save_article` の、置き場の並びを指定する版。V7では `type: area` を、inboxへ逃がさず断る。
pub fn save_article_with(
    layout: Layout,
    root: &Path,
    file_name: &str,
    mut fm: Frontmatter,
    body: &str,
) -> Result<SavedArticle, StoreError> {
    if fm.kind.as_deref() == Some("area") {
        if let Some(reason) = layout.area_rejection() {
            return Err(StoreError::Invalid(reason.to_string()));
        }
    }
    if file_name.contains('/') || file_name.contains('\\') {
        return Err(StoreError::Invalid(format!("ファイル名に区切りは使えません: {file_name}")));
    }
    let stem = file_name
        .strip_suffix(".md")
        .ok_or_else(|| StoreError::Invalid(format!("拡張子が.mdではありません: {file_name}")))?;

    // タグは常に正規化する(inbox行きになる場合でも、表記ゆれの統一自体は後で見返したときに有用なため)。
    let mut newly_pending_tags = Vec::new();
    let mut tags = Vec::with_capacity(fm.tags.len());
    for raw in &fm.tags {
        tags.push(registry::normalize_tag(root, raw, &mut newly_pending_tags)?);
    }
    fm.tags = tags;

    let (dir, inbox_reason) = match validate_with(layout, &fm) {
        Some(reason) => {
            fm.reason = Some(reason.clone());
            (INBOX_DIR.to_string(), Some(reason))
        }
        None => (decide_destination_with(layout, root, &fm)?, None),
    };

    let frontmatter_yaml = serde_yaml::to_string(&fm)
        .map_err(|e| StoreError::Invalid(format!("frontmatterのYAML化に失敗: {e}")))?;
    let content = format!("---\n{frontmatter_yaml}---\n{body}");

    // 同名があれば、source-2.md、source-3.md … と連番を振る。
    let mut n = 1;
    loop {
        let name = if n == 1 { file_name.to_string() } else { format!("{stem}-{n}.md") };
        let rel = format!("{dir}/{name}");
        match fsx::write(root, &rel, content.as_bytes(), Cond::CreateNew) {
            Ok(w) => {
                return Ok(SavedArticle { path: w.path, rel, rev: w.rev, inbox_reason, newly_pending_tags });
            }
            Err(StoreError::Exists(_)) => n += 1,
            Err(e) => return Err(e),
        }
    }
}

// ---- 共通記憶MD(詩織Ver4.1) ----
// kind: memoryは「いま有効な状態・やること・申し送り」を持つ丸ごと上書き型の記事で、通常記事のように
// 連番を付けて積み上げず、保存先のファイル名も固定する(プロジェクトにつき1ファイル)。

pub const MEMORY_KIND: &str = "memory";
const MEMORY_FILE_NAME: &str = "memory.md";
// 上書き前の旧版を退避する先(library/_system/配下は索引から除外される)。
const MEMORY_HISTORY_DIR: &str = "_system/memory-history";
const MEMORY_HISTORY_GENERATIONS: usize = 3;
// 退避ファイル名でproject idと区別するための、全体用memoryの識別子。
pub const MEMORY_GLOBAL_KEY: &str = "_global";
// 上限(本文だけ。frontmatterは数えない)。超えても保存は止めず、警告で整理を促す。
// frontmatterを外すのは、`related` が記録の増えるほど伸び、整理しても減らせないため(2026-10-24、ふらる)。
pub const MEMORY_MAX_LINES: usize = 200;
pub const MEMORY_MAX_CHARS: usize = 6000;

/// memoryの本文の行数と字数が上限を超えていれば、その行数と字数を返す。
pub fn memory_over_limit(body: &str) -> Option<(usize, usize)> {
    let lines = body.lines().count();
    let chars = body.chars().count();
    (lines > MEMORY_MAX_LINES || chars > MEMORY_MAX_CHARS).then_some((lines, chars))
}

/// memoryの保存先(libraryからの相対パス)と、退避ファイル名に使う識別子(project id、全体用は"_global")。
/// 副作用なし(projects.yamlへの登録は、競合検知を通ってから行う)。
pub fn memory_target(fm: &Frontmatter) -> Result<(String, String), StoreError> {
    memory_target_with(Layout::Legacy, fm)
}

/// `memory_target` の、置き場の並びを指定する版。
pub fn memory_target_with(layout: Layout, fm: &Frontmatter) -> Result<(String, String), StoreError> {
    let project = fm.project.as_deref().map(str::trim).filter(|p| !p.is_empty());
    match fm.kind.as_deref() {
        Some(t @ ("project" | "area")) => {
            let p = project.ok_or_else(|| {
                StoreError::Invalid("typeがproject/areaのmemoryにはprojectが必要です(保存先が決まりません)".into())
            })?;
            if p.contains(['/', '\\']) || p.contains("..") {
                return Err(StoreError::Invalid(format!("projectにパス区切りや..は使えません: {p}")));
            }
            Ok((format!("{}/{MEMORY_FILE_NAME}", layout.project_dir(t, p)?), p.to_string()))
        }
        // 全体用。projectが付いていても無視する。
        Some("profile") => Ok((layout.global_memory_rel().to_string(), MEMORY_GLOBAL_KEY.to_string())),
        other => Err(StoreError::Invalid(format!(
            "memoryのtypeが不正です(値: {other:?}, 許可値: project/area/profile)"
        ))),
    }
}

/// 既存memoryの版を表す値。updatedを持たない初期の手書き版はdateで代用する。
pub fn memory_version(fm: &Frontmatter) -> Result<String, StoreError> {
    fm.updated
        .as_deref()
        .or(fm.date.as_deref())
        .map(|v| v.trim().to_string())
        .ok_or_else(|| {
            StoreError::Invalid("既存のmemoryにupdatedもdateもありません。手作業でfrontmatterを直してください".into())
        })
}

/// 退避ファイル名。ISO8601の':'はWindowsのファイル名に使えないため'-'へ置き換える。
pub fn memory_history_file_name(key: &str, version: &str) -> String {
    format!("{key}-{MEMORY_KIND}-{}.md", version.replace(':', "-"))
}

/// keyの退避ファイルを新しい順に並べ、MEMORY_HISTORY_GENERATIONSを超えた古いものを消す。
/// 版はISO8601なので、同じ端末で作られた名前は辞書順がそのまま時系列になる。
pub fn prune_memory_history(root: &Path, key: &str) -> Result<(), StoreError> {
    let prefix = format!("{key}-{MEMORY_KIND}-");
    let mut names: Vec<String> = fsx::list_names(root, MEMORY_HISTORY_DIR)?
        .into_iter()
        .filter(|n| {
            // 版(年の数字)が続くものだけを対象にし、別projectの退避を巻き込まない。
            n.strip_prefix(&prefix)
                .is_some_and(|rest| n.ends_with(".md") && rest.starts_with(|c: char| c.is_ascii_digit()))
        })
        .collect();
    names.sort_unstable_by(|a, b| b.cmp(a));
    for old in names.iter().skip(MEMORY_HISTORY_GENERATIONS) {
        fsx::remove(root, &format!("{MEMORY_HISTORY_DIR}/{old}"), None)?;
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub struct SavedMemory {
    pub path: PathBuf,
    pub rel: String,
    pub rev: String,
    /// 書いた`updated`(次の更新で`--expect-updated`に渡す値)。
    pub updated: String,
    /// 旧版を退避した先(libraryからの相対)。新規作成のときはNone。
    pub history_rel: Option<String>,
    pub newly_pending_tags: Vec<String>,
    /// 本文が上限を超えたときの(行数, 字数)。
    pub over_limit: Option<(usize, usize)>,
    /// 保存は済んだが、古い退避の整理に失敗したときの理由。
    pub prune_error: Option<String>,
}

/// memoryの上限超過の警告文。
pub fn over_limit_warning(lines: usize, chars: usize) -> String {
    format!(
        "memoryが上限を超えています(本文{lines}行・{chars}字。上限は本文{MEMORY_MAX_LINES}行・{MEMORY_MAX_CHARS}字)。完了した項目を消し、履歴が要る内容はjournal/decisionへ移して整理してください"
    )
}

/// 共通記憶MDを保存する。通常記事と違い、不備はinboxへ逃がさずエラーにする
/// (固定の保存先に置く前提のため、別の場所へ置くと「プロジェクトのmemory」でなくなる)。
///
/// 既存のmemoryを更新するときは、読んだ時点のupdatedを`expect_updated`で渡す必要がある。
/// 現在のupdated(またはdate)と違えば、他のセッションが先に更新したとみなして何も書かずに断る。
/// さらに、判定のあとの書き込みも、読んだ版(rev)のときだけ置き換える
/// (判定と書き込みの間に別の保存が入っても、片方の変更が消えない)。
/// `now` は呼び出し側が渡す現在時刻(ISO8601)。元ファイルの削除と索引への反映も、呼び出し側が行う。
pub fn save_memory(
    root: &Path,
    fm: Frontmatter,
    body: &str,
    expect_updated: Option<&str>,
    now: &str,
) -> Result<SavedMemory, StoreError> {
    save_memory_with(Layout::Legacy, root, fm, body, expect_updated, now)
}

/// `save_memory` の、置き場の並びを指定する版。
pub fn save_memory_with(
    layout: Layout,
    root: &Path,
    mut fm: Frontmatter,
    body: &str,
    expect_updated: Option<&str>,
    now: &str,
) -> Result<SavedMemory, StoreError> {
    if fm.title.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err(StoreError::Invalid("titleが空です".into()));
    }
    if fm.tags.is_empty() {
        return Err(StoreError::Invalid("tagsが空です".into()));
    }
    let (rel, key) = memory_target_with(layout, &fm)?;

    let old: Option<(String, String)> = match fsx::read(root, &rel) {
        Ok((bytes, rev)) => Some((
            String::from_utf8(bytes).map_err(|_| StoreError::Invalid(format!("{rel}がUTF-8ではありません")))?,
            rev,
        )),
        Err(StoreError::NotFound(_)) => None,
        Err(e) => return Err(e),
    };

    let old_version = match (&old, expect_updated) {
        (Some((text, _)), Some(expected)) => {
            let (old_fm, _) = split_frontmatter(text)?;
            let current = memory_version(&old_fm)?;
            if current != expected.trim() {
                return Err(StoreError::Stale(format!(
                    "競合: 渡されたupdated({})が現在のmemoryと一致しません。他のセッションが先に更新した可能性があります。何も書き込んでいません。memoryを読み直し、その内容に今回の変更を反映したうえで、読み直した時点のupdatedを--expect-updatedに渡して再度保存してください: {rel}",
                    expected.trim()
                )));
            }
            // dateは作成日時のまま引き継ぐ。
            fm.date = old_fm.date.or_else(|| Some(now.to_string()));
            Some(current)
        }
        (Some(_), None) => {
            return Err(StoreError::Invalid(format!(
                "既存のmemoryの更新には--expect-updated <読んだ時点のupdated>が必要です。memoryを読み、そのupdatedを渡してください: {rel}"
            )))
        }
        (None, Some(_)) => {
            return Err(StoreError::Invalid(format!(
                "memoryがまだ存在しません。--expect-updatedを付けずに新規作成してください: {rel}"
            )))
        }
        (None, None) => {
            fm.date = Some(now.to_string());
            None
        }
    };
    fm.updated = Some(now.to_string());

    // ここから先は書き込みを伴う。競合検知を通った後にだけ、タグとprojectを登録する。
    let mut newly_pending_tags = Vec::new();
    let mut tags = Vec::with_capacity(fm.tags.len());
    for raw in &fm.tags {
        tags.push(registry::normalize_tag(root, raw, &mut newly_pending_tags)?);
    }
    fm.tags = tags;
    if matches!(fm.kind.as_deref(), Some("project" | "area")) {
        registry::ensure_project(root, &key)?;
    }

    // 旧版の退避が済んでから上書きする(退避に失敗したら上書きしない)。
    let mut history_rel = None;
    if let (Some((text, _)), Some(version)) = (&old, &old_version) {
        let rel_h = format!("{MEMORY_HISTORY_DIR}/{}", memory_history_file_name(&key, version));
        fsx::write(root, &rel_h, text.as_bytes(), Cond::Any)
            .map_err(|e| StoreError::Io(format!("旧版の退避に失敗: {e}")))?;
        history_rel = Some(rel_h);
    }

    let frontmatter_yaml = serde_yaml::to_string(&fm)
        .map_err(|e| StoreError::Invalid(format!("frontmatterのYAML化に失敗: {e}")))?;
    let content = format!("---\n{frontmatter_yaml}---\n{body}");
    let cond = match &old {
        Some((_, rev)) => Cond::Overwrite { expect_rev: rev.clone() },
        None => Cond::CreateNew,
    };
    let written = fsx::write(root, &rel, content.as_bytes(), cond).map_err(|e| match e {
        StoreError::Conflict { .. } | StoreError::Exists(_) => StoreError::Stale(format!(
            "競合: 保存の途中で、他のセッションが先にmemoryを更新しました。何も上書きしていません。memoryを読み直してやり直してください: {rel}"
        )),
        other => other,
    })?;

    let prune_error = if history_rel.is_some() {
        prune_memory_history(root, &key).err().map(|e| e.to_string())
    } else {
        None
    };

    Ok(SavedMemory {
        path: written.path,
        rel,
        rev: written.rev,
        updated: now.to_string(),
        history_rel,
        newly_pending_tags,
        over_limit: memory_over_limit(body),
        prune_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lib(PathBuf);
    impl Lib {
        fn new(tag: &str) -> Lib {
            let p = std::env::temp_dir().join(format!("library_store-notes-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(p.join("_system")).unwrap();
            std::fs::write(p.join(registry::PROJECTS_YAML), "# 一覧\nprojects:\n  - id: shiori\n    status: confirmed\n").unwrap();
            std::fs::write(
                p.join(registry::TAGS_YAML),
                "# タグ\ntags:\n  - canonical: 詩織\n    aliases: [shiori]\n    status: confirmed\n",
            )
            .unwrap();
            Lib(p)
        }
        fn read(&self, rel: &str) -> String {
            std::fs::read_to_string(self.0.join(rel)).unwrap()
        }
    }
    impl Drop for Lib {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fm(yaml: &str) -> Frontmatter {
        split_frontmatter(&format!("---\n{yaml}\n---\n本文\n")).unwrap().0
    }

    #[test]
    fn frontmatterと本文に分ける() {
        let (f, body) = split_frontmatter("---\ntitle: あ\ntype: resource\ntags: [x]\n---\n本文1\n本文2\n").unwrap();
        assert_eq!(f.title.as_deref(), Some("あ"));
        assert_eq!(f.kind.as_deref(), Some("resource"));
        assert!(f.index);
        assert_eq!(f.status, "new");
        assert_eq!(body, "本文1\n本文2\n");
    }

    #[test]
    fn 改行がcrlfでもbomつきでも読める() {
        let (f, body) = split_frontmatter("\u{feff}---\r\ntitle: あ\r\n---\r\n本文\r\n").unwrap();
        assert_eq!(f.title.as_deref(), Some("あ"));
        assert_eq!(body, "本文\r\n");
    }

    #[test]
    fn frontmatterが無い記事は断る() {
        assert!(matches!(split_frontmatter("本文だけ"), Err(StoreError::Invalid(_))));
        assert!(matches!(split_frontmatter("---\ntitle: あ\n"), Err(StoreError::Invalid(_))));
    }

    #[test]
    fn 既知でない項目も消さずに残す() {
        let (f, _) = split_frontmatter("---\ntitle: あ\nauthor: ふらる\n---\n").unwrap();
        assert!(f.extra.contains_key("author"));
        assert!(serde_yaml::to_string(&f).unwrap().contains("author: ふらる"));
    }

    #[test]
    fn 検証は不備の理由を返す() {
        assert_eq!(validate(&fm("title: ' '\ntype: resource\ntags: [a]")).unwrap(), "titleが空です");
        assert_eq!(validate(&fm("title: あ\ntype: resource")).unwrap(), "tagsが空です");
        assert!(validate(&fm("title: あ\ntags: [a]")).unwrap().starts_with("typeが不正です"));
        assert!(validate(&fm("title: あ\ntype: journal\ntags: [a]")).unwrap().starts_with("typeが不正です"));
        // project/areaは kind が必須
        assert!(validate(&fm("title: あ\ntype: project\ntags: [a]")).unwrap().starts_with("kindが不正です"));
        assert!(validate(&fm("title: あ\ntype: area\nkind: memo\ntags: [a]")).unwrap().starts_with("kindが不正です"));
        // profileのcategoryは、未指定は可、指定するなら6分類のどれか
        assert!(validate(&fm("title: あ\ntype: profile\ntags: [a]")).is_none());
        assert!(validate(&fm("title: あ\ntype: profile\ncategory: values\ntags: [a]")).is_none());
        assert!(validate(&fm("title: あ\ntype: profile\ncategory: x\ntags: [a]")).unwrap().starts_with("categoryが不正です"));
        // resourceは project も kind も見ない
        assert!(validate(&fm("title: あ\ntype: resource\nkind: でたらめ\ntags: [a]")).is_none());
    }

    #[test]
    fn 配置先はtypeごとに決まる() {
        let l = Lib::new("dest");
        let dest = |y: &str| decide_destination(&l.0, &fm(y)).unwrap();
        assert_eq!(dest("type: project\nkind: journal\nproject: shiori\ntitle: a\ntags: [a]"), "10-projects/shiori/journal");
        assert_eq!(dest("type: area\nkind: resource\nproject: shiori\ntitle: a\ntags: [a]"), "20-areas/shiori/resource");
        assert_eq!(dest("type: resource\nproject: shiori\ntitle: a\ntags: [a]"), "30-resources");
        assert_eq!(dest("type: profile\ntitle: a\ntags: [a]"), "40-profile");
        assert_eq!(dest("type: profile\ncategory: values\ntitle: a\ntags: [a]"), "40-profile/values");
        // projectが無ければ、project/areaはinboxへ
        assert_eq!(dest("type: project\nkind: journal\ntitle: a\ntags: [a]"), "00-inbox");
        assert_eq!(dest("type: area\nkind: journal\nproject: '  '\ntitle: a\ntags: [a]"), "00-inbox");
    }

    #[test]
    fn 未登録のprojectは配置先を決めるときにpendingで登録される() {
        let l = Lib::new("register");
        decide_destination(&l.0, &fm("type: project\nkind: journal\nproject: 新作\ntitle: a\ntags: [a]")).unwrap();
        assert!(l.read(registry::PROJECTS_YAML).ends_with("  - id: 新作\n    status: pending\n"));
        // resourceでは、projectがあっても登録しない
        decide_destination(&l.0, &fm("type: resource\nproject: 別\ntitle: a\ntags: [a]")).unwrap();
        assert!(!l.read(registry::PROJECTS_YAML).contains("別"));
    }

    #[test]
    fn 記事を保存しタグを正規化して台帳に足す() {
        let l = Lib::new("save");
        let f = fm("title: あ\ntype: project\nkind: journal\nproject: shiori\ntags: [shiori, 未知]\ndate: x");
        let saved = save_article(&l.0, "a.md", f, "本文\n").unwrap();
        assert_eq!(saved.rel, "10-projects/shiori/journal/a.md");
        assert_eq!(saved.inbox_reason, None);
        assert_eq!(saved.newly_pending_tags, vec!["未知".to_string()]);
        let text = l.read(&saved.rel);
        assert!(text.starts_with("---\n"));
        assert!(text.contains("- 詩織") && text.contains("- 未知"), "{text}");
        assert!(text.ends_with("---\n本文\n"));
        assert_eq!(saved.rev, crate::fsx::rev_of(text.as_bytes()));
    }

    #[test]
    fn 不備のある記事は理由つきでinboxに置く() {
        let l = Lib::new("inbox");
        let saved = save_article(&l.0, "b.md", fm("title: あ\ntype: resource\ntags: []"), "本文\n").unwrap();
        assert_eq!(saved.rel, "00-inbox/b.md");
        assert_eq!(saved.inbox_reason.as_deref(), Some("tagsが空です"));
        assert!(l.read(&saved.rel).contains("reason: tagsが空です"));
    }

    #[test]
    fn 同名があれば連番を付けて上書きしない() {
        let l = Lib::new("dup");
        let y = "title: あ\ntype: resource\ntags: [a]";
        let first = save_article(&l.0, "a.md", fm(y), "1\n").unwrap();
        let second = save_article(&l.0, "a.md", fm(y), "2\n").unwrap();
        let third = save_article(&l.0, "a.md", fm(y), "3\n").unwrap();
        assert_eq!(
            (first.rel.as_str(), second.rel.as_str(), third.rel.as_str()),
            ("30-resources/a.md", "30-resources/a-2.md", "30-resources/a-3.md")
        );
        assert!(l.read(&first.rel).ends_with("---\n1\n"));
        assert!(l.read(&second.rel).ends_with("---\n2\n"));
    }

    #[test]
    fn mdでない名前や区切りを含む名前は何も書かずに断る() {
        let l = Lib::new("badname");
        let y = "title: あ\ntype: resource\ntags: [未知]";
        for bad in ["a.txt", "a", "x/a.md", "x\\a.md"] {
            assert!(matches!(save_article(&l.0, bad, fm(y), "本文\n"), Err(StoreError::Invalid(_))), "{bad}");
        }
        // 断った保存では、タグも台帳に入らない
        assert!(!l.read(registry::TAGS_YAML).contains("未知"));
    }

    #[test]
    fn projectの名前でlibraryの外へ書けない() {
        let l = Lib::new("escape");
        let f = fm("title: あ\ntype: project\nkind: journal\nproject: '../escape'\ntags: [a]");
        assert!(matches!(save_article(&l.0, "a.md", f, "本文\n"), Err(StoreError::Invalid(_))));
        assert!(!l.0.parent().unwrap().join("escape").exists());
    }

    // ---- memory ----

    fn mfm(yaml: &str) -> Frontmatter {
        fm(&format!("title: t\ntags: [詩織]\n{yaml}"))
    }

    #[test]
    fn memoryの保存先はtypeごとに固定される() {
        let (p, key) = memory_target(&fm("type: area\nproject: shiori")).unwrap();
        assert_eq!((p.as_str(), key.as_str()), ("20-areas/shiori/memory.md", "shiori"));
        let (p, _) = memory_target(&fm("type: project\nproject: tanker")).unwrap();
        assert_eq!(p, "10-projects/tanker/memory.md");
        // 全体用はprojectがあっても無視する。
        let (p, key) = memory_target(&fm("type: profile\nproject: shiori")).unwrap();
        assert_eq!((p.as_str(), key.as_str()), ("40-profile/memory-global.md", MEMORY_GLOBAL_KEY));
    }

    #[test]
    fn memoryの保存先が決まらない入力は拒否する() {
        assert!(memory_target(&fm("type: area")).is_err());
        assert!(memory_target(&fm("type: resource")).is_err());
        assert!(memory_target(&fm("type: area\nproject: '../x'")).is_err());
        assert!(memory_target(&fm("type: area\nproject: 'a/b'")).is_err());
    }

    #[test]
    fn 退避ファイル名にコロンを含めない() {
        assert_eq!(
            memory_history_file_name("shiori", "2026-10-10T15:31:52.928312+09:00"),
            "shiori-memory-2026-10-10T15-31-52.928312+09-00.md"
        );
    }

    #[test]
    fn memoryの上限は本文だけで数える() {
        assert_eq!(memory_over_limit(&"あ".repeat(MEMORY_MAX_CHARS)), None);
        assert_eq!(memory_over_limit(&"あ".repeat(MEMORY_MAX_CHARS + 1)), Some((1, MEMORY_MAX_CHARS + 1)));
        assert_eq!(
            memory_over_limit(&"行\n".repeat(MEMORY_MAX_LINES + 1)),
            Some((MEMORY_MAX_LINES + 1, (MEMORY_MAX_LINES + 1) * 2))
        );
    }

    #[test]
    fn 既存memoryの版はupdatedを優先しなければdateを使う() {
        assert_eq!(memory_version(&fm("updated: u\ndate: d")).unwrap(), "u");
        assert_eq!(memory_version(&fm("date: ' d '")).unwrap(), "d");
        assert!(memory_version(&fm("title: t")).is_err());
    }

    #[test]
    fn 退避は新しい3世代だけ残し別projectの退避は触らない() {
        let l = Lib::new("prune");
        let dir = l.0.join("_system").join("memory-history");
        std::fs::create_dir_all(&dir).unwrap();
        for day in 1..=5 {
            let v = format!("2026-10-0{day}T10:00:00+09:00");
            std::fs::write(dir.join(memory_history_file_name("shiori", &v)), "x").unwrap();
        }
        let other = dir.join(memory_history_file_name("shiori-memory-x", "2026-10-01T10:00:00+09:00"));
        std::fs::write(&other, "x").unwrap();
        let tanker = dir.join(memory_history_file_name("tanker", "2026-10-01T10:00:00+09:00"));
        std::fs::write(&tanker, "x").unwrap();

        prune_memory_history(&l.0, "shiori").unwrap();

        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("shiori-memory-2026"))
            .collect();
        left.sort();
        assert_eq!(left.len(), 3);
        assert!(left[0].contains("2026-10-03"), "古い2世代が消え、03〜05が残る: {left:?}");
        assert!(other.exists() && tanker.exists());
    }

    const AREA: &str = "type: area\nproject: shiori";

    #[test]
    fn memoryを新規に作る() {
        let l = Lib::new("mem-new");
        let saved = save_memory(&l.0, mfm(AREA), "本文\n", None, "2026-11-01T10:00:00+09:00").unwrap();
        assert_eq!(saved.rel, "20-areas/shiori/memory.md");
        assert_eq!(saved.updated, "2026-11-01T10:00:00+09:00");
        assert_eq!(saved.history_rel, None);
        let text = l.read(&saved.rel);
        assert!(text.contains("date: 2026-11-01T10:00:00+09:00") && text.contains("updated: 2026-11-01T10:00:00+09:00"), "{text}");
        assert!(text.ends_with("---\n本文\n"));
    }

    #[test]
    fn memoryの更新は旧版を退避しdateを引き継ぐ() {
        let l = Lib::new("mem-update");
        let v1 = "2026-11-01T10:00:00+09:00";
        let v2 = "2026-11-02T10:00:00+09:00";
        let first = save_memory(&l.0, mfm(AREA), "旧\n", None, v1).unwrap();
        let old_text = l.read(&first.rel);
        let second = save_memory(&l.0, mfm(AREA), "新\n", Some(v1), v2).unwrap();

        let text = l.read(&second.rel);
        assert!(text.contains(&format!("date: {v1}")), "作成日時を引き継ぐ: {text}");
        assert!(text.contains(&format!("updated: {v2}")));
        assert!(text.ends_with("---\n新\n"));
        let history = second.history_rel.unwrap();
        assert_eq!(history, "_system/memory-history/shiori-memory-2026-11-01T10-00-00+09-00.md");
        assert_eq!(l.read(&history), old_text, "旧版がそのまま退避される");
    }

    #[test]
    fn 退避は更新を重ねても3世代で止まる() {
        let l = Lib::new("mem-gen");
        let mut prev: Option<String> = None;
        for day in 1..=6 {
            let now = format!("2026-11-0{day}T10:00:00+09:00");
            save_memory(&l.0, mfm(AREA), "本文\n", prev.as_deref(), &now).unwrap();
            prev = Some(now);
        }
        let names = fsx::list_names(&l.0, "_system/memory-history").unwrap();
        assert_eq!(names.len(), 3, "{names:?}");
    }

    #[test]
    fn 競合や条件の不備は何も書かずに断る() {
        let l = Lib::new("mem-conflict");
        let v1 = "2026-11-01T10:00:00+09:00";
        let first = save_memory(&l.0, mfm(AREA), "旧\n", None, v1).unwrap();
        let before = l.read(&first.rel);

        // updatedが違う → Stale。memoryも退避も変わらない
        let r = save_memory(&l.0, mfm(AREA), "新\n", Some("古いupdated"), "2026-11-02T10:00:00+09:00");
        assert!(matches!(r, Err(StoreError::Stale(_))), "{r:?}");
        // 既存のmemoryに、expectなし
        let r = save_memory(&l.0, mfm(AREA), "新\n", None, "2026-11-02T10:00:00+09:00");
        assert!(matches!(r, Err(StoreError::Invalid(_))), "{r:?}");
        // 無いmemoryに、expectあり
        let r = save_memory(&l.0, mfm("type: area\nproject: 無い"), "新\n", Some(v1), "2026-11-02T10:00:00+09:00");
        assert!(matches!(r, Err(StoreError::Invalid(_))), "{r:?}");

        assert_eq!(l.read(&first.rel), before);
        assert!(fsx::list_names(&l.0, "_system/memory-history").unwrap().is_empty(), "退避も作られない");
    }

    #[test]
    fn titleやtagsが無いmemoryは断る() {
        let l = Lib::new("mem-invalid");
        assert!(matches!(save_memory(&l.0, fm("type: area\nproject: shiori\ntags: [a]"), "x", None, "n"), Err(StoreError::Invalid(_))));
        assert!(matches!(save_memory(&l.0, fm("type: area\nproject: shiori\ntitle: t"), "x", None, "n"), Err(StoreError::Invalid(_))));
    }

    #[test]
    fn memoryの保存でタグを正規化しprojectを台帳へ登録する() {
        let l = Lib::new("mem-register");
        let f = fm("title: t\ntype: area\nproject: 新作\ntags: [shiori, 未知]");
        let saved = save_memory(&l.0, f, "本文\n", None, "2026-11-01T10:00:00+09:00").unwrap();
        assert_eq!(saved.newly_pending_tags, vec!["未知".to_string()]);
        assert!(l.read(registry::PROJECTS_YAML).ends_with("  - id: 新作\n    status: pending\n"));
        assert!(l.read(&saved.rel).contains("- 詩織"));
        // 全体用は、projectを登録しない
        let g = fm("title: t\ntype: profile\nproject: 登録しない\ntags: [詩織]");
        save_memory(&l.0, g, "本文\n", None, "2026-11-01T10:00:00+09:00").unwrap();
        assert!(!l.read(registry::PROJECTS_YAML).contains("登録しない"));
    }

    #[test]
    fn 上限を超えたmemoryは保存したうえで知らせる() {
        let l = Lib::new("mem-limit");
        let body = "行\n".repeat(MEMORY_MAX_LINES + 1);
        let saved = save_memory(&l.0, mfm(AREA), &body, None, "2026-11-01T10:00:00+09:00").unwrap();
        assert_eq!(saved.over_limit, Some((MEMORY_MAX_LINES + 1, (MEMORY_MAX_LINES + 1) * 2)));
        assert!(over_limit_warning(201, 402).contains("201行・402字"));
    }

    #[test]
    fn 同じupdatedから同時に更新すると片方だけ通る() {
        let l = Lib::new("mem-race");
        let v1 = "2026-11-01T10:00:00+09:00";
        save_memory(&l.0, mfm(AREA), "旧\n", None, v1).unwrap();
        let root = std::sync::Arc::new(l.0.clone());
        let handles: Vec<_> = (0..6)
            .map(|i| {
                let root = root.clone();
                std::thread::spawn(move || {
                    let now = format!("2026-11-02T10:00:0{i}+09:00");
                    save_memory(&root, mfm(AREA), &format!("本文{i}\n"), Some(v1), &now)
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1, "{results:?}");
        assert_eq!(results.iter().filter(|r| matches!(r, Err(StoreError::Stale(_)))).count(), 5);
    }


    // ---- V7の置き場 ----

    #[test]
    fn v7では記事とmemoryを20_projectsと10_commonへ置きareaを断る() {
        let l = Lib::new("v7");
        let f = fm("title: あ
type: project
kind: journal
project: shiori
tags: [詩織]");
        let a = save_article_with(Layout::V7, &l.0, "a.md", f, "本文
").unwrap();
        assert_eq!(a.rel, "20-projects/shiori/journal/a.md");
        // 全体用のmemory
        let g = save_memory_with(Layout::V7, &l.0, fm("title: t
type: profile
tags: [詩織]"), "x
", None, "2026-11-22T10:00:00+09:00").unwrap();
        assert_eq!(g.rel, "10-common/memory-global.md");
        let m = save_memory_with(Layout::V7, &l.0, fm("title: t
type: project
project: shiori
tags: [詩織]"), "x
", None, "2026-11-22T10:00:00+09:00").unwrap();
        assert_eq!(m.rel, "20-projects/shiori/memory.md");
        // type: area は、inboxへ逃がさず断る(記事もmemoryも)
        let area = fm("title: あ
type: area
kind: journal
project: shiori
tags: [詩織]");
        assert!(matches!(save_article_with(Layout::V7, &l.0, "b.md", area, "x
"), Err(StoreError::Invalid(_))));
        assert!(save_memory_with(Layout::V7, &l.0, fm("title: t
type: area
project: shiori
tags: [詩織]"), "x
", None, "n").is_err());
        assert!(!l.0.join("00-inbox").exists() && !l.0.join("20-areas").exists());
        // Legacyでは、いままでどおり
        let f = fm("title: あ
type: area
kind: journal
project: shiori
tags: [詩織]");
        assert_eq!(save_article(&l.0, "c.md", f, "x
").unwrap().rel, "20-areas/shiori/journal/c.md");
    }

}
