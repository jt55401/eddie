// SPDX-License-Identifier: GPL-3.0-only

//! Eddie: semantic search and simple Q&A for static sites.
//!
//! This library provides the shared core used by both the CLI indexer
//! and the browser WASM module.

pub mod bm25;
pub mod chunk;
pub mod embed;
pub mod index;
#[cfg(not(target_arch = "wasm32"))]
pub mod parse;
pub mod search;
#[cfg(target_arch = "wasm32")]
pub mod wasm;
