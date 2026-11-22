//! libraryへの書き込みを1か所に集める部品(詩織Ver7.0 Phase 1)。
//! Tauriに依存しない。アプリ・CLI・サーバーが同じ書き方で書くための土台。

pub mod fsx;

pub use fsx::{Cond, StoreError, Written};
