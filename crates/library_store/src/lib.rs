//! libraryへの書き込みを1か所に集める部品(詩織Ver7.0 Phase 1)。
//! Tauriに依存しない。アプリ・CLI・サーバーが同じ書き方で書くための土台。

pub mod assets;
pub mod fsx;
pub mod inbox;
pub mod notes;
pub mod registry;
pub mod shelf;

pub use fsx::{Cond, StoreError, Written};
