//! 端末ごとのトークン。`tokens.json` にはSHA-256のハッシュだけを置く(トークンそのものは残さない)。
//! 要求のたびにファイルを読むので、失効はサーバーを止めずに効く。

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize, Default)]
pub struct Tokens {
    #[serde(default)]
    pub devices: BTreeMap<String, Device>,
}

#[derive(Serialize, Deserialize)]
pub struct Device {
    pub sha256: String,
    #[serde(default)]
    pub revoked: bool,
}

fn hash(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// 時間が内容に依らないように比べる
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn load(path: &Path) -> Tokens {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(path: &Path, t: &Tokens) -> Result<(), String> {
    let text = serde_json::to_string_pretty(t).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// `Authorization: Bearer <token>` の値から、端末名を返す。失効・不明・形の違いは None
pub fn authenticate(tokens_path: &Path, header: Option<&str>) -> Option<String> {
    let token = header?.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        return None;
    }
    let h = hash(token);
    let tokens = load(tokens_path);
    tokens
        .devices
        .into_iter()
        .find(|(_, d)| !d.revoked && same(&d.sha256, &h))
        .map(|(name, _)| name)
}

/// 端末を足してトークンを返す(表示はこの1回だけ)
pub fn add(tokens_path: &Path, name: &str) -> Result<String, String> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("端末名は英数字と - _ だけ".into());
    }
    let mut t = load(tokens_path);
    if t.devices.contains_key(name) {
        return Err(format!("{name} はもうある(失効してから作り直す場合は、別の名前にする)"));
    }
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).map_err(|e| e.to_string())?;
    let token: String = buf.iter().map(|b| format!("{b:02x}")).collect();
    t.devices.insert(name.to_string(), Device { sha256: hash(&token), revoked: false });
    save(tokens_path, &t)?;
    Ok(token)
}

/// 端末の一覧(名前と、失効しているか)。トークンそのものは持っていない。
pub fn list(tokens_path: &Path) -> Vec<(String, bool)> {
    load(tokens_path).devices.into_iter().map(|(n, d)| (n, d.revoked)).collect()
}

pub fn revoke(tokens_path: &Path, name: &str) -> Result<(), String> {
    let mut t = load(tokens_path);
    let d = t.devices.get_mut(name).ok_or_else(|| format!("{name} は無い"))?;
    d.revoked = true;
    save(tokens_path, &t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 発行と照合と失効() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("tokens.json");
        let tok = add(&p, "mac").unwrap();
        assert_eq!(authenticate(&p, Some(&format!("Bearer {tok}"))), Some("mac".into()));
        assert_eq!(authenticate(&p, Some("Bearer wrong")), None);
        assert_eq!(authenticate(&p, None), None);
        assert_eq!(authenticate(&p, Some(&tok)), None); // Bearer が無い
        revoke(&p, "mac").unwrap();
        assert_eq!(authenticate(&p, Some(&format!("Bearer {tok}"))), None);
        // ファイルに、トークンそのものは残らない
        assert!(!std::fs::read_to_string(&p).unwrap().contains(&tok));
    }
}
