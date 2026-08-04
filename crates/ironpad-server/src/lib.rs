//! ironpad collaboration server: session store + WebSocket relay.
//!
//! The server is a stateless *relay* between browser hosts (the notebook model)
//! and CLI guests (agents): it validates session tokens, enforces permissions,
//! and routes protocol messages without ever interpreting notebook state. See
//! [`ws`] for the relay handlers, [`sessions`] for the token/session store, and
//! [`state`] for shared connection state.

pub mod auth;
pub mod crawl;
pub mod oembed;
pub mod og;
pub mod sessions;
pub mod state;
pub mod ws;
