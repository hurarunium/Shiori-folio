//! ログの出力先(差し込み式)。`library_store` はログの仕組みを持たない。アプリ・CLIが、起動時に
//! `set_sink` で自分のログへつなぐ。つながっていなければ、何も出さない。

use std::fmt::Display;
use std::sync::OnceLock;

/// `(重さ "info"/"warn", 部品, 出来事, キー=値の並び)`
pub type Sink = fn(&str, &str, &str, &[(&str, &dyn Display)]);

static SINK: OnceLock<Sink> = OnceLock::new();

/// ログの出力先をつなぐ(最初の1回だけ効く)。
pub fn set_sink(sink: Sink) {
    let _ = SINK.set(sink);
}

pub fn info(part: &str, event: &str, fields: &[(&str, &dyn Display)]) {
    if let Some(s) = SINK.get() {
        s("info", part, event, fields);
    }
}

pub fn warn(part: &str, event: &str, fields: &[(&str, &dyn Display)]) {
    if let Some(s) = SINK.get() {
        s("warn", part, event, fields);
    }
}
