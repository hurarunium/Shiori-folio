//! library(と棚)の記事・素材の説明(`.meta`)を、索引へ差分で同期する(`services/rag/indexing.py` と同じ手順)。
//!
//! - 起動時に1回だけ全部を見て、前回からの新規・変更・削除だけを反映する(`sync`)
//! - 保存の直後は、`shiori-save` が `/reindex_file` で1件だけを登録し直す(`reindex_one`)
//! - 埋め込みの作り方を変えたら `SCHEMA_VERSION` を上げる。版が違えば全部を作り直す

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::UNIX_EPOCH;

use serde_json::{Map, Value, json};

use crate::chunking::{Chunk, chunk_markdown, split_section};
use crate::embed::Embedder;
use crate::errors;
use crate::extract;
use crate::filter::Meta;
use crate::frontmatter;
use crate::lexical::LexicalIndex;
use crate::log;
use crate::passage::passage_text;
use crate::store::{Collection, Record};
use crate::text::read_text;

/// 索引の形の版。Python版の5(memoryのsourceとupdated)までと同じ中身を、新しい置き場で1から始める。
pub const SCHEMA_VERSION: u32 = 1;
const STATE_FILE: &str = ".index_state.json";
const SCHEMA_FILE: &str = ".index_schema";
const PROGRESS_FILE: &str = ".index_progress.json";
const WARNINGS_FILE: &str = ".extract_warnings.json";
const EXTRACT_CACHE_DIR: &str = "extract-cache";
const META_SUFFIX: &str = ".meta";
const MEMORY_FILE_NAME: &str = "memory.md";
const MEMORY_PARENT_CATEGORIES: &[&str] = &["10-projects", "20-areas"];

/// 1つのコレクション(libraryか棚)の、索引と、その元のフォルダ。
pub struct Index {
    pub name: &'static str,
    /// 記事の置き場(library/ か knowledge/)
    pub knowledge_dir: PathBuf,
    /// 索引の置き場
    pub state_dir: PathBuf,
    pub collection: RwLock<Collection>,
    lexical: Mutex<Option<Arc<LexicalCache>>>,
    /// 書き換えを1つずつにする(起動時の同期と `/reindex_file` が重ならないように)
    write_lock: Mutex<()>,
}

/// 語句の一致の索引と、それを作ったときのチャンク(添字で対応する)。
pub struct LexicalCache {
    pub index: LexicalIndex,
    pub records: Vec<Record>,
}

impl Index {
    pub fn open(name: &'static str, knowledge_dir: PathBuf, state_dir: PathBuf) -> std::io::Result<Self> {
        let (collection, report) = Collection::open(&state_dir)?;
        let index = Self {
            name,
            knowledge_dir,
            state_dir,
            collection: RwLock::new(collection),
            lexical: Mutex::new(None),
            write_lock: Mutex::new(()),
        };
        if report.broken_files > 0 {
            // 壊れていた記事の名前は分からないので、索引に無い記事を「未登録」に戻して作り直させる
            let col = index.collection.read().unwrap();
            let mut state = index.load_state();
            state.retain(|rel, _| col.has_source(&source_name_for(rel)));
            drop(col);
            index.save_state(&state);
        }
        Ok(index)
    }

    /// 語句の一致の索引(書き換えがあれば作り直す)。
    pub fn lexical(&self) -> Arc<LexicalCache> {
        let mut guard = self.lexical.lock().unwrap();
        if let Some(c) = guard.as_ref() {
            return c.clone();
        }
        let col = self.collection.read().unwrap();
        // 語句の一致では埋め込みを使わないので、いちばん大きいベクトルは写さない(1件あたり約3KB)
        let records: Vec<Record> = col
            .records()
            .map(|r| Record { id: r.id.clone(), document: r.document.clone(), metadata: r.metadata.clone(), embedding: Vec::new() })
            .collect();
        drop(col);
        let texts: Vec<String> = records.iter().map(|r| passage_of(r)).collect();
        let cache = Arc::new(LexicalCache { index: LexicalIndex::new(&texts), records });
        *guard = Some(cache.clone());
        cache
    }

    fn mark_dirty(&self) {
        *self.lexical.lock().unwrap() = None;
    }

    fn load_state(&self) -> BTreeMap<String, f64> {
        fs::read_to_string(self.state_dir.join(STATE_FILE))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save_state(&self, state: &BTreeMap<String, f64>) {
        let path = self.state_dir.join(STATE_FILE);
        if let Err(e) = write_atomic(&path, serde_json::to_string_pretty(state).unwrap_or_default().as_bytes()) {
            log::error(errors::RAG_004.code, errors::RAG_004.message, &[("場所", &path.display()), ("理由", &e)]);
        }
    }

    fn load_schema(&self) -> u32 {
        fs::read_to_string(self.state_dir.join(SCHEMA_FILE)).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(0)
    }

    pub fn load_warnings(&self) -> Map<String, Value> {
        fs::read_to_string(self.state_dir.join(WARNINGS_FILE))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// 素材の抽出の結果を記録する。reasonがNone(抽出できた)なら、前回までの記録を消す。
    fn record_extract_status(&self, rel: &str, reason: Option<&str>, warn: bool) {
        let mut records = self.load_warnings();
        match reason {
            None => {
                if records.remove(rel).is_none() {
                    return;
                }
            }
            Some(reason) => {
                records.insert(rel.to_string(), json!({"reason": reason, "warn": warn}));
                if warn {
                    log::warn(errors::RAG_009.code, "素材の抽出を止めた", &[("場所", &rel), ("理由", &reason)]);
                }
            }
        }
        let _ = write_atomic(
            &self.state_dir.join(WARNINGS_FILE),
            serde_json::to_string_pretty(&records).unwrap_or_default().as_bytes(),
        );
    }

    fn write_progress(&self, phase: &str, done: usize, total: usize) {
        let _ = fs::write(
            self.state_dir.join(PROGRESS_FILE),
            json!({"phase": phase, "done": done, "total": total}).to_string(),
        );
    }

    fn clear_progress(&self) {
        let _ = fs::remove_file(self.state_dir.join(PROGRESS_FILE));
    }

    /// 起動時の同期。変更が無ければ、埋め込みのサーバーには問い合わせない。
    pub fn sync(&self, embedder: &Embedder) -> SyncReport {
        let _w = self.write_lock.lock().unwrap();
        let mut state = self.load_state();
        let schema_changed = self.load_schema() != SCHEMA_VERSION;
        if schema_changed {
            state.clear();
        }
        let current: BTreeMap<String, PathBuf> = list_indexable_files(&self.knowledge_dir)
            .into_iter()
            .map(|p| (rel_of(&p, &self.knowledge_dir), p))
            .collect();
        let current_names: HashSet<String> = current.keys().map(|r| source_name_for(r)).collect();
        let mut report = SyncReport::default();

        if schema_changed {
            // 状態を捨てたので、消えたファイルのチャンクを「削除」として拾えない。現存しない名前のものをまとめて消す
            let mut col = self.collection.write().unwrap();
            let stale: Vec<String> = col.sources().filter(|s| !current_names.contains(*s)).cloned().collect();
            for s in stale {
                let _ = col.delete_source(&s);
            }
            drop(col);
            // 版の印は、登録を始める前に保存する(途中で止まっても、次は続きから登録する)
            self.save_state(&state);
            let _ = write_atomic(&self.state_dir.join(SCHEMA_FILE), SCHEMA_VERSION.to_string().as_bytes());
        }

        let gone: Vec<String> = state.keys().filter(|r| !current.contains_key(*r)).cloned().collect();
        for rel in gone {
            // 同じ名前のファイルが別の場所にある(移動した)ときは、チャンクを消さず状態だけ消す
            if !current_names.contains(&source_name_for(&rel)) {
                let _ = self.collection.write().unwrap().delete_source(&source_name_for(&rel));
            }
            self.record_extract_status(&rel, None, false);
            state.remove(&rel);
            report.removed.push(rel);
        }

        let todo: Vec<(String, PathBuf, f64)> = current
            .iter()
            .filter_map(|(rel, p)| {
                let m = state_mtime(p)?;
                (state.get(rel) != Some(&m)).then(|| (rel.clone(), p.clone(), m))
            })
            .collect();
        let phase = if schema_changed { "rebuilding" } else { "syncing" };
        if !todo.is_empty() {
            log::info(self.name, "索引を同期する", &[("件数", &todo.len()), ("種類", &phase)]);
        }
        for (done, (rel, path, mtime)) in todo.iter().enumerate() {
            self.write_progress(phase, done, todo.len());
            let prior = state.contains_key(rel);
            match self.embed_file(embedder, path, rel) {
                Ok(_) => {}
                Err(EmbedError::Read(e)) => {
                    // 1つの記事が読めなくても、全体(起動)を巻き込まない。その記事が検索に出ないだけにする
                    log::warn(errors::RAG_005.code, errors::RAG_005.message, &[("場所", rel), ("理由", &e)]);
                    continue;
                }
                Err(EmbedError::Other(e)) => {
                    log::error(errors::RAG_006.code, "記事を登録できない", &[("場所", rel), ("理由", &e)]);
                    continue;
                }
            }
            if prior { report.updated.push(rel.clone()) } else { report.added.push(rel.clone()) }
            state.insert(rel.clone(), *mtime);
            // 1件ごとに状態を保存し、途中で止まっても登録済みの分は次に引き継ぐ
            self.save_state(&state);
        }
        self.clear_progress();
        if !report.removed.is_empty() || schema_changed {
            self.save_state(&state);
        }
        self.mark_dirty();
        report
    }

    /// 保存の直後に、1件だけを登録し直す。ファイルが無ければNotFound。戻り値は登録したチャンクの数。
    pub fn reindex_one(&self, embedder: &Embedder, relative_path: &str) -> Result<usize, ReindexError> {
        // libraryの中の絶対パスで呼ばれても、状態のキーは同期と同じ相対の名前にする
        let path = self.knowledge_dir.join(relative_path.replace('\\', "/"));
        if !path.is_file() {
            return Err(ReindexError::NotFound);
        }
        // 名前は、確かめたのと同じ正規化したパスから作る(`sub/../a.md` を別の名前で登録しない)。
        // 読むのは従来どおり渡されたパス(正規化でファイル名の濁点の形が変わっても、これまでと同じに扱う)。
        let rel = reindex_rel(&path, &self.knowledge_dir).ok_or(ReindexError::Rejected)?;
        let _w = self.write_lock.lock().unwrap();
        let count = self.embed_file(embedder, &path, &rel).map_err(|e| ReindexError::Failed(e.to_string()))?;
        let mut state = self.load_state();
        if let Some(m) = state_mtime(&path) {
            state.insert(rel, m);
        }
        self.save_state(&state);
        self.mark_dirty();
        Ok(count)
    }

    /// メモの即時登録(`/add_document`)。同じIDなら上書き。
    pub fn add_document(&self, embedder: &Embedder, id: &str, text: &str, heading: &str, source: &str, category: &str) -> Result<(), String> {
        let embed_text = if !heading.is_empty() && heading != crate::chunking::NO_HEADING { format!("{heading}: {text}") } else { text.to_string() };
        let embedding = embedder.embed(&embed_text, false)?;
        let mut metadata = Map::new();
        metadata.insert("source".into(), source.into());
        metadata.insert("heading".into(), heading.into());
        metadata.insert("source_category".into(), category.into());
        let record = Record { id: id.into(), document: text.into(), metadata, embedding };
        let _w = self.write_lock.lock().unwrap();
        self.collection.write().unwrap().upsert(record).map_err(|e| e.to_string())?;
        self.mark_dirty();
        Ok(())
    }

    /// 1つの記事のチャンクを作り、埋め込み、索引の中のその記事の分を丸ごと置き換える。
    fn embed_file(&self, embedder: &Embedder, path: &Path, rel: &str) -> Result<usize, EmbedError> {
        let source = source_name_for(rel);
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let is_memory = source != file_name;
        let stem = if is_memory {
            format!("memory@{}", source.split('/').next().unwrap_or(""))
        } else if file_name.ends_with(META_SUFFIX) {
            // 「X.zip.meta」のstemは「X.zip」で、X.zip.mdと重なりうるので、.metaは名前全体を使う
            file_name.clone()
        } else {
            Path::new(&file_name).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()
        };
        let category = source_category_for(rel);

        let text = match read_text(path) {
            Ok(t) => t,
            Err(e) => {
                // Python版と同じく、読む前に古いチャンクは消えている扱いにする
                let _ = self.collection.write().unwrap().delete_source(&source);
                return Err(EmbedError::Read(e.to_string()));
            }
        };
        let mut records = Vec::new();
        if frontmatter::is_indexable(&text) {
            let chunks = chunk_markdown(&text);
            if !chunks.is_empty() {
                let f = frontmatter::read(&text);
                let base = base_metadata(&f, &source, &category, rel, path);
                for (i, chunk) in chunks.iter().enumerate() {
                    let embedding = embedder
                        .embed(&passage_text(&f.title, &f.summary, &chunk.heading, &chunk.text), false)
                        .map_err(EmbedError::Other)?;
                    let mut metadata = base.clone();
                    metadata.insert("heading".into(), chunk.heading.clone().into());
                    records.push(Record { id: format!("{category}-{stem}-{i}"), document: chunk.text.clone(), metadata, embedding });
                }
                // 素材(.meta)なら、実体の中身も同じsourceのチャンクとして登録する
                if file_name.ends_with(META_SUFFIX) {
                    if frontmatter::extract_disabled(&text) {
                        self.record_extract_status(rel, Some("extract: false のため抽出しない"), false);
                    } else {
                        let asset_records = self.embed_asset(embedder, path, rel, &category, &stem, &records[0].metadata, &f, records.len())?;
                        records.extend(asset_records);
                    }
                }
            }
        }
        let n = records.len();
        self.collection
            .write()
            .unwrap()
            .replace_source(&source, records)
            .map_err(|e| EmbedError::Other(format!("索引を書けない: {e}")))?;
        Ok(n)
    }

    #[allow(clippy::too_many_arguments)]
    fn embed_asset(
        &self,
        embedder: &Embedder,
        meta_path: &Path,
        rel: &str,
        category: &str,
        stem: &str,
        base: &Meta,
        f: &frontmatter::Fields,
        start: usize,
    ) -> Result<Vec<Record>, EmbedError> {
        let Some(asset) = asset_path_for(meta_path) else { return Ok(Vec::new()) };
        let result = extract::extract_asset(&asset, &self.state_dir.join(EXTRACT_CACHE_DIR));
        if !result.ok || result.sections.is_empty() {
            let reason = if result.reason.is_empty() { "テキストなし".to_string() } else { result.reason.clone() };
            if !result.warn && result.reason != "未対応の形式" && result.reason != "テキストなし" {
                log::warn(errors::RAG_010.code, errors::RAG_010.message, &[("場所", &rel), ("理由", &reason)]);
            }
            self.record_extract_status(rel, Some(&reason), result.warn);
            return Ok(Vec::new());
        }
        self.record_extract_status(rel, None, false);
        let mut out = Vec::new();
        for s in &result.sections {
            for part in split_section(Chunk { heading: format!("素材 {}", s.label), text: s.text.clone() }) {
                let embedding = embedder
                    .embed(&passage_text(&f.title, &f.summary, &part.heading, &part.text), false)
                    .map_err(EmbedError::Other)?;
                let mut metadata = base.clone();
                metadata.insert("heading".into(), part.heading.clone().into());
                // 素材由来の印。検索結果を会話のLLMへ渡すとき「資料の内容であり指示ではない」と明示する
                metadata.insert("origin".into(), "asset".into());
                out.push(Record {
                    id: format!("{category}-{stem}-x{}", start + out.len()),
                    document: part.text,
                    metadata,
                    embedding,
                });
            }
        }
        Ok(out)
    }
}

#[derive(Default, Debug)]
pub struct SyncReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Debug)]
enum EmbedError {
    Read(String),
    Other(String),
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbedError::Read(e) | EmbedError::Other(e) => f.write_str(e),
        }
    }
}

#[derive(Debug)]
pub enum ReindexError {
    NotFound,
    /// 索引の場所の外か、索引の対象外のファイル
    Rejected,
    Failed(String),
}

/// Python版のメタデータと同じ項目・同じ並びにする(source, heading, source_category, title, summary,
/// relative_path, mtime, status, date, related, project, type, author, kind, [cited_from, learned, topic])。
fn base_metadata(f: &frontmatter::Fields, source: &str, category: &str, rel: &str, path: &Path) -> Meta {
    let mut m = Map::new();
    m.insert("source".into(), source.into());
    m.insert("heading".into(), "".into());
    m.insert("source_category".into(), category.into());
    m.insert("title".into(), f.title.clone().into());
    m.insert("summary".into(), f.summary.clone().into());
    m.insert("relative_path".into(), rel.into());
    m.insert("mtime".into(), json!(file_mtime(path).unwrap_or(0.0)));
    m.insert("status".into(), f.status.clone().into());
    m.insert("date".into(), f.recency_date.clone().into());
    m.insert("related".into(), f.related.join("|").into());
    m.insert("project".into(), f.project.clone().into());
    m.insert("type".into(), f.type_.clone().into());
    m.insert("author".into(), f.author.clone().into());
    m.insert("kind".into(), f.kind.clone().into());
    if let Some((cited, learned, topic)) = &f.shelf {
        m.insert("cited_from".into(), cited.clone().into());
        m.insert("learned".into(), learned.clone().into());
        m.insert("topic".into(), topic.clone().into());
    }
    m
}

/// 検索用の本文(語句の一致・リランクに使う)。
pub fn passage_of(r: &Record) -> String {
    let s = |k: &str| r.metadata.get(k).and_then(Value::as_str).unwrap_or("");
    passage_text(s("title"), s("summary"), s("heading"), &r.document)
}

/// library/からの相対パスから、メタデータの "source" に入れる名前を決める。
/// 通常はファイル名。memory(`10-projects|20-areas/{project}/memory.md`)だけは `{project}/memory.md`。
pub fn source_name_for(relative_path: &str) -> String {
    let rel = relative_path.replace('\\', "/");
    let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() == 3 && MEMORY_PARENT_CATEGORIES.contains(&parts[0]) && parts[2] == MEMORY_FILE_NAME {
        return format!("{}/{MEMORY_FILE_NAME}", parts[1]);
    }
    parts.last().copied().unwrap_or("").to_string()
}

/// library/直下のサブフォルダ名。直下のファイルは "uncategorized"。
pub fn source_category_for(rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.len() > 1 { parts[0].to_string() } else { "uncategorized".into() }
}

/// `.meta` に対応する実体(`.meta` を除いた名前のファイル)。無ければNone。
pub fn asset_path_for(meta_path: &Path) -> Option<PathBuf> {
    let name = meta_path.file_name()?.to_str()?;
    let asset = meta_path.with_file_name(name.strip_suffix(META_SUFFIX)?);
    asset.is_file().then_some(asset)
}

fn rel_of(path: &Path, base: &Path) -> String {
    index_key(
        &path
            .strip_prefix(base)
            .unwrap_or(path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// 索引の中での記事の名前(状態のキーと `source`)。濁点などを1文字にまとめた形(NFC)にそろえる。
/// MacはexfatのSSDの名前を分けた形(NFD)で返し、Windowsはまとめた形で返すので、そろえないと
/// OSを切り替えるたびに同じ記事を「消して足す」ことになる(段階6、別のWindowsのPCで見つかった)。
/// ファイルを開くときは、この名前ではなく、見つけたときのパスをそのまま使う。
fn index_key(rel: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    rel.replace('\\', "/").nfc().collect()
}

/// 索引の対象(`*.md` と `*.meta`)。`_system/` の下と、`._` で始まるもの(macOSが作る付属ファイル)は除く。
pub fn list_indexable_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() && is_indexable_path(&p, dir) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// `/reindex_file` で受け付けるなら、正規化したパスから作った、同期と同じ相対の名前を返す。
/// `../` や別の場所の絶対パスで、索引の場所の外のファイルを入れさせない。
/// 同期の対象外(`_system/`、`._`、md・meta以外)も、同期と同じ基準で断る。
fn reindex_rel(path: &Path, base: &Path) -> Option<String> {
    let p = path.canonicalize().ok()?;
    let base = base.canonicalize().ok()?;
    (p.starts_with(&base) && is_indexable_path(&p, &base)).then(|| rel_of(&p, &base))
}

fn is_indexable_path(path: &Path, base: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ext_ok = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e == "md" || e == "meta");
    ext_ok && !name.starts_with("._") && !rel_of(path, base).split('/').any(|p| p == "_system")
}

fn file_mtime(path: &Path) -> Option<f64> {
    let m = fs::metadata(path).ok()?.modified().ok()?;
    Some(m.duration_since(UNIX_EPOCH).ok()?.as_secs_f64())
}

/// 同期の状態に残す更新時刻。素材(.meta)は、実体だけが差し替わっても気づけるよう、新しい方を使う。
fn state_mtime(path: &Path) -> Option<f64> {
    let m = file_mtime(path)?;
    Some(match asset_path_for(path).and_then(|a| file_mtime(&a)) {
        Some(a) => m.max(a),
        None => m,
    })
}

pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp-write");
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sourceの名前() {
        assert_eq!(source_name_for("20-areas/shiori/memory.md"), "shiori/memory.md");
        assert_eq!(source_name_for("20-areas\\shiori\\memory.md"), "shiori/memory.md");
        assert_eq!(source_name_for("20-areas/shiori/resource/memory.md"), "memory.md");
        assert_eq!(source_name_for("40-profile/memory-global.md"), "memory-global.md");
        assert_eq!(source_category_for("a.md"), "uncategorized");
        assert_eq!(source_category_for("30-resources/skills/x.md"), "30-resources");
    }

    #[test]
    fn 更新時刻は読み書きで変わらない() {
        // serde_jsonのfloat_roundtripが無いと、最後の1桁がずれて、毎回の起動で登録し直してしまう
        let state: BTreeMap<String, f64> =
            (0..2000).map(|i| (i.to_string(), 1_790_000_000.0 + i as f64 * 0.123_456_789_1)).collect();
        let back: BTreeMap<String, f64> = serde_json::from_str(&serde_json::to_string_pretty(&state).unwrap()).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn 対象のファイル() {
        let d = tempfile::tempdir().unwrap();
        for p in ["a.md", "_system/CLAUDE.md", "x/._b.md", "x/c.meta", "x/c", "x/d.txt"] {
            let path = d.path().join(p);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "x").unwrap();
        }
        let got: Vec<String> = list_indexable_files(d.path()).iter().map(|p| rel_of(p, d.path())).collect();
        assert_eq!(got, vec!["a.md", "x/c.meta"]);
        assert_eq!(asset_path_for(&d.path().join("x/c.meta")), Some(d.path().join("x/c")));
    }

    #[test]
    fn 登録し直せるのは索引の場所の中の対象だけ() {
        let d = tempfile::tempdir().unwrap();
        let lib = d.path().join("library");
        for p in ["library/a.md", "library/x/c.meta", "library/_system/CLAUDE.md", "library/x/._b.md", "library/x/d.txt", "outside.md"] {
            let path = d.path().join(p);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "x").unwrap();
        }
        let rel = |p: &str| reindex_rel(&lib.join(p), &lib);
        // 相対の名前と、中の絶対パスは通し、同期と同じ名前にする
        assert_eq!(rel("a.md").as_deref(), Some("a.md"));
        assert_eq!(rel("x/c.meta").as_deref(), Some("x/c.meta"));
        assert_eq!(reindex_rel(&lib.join(lib.join("a.md")), &lib).as_deref(), Some("a.md"));
        // 中で戻る `..` は、行き着いた先の名前で登録する
        assert_eq!(rel("x/../a.md").as_deref(), Some("a.md"));
        // 外へ出る `../` と、外の絶対パスは断る
        assert_eq!(rel("../outside.md"), None);
        assert_eq!(reindex_rel(&lib.join(d.path().join("outside.md")), &lib), None);
        // 同期の対象外も断る(`..` で `_system/` へ入るものも)
        assert_eq!(rel("_system/CLAUDE.md"), None);
        assert_eq!(rel("x/../_system/CLAUDE.md"), None);
        assert_eq!(rel("x/._b.md"), None);
        assert_eq!(rel("x/d.txt"), None);
    }

    #[test]
    fn 記事の名前は濁点の表し方と区切りと絶対パスによらない() {
        let base = Path::new("/ssd/library");
        let nfd = "30-resources/\u{30b1}\u{3099}\u{30fc}\u{30e0}.md"; // ケ+濁点(Macの形)
        let nfc = "30-resources/\u{30b2}\u{30fc}\u{30e0}.md"; // ゲ(Windowsの形)
        assert_eq!(rel_of(&base.join(nfd), base), nfc);
        assert_eq!(rel_of(&base.join(nfc), base), nfc);
        assert_eq!(index_key("30-resources\\x.md"), "30-resources/x.md");
        assert_eq!(rel_of(&base.join(base.join("a/b.md")), base), "a/b.md", "libraryの中の絶対パス");
        assert_eq!(source_name_for(&rel_of(&base.join(nfd), base)), "\u{30b2}\u{30fc}\u{30e0}.md");
    }
}
