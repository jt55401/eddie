// SPDX-License-Identifier: GPL-3.0-only

//! Eddie: semantic search and simple Q&A for static sites.
//!
//! This library provides the shared core used by both the CLI indexer
//! and the browser WASM module.
//!
//! Module availability by target and feature:
//!
//! - everywhere: `bm25`, `index`, `manifest`, `records`, `search`, `sparse`,
//!   `wordpiece` (the lite WASM build is exactly this set plus `wasm`);
//! - native, or wasm32 with the `dense-wasm` feature: `embed`, `models`
//!   (candle + tokenizers);
//! - native only: `chunk`, `claims`, `eval`, `parse`, `qa` (content parsing
//!   and build-time synthesis; regex, unicode-segmentation, HTTP clients).

pub mod bm25;
#[cfg(not(target_arch = "wasm32"))]
pub mod chunk;
#[cfg(not(target_arch = "wasm32"))]
pub mod claims;
#[cfg(any(not(target_arch = "wasm32"), feature = "dense-wasm"))]
pub mod embed;
#[cfg(not(target_arch = "wasm32"))]
pub mod eval;
pub mod index;
pub mod manifest;
#[cfg(any(not(target_arch = "wasm32"), feature = "dense-wasm"))]
pub mod models;
#[cfg(not(target_arch = "wasm32"))]
pub mod parse;
#[cfg(not(target_arch = "wasm32"))]
pub mod qa;
pub mod records;
pub mod search;
pub mod sparse;
#[cfg(target_arch = "wasm32")]
pub mod wasm;
pub mod wordpiece;
mod wordpiece_tables;
