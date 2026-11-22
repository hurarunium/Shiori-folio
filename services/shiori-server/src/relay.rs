//! 検索の中継(`shiori-rag` は127.0.0.1だけで待つ。端末へは、サーバーの窓口が中継する)と、索引の更新。
//! サーバーの絶対パスは外へ出さない。結果の `path` は、`library/…`・`knowledge/…` の相対パスにする。

use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

fn norm(s: &str) -> String {
    s.replace('\\', "/")
}

/// 絶対パスを、`{prefix}/…` の相対パスにする。どの置き場にも入らなければ、空にする。
pub fn relativize(path: &str, bases: &[(&Path, &str)]) -> String {
    let p = norm(path);
    for (base, prefix) in bases {
        let b = norm(&base.to_string_lossy());
        let b = format!("{}/", b.trim_end_matches('/'));
        if p.len() > b.len() && p[..b.len()].eq_ignore_ascii_case(&b) {
            return format!("{prefix}/{}", &p[b.len()..]);
        }
    }
    String::new()
}

fn rewrite(value: &mut Value, bases: &[(&Path, &str)]) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(|v| rewrite(v, bases)),
        Value::Object(map) => {
            for key in ["path", "asset_path"] {
                if let Some(s) = map.get(key).and_then(Value::as_str).map(String::from) {
                    map.insert(key.into(), Value::String(relativize(&s, bases)));
                }
            }
            map.values_mut().for_each(|v| rewrite(v, bases));
        }
        _ => {}
    }
}

/// RAGへ中継する。`endpoint` は `search_library`・`search_shelf`・`search`。
pub fn search(rag_port: u16, endpoint: &str, request: &Value, bases: &[(&Path, &str)]) -> Result<Value, (u16, String)> {
    if request.get("query").and_then(Value::as_str).is_none() {
        return Err((422, "query が必要".into()));
    }
    let url = format!("http://127.0.0.1:{rag_port}/{endpoint}");
    match ureq::post(&url).timeout(Duration::from_secs(120)).send_json(request) {
        Ok(r) => match r.into_json::<Value>() {
            Ok(mut v) => {
                rewrite(&mut v, bases);
                Ok(v)
            }
            Err(e) => Err((502, format!("検索の応答を読めない: {e}"))),
        },
        Err(ureq::Error::Status(c, _)) => Err((502, format!("検索が{c}を返した"))),
        Err(e) => Err((503, format!("検索につながらない: {e}"))),
    }
}

/// 蔵書の一覧(RAGの `/list_all_library`)を中継する。`path`・`asset_path` は相対パスにする。
pub fn list_library(rag_port: u16, bases: &[(&Path, &str)]) -> Result<Value, (u16, String)> {
    let url = format!("http://127.0.0.1:{rag_port}/list_all_library");
    match ureq::get(&url).timeout(Duration::from_secs(60)).call() {
        Ok(r) => match r.into_json::<Value>() {
            Ok(mut v) => {
                rewrite(&mut v, bases);
                Ok(v)
            }
            Err(e) => Err((502, format!("蔵書一覧の応答を読めない: {e}"))),
        },
        Err(ureq::Error::Status(c, _)) => Err((502, format!("蔵書一覧が{c}を返した"))),
        Err(e) => Err((503, format!("検索につながらない: {e}"))),
    }
}

/// 一覧・検索結果の各項目に、記事の属性と冒頭の文(`preview`)を添える。
/// `path`(`library/…`・`knowledge/…`)の記事を読み、読めない・.md/.meta でないものは、添えない。
pub fn add_previews(value: &mut Value, bases: &[(&Path, &str)]) {
    let Value::Array(items) = value else { return };
    for item in items {
        let Some(p) = item.get("path").and_then(Value::as_str) else { continue };
        if !(p.ends_with(".md") || p.ends_with(".meta")) {
            continue;
        }
        let Some((base, rel)) = bases.iter().find_map(|(b, prefix)| p.strip_prefix(&format!("{prefix}/")).map(|r| (*b, r))) else {
            continue;
        };
        let Ok((bytes, _)) = library_store::fsx::read(base, rel) else { continue };
        let Ok(text) = String::from_utf8(bytes) else { continue };
        if let (Some(o), Ok(pv)) = (item.as_object_mut(), serde_json::to_value(library_store::preview::preview_of(&text))) {
            o.insert("preview".into(), pv);
        }
    }
}

/// 保存のあとの索引の更新。RAGが動いていなければ、何もしない(次回の起動時の全件スキャンが拾う)。戻り値は、更新できたか。
pub fn reindex(rag_port: u16, endpoint: &str, path: &str) -> bool {
    let health = format!("http://127.0.0.1:{rag_port}/health");
    if ureq::get(&health).timeout(Duration::from_millis(300)).call().is_err() {
        return false;
    }
    let url = format!("http://127.0.0.1:{rag_port}/{endpoint}");
    ureq::post(&url).timeout(Duration::from_secs(180)).send_json(json!({ "path": path })).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn 絶対パスは置き場ごとの相対パスになり置き場の外は空になる() {
        let lib = PathBuf::from(r"C:\ShioriServer\library");
        let shelf = PathBuf::from(r"C:\ShioriServer\knowledge");
        let bases: [(&Path, &str); 2] = [(&lib, "library"), (&shelf, "knowledge")];
        assert_eq!(relativize(r"C:\ShioriServer\library\20-projects\a\memory.md", &bases), "library/20-projects/a/memory.md");
        assert_eq!(relativize("c:/shiorisever/library/x.md", &bases), "", "別の置き場");
        assert_eq!(relativize(r"c:\shioriserver\knowledge\a.md", &bases), "knowledge/a.md", "大文字小文字は区別しない");
        assert_eq!(relativize(r"D:\other\x.md", &bases), "");
        assert_eq!(relativize("", &bases), "");
    }

    #[test]
    fn 結果の中のpathを全部直す() {
        let lib = PathBuf::from("/srv/library");
        let bases: [(&Path, &str); 1] = [(&lib, "library")];
        let mut v = json!([{"path": "/srv/library/a.md", "asset_path": "", "text": "本文", "nested": {"path": "/srv/secret/x"}}]);
        rewrite(&mut v, &bases);
        assert_eq!(v[0]["path"], "library/a.md");
        assert_eq!(v[0]["nested"]["path"], "");
        assert_eq!(v[0]["text"], "本文");
    }
}
