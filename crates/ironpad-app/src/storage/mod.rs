//! Client-side storage layer backed by IndexedDB.
//!
//! This module provides Rust wrappers around the `window.IronpadStorage` JavaScript
//! API (defined in `public/storage.js`). All functions are async and only available
//! when the `hydrate` feature is enabled (browser context).

#[cfg(feature = "hydrate")]
pub mod client;
