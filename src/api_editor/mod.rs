//! Editor API: profiles, schema, validation, MIDI port enumeration, and SPA serving.
//!
//! Mounted under `/api/*` (data endpoints) and `/editor/*` (static SPA) by the
//! parent router when an `EditorState` is configured.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};

pub mod action_catalog;
pub mod actions;
pub mod live;
pub mod midi_picker;
pub mod obs_picker;
pub mod page;
pub mod profiles;
pub mod schema;
pub mod static_spa;
pub mod validate;

use crate::event_bus::LiveEventTx;

pub use action_catalog::{ActionDescriptor, ParamDescriptor, ParamKind};
pub use actions::DriverCatalogs;
pub use obs_picker::{ObsPickerSource, ObsPickerSourceArc};

/// Reads the current 14-bit setpoint for a fader channel (1..=9).
pub type FaderSetpointReader = Arc<dyn Fn(u8) -> Option<u16> + Send + Sync>;

/// Boxed async future returned by page accessors.
pub type PageFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

/// Reads the current active page (index, name).
pub type ActivePageReader = Arc<dyn Fn() -> PageFuture<Option<(usize, String)>> + Send + Sync>;

/// Sets the active page by index. Returns Ok on success.
pub type ActivePageSetter = Arc<dyn Fn(usize) -> PageFuture<anyhow::Result<()>> + Send + Sync>;

/// Shared state for editor API handlers.
///
/// All non-profile fields are optional so tests can construct a minimal
/// `EditorState { profiles, ..Default::default() }`-style instance via the
/// [`EditorState::with_profiles`] helper.
pub struct EditorState {
    pub profiles: Arc<crate::config::profiles::ProfileStore>,
    /// Broadcast sender for live editor events. `None` in tests / when the
    /// bus has not been wired in.
    pub live_tx: Option<LiveEventTx>,
    /// OBS picker source (scenes / inputs / scene items). `None` when OBS is
    /// not configured or not yet connected — picker endpoints then return 503.
    pub obs: Option<ObsPickerSourceArc>,
    /// Action catalogs keyed by driver name.
    pub drivers: DriverCatalogs,
    /// Reader for current fader setpoints (channel 1..=9 → 14-bit value).
    /// Used by the `/api/live` WS handler to push an initial fader snapshot
    /// on connect so the editor's virtual surface matches reality without
    /// waiting for the user to wiggle a fader. We expose only a closure so
    /// `EditorState` doesn't need to depend on the binary-only `xtouch` module.
    pub fader_setpoint: Option<FaderSetpointReader>,
    /// Reader for current active page (index, name). `None` in tests.
    pub active_page_reader: Option<ActivePageReader>,
    /// Setter for active page. `None` in tests; endpoints return 503 when unset.
    pub active_page_setter: Option<ActivePageSetter>,
}

impl EditorState {
    /// Build an `EditorState` with just the profile store (used by tests and
    /// callers that wire optional fields incrementally).
    pub fn with_profiles(profiles: Arc<crate::config::profiles::ProfileStore>) -> Self {
        Self {
            profiles,
            live_tx: None,
            obs: None,
            drivers: Arc::new(HashMap::new()),
            fader_setpoint: None,
            active_page_reader: None,
            active_page_setter: None,
        }
    }
}

/// Build the editor data routes (everything under `/api`).
pub fn routes() -> Router<Arc<EditorState>> {
    Router::new()
        // profiles
        .route(
            "/api/profiles",
            get(profiles::list).post(profiles::create),
        )
        .route("/api/profiles/active", get(profiles::active))
        .route(
            "/api/profiles/:name",
            get(profiles::read).put(profiles::save).delete(profiles::delete_),
        )
        .route("/api/profiles/:name/duplicate", post(profiles::duplicate))
        .route("/api/profiles/:name/rename", post(profiles::rename))
        .route("/api/profiles/:name/activate", post(profiles::activate))
        .route("/api/profiles/:name/history", get(profiles::history))
        .route(
            "/api/profiles/:name/history/:timestamp",
            get(profiles::history_read),
        )
        .route(
            "/api/profiles/:name/history/:timestamp/restore",
            post(profiles::history_restore),
        )
        // schema
        .route("/api/schema", get(schema::schema))
        // validate
        .route("/api/validate", post(validate::validate))
        // midi
        .route("/api/midi/ports", get(midi_picker::ports))
        // live event WS
        .route("/api/live", get(live::ws))
        // OBS pickers
        .route("/api/obs/scenes", get(obs_picker::scenes))
        .route(
            "/api/obs/scenes/:scene/sources",
            get(obs_picker::scene_sources),
        )
        .route("/api/obs/inputs", get(obs_picker::inputs))
        // driver action catalogs
        .route("/api/drivers", get(actions::list_drivers))
        .route("/api/drivers/:name/actions", get(actions::driver_actions))
        // active page (read / set)
        .route("/api/page", get(page::active).post(page::set_active))
}

/// Build the SPA routes mounted under `/editor`.
pub fn spa_routes() -> Router<Arc<EditorState>> {
    static_spa::routes()
}
