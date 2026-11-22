//! 記事の一覧・検索結果に添える、属性と冒頭の文(詩織Ver3.9の `read_article_preview` を、サーバーでも使えるように移したもの)。
//! 読めない・frontmatterが壊れている記事は、分かる範囲だけ埋める(一覧の表示を止めない)。

use serde::{Deserialize, Serialize};

/// 冒頭の文として切り出す最大の文字数(一覧では2〜3行に収めて省略表示する)。
pub const PREVIEW_MAX_CHARS: usize = 200;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Preview {
    pub summary: String,
    pub project: String,
    pub entry_kind: String,
    pub status: String,
    pub date: String,
}

#[derive(Debug, Default, Deserialize)]
struct Fm {
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(rename = "kind", default)]
    entry_kind: Option<String>,
    #[serde(default)]
    date: Option<serde_yaml::Value>,
    #[serde(default)]
    status: Option<String>,
}

fn parse_fm(content: &str) -> Option<Fm> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let rest = content.strip_prefix("---\r\n").or_else(|| content.strip_prefix("---\n"))?;
    let end = rest.find("\n---")?;
    serde_yaml::from_str(&rest[..end]).ok()
}

/// 日付(日付だけ・日時・引用符の有無がまちまち)を `YYYY-MM-DD` にする。
fn display_date(value: &serde_yaml::Value) -> String {
    let text = match value {
        serde_yaml::Value::String(s) => s.clone(),
        other => serde_yaml::to_string(other).unwrap_or_default(),
    };
    text.trim().chars().take(10).collect()
}

/// frontmatterより後の本文から、見出し・表・コードなどを除いた最初の文を取り出す。
pub fn body_excerpt(content: &str) -> String {
    let body = content
        .strip_prefix("---")
        .and_then(|rest| rest.find("\n---").map(|end| &rest[end + 4..]))
        .unwrap_or(content);
    let mut excerpt = String::new();
    let mut in_code = false;
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || line.is_empty() || line.starts_with(['#', '|', '<']) || line.starts_with("---") {
            continue;
        }
        let line = line.trim_start_matches(['-', '*', '>', ' ']);
        let line = line.replace("**", "").replace('`', "").replace("[[", "").replace("]]", "");
        if !excerpt.is_empty() {
            excerpt.push(' ');
        }
        excerpt.push_str(&line);
        if excerpt.chars().count() >= PREVIEW_MAX_CHARS {
            break;
        }
    }
    excerpt
}

/// 記事(.md、または素材の.meta)の中身から、一覧用の属性と冒頭の文を作る。
/// 冒頭の文は、frontmatterの `summary`、無ければ本文の最初の数行。
pub fn preview_of(content: &str) -> Preview {
    let fm = parse_fm(content).unwrap_or_default();
    let summary = fm.summary.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| body_excerpt(content));
    Preview {
        summary: summary.chars().take(PREVIEW_MAX_CHARS).collect(),
        project: fm.project.unwrap_or_default(),
        entry_kind: fm.entry_kind.unwrap_or_default(),
        status: fm.status.unwrap_or_else(|| "new".to_string()),
        date: fm.date.as_ref().map(display_date).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatterのsummaryと属性を使う() {
        let p = preview_of("---\ntitle: a\nproject: shiori\nkind: decision\nstatus: done\ndate: 2026-11-22T10:00:00\nsummary: 要点です\n---\n本文");
        assert_eq!(p.summary, "要点です");
        assert_eq!(p.project, "shiori");
        assert_eq!(p.entry_kind, "decision");
        assert_eq!(p.status, "done");
        assert_eq!(p.date, "2026-11-22");
    }

    #[test]
    fn summaryが無ければ本文の最初の文で見出しとコードは飛ばす() {
        let p = preview_of("---\ntitle: a\n---\n# 見出し\n\n```\ncode\n```\n- **太字**の文\n| 表 |\n次の行");
        assert_eq!(p.summary, "太字の文 次の行");
        assert_eq!(p.status, "new", "statusが無ければnew");
    }

    #[test]
    fn frontmatterが無くても壊れても止まらない() {
        assert_eq!(preview_of("ただの文").summary, "ただの文");
        let p = preview_of("---\n: : 壊れ\n---\n本文だけ");
        assert_eq!(p.summary, "本文だけ");
        assert_eq!(p.project, "");
    }

    #[test]
    fn 長い冒頭は200文字までで切る() {
        let body = "あ".repeat(500);
        assert_eq!(preview_of(&format!("---\ntitle: a\n---\n{body}")).summary.chars().count(), 200);
    }

    #[test]
    fn bomつきでも読める() {
        assert_eq!(preview_of("\u{feff}---\nproject: p\n---\n本文").project, "p");
    }
}
