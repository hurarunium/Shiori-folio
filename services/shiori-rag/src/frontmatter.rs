//! frontmatterの読み取り。`services/rag/indexing.py` と同じく、YAMLとしては読まず、
//! 正規表現で1行ずつ取り出す(同じ記事から同じ値が出るように、正規表現もそろえている)。

use regex::Regex;
use std::sync::LazyLock;

static FRONTMATTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)\A---\r?\n(.*?)\r?\n---\r?\n?").unwrap());
static INDEX_FALSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?mi)^index:\s*false\s*$").unwrap());
static EXTRACT_FALSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?mi)^extract:\s*false\s*$").unwrap());

fn field_re(name: &str) -> Regex {
    Regex::new(&format!(r"(?m)^{name}:\s*(.+?)\s*$")).unwrap()
}

static TITLE: LazyLock<Regex> = LazyLock::new(|| field_re("title"));
static SUMMARY: LazyLock<Regex> = LazyLock::new(|| field_re("summary"));
static STATUS: LazyLock<Regex> = LazyLock::new(|| field_re("status"));
static DATE: LazyLock<Regex> = LazyLock::new(|| field_re("date"));
static PROJECT: LazyLock<Regex> = LazyLock::new(|| field_re("project"));
static TYPE: LazyLock<Regex> = LazyLock::new(|| field_re("type"));
static AUTHOR: LazyLock<Regex> = LazyLock::new(|| field_re("author"));
static KIND: LazyLock<Regex> = LazyLock::new(|| field_re("kind"));
static UPDATED: LazyLock<Regex> = LazyLock::new(|| field_re("updated"));
// 棚の項目。frontmatterのsourceは、チャンクのメタデータ"source"(ファイル名)と名前が
// 衝突するため、メタデータでは"cited_from"に入れる。
static CITED_FROM: LazyLock<Regex> = LazyLock::new(|| field_re("source"));
static LEARNED: LazyLock<Regex> = LazyLock::new(|| field_re("learned"));
static TOPIC: LazyLock<Regex> = LazyLock::new(|| field_re("topic"));

/// frontmatterの中身(`---` の間)。無ければNone。
fn block(text: &str) -> Option<&str> {
    FRONTMATTER.captures(text).and_then(|c| c.get(1)).map(|m| m.as_str())
}

/// 値がダブル/シングルクォートで囲まれていれば剥がす。
pub fn strip_quotes(raw: &str) -> &str {
    let b = raw.as_bytes();
    if b.len() >= 2 && b[0] == b[b.len() - 1] && (b[0] == b'"' || b[0] == b'\'') {
        &raw[1..raw.len() - 1]
    } else {
        raw
    }
}

fn field(re: &Regex, text: &str) -> String {
    block(text)
        .and_then(|fm| re.captures(fm))
        .and_then(|c| c.get(1))
        .map(|m| strip_quotes(m.as_str()).to_string())
        .unwrap_or_default()
}

/// `index: false` なら検索の対象から外す。frontmatterが無ければ対象。
pub fn is_indexable(text: &str) -> bool {
    block(text).is_none_or(|fm| !INDEX_FALSE.is_match(fm))
}

/// 素材の `.meta` に `extract: false` があれば、中身を抽出しない。
pub fn extract_disabled(text: &str) -> bool {
    block(text).is_some_and(|fm| EXTRACT_FALSE.is_match(fm))
}

/// 1つの記事から取り出す値(`indexing.py` の `_embed_file` が使うもの)。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Fields {
    pub title: String,
    pub summary: String,
    pub status: String,
    pub related: Vec<String>,
    pub project: String,
    pub type_: String,
    pub author: String,
    pub kind: String,
    /// 検索の新しさに使う日付(memory・日誌(journal)は、追記・更新のたびに変わる `updated` があればそちら)
    pub recency_date: String,
    /// 棚の記事なら (cited_from, learned, topic)。学んだ日も分野も無い記事は棚の記事ではない
    pub shelf: Option<(String, String, String)>,
}

pub fn read(text: &str) -> Fields {
    let kind = field(&KIND, text);
    // 日誌(P-16、Ver7.0)は何度も追記されるので、作成日(`date`)だけでは新しさが上がらない
    let recency_date = if kind == "memory" || kind == "journal" {
        let updated = field(&UPDATED, text);
        if updated.is_empty() { field(&DATE, text) } else { updated }
    } else {
        field(&DATE, text)
    };
    let learned = field(&LEARNED, text);
    let topic = field(&TOPIC, text);
    let shelf = if learned.is_empty() && topic.is_empty() {
        None
    } else {
        Some((field(&CITED_FROM, text), learned, topic))
    };
    Fields {
        title: field(&TITLE, text),
        summary: field(&SUMMARY, text),
        status: field(&STATUS, text),
        related: related(text),
        project: field(&PROJECT, text),
        type_: field(&TYPE, text),
        author: field(&AUTHOR, text),
        kind,
        recency_date,
        shelf,
    }
}

/// `related` を取り出す。`shiori-save`(serde_yaml)が出す2つの形(ブロックの `- item` 列と、
/// 1行の `[a, b]`)に対応する。
pub fn related(text: &str) -> Vec<String> {
    let Some(fm) = block(text) else { return Vec::new() };
    let lines: Vec<&str> = crate::text::py_splitlines(fm);
    for (i, line) in lines.iter().enumerate() {
        let stripped = line.trim();
        let Some(inline) = stripped.strip_prefix("related:") else { continue };
        let inline = inline.trim();
        if inline.starts_with('[') {
            let inner = inline.trim_matches(|c| c == '[' || c == ']').trim();
            return inner
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| strip_quotes(s.trim()).to_string())
                .collect();
        }
        let mut items = Vec::new();
        for next in &lines[i + 1..] {
            let t = next.trim();
            if let Some(item) = t.strip_prefix("- ") {
                items.push(strip_quotes(item.trim()).to_string());
            } else if t.is_empty() {
                continue;
            } else {
                break;
            }
        }
        return items;
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "---\ntitle: \"A: B\"\ntype: area\nkind: memory\nproject: shiori\nsummary: 要約です\nstatus: new\nrelated:\n- one\n- 'two'\n\ndate: 2026-10-10T20:22:59+09:00\nupdated: 2026-10-23T16:41:41+09:00\n---\n\n## 本文\n";

    #[test]
    fn 項目を取り出す() {
        let f = read(DOC);
        assert_eq!(f.title, "A: B");
        assert_eq!(f.summary, "要約です");
        assert_eq!(f.kind, "memory");
        assert_eq!(f.related, vec!["one", "two"]);
        assert_eq!(f.recency_date, "2026-10-23T16:41:41+09:00");
        assert!(f.shelf.is_none());
        assert!(is_indexable(DOC));
    }

    #[test]
    fn 一行のrelatedと空() {
        assert_eq!(related("---\nrelated: [a, \"b\"]\n---\n"), vec!["a", "b"]);
        assert!(related("---\nrelated: []\n---\n").is_empty());
        assert!(related("本文だけ").is_empty());
    }

    #[test]
    fn indexとextractのfalse() {
        assert!(!is_indexable("---\ntitle: x\nindex: False\n---\n"));
        assert!(extract_disabled("---\nextract: false\n---\n"));
        assert!(!extract_disabled("---\ntitle: x\n---\nextract: false\n"));
    }

    #[test]
    fn 棚の項目() {
        let f = read("---\ntitle: t\nsource: https://x\nlearned: 2026-10-20\ntopic: 数学\n---\n");
        assert_eq!(f.shelf, Some(("https://x".into(), "2026-10-20".into(), "数学".into())));
    }

    #[test]
    fn 日誌も更新日を新しさに使いresourceは作成日のまま() {
        let journal = "---
title: j
type: project
kind: journal
project: shiori
date: 2026-11-22T09:00:00+09:00
updated: 2026-11-22T17:30:00+09:00
---
";
        assert_eq!(read(journal).recency_date, "2026-11-22T17:30:00+09:00");
        // updatedが無い日誌(v6.2までのjournal)は、これまでどおり作成日
        let old = "---
title: j
type: area
kind: journal
date: 2026-10-01T09:00:00+09:00
---
";
        assert_eq!(read(old).recency_date, "2026-10-01T09:00:00+09:00");
        // resourceは、updatedがあっても作成日(これまでどおり)
        let res = "---
title: r
type: project
kind: resource
date: 2026-11-01T09:00:00+09:00
updated: 2026-11-22T09:00:00+09:00
---
";
        assert_eq!(read(res).recency_date, "2026-11-01T09:00:00+09:00");
    }

}
