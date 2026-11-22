//! 書き方の規則(Ver7.0、`docs/ver7-writing-rules-draft.md`)の、機械で確かめられる条件。
//!
//! **すべて警告であり、保存は止めない。** 条件のしきい値は、実機のlibrary(journal 168件・memory 17件)に当てて調整した。
//! 長さ(字数・行数)の上限は、設けない。内容の密度(具体的か、重複・再掲が無いか)を見る。
//!
//! | ID | 対象 | 条件 | 重さ |
//! |---|---|---|---|
//! | J1 | 日誌 | 見出しが「申し送り・残りの・未完了・やること…」で始まる(分担の混入) | 通常 |
//! | J2 | 日誌・memory | 子の節を持たない節が30字未満(「なし」「未定」で始まる節は除く)。見出しの無い本文は、全体で見る | 通常 |
//! | J3 | 日誌 | 節の本文に `#`・`##` の見出しがある | 通常 |
//! | D1 | 日誌・memory | 具体的な要素の無い行が、5行以上で7割以上 | 弱い |
//! | D2 | 日誌・memory | 25字以下で、曖昧語を含み、具体的な要素の無い行 | 弱い |
//! | D3 | 日誌・memory | 句点が6つ以上、または300字を超えて句点が2つ以上の行 | 弱い |
//! | D4 | 日誌・memory | 同じ記事の中の重複行。日誌の行と memory の行の再掲(類似度0.6以上) | 通常 |
//! | M1 | memory | 「やること」「申し送り・待ち」の項目に、日付が無い | 弱い |

use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Normal,
    Weak,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Warning {
    pub id: &'static str,
    pub severity: Severity,
    pub message: String,
    /// 該当の行(あれば)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
}

impl Warning {
    fn new(id: &'static str, severity: Severity, message: impl Into<String>, excerpt: Option<&str>) -> Warning {
        Warning { id, severity, message: message.into(), excerpt: excerpt.map(|s| s.chars().take(80).collect()) }
    }

    /// 人が読む1行(`[D4] …`)。
    pub fn to_line(&self) -> String {
        match &self.excerpt {
            Some(e) => format!("[{}] {}: {e}", self.id, self.message),
            None => format!("[{}] {}", self.id, self.message),
        }
    }
}

// ---- 判定の部品 ----

const EXTENSIONS: &str = "rs|md|ts|tsx|py|json|ps1|yaml|yml|toml|html|css|exe|sh|zip|png|csv|txt|js|rtf|lock|log|bak";
const VAGUE: &[&str] = &["いろいろ", "色々", "適宜", "問題なし", "特になし", "その他", "対応した", "いい感じ", "ちゃんと"];
const PUNCT: &[char] = &[
    ' ', '\t', '\r', '\n', '\u{3000}', '\u{3001}', '\u{3002}', ',', '.', '-', '\u{30fb}', '\u{28}', '\u{29}', '\u{ff08}', '\u{ff09}',
    '\u{300c}', '\u{300d}', '`', '*',
];

struct Patterns {
    concrete: Vec<Regex>,
    id_token: Regex,
    handoff: Regex,
    heading: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        concrete: vec![
            Regex::new(r"\d{2,}").unwrap(),
            // 日本語に隣り合っていても拾えるよう、ASCIIだけの境界(?-u:\b)を使う
            Regex::new(r"(?-u:\b)\d{1,2}/\d{1,2}(?-u:\b)").unwrap(),
            Regex::new(r"`[^`]+`").unwrap(),
            Regex::new(&format!(r"[\w\-]+\.(?:{EXTENSIONS})(?-u:\b)")).unwrap(),
            Regex::new(r"[\w\-]+/[\w\-]+").unwrap(),
            Regex::new(r"(?-u:\b)[a-z][a-z0-9_\-]+\s+(?:--?[a-z]|[a-z][a-z0-9_\-]+)(?-u:\b)").unwrap(),
            Regex::new(r"[A-Za-z]+_[A-Za-z_]+|[a-z]+[A-Z][A-Za-z]+|\w+\(\)").unwrap(),
            // 大文字の略語(HTTP・JSON・MCPなど。2文字のUI・AIは、具体的とはみなさない)
            Regex::new(r"[A-Z][a-z]+[A-Z]\w*|[A-Z]{3,}").unwrap(),
        ],
        // 英数字6文字以上の語のうち、英字と数字の両方を含むもの(ID・ハッシュ)を、あとで確かめる
        id_token: Regex::new(r"[A-Za-z0-9\-]{6,}").unwrap(),
        handoff: Regex::new(r"^#{2,6}\s*(?:\d+\.\s*)?(?:申し送り|残りの|残り作業|未完了|やること|TODO|次にやる)").unwrap(),
        heading: Regex::new(r"^(#{1,6})\s").unwrap(),
    })
}

/// 具体的な要素(数字・日付・`コード`・ファイル名・パス・ID・コマンド・関数名らしい語)を含むか。
pub fn is_concrete(line: &str) -> bool {
    let p = patterns();
    if p.concrete.iter().any(|r| r.is_match(line)) {
        return true;
    }
    p.id_token.find_iter(line).any(|m| {
        let t = m.as_str();
        t.chars().any(|c| c.is_ascii_digit()) && t.chars().any(|c| c.is_ascii_alphabetic())
    })
}

/// 空行・見出し・コードブロック・表の罫線を除き、行頭の記号(`- `・`1. `・`> `)を外した、文の行。
pub fn content_lines(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_code = false;
    for raw in body.lines() {
        let s = raw.trim();
        if s.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || s.is_empty() || s.starts_with('#') || s == "---" || s.chars().all(|c| matches!(c, '|' | '-' | ' ' | ':')) {
            continue;
        }
        let s = strip_marker(s);
        if s.chars().count() >= 4 {
            out.push(s.to_string());
        }
    }
    out
}

fn strip_marker(s: &str) -> &str {
    for m in ["- ", "* ", "+ ", "> "] {
        if let Some(rest) = s.strip_prefix(m) {
            return rest.trim_start();
        }
    }
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        if let Some(rest) = s[digits..].strip_prefix(". ") {
            return rest.trim_start();
        }
    }
    s
}

fn bigrams(s: &str) -> BTreeSet<(char, char)> {
    let cs: Vec<char> = s.chars().filter(|c| !PUNCT.contains(c)).collect();
    cs.windows(2).map(|w| (w[0], w[1])).collect()
}

/// 文字2つの切り出しの類似度(0〜1)。
pub fn similarity(a: &str, b: &str) -> f64 {
    let (x, y) = (bigrams(a), bigrams(b));
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    x.intersection(&y).count() as f64 / x.union(&y).count() as f64
}

fn normalized(s: &str) -> String {
    s.chars().filter(|c| !PUNCT.contains(c)).collect()
}

/// 本文を、見出しで節に分ける。戻り値は (見出し行, 見出しの段, 節の本文, 子の節があるか)。
fn sections(body: &str) -> Vec<(String, usize, String, bool)> {
    let p = patterns();
    let mut heads: Vec<(usize, usize, String)> = Vec::new(); // (行番号, 段, 見出し行)
    let lines: Vec<&str> = body.lines().collect();
    let mut in_code = false;
    for (i, l) in lines.iter().enumerate() {
        if l.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        if let Some(c) = p.heading.captures(l) {
            let level = c[1].len();
            if level >= 2 {
                heads.push((i, level, l.to_string()));
            }
        }
    }
    let mut out = Vec::new();
    for (k, (line_no, level, head)) in heads.iter().enumerate() {
        let end = heads.get(k + 1).map(|h| h.0).unwrap_or(lines.len());
        let text = lines[line_no + 1..end].join("\n");
        let has_child = heads.get(k + 1).is_some_and(|h| h.1 > *level);
        out.push((head.clone(), *level, text, has_child));
    }
    out
}

// ---- 規則 ----

/// J1: 見出しが、申し送り・残りの作業の語で始まる。
fn j1(body: &str, out: &mut Vec<Warning>) {
    let p = patterns();
    for l in body.lines().filter(|l| p.handoff.is_match(l)) {
        out.push(Warning::new(
            "J1",
            Severity::Normal,
            "残りの作業・申し送りは、日誌ではなく memory の「やること」「申し送り・待ち」に書く",
            Some(l.trim()),
        ));
    }
}

/// J2: 中身の無い節(子の節を持たない節が30字未満)。見出しの無い本文は、全体で見る。
fn j2(body: &str, out: &mut Vec<Warning>) {
    let count = |s: &str| s.chars().filter(|c| !c.is_whitespace()).count();
    let skip = |s: &str| {
        let t = s.trim_start();
        t.starts_with("なし") || t.starts_with("未定") || t.starts_with("特になし")
    };
    let secs = sections(body);
    if secs.is_empty() {
        if count(body) < 30 && !skip(body) {
            out.push(Warning::new("J2", Severity::Normal, "本文が30字未満です(何をしたかが分かる内容にする)", Some(body.trim())));
        }
        return;
    }
    for (head, _, text, has_child) in secs {
        if !has_child && count(&text) < 30 && !skip(&text) {
            out.push(Warning::new("J2", Severity::Normal, "節の中身が30字未満です", Some(head.trim())));
        }
    }
}

/// J3: 節の本文に `#`・`##` の見出しがある(階層が崩れる。節の中は `###` から)。
fn j3(body: &str, out: &mut Vec<Warning>) {
    let p = patterns();
    let mut in_code = false;
    for l in body.lines() {
        if l.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        if let Some(c) = p.heading.captures(l) {
            if c[1].len() <= 2 {
                out.push(Warning::new("J3", Severity::Normal, "節の中の見出しは `###` から始める", Some(l.trim())));
            }
        }
    }
}

/// D1: 具体的な要素の無い行が、5行以上で7割以上。
fn d1(body: &str, out: &mut Vec<Warning>) {
    let lines = content_lines(body);
    if lines.len() < 5 {
        return;
    }
    let vague: Vec<&String> = lines.iter().filter(|l| !is_concrete(l)).collect();
    if vague.len() * 10 >= lines.len() * 7 {
        out.push(Warning::new(
            "D1",
            Severity::Weak,
            format!(
                "{}行中{}行に、ファイル名・数値・日付・コマンドなどの具体的な要素がありません(何が・どこで、を足せないか)",
                lines.len(),
                vague.len()
            ),
            vague.first().map(|s| s.as_str()),
        ));
    }
}

/// D2: 25字以下で、曖昧語を含み、具体的な要素の無い行。
fn d2(body: &str, out: &mut Vec<Warning>) {
    for l in content_lines(body) {
        if l.chars().count() <= 25 && VAGUE.iter().any(|v| l.contains(v)) && !is_concrete(&l) {
            out.push(Warning::new("D2", Severity::Weak, "曖昧な表現です(何を・どうしたかを書く)", Some(&l)));
        }
    }
}

/// D3: 1行に事実が詰め込まれている(句点が6つ以上、または300字を超えて句点が2つ以上)。
fn d3(body: &str, out: &mut Vec<Warning>) {
    for l in content_lines(body) {
        let kuten = l.matches('。').count();
        if kuten >= 6 || (l.chars().count() > 300 && kuten >= 2) {
            out.push(Warning::new("D3", Severity::Weak, "1行に事実が詰め込まれています(1行1事実に分けられないか)", Some(&l)));
        }
    }
}

/// D4: 同じ記事の中の重複行。
fn d4_duplicate(body: &str, out: &mut Vec<Warning>) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for l in content_lines(body) {
        let n = normalized(&l);
        if n.chars().count() >= 10 {
            let c = seen.entry(n).or_insert(0);
            *c += 1;
            if *c == 2 {
                out.push(Warning::new("D4", Severity::Normal, "同じ行が繰り返されています", Some(&l)));
            }
        }
    }
}

/// D4: 日誌の行が、memory の行とほぼ同じ(再掲)。参照で示す。
fn d4_reprise(body: &str, memory_lines: &[String], out: &mut Vec<Warning>) {
    let mem: Vec<&String> = memory_lines.iter().filter(|l| l.chars().count() >= 20).collect();
    if mem.is_empty() {
        return;
    }
    for l in content_lines(body) {
        if l.chars().count() >= 20 && mem.iter().any(|m| similarity(&l, m) >= 0.6) {
            out.push(Warning::new("D4", Severity::Normal, "memory にある内容の再掲です(参照で示す)", Some(&l)));
        }
    }
}

fn has_date(line: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\(\s*(?:\d{1,2}/\d{1,2}|\d{4}-\d{2}-\d{2})").unwrap()).is_match(line)
}

/// M1: 「やること」「申し送り・待ち」の項目に、日付が無い。
fn m1(body: &str, out: &mut Vec<Warning>) {
    let mut sec = "";
    for raw in body.lines() {
        if let Some(h) = raw.strip_prefix("## ") {
            sec = h.trim();
        } else if let Some(item) = raw.strip_prefix("- ") {
            if (sec == "やること" || sec == "申し送り・待ち")
                && !["なし", "未定", "特になし"].iter().any(|p| item.trim().starts_with(p))
                && !has_date(raw)
            {
                out.push(Warning::new("M1", Severity::Weak, "いつから残っているかが分かるよう、末尾に日付 `(11/22)` を付ける", Some(raw)));
            }
        }
    }
}

/// 日誌の節(1回の追記の本文)を点検する。`memory_lines` は、同じプロジェクトの memory の行(再掲の確認用。無ければ空)。
pub fn lint_journal_section(content: &str, memory_lines: &[String]) -> Vec<Warning> {
    let mut w = Vec::new();
    j1(content, &mut w);
    j2(content, &mut w);
    j3(content, &mut w);
    d1(content, &mut w);
    d2(content, &mut w);
    d3(content, &mut w);
    d4_duplicate(content, &mut w);
    d4_reprise(content, memory_lines, &mut w);
    w
}

/// memoryの本文(frontmatterを除く)を点検する。長さの上限は、設けない。
pub fn lint_memory(body: &str) -> Vec<Warning> {
    let mut w = Vec::new();
    j2(body, &mut w);
    d1(body, &mut w);
    d2(body, &mut w);
    d3(body, &mut w);
    d4_duplicate(body, &mut w);
    m1(body, &mut w);
    w
}

/// 前の版にあって新しい版に無い、memoryの項目(`- ` の行)。**書き換えられた項目**(新しい版に、類似度0.6以上の行がある)は除く。
/// 外した項目は、サーバーが日誌へ記録する(失われないように)。
pub fn removed_items(old_body: &str, new_body: &str) -> Vec<String> {
    let new_items: Vec<&str> = new_body.lines().filter(|l| l.starts_with("- ")).collect();
    let new_norm: BTreeSet<String> = new_items.iter().map(|l| normalized(l)).collect();
    old_body
        .lines()
        .filter(|l| l.starts_with("- "))
        .filter(|l| !new_norm.contains(&normalized(l)))
        .filter(|l| !new_items.iter().any(|n| similarity(l, n) >= 0.6))
        .map(|l| l.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(w: &[Warning]) -> Vec<&'static str> {
        let mut v: Vec<&'static str> = w.iter().map(|x| x.id).collect();
        v.dedup();
        v
    }

    #[test]
    fn 具体的な要素の判定() {
        for yes in [
            "`journal.rs` を直した。",
            "11/22に試験した。",
            "cargo test が通った。",
            "task-20261122-140917-de8 を削除した。",
            "library_store の journal を直した",
            "20-projects/shiori に置く",
            "コミット 4fd73a0 で入れた",
            "2026-11-22 に確認した",
            "HTTPの窓口を作った",
        ] {
            assert!(is_concrete(yes), "具体的なはず: {yes}");
        }
        for no in ["いろいろ直した。", "ペットの方針を整理した。", "問題なしだった。", "ふらるが確認した。"] {
            assert!(!is_concrete(no), "具体語なしのはず: {no}");
        }
    }

    #[test]
    fn 悪い日誌は拾い具体的な日誌は拾わない() {
        let bad = "### やったこと\n\nいろいろ直した。\n色々調整した。\n適宜対応した。\n問題なし。\n";
        let w = lint_journal_section(bad, &[]);
        assert!(ids(&w).contains(&"D2"), "{w:?}");
        let good = "### やったこと\n\n`journal.rs` で、先頭の BOM を本文に残さないようにした(PowerShell の `Set-Content` が付けるため)。\n\n### 起きたこと\n\n目視テストで、節の先頭に見えない文字が入っているのを見つけた。試験を1件足して、112件が通った。\n";
        assert_eq!(lint_journal_section(good, &[]), vec![], "良い例は警告なし");
    }

    #[test]
    fn j1_申し送りの見出しで始まるものだけ() {
        assert_eq!(ids(&lint_journal_section("### 次にやること\n\n- 設計を読む(11/22)と実装する(12/01)ところまでを、`design-shiori-ver7.md` の Phase 2 に沿って進める\n", &[])), vec!["J1"]);
        assert_eq!(ids(&lint_journal_section("### 残り作業\n\n`a.rs` を直す、`b.rs` を直す、`c.rs` を直す。そのあと `cargo test` で全体の試験を流す\n", &[])), vec!["J1"]);
        // 見出しの途中に含むだけでは、警告しない
        let w = lint_journal_section("### memoryの「やること」を確かめた\n\n`memory.md` の「やること」が、実際の状態と合っているかを、`git log` と見比べた。\n", &[]);
        assert!(!ids(&w).contains(&"J1"), "{w:?}");
    }

    #[test]
    fn j2_子のある親は判定せず空の節と短い本文を拾う() {
        let parent = "## 行ったこと\n\n### 実装\n\n`library_store` の `journal.rs` を直して、試験を足した。ファイルは3つ。\n";
        assert!(!ids(&lint_journal_section(parent, &[])).contains(&"J2"));
        assert_eq!(ids(&lint_journal_section("### 気づき・教訓\n\nなし\n\n### やったこと\n\nx\n", &[])), vec!["J2"]);
        // 「なし」だけの節は、警告しない
        assert!(lint_journal_section("### 気づき・教訓\n\nなし\n\n### やったこと\n\n`journal.rs` で、先頭の BOM を本文に残さないようにした。試験が112件通った。\n", &[]).is_empty());
        // 見出しの無い本文は、全体で見る
        assert_eq!(ids(&lint_journal_section("やった。", &[])), vec!["J2"]);
    }

    #[test]
    fn j3_節の中の大きな見出し() {
        let w = lint_journal_section("## 大きな見出し\n\n`a.rs` を直して、`b.rs` の試験を足した。確認の結果は、112件が通った。\n", &[]);
        assert!(ids(&w).contains(&"J3"), "{w:?}");
        // コードブロックの中の # は、見出しではない
        assert!(!ids(&lint_journal_section("```\n# コメント\n```\n`a.rs` を直して、`b.rs` の試験を足した。112件が通った。\n", &[])).contains(&"J3"));
    }

    #[test]
    fn d1_具体語の無い行が5行以上で7割以上のとき() {
        let abstract5 = "方針を整理した。\n進め方を決めた。\n理由を確かめた。\n次を考えた。\n全体を見直した。\n";
        assert!(ids(&lint_journal_section(abstract5, &[])).contains(&"D1"));
        let four = "方針を整理した。\n進め方を決めた。\n理由を確かめた。\n次を考えた。\n";
        assert!(!ids(&lint_journal_section(four, &[])).contains(&"D1"), "4行では警告しない");
        let w = lint_journal_section(abstract5, &[]);
        assert_eq!(w.iter().find(|x| x.id == "D1").unwrap().severity, Severity::Weak);
    }

    #[test]
    fn d2_短い曖昧な行だけ() {
        assert!(ids(&lint_journal_section("いろいろ直した。\n`a.rs` を直した、`b.rs` も直した、`c.rs` も直した。ファイルは3つ。\n", &[])).contains(&"D2"));
        // 具体的な文は、曖昧語を含んでも拾わない
        assert!(!ids(&lint_journal_section("ふらるが開発版で、タスクの画面とホームの札を見て、問題なしの回答だった。11/06のことで、`tasks.rs` の件。\n", &[])).contains(&"D2"));
    }

    #[test]
    fn d3_詰め込みの行() {
        let seven = "A を直した。B を直した。C を直した。D を直した。E を直した。F を直した。G を直した。H。";
        assert!(ids(&lint_journal_section(seven, &[])).contains(&"D3"));
        let five = "A を直した。B を直した。C を直した。D を直した。E を直した。`a.rs` の件";
        assert!(!ids(&lint_journal_section(five, &[])).contains(&"D3"), "句点5つは警告しない");
    }

    #[test]
    fn d4_重複と再掲() {
        let w = lint_journal_section("ライブラリの書き込みを library_store に集めた。\nライブラリの書き込みを library_store に集めた。\n", &[]);
        assert!(ids(&w).contains(&"D4"));
        let mem = vec!["Phase 2: services/shiori-proto-server の窓口を本番の形にする(子のPID引き継ぎと、1分おきの自動起動を含む)。(11/22)".to_string()];
        let w = lint_journal_section("Phase 2: services/shiori-proto-server の窓口を本番の形にする。子のPID引き継ぎと1分おきの自動起動を含む。\n", &mem);
        assert!(w.iter().any(|x| x.id == "D4" && x.message.contains("再掲")), "{w:?}");
        // 別の内容は、再掲ではない
        assert!(lint_journal_section("`journal.rs` の BOM の不具合を、目視テストで見つけて直した。112件が通った。\n", &mem).is_empty());
    }

    #[test]
    fn memoryは長さで警告せず日付の無い項目を知らせる() {
        let big: String = (0..300).map(|i| format!("- 項目{i}: `file{i}.rs` の状態を記録した(11/22)\n")).collect();
        let body = format!("## 概要\n\n{big}");
        assert!(lint_memory(&body).is_empty(), "長くても警告しない(上限は設けない)");

        let body = "## やること\n\n- 設計を読む(11/22)\n- 実装する\n- なし\n\n## 申し送り・待ち\n\n- Macで確認を待つ。日付は(11/20)にある\n- 未定(10/10時点で決まったことは無い)\n- 取り外せないときは `Kernel-PnP` のID 225で調べる\n\n## 約束事\n\n- 日付が無くてもよい節\n";
        let w = lint_memory(body);
        let m1: Vec<&str> = w.iter().filter(|x| x.id == "M1").filter_map(|x| x.excerpt.as_deref()).collect();
        assert_eq!(m1, vec!["- 実装する", "- 取り外せないときは `Kernel-PnP` のID 225で調べる"], "{w:?}");
    }

    #[test]
    fn 外した項目は書き換えられたものを除いて見つける() {
        let old = "## やること\n\n- Phase 1(`library_store`)を進める(11/22)\n- 4-6の2回目を集計し、合否を決める(11/22)\n- 外のネットワークから、4-1・4-8を試す。(11/20)\n";
        let new = "## やること\n\n- Phase 1(`library_store`)を進める(11/23)\n- 外のネットワークから、4-1・4-8を試す。(11/20)\n";
        // 「Phase 1…」は日付だけが変わったので、書き換え。「4-6…」だけが、外された
        assert_eq!(removed_items(old, new), vec!["- 4-6の2回目を集計し、合否を決める(11/22)".to_string()]);
        assert!(removed_items(old, old).is_empty());
        assert!(removed_items("", new).is_empty());
    }
}
