//! RAG検索サーバー(services/rag/app.py、/search)への問い合わせ。

use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct SearchRequest<'a> {
    query: &'a str,
    top_k: u32,
    // 詩織Ver3.3(時間認識検索)で新設。"current"/"history"。省略時は
    // services/rag/app.py側のデフォルト("current")に委ねる。
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<&'a str>,
}

#[derive(Deserialize)]
pub struct SearchResultItem {
    // RAGAS評価用にPython側で追加されたchunk ID。KnowledgeResultDto経由でUIにも渡す。
    pub id: String,
    pub text: String,
    pub source: String,
    pub heading: String,
    pub distance: f64,
    // リランカー(Python側/search、2026-08-07追加)のスコア。Rust側では現状使わない。
    #[allow(dead_code)]
    pub rerank_score: f64,
    pub source_category: String,
}

pub fn search(port: u16, query: &str, top_k: u32) -> Result<Vec<SearchResultItem>, String> {
    search_with_mode(port, query, top_k, None)
}

// history_guard(詩織Ver3.3、時間認識検索)向け。deprecated/archive込みで
// date降順に並べ替えて返す経緯モードで検索する(app.pyの_retrieve_and_rerank
// 参照)。
pub fn search_history(port: u16, query: &str, top_k: u32) -> Result<Vec<SearchResultItem>, String> {
    search_with_mode(port, query, top_k, Some("history"))
}

fn search_with_mode(
    port: u16,
    query: &str,
    top_k: u32,
    mode: Option<&str>,
) -> Result<Vec<SearchResultItem>, String> {
    let body = SearchRequest { query, top_k, mode };

    // サーバーに接続しているときは、サーバーの検索(`/api/v1/search`)を使う。手元のRAGは見ない(詩織Ver7.0)。
    if let Some(c) = crate::remote::client() {
        let value = c
            .post("/api/v1/search", &serde_json::to_value(&body).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        return serde_json::from_value(value).map_err(|e| format!("RAG応答の解析に失敗: {e}"));
    }

    let url = format!("http://127.0.0.1:{port}/search");
    ureq::post(&url)
        .timeout(Duration::from_secs(30))
        .send_json(&body)
        .map_err(|e| format!("RAG検索に失敗: {e}"))?
        .into_json()
        .map_err(|e| format!("RAG応答の解析に失敗: {e}"))
}

#[derive(Serialize)]
struct AddDocumentRequest<'a> {
    id: &'a str,
    text: &'a str,
    heading: &'a str,
    source: &'a str,
    source_category: &'a str,
}

// メモ機能(memo_guard)向け。ingest.pyの全件再投入を待たず、単一ドキュメントを
// その場で埋め込み・ChromaDBへ追加する(services/rag/app.pyの/add_document)。
pub fn add_document(
    port: u16,
    id: &str,
    text: &str,
    heading: &str,
    source: &str,
    source_category: &str,
) -> Result<(), String> {
    let url = format!("http://127.0.0.1:{port}/add_document");
    let body = AddDocumentRequest { id, text, heading, source, source_category };

    ureq::post(&url)
        .timeout(Duration::from_secs(30))
        .send_json(&body)
        .map_err(|e| format!("メモの即時登録に失敗: {e}"))?;
    Ok(())
}

#[derive(Serialize)]
struct ReindexFileRequest<'a> {
    path: &'a str,
}

// 書き込み時フック(詩織Ver3.0、データ管理法見直し2-5節)向け。shiori-save CLI
// 等がlibrary/への保存直後に呼び、検索の都度の全件スキャンを待たず対象
// ファイル1件だけを即座に埋め込む(services/rag/app.pyの/reindex_file)。
pub fn reindex_file(port: u16, relative_path: &str) -> Result<(), String> {
    let url = format!("http://127.0.0.1:{port}/reindex_file");
    let body = ReindexFileRequest { path: relative_path };

    // 素材(PDF等)は、実体の中身を抽出して埋め込むため、記事より時間がかかる
    // (抽出の上限60秒+埋め込み、詩織Ver3.8)。記事だけなら数秒で終わるので、待つ上限だけ延ばす。
    ureq::post(&url)
        .timeout(Duration::from_secs(180))
        .send_json(&body)
        .map_err(|e| format!("即時re-indexに失敗: {e}"))?;
    Ok(())
}

// RAGサーバーが応答するかを短いタイムアウトで確認する。shiori-save CLIのような
// 書き込み系CLIが、未起動のRAGサーバーの起動待ちで応答を止めてしまわないよう、
// 起動を試みる前にこれで疎通確認する用途(未起動なら即時re-indexを諦め、次回の
// RAGサーバー起動時の全件スキャンに任せる)。
pub fn is_running(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/health");
    ureq::get(&url).timeout(Duration::from_millis(500)).call().is_ok()
}

#[derive(Serialize, Default)]
pub struct LibrarySearchFilter<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "type")]
    pub kind: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<&'a str>,
    // 以下は詩織Ver3.9(図書館の絞り込み)で追加。MCPは使わない。
    // 記事の種類(frontmatterのkind。journal・resourceなど)。上のkindは`type`(棚)。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "kind")]
    pub entry_kind: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_categories: Option<&'a [String]>,
    // アーカイブ(90-archive)と、statusがoutdated/deprecatedのものを除く。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub exclude_stale: bool,
}

#[derive(Serialize)]
struct SearchLibraryRequest<'a> {
    query: &'a str,
    limit: u32,
    offset: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    filter: Option<LibrarySearchFilter<'a>>,
    // 詩織Ver3.3(時間認識検索)で新設。"current"/"history"。省略時は
    // services/rag/app.py側のデフォルト("current")に委ねる。
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<&'a str>,
}

#[derive(Deserialize, Serialize)]
pub struct FileHeading {
    pub heading: String,
    pub rerank_score: f64,
}

#[derive(Deserialize, Serialize)]
pub struct SearchLibraryResultItem {
    pub source: String,
    pub source_category: String,
    pub headings: Vec<FileHeading>,
    pub best_score: f64,
    // 実ファイルへの絶対パス(見つからなければ空文字列)。app.py側で
    // source_category配下をrglobして解決している(2026-09-16追加)。
    pub path: String,
    // sourceが.meta(素材のサイドカー、詩織Ver3.5)のとき、対応する実体の絶対パス。
    // pathは説明(.meta)自体を指し、こちらが素材本体を指す。無ければ空文字列。
    #[serde(default)]
    pub asset_path: String,
    // frontmatterのtitle(無ければ空文字列、2026-09-16追加)。
    pub title: String,
    // frontmatterのrelated(詩織Ver3.3、時間認識検索で追加)。経緯モードで
    // 新旧の記事を辿るための手がかり。
    #[serde(default)]
    pub related: Vec<String>,
    // サーバー経由のとき、サーバーが添える属性と冒頭の文(詩織Ver7.0)。手元のRAGは返さない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<ItemPreview>,
}

/// 一覧に出す、記事の属性と冒頭の文(`library_store::preview::Preview` と同じ形)。
#[derive(Deserialize, Serialize, Default, Clone)]
pub struct ItemPreview {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub entry_kind: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub date: String,
}

// MCPサーバーのsearch_libraryツール向け(詩織Ver2.0設計指示書v3、9章)。
// services/rag/app.pyの/search_libraryを呼ぶ。/searchと異なりファイル単位に
// 集約された結果(本文を含まない)が返る。
//
// 【Ver3.0で変更】スコア閾値による足切りを廃止し、offset+limitのページング
// 方式に変更した(データ管理法見直し3-3節)。呼び出し側は1回目offset=0で呼び、
// 欲しい情報が見つからなければoffsetを増やして(20, 40...)再クエリする。
pub fn search_library(
    port: u16,
    query: &str,
    limit: u32,
    offset: u32,
    filter: Option<LibrarySearchFilter>,
    include_history: bool,
) -> Result<Vec<SearchLibraryResultItem>, String> {
    let url = format!("http://127.0.0.1:{port}/search_library");
    let mode = include_history.then_some("history");
    let body = SearchLibraryRequest { query, limit, offset, filter, mode };

    ureq::post(&url)
        .timeout(Duration::from_secs(30))
        .send_json(&body)
        .map_err(|e| format!("図書館検索に失敗: {e}"))?
        .into_json()
        .map_err(|e| format!("図書館検索応答の解析に失敗: {e}"))
}

#[derive(Serialize)]
struct SearchShelfRequest<'a> {
    query: &'a str,
    limit: u32,
    offset: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    topic: Option<&'a str>,
}

#[derive(Deserialize, Serialize)]
pub struct SearchShelfResultItem {
    pub source: String,
    pub title: String,
    pub topic: String,
    pub learned: String,
    // 何を材料に学んだか(記事のfrontmatterのsource)。
    pub cited_from: String,
    pub headings: Vec<FileHeading>,
    pub best_score: f64,
    pub path: String,
}

// MCPサーバーのsearch_shelfツール向け。知識の棚(knowledge/)だけを検索する
// (services/rag/app.pyの/search_shelf)。libraryの記録とはコレクションが別。
pub fn search_shelf(
    port: u16,
    query: &str,
    limit: u32,
    offset: u32,
    topic: Option<&str>,
) -> Result<Vec<SearchShelfResultItem>, String> {
    let url = format!("http://127.0.0.1:{port}/search_shelf");
    let body = SearchShelfRequest { query, limit, offset, topic };

    ureq::post(&url)
        .timeout(Duration::from_secs(30))
        .send_json(&body)
        .map_err(|e| format!("知識の棚の検索に失敗: {e}"))?
        .into_json()
        .map_err(|e| format!("知識の棚の検索応答の解析に失敗: {e}"))
}

// 知識の棚への保存直後に、対象1件だけを即座に埋め込む(/reindex_shelf_file)。
// pathはknowledge/からの相対パス。
pub fn reindex_shelf_file(port: u16, relative_path: &str) -> Result<(), String> {
    let url = format!("http://127.0.0.1:{port}/reindex_shelf_file");
    let body = ReindexFileRequest { path: relative_path };

    ureq::post(&url)
        .timeout(Duration::from_secs(60))
        .send_json(&body)
        .map_err(|e| format!("棚の即時re-indexに失敗: {e}"))?;
    Ok(())
}

#[derive(Deserialize)]
pub struct ChunkItem {
    pub id: String,
    pub text: String,
    pub source: String,
    pub heading: String,
    pub source_category: String,
}

// スタンドアロン図書館UI(2026-08-12、図書館ビジョン統合仕様書3-2)向け。
// ChromaDBの全チャンクを検索なしで一括取得する(services/rag/app.pyの/list_all)。
pub fn list_all(port: u16) -> Result<Vec<ChunkItem>, String> {
    let url = format!("http://127.0.0.1:{port}/list_all");

    ureq::get(&url)
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(|e| format!("蔵書一覧の取得に失敗: {e}"))?
        .into_json()
        .map_err(|e| format!("蔵書一覧の応答解析に失敗: {e}"))
}

#[derive(Deserialize)]
pub struct LibraryFileHeading {
    pub heading: String,
}

#[derive(Deserialize)]
pub struct LibraryFileItem {
    pub source: String,
    pub source_category: String,
    pub headings: Vec<LibraryFileHeading>,
    // frontmatterのtitle・実ファイルへの絶対パス(2026-09-16追加、
    // エクスプローラー風UI刷新向け)。
    pub title: String,
    pub path: String,
    // sourceが.metaのとき、対応する実体の絶対パス(無ければ空文字列、詩織Ver3.5)。
    #[serde(default)]
    pub asset_path: String,
    // library_rootからの相対パス(/区切り)。GUIのフォルダツリー構築に使う。
    pub relative_path: String,
    // ファイルの更新日時(Unixタイムスタンプ)。
    pub mtime: f64,
}

// スタンドアロン図書館UI(Phase 7、1冊=1ファイルの表示単位への変更)向け。
// list_all()と違い、ファイル(source)単位に集約された結果(本文を含まない)が
// 返る(services/rag/app.pyの/list_all_library)。
pub fn list_all_library(port: u16) -> Result<Vec<LibraryFileItem>, String> {
    let url = format!("http://127.0.0.1:{port}/list_all_library");

    ureq::get(&url)
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(|e| format!("蔵書一覧の取得に失敗: {e}"))?
        .into_json()
        .map_err(|e| format!("蔵書一覧の応答解析に失敗: {e}"))
}
