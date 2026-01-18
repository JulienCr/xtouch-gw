//! Shared stick buffering types for radial normalization
//!
//! These types are used by both `provider.rs` and `hybrid_provider.rs` to
//! buffer X/Y axis pairs for applying radial (circular) normalization to
//! stick inputs.

/// Stick identifier for buffering X/Y pairs
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum StickId {
    Left,
    Right,
}

/// Buffered stick state for radial normalization
///
/// Stores the most recent X and Y values for a stick, allowing
/// radial normalization to be applied when either axis changes.
#[derive(Debug, Clone, Default)]
pub struct StickBuffer {
    pub x: f32,
    pub y: f32,
}
