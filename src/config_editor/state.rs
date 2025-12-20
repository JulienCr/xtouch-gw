//! Editor state management
//!
//! Manages the mutable config being edited, validation errors, dirty tracking,
//! and tab navigation state.

use crate::config::AppConfig;
use std::collections::HashMap;
use std::cell::RefCell;

/// Main editor state
pub struct EditorState {
    /// The configuration being edited (mutable clone)
    pub config: AppConfig,

    /// Path to the config file for saving
    pub config_path: String,

    /// Validation errors per field (key: field path like "midi.input_port")
    /// Uses RefCell for interior mutability to allow mutation during rendering
    validation_errors: RefCell<HashMap<String, String>>,

    /// Dirty flag (unsaved changes)
    /// Uses RefCell for interior mutability to allow mutation during rendering
    has_unsaved_changes: RefCell<bool>,

    /// Currently active tab
    pub active_tab: EditorTab,

    /// Active page index (for page tabs, 0-7)
    pub active_page_idx: usize,

    /// Show unsaved changes warning dialog
    pub show_unsaved_warning: bool,
}

/// Editor tab navigation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorTab {
    /// General settings (MIDI, OBS, X-Touch, Paging, Tray)
    General,
    /// Gamepad configuration
    Gamepad,
    /// Global page defaults
    GlobalDefaults,
    /// Individual page editor (0-7 for Page 1-8)
    Page(usize),
}

impl EditorState {
    /// Create new editor state from config
    pub fn new(config: AppConfig, config_path: String) -> Self {
        Self {
            config,
            config_path,
            validation_errors: RefCell::new(HashMap::new()),
            has_unsaved_changes: RefCell::new(false),
            active_tab: EditorTab::General,
            active_page_idx: 0,
            show_unsaved_warning: false,
        }
    }

    /// Mark config as dirty (unsaved changes)
    pub fn mark_dirty(&self) {
        *self.has_unsaved_changes.borrow_mut() = true;
    }

    /// Clear dirty flag (after save)
    pub fn mark_clean(&self) {
        *self.has_unsaved_changes.borrow_mut() = false;
    }

    /// Check if there are unsaved changes
    pub fn has_unsaved_changes(&self) -> bool {
        *self.has_unsaved_changes.borrow()
    }

    /// Check if there are any validation errors
    pub fn has_errors(&self) -> bool {
        !self.validation_errors.borrow().is_empty()
    }

    /// Add or update a validation error for a field
    pub fn set_error(&self, field_path: impl Into<String>, error: impl Into<String>) {
        self.validation_errors.borrow_mut().insert(field_path.into(), error.into());
    }

    /// Clear validation error for a field
    pub fn clear_error(&self, field_path: &str) {
        self.validation_errors.borrow_mut().remove(field_path);
    }

    /// Get error message for a field, if any (returns cloned String)
    pub fn get_error(&self, field_path: &str) -> Option<String> {
        self.validation_errors.borrow().get(field_path).cloned()
    }

    /// Validate all fields and return true if valid
    pub fn validate_all(&self) -> bool {
        // This will be called from validation.rs
        // For now, just check if errors exist
        !self.has_errors()
    }

    /// Get total error count
    pub fn error_count(&self) -> usize {
        self.validation_errors.borrow().len()
    }

    /// Clear all validation errors
    pub fn clear_all_errors(&self) {
        self.validation_errors.borrow_mut().clear();
    }
}
