//! 記事(frontmatter+本文のmd)の規則と保存。
//!
//! `library/_system/CLAUDE.md` の「配置先の決め方」を実装する(詩織Ver3.0、データ管理法見直し2-2節)。
//! typeの判定・付与はAI(Claude Codeなど)の役目で、ここは決まったtypeを機械的にフォルダへ変換するだけの、
//! 決定的な規則を担う。アプリ・CLI・サーバーが、同じ規則で保存するために、ここに置く。

use crate::fsx::{self, Cond, StoreError};
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
    if fm.title.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Some("titleが空です".to_string());
    }
    if fm.tags.is_empty() {
        return Some("tagsが空です".to_string());
    }
    match fm.kind.as_deref() {
        Some(k) if VALID_TYPES.contains(&k) => {
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
        other => Some(format!("typeが不正です(値: {other:?}, 許可値: {VALID_TYPES:?})")),
    }
}

/// 配置先のフォルダ(libraryからの相対。区切りは`/`)を決める。`validate`を通った記事だけを渡す。
/// project/areaで、projectが未登録なら、`projects.yaml` へ `pending` で登録する。
pub fn decide_destination(root: &Path, fm: &Frontmatter) -> Result<String, StoreError> {
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
                Ok(format!("10-projects/{p}/{}", sub_kind()?))
            }
            None => Ok(INBOX_DIR.to_string()),
        },

        // 終わりのない継続的関心領域。projectフィールドを識別子として流用する
        // (huraru.com運営・詩織開発等、type: projectと同じ命名空間で管理する)。
        "area" => match project {
            Some(p) => {
                registry::ensure_project(root, p)?;
                Ok(format!("20-areas/{p}/{}", sub_kind()?))
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
pub fn save_article(root: &Path, file_name: &str, mut fm: Frontmatter, body: &str) -> Result<SavedArticle, StoreError> {
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

    let (dir, inbox_reason) = match validate(&fm) {
        Some(reason) => {
            fm.reason = Some(reason.clone());
            (INBOX_DIR.to_string(), Some(reason))
        }
        None => (decide_destination(root, &fm)?, None),
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
}
