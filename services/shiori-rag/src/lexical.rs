//! 語句の一致による検索(文字bigramのBM25)。`services/rag/lexical.py` と同じ式。
//! 埋め込み検索が言い回しの違いで取りこぼしたチャンクを、候補に足すために使う。
//!
//! Ver7.0(Phase 1-B、メモリ削減): 語句を `String` で持つ「チャンクごとの表」をやめ、
//! 語句を `u64` の鍵にした「語句 → (チャンクの添字, 出現回数)」の転置索引にした。
//! 点数の足し方の順序(問い合わせの語句の順)と、同点のときの順(添字の大きい方が先)は、
//! 旧実装と同じにしてある(テストの中の旧実装と、点数のビットまで一致することを確かめる)。

use std::collections::HashMap;
use unicode_normalization::UnicodeNormalization;

pub const K1: f64 = 1.5;
pub const B: f64 = 0.75;

/// NFCにそろえ、小文字にし、空白を除いた文字の並び。
fn normalized_chars(text: &str) -> Vec<char> {
    text.nfc()
        .collect::<String>()
        .to_lowercase()
        .chars()
        .filter(|c| !is_py_regex_space(*c))
        .collect()
}

/// 2文字ずつに切る。1文字だけならその1文字。(テストの中の旧実装と比べるためだけに使う)
#[cfg(test)]
pub fn bigrams(text: &str) -> Vec<String> {
    let s = normalized_chars(text);
    if s.len() < 2 {
        return if s.is_empty() { Vec::new() } else { vec![s.iter().collect()] };
    }
    s.windows(2).map(|w| w.iter().collect()).collect()
}

/// 1文字だけの語句を表す、2文字目の代わりの値(文字は0x10FFFF以下なので、2文字の語句とぶつからない)
const SINGLE: u64 = 0xFFFF_FFFF;

/// 語句を1つの数にする(上位32ビットが1文字目、下位32ビットが2文字目)。
fn gram_key(first: char, second: Option<char>) -> u64 {
    ((first as u64) << 32) | second.map_or(SINGLE, |c| c as u64)
}

/// `bigrams` と同じ切り方で、語句を数にして並べる。
fn bigram_keys(text: &str) -> Vec<u64> {
    let s = normalized_chars(text);
    if s.len() < 2 {
        return if s.is_empty() { Vec::new() } else { vec![gram_key(s[0], None)] };
    }
    s.windows(2).map(|w| gram_key(w[0], Some(w[1]))).collect()
}

/// Pythonの正規表現の `\s`(Unicodeの文字列)と同じ判定。
fn is_py_regex_space(c: char) -> bool {
    crate::text::py_isspace(c)
}

pub struct LexicalIndex {
    /// チャンクごとの語句の数(重複を含む)
    len: Vec<u32>,
    avg: f64,
    /// チャンクの数
    n: f64,
    /// 出現する語句の鍵(昇順、重複なし)
    keys: Vec<u64>,
    /// `keys[k]` の出現は `posts[offsets[k]..offsets[k + 1]]`
    offsets: Vec<u32>,
    /// (チャンクの添字, そのチャンクでの出現回数)。同じ語句の中では、添字の小さい順
    posts: Vec<(u32, u32)>,
}

impl LexicalIndex {
    pub fn new(texts: &[String]) -> Self {
        let mut len: Vec<u32> = Vec::with_capacity(texts.len());
        let mut triples: Vec<(u64, u32, u32)> = Vec::new();
        for (i, t) in texts.iter().enumerate() {
            let mut ks = bigram_keys(t);
            len.push(ks.len() as u32);
            ks.sort_unstable();
            let mut j = 0;
            while j < ks.len() {
                let mut k = j + 1;
                while k < ks.len() && ks[k] == ks[j] {
                    k += 1;
                }
                triples.push((ks[j], i as u32, (k - j) as u32));
                j = k;
            }
        }
        triples.sort_unstable();

        let mut keys: Vec<u64> = Vec::new();
        let mut offsets: Vec<u32> = Vec::new();
        let mut posts: Vec<(u32, u32)> = Vec::with_capacity(triples.len());
        for (key, i, f) in triples {
            if keys.last() != Some(&key) {
                keys.push(key);
                offsets.push(posts.len() as u32);
            }
            posts.push((i, f));
        }
        offsets.push(posts.len() as u32);
        keys.shrink_to_fit();
        offsets.shrink_to_fit();

        let avg = if len.is_empty() { 0.0 } else { len.iter().map(|&l| l as f64).sum::<f64>() / len.len() as f64 };
        Self { len, avg, n: texts.len() as f64, keys, offsets, posts }
    }

    /// queryに語句が重なるチャンクの(点数, 添字)。点数0は含まない。
    /// 点数は、問い合わせの語句の順に足す(旧実装と同じ順序なので、結果のビットまで同じ)。
    /// `accept` はメタデータの絞り込み。
    fn scores(&self, query: &str, accept: impl Fn(usize) -> bool) -> Vec<(f64, usize)> {
        let mut acc: HashMap<u32, f64> = HashMap::new();
        for w in bigram_keys(query) {
            let Ok(k) = self.keys.binary_search(&w) else { continue };
            let post = &self.posts[self.offsets[k] as usize..self.offsets[k + 1] as usize];
            let c = post.len() as u32;
            let idf = (1.0 + (self.n - c as f64 + 0.5) / (c as f64 + 0.5)).ln();
            for &(i, f) in post {
                let f = f as f64;
                let ln = self.len[i as usize];
                *acc.entry(i).or_insert(0.0) += idf * f * (K1 + 1.0) / (f + K1 * (1.0 - B + B * ln as f64 / self.avg));
            }
        }
        acc.into_iter()
            .filter(|&(i, s)| s > 0.0 && accept(i as usize))
            .map(|(i, s)| (s, i as usize))
            .collect()
    }

    /// queryに語句が重なるものを、BM25の高い順にn件(添字で)返す。点数0は除く。
    /// `accept` はメタデータの絞り込み。同点はPython版と同じく、添字の大きい方を先にする。
    pub fn top(&self, query: &str, n: usize, accept: impl Fn(usize) -> bool) -> Vec<usize> {
        let mut scored = self.scores(query, accept);
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)));
        scored.into_iter().take(n).map(|(_, i)| i).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧実装(Ver6.2まで)。新しい実装と、点数のビットまで一致することを確かめるために残す。
    struct OldIndex {
        tf: Vec<HashMap<String, u32>>,
        len: Vec<u32>,
        avg: f64,
        idf: HashMap<String, f64>,
    }

    impl OldIndex {
        fn new(texts: &[String]) -> Self {
            let tf: Vec<HashMap<String, u32>> = texts
                .iter()
                .map(|t| {
                    let mut m = HashMap::new();
                    for g in bigrams(t) {
                        *m.entry(g).or_insert(0) += 1;
                    }
                    m
                })
                .collect();
            let len: Vec<u32> = tf.iter().map(|m| m.values().sum()).collect();
            let avg = if len.is_empty() { 0.0 } else { len.iter().map(|&l| l as f64).sum::<f64>() / len.len() as f64 };
            let mut df: HashMap<&str, u32> = HashMap::new();
            for m in &tf {
                for w in m.keys() {
                    *df.entry(w.as_str()).or_insert(0) += 1;
                }
            }
            let n = tf.len() as f64;
            let idf = df
                .into_iter()
                .map(|(w, c)| (w.to_string(), (1.0 + (n - c as f64 + 0.5) / (c as f64 + 0.5)).ln()))
                .collect();
            Self { tf, len, avg, idf }
        }

        fn scores(&self, query: &str, accept: impl Fn(usize) -> bool) -> Vec<(f64, usize)> {
            let qs = bigrams(query);
            let mut scored: Vec<(f64, usize)> = Vec::new();
            for (i, (tf, &ln)) in self.tf.iter().zip(&self.len).enumerate() {
                if !accept(i) {
                    continue;
                }
                let mut s = 0.0;
                for w in &qs {
                    if let Some(&f) = tf.get(w) {
                        let f = f as f64;
                        s += self.idf.get(w).copied().unwrap_or(0.0) * f * (K1 + 1.0)
                            / (f + K1 * (1.0 - B + B * ln as f64 / self.avg));
                    }
                }
                if s > 0.0 {
                    scored.push((s, i));
                }
            }
            scored
        }

        fn top(&self, query: &str, n: usize, accept: impl Fn(usize) -> bool) -> Vec<usize> {
            let mut scored = self.scores(query, accept);
            scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)));
            scored.into_iter().take(n).map(|(_, i)| i).collect()
        }
    }

    /// 決まった順に並ぶ、擬似乱数(再現できるように、種を固定する)
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
            &v[(self.next() as usize) % v.len()]
        }
    }

    fn corpus() -> Vec<String> {
        let words = [
            "起動", "検索", "索引", "改善", "遅い", "速い", "記事", "保存", "詩織", "サーバー", "メモリ", "測定",
            "search", "Index", "Memory", "server", "ペット", "ライブラリ", "日誌", "移行", "ああ", "ああああ", "ー",
            "か\u{3099}く", " ", "\n", "A", "ＡＢＣ", "Ｓｅａｒｃｈ",
        ];
        let mut r = Lcg(12345);
        let mut v: Vec<String> = Vec::new();
        for _ in 0..300 {
            let n = (r.next() % 40) as usize;
            let t: String = (0..n).map(|_| *r.pick(&words)).collect();
            v.push(t);
        }
        // 境界の例
        v.push(String::new());
        v.push("詩".to_string());
        v.push("a".to_string());
        v.push("  \n ".to_string());
        v.push("起動が遅い件の対策".to_string());
        v.push("起動の並列化で起動が速くなった".to_string());
        v
    }

    fn queries() -> Vec<&'static str> {
        vec![
            "起動が遅い", "起動", "search", "SEARCH", "Ｓｅａｒｃｈ", "詩", "a", "ああああ", "ああ", "検索 索引", "メモリの測定",
            "", " ", "存在しない語句ですよ", "起動起動起動", "ー", "か\u{3099}く", "がく", "日誌の移行", "server memory",
        ]
    }

    #[test]
    fn bigramはpythonと同じ() {
        assert_eq!(bigrams("Ab c"), vec!["ab", "bc"]);
        assert_eq!(bigrams("詩"), vec!["詩"]);
        assert!(bigrams(" ").is_empty());
        // NFDの「が」もNFCにそろう
        assert_eq!(bigrams("か\u{3099}く"), vec!["がく"]);
    }

    #[test]
    fn 語句の鍵は文字列の語句と1対1() {
        // 2文字の語句の鍵と、1文字の語句の鍵は、ぶつからない
        assert_ne!(gram_key('a', Some('b')), gram_key('a', None));
        assert_eq!(bigram_keys("Ab c").len(), bigrams("Ab c").len());
        assert_eq!(bigram_keys("詩").len(), 1);
        assert!(bigram_keys(" ").is_empty());
        // 文字列の語句が同じなら、鍵も同じ(NFDとNFC)
        assert_eq!(bigram_keys("か\u{3099}く"), bigram_keys("がく"));
    }

    #[test]
    fn 重なりの多いものが上() {
        let texts: Vec<String> = ["起動が遅い件の対策", "検索の改善", "起動の並列化で起動が速くなった"].map(String::from).to_vec();
        let idx = LexicalIndex::new(&texts);
        let top = idx.top("起動が遅い", 10, |_| true);
        assert_eq!(top[0], 0);
        assert!(!top.contains(&1));
        assert!(idx.top("起動", 10, |i| i != 2).iter().all(|&i| i != 2));
    }

    #[test]
    fn 旧実装と点数のビットまで一致する() {
        let texts = corpus();
        let old = OldIndex::new(&texts);
        let new = LexicalIndex::new(&texts);
        assert_eq!(old.avg.to_bits(), new.avg.to_bits());
        for q in queries() {
            for accept_name in ["全部", "3で割って余り0を除く"] {
                let accept = |i: usize| accept_name == "全部" || i % 3 != 0;
                let mut a = old.scores(q, accept);
                let mut b = new.scores(q, accept);
                a.sort_by_key(|&(_, i)| i);
                b.sort_by_key(|&(_, i)| i);
                assert_eq!(a.len(), b.len(), "件数が違う: 問い合わせ={q:?} {accept_name}");
                for ((sa, ia), (sb, ib)) in a.iter().zip(&b) {
                    assert_eq!(ia, ib, "添字が違う: 問い合わせ={q:?} {accept_name}");
                    assert_eq!(sa.to_bits(), sb.to_bits(), "点数が違う: 問い合わせ={q:?} 添字={ia} {accept_name}");
                }
            }
        }
    }

    #[test]
    fn 旧実装と上位の並びが完全に一致する() {
        let texts = corpus();
        let old = OldIndex::new(&texts);
        let new = LexicalIndex::new(&texts);
        for q in queries() {
            for n in [1usize, 5, 20, 1000] {
                assert_eq!(old.top(q, n, |_| true), new.top(q, n, |_| true), "並びが違う: 問い合わせ={q:?} n={n}");
                assert_eq!(
                    old.top(q, n, |i| i % 2 == 0),
                    new.top(q, n, |i| i % 2 == 0),
                    "絞り込みつきの並びが違う: 問い合わせ={q:?} n={n}"
                );
            }
        }
    }

    #[test]
    fn 空のコーパスでも動く() {
        let new = LexicalIndex::new(&[]);
        assert!(new.top("起動", 10, |_| true).is_empty());
        let old = OldIndex::new(&[]);
        assert!(old.top("起動", 10, |_| true).is_empty());
    }
}
