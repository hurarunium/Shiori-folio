//! libraryへの書き込みを1か所に集める部品(詩織Ver7.0 Phase 1)。
//! Tauriに依存しない。アプリ・CLI・サーバーが同じ書き方で書くための土台。

pub mod assets;
pub mod docs;
pub mod fsx;
pub mod handoff;
pub mod inbox;
pub mod journal;
pub mod layout;
pub mod log;
pub mod notes;
pub mod pending;
pub mod preview;
pub mod registry;
pub mod rules;
pub mod shelf;
pub mod shelf_view;
pub mod system_docs;
pub mod source_path;
pub mod tasks;

pub use fsx::{Cond, StoreError, Written};
pub use layout::Layout;
