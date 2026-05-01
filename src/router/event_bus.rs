//! Live event bus re-export.
//!
//! Canonical types live in `crate::event_bus` (which is also exposed through
//! the library crate). This module re-exports them for convenience to
//! existing call sites under `crate::router::event_bus::*`.

pub use crate::event_bus::*;
