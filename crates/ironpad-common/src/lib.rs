//! Shared types for ironpad (server ↔ client).

pub mod cache_key;
pub mod config;
pub mod protocol;
pub mod types;

pub use config::{absolute_url, AppConfig};
pub use types::*;
