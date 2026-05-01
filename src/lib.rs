//! Library facade for XTouch GW.
//!
//! Exposes a minimal surface so auxiliary binaries (e.g. `export-schema`)
//! and integration tests can reuse the same configuration types and editor
//! API surface the runtime parses at startup.

pub mod api_editor;
pub mod config;
pub mod event_bus;
