//! Unified controller identification across backends
//!
//! This module provides a common identifier type that abstracts over both
//! gilrs (WGI backend) and XInput controller IDs, enabling a hybrid provider
//! to manage controllers from both sources uniformly.

use gilrs::GamepadId;

/// Unified controller identifier across backends
///
/// This enum allows the hybrid provider to manage controllers from both
/// XInput and gilrs (WGI) backends using a single identifier type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HybridControllerId {
    /// Controller detected via gilrs (Windows.Gaming.Input backend)
    ///
    /// These controllers may be non-XInput devices (like FaceOff)
    /// or XInput devices that were detected by WGI.
    Gilrs(GamepadId),

    /// Controller detected via XInput (user index 0-3)
    ///
    /// XInput provides direct polling access to Xbox-compatible controllers
    /// without requiring a focused window.
    XInput(usize),
}

impl HybridControllerId {
    /// Create a hybrid ID from a gilrs GamepadId
    pub fn from_gilrs(id: GamepadId) -> Self {
        Self::Gilrs(id)
    }

    /// Create a hybrid ID from an XInput user index (0-3)
    pub fn from_xinput(user_index: usize) -> Self {
        Self::XInput(user_index)
    }

    /// Get a stable string identifier for logging and debugging
    ///
    /// # Examples
    /// - Gilrs: "gilrs:0"
    /// - XInput: "xinput:0"
    pub fn to_string(&self) -> String {
        match self {
            Self::Gilrs(id) => format!("gilrs:{:?}", id),
            Self::XInput(idx) => format!("xinput:{}", idx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_gilrs() {
        let gilrs_id = GamepadId::from(0);
        let hybrid_id = HybridControllerId::from_gilrs(gilrs_id);
        assert!(matches!(hybrid_id, HybridControllerId::Gilrs(_)));
    }

    #[test]
    fn test_from_xinput() {
        let hybrid_id = HybridControllerId::from_xinput(0);
        assert!(matches!(hybrid_id, HybridControllerId::XInput(0)));
    }

    #[test]
    fn test_to_string() {
        let gilrs_id = GamepadId::from(0);
        let hybrid_gilrs = HybridControllerId::from_gilrs(gilrs_id);
        assert!(hybrid_gilrs.to_string().starts_with("gilrs:"));

        let hybrid_xinput = HybridControllerId::from_xinput(2);
        assert_eq!(hybrid_xinput.to_string(), "xinput:2");
    }

    #[test]
    fn test_equality() {
        let id1 = HybridControllerId::from_xinput(0);
        let id2 = HybridControllerId::from_xinput(0);
        let id3 = HybridControllerId::from_xinput(1);

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }
}
