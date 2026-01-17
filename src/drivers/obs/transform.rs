//! OBS transform operations
//!
//! Handles scene item transformations including position, scale, and bounds.
//! Provides caching for performance and handles center-based zoom operations.

use anyhow::{Context, Result};
use tracing::{debug, info, trace, warn};

use super::driver::ObsDriver;

/// OBS item transformation state
#[derive(Debug, Clone)]
pub(super) struct ObsItemState {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) scale_x: f64,
    pub(super) scale_y: f64,
    pub(super) width: Option<f64>,
    pub(super) height: Option<f64>,
    pub(super) bounds_width: Option<f64>,
    pub(super) bounds_height: Option<f64>,
    pub(super) alignment: u32, // OBS alignment flags (LEFT=1, RIGHT=2, TOP=4, BOTTOM=8, CENTER=0)
}

/// Compute anchor point from OBS alignment flags
///
/// OBS alignment bit flags (from libobs):
/// - LEFT = 1, RIGHT = 2, TOP = 4, BOTTOM = 8
/// - CENTER = 0 (no flags set)
///
/// Returns (anchor_x, anchor_y) where 0.0=left/top, 0.5=center, 1.0=right/bottom
pub(super) fn compute_anchor_from_alignment(alignment: u32) -> (f64, f64) {
    let left = (alignment & 1) != 0;
    let right = (alignment & 2) != 0;
    let top = (alignment & 4) != 0;
    let bottom = (alignment & 8) != 0;

    let anchor_x = if left {
        0.0
    } else if right {
        1.0
    } else {
        0.5 // center
    };

    let anchor_y = if top {
        0.0
    } else if bottom {
        1.0
    } else {
        0.5 // center
    };

    (anchor_x, anchor_y)
}

impl ObsDriver {
    /// Get the cache key for a scene item
    pub(super) fn cache_key(&self, scene_name: &str, source_name: &str) -> String {
        format!("{}::{}", scene_name, source_name)
    }

    /// Resolve scene item ID with caching
    pub(super) async fn resolve_item_id(&self, scene_name: &str, source_name: &str) -> Result<i64> {
        let cache_key = self.cache_key(scene_name, source_name);

        // Check cache first
        {
            let cache = self.item_id_cache.read();
            if let Some(&id) = cache.get(&cache_key) {
                trace!("OBS item ID cache hit: {} -> {}", cache_key, id);
                return Ok(id);
            }
        }

        // Cache miss, resolve from OBS
        let guard = self.client.read().await;
        let client = guard.as_ref().context("OBS client not connected")?;

        debug!(
            "Resolving OBS item ID: scene='{}' source='{}'",
            scene_name, source_name
        );

        let item_id = client.scene_items()
            .id(obws::requests::scene_items::Id {
                scene: scene_name,
                source: source_name,
                search_offset: None,
            })
            .await
            .with_context(|| format!("Failed to get scene item ID for '{}/{}' - verify scene and source names in OBS", scene_name, source_name))?;

        // Cache for future use
        self.item_id_cache
            .write()
            .insert(cache_key.clone(), item_id);
        debug!(
            "OBS item ID resolved and cached: {} -> {}",
            cache_key, item_id
        );

        Ok(item_id)
    }

    /// Read current transform from OBS
    pub(super) async fn read_transform(
        &self,
        scene_name: &str,
        item_id: i64,
    ) -> Result<ObsItemState> {
        let guard = self.client.read().await;
        let client = guard.as_ref().context("OBS client not connected")?;

        let transform = client
            .scene_items()
            .transform(scene_name, item_id)
            .await
            .context("Failed to get scene item transform")?;

        // Convert Alignment enum to u32 bits
        let alignment_bits = transform.alignment.bits() as u32;

        debug!("OBS read_transform: scene='{}' item={} → pos=({:.1},{:.1}) scale=({:.3},{:.3}) size=({:.0}×{:.0}) bounds=({:.0}×{:.0}) align={:?}",
            scene_name, item_id,
            transform.position_x, transform.position_y,
            transform.scale_x, transform.scale_y,
            transform.width, transform.height,
            transform.bounds_width, transform.bounds_height,
            transform.alignment
        );

        Ok(ObsItemState {
            x: transform.position_x as f64,
            y: transform.position_y as f64,
            scale_x: transform.scale_x as f64,
            scale_y: transform.scale_y as f64,
            width: Some(transform.width as f64),
            height: Some(transform.height as f64),
            bounds_width: Some(transform.bounds_width as f64),
            bounds_height: Some(transform.bounds_height as f64),
            alignment: alignment_bits,
        })
    }

    /// Get OBS canvas (base) dimensions
    pub(super) async fn get_canvas_dimensions(&self) -> Result<(f64, f64)> {
        let guard = self.client.read().await;
        let client = guard.as_ref().context("OBS client not connected")?;

        let video_settings = client
            .config()
            .video_settings()
            .await
            .context("Failed to get OBS video settings")?;

        let width = video_settings.base_width as f64;
        let height = video_settings.base_height as f64;

        trace!("OBS canvas dimensions: {}×{}", width, height);
        Ok((width, height))
    }

    /// Apply position/scale delta to an item
    pub(super) async fn apply_delta(
        &self,
        scene_name: &str,
        source_name: &str,
        dx: Option<f64>,
        dy: Option<f64>,
        ds: Option<f64>,
    ) -> Result<()> {
        trace!(
            "OBS transform delta: scene='{}' source='{}' dx={:?} dy={:?} ds={:?}",
            scene_name,
            source_name,
            dx,
            dy,
            ds
        );

        // Resolve item ID
        let item_id = self.resolve_item_id(scene_name, source_name).await?;

        // Get current transform from cache or OBS
        let cache_key = self.cache_key(scene_name, source_name);

        // Try to get from cache first
        let cached_opt = {
            let cache = self.transform_cache.read();
            cache.get(&cache_key).cloned()
        };

        let current = if let Some(cached) = cached_opt {
            // Check if cached scale looks suspicious (extremely small, likely corrupted)
            // Only invalidate if scale is below 1% - normal zoom out can go much lower than 50%
            if cached.scale_x < 0.01 || cached.scale_y < 0.01 {
                warn!("OBS transform cache has suspicious scale ({:.3},{:.3}) for '{}' - invalidating and re-reading from OBS",
                    cached.scale_x, cached.scale_y, cache_key);
                // Invalidate cache and re-read
                self.transform_cache.write().remove(&cache_key);
                let state = self.read_transform(scene_name, item_id).await?;
                self.transform_cache
                    .write()
                    .insert(cache_key.clone(), state.clone());
                state
            } else {
                trace!(
                    "OBS transform cache HIT: '{}' scale=({:.3},{:.3})",
                    cache_key,
                    cached.scale_x,
                    cached.scale_y
                );
                cached
            }
        } else {
            // Not in cache, read from OBS
            debug!(
                "OBS transform cache MISS: '{}' - reading from OBS",
                cache_key
            );
            let state = self.read_transform(scene_name, item_id).await?;
            self.transform_cache
                .write()
                .insert(cache_key.clone(), state.clone());
            state
        };

        // Apply deltas
        let mut new_state = current.clone();
        if let Some(dx_val) = dx {
            new_state.x += dx_val;
        }
        if let Some(dy_val) = dy {
            new_state.y += dy_val;
        }
        if let Some(ds_val) = ds {
            // Apply scale delta multiplicatively (matching TypeScript implementation)
            // Formula: new_scale/bounds = current × (1 + delta)
            let factor = 1.0 + ds_val;

            // Get canvas dimensions to calculate center-based zoom
            let (canvas_width, canvas_height) = self.get_canvas_dimensions().await?;
            let canvas_center_x = canvas_width / 2.0;
            let canvas_center_y = canvas_height / 2.0;

            // Determine if we should use bounds-based or scale-based transform
            let use_bounds =
                if let (Some(bw), Some(bh)) = (current.bounds_width, current.bounds_height) {
                    bw > 0.0 && bh > 0.0
                } else {
                    false
                };

            // Compute anchor point from alignment (needed for position calculations)
            let (anchor_x, anchor_y) = compute_anchor_from_alignment(current.alignment);

            if use_bounds {
                // PATH 1: Bounds-based scaling
                let bounds_w = current.bounds_width.unwrap();
                let bounds_h = current.bounds_height.unwrap();

                let new_w = (bounds_w * factor).max(1.0).round();
                let new_h = (bounds_h * factor).max(1.0).round();

                // Calculate effective factor (may be limited by bounds constraints)
                // If bounds hit a limit (min 1.0), the effective factor is smaller than requested
                let effective_factor_x = new_w / bounds_w;
                let effective_factor_y = new_h / bounds_h;
                let effective_factor = effective_factor_x.min(effective_factor_y);

                // Step 1: Calculate object's visual center (accounting for alignment)
                // The position (current.x, current.y) refers to the anchor point (determined by alignment)
                // We need to convert this to the object's center
                let object_center_x = current.x + (0.5 - anchor_x) * bounds_w;
                let object_center_y = current.y + (0.5 - anchor_y) * bounds_h;

                // Step 2: Zoom the object's center toward/from canvas center
                // IMPORTANT: Use effective_factor, not factor, to avoid decentering when bounds are capped
                let new_object_center_x =
                    canvas_center_x + (object_center_x - canvas_center_x) * effective_factor;
                let new_object_center_y =
                    canvas_center_y + (object_center_y - canvas_center_y) * effective_factor;

                // Step 3: Calculate new anchor position (convert from center back to anchor)
                new_state.x = new_object_center_x - (0.5 - anchor_x) * new_w;
                new_state.y = new_object_center_y - (0.5 - anchor_y) * new_h;
                new_state.bounds_width = Some(new_w);
                new_state.bounds_height = Some(new_h);

                debug!("OBS bounds zoom: {:.0}×{:.0} * {:.3} (eff={:.3}) = {:.0}×{:.0} align=({:.1},{:.1}) center {:.1},{:.1} → {:.1},{:.1} anchor {:.1},{:.1} → {:.1},{:.1}",
                    bounds_w, bounds_h, factor, effective_factor, new_w, new_h,
                    anchor_x, anchor_y,
                    object_center_x, object_center_y, new_object_center_x, new_object_center_y,
                    current.x, current.y, new_state.x, new_state.y);
            } else {
                // PATH 2: Scale-based scaling
                new_state.scale_x = (current.scale_x * factor).max(0.01).min(10.0);
                new_state.scale_y = (current.scale_y * factor).max(0.01).min(10.0);

                // Calculate effective factor (may be capped by scale limits)
                // If scale hits a limit (min 0.01 or max 10.0), the effective factor differs from requested
                let effective_factor_x = new_state.scale_x / current.scale_x;
                let effective_factor_y = new_state.scale_y / current.scale_y;
                let effective_factor = effective_factor_x.min(effective_factor_y);

                // Step 1: Calculate object's visual center (accounting for alignment)
                // For scale-based, we need the base dimensions
                if let (Some(w_base), Some(h_base)) = (current.width, current.height) {
                    if w_base > 0.0 && h_base > 0.0 {
                        let object_width = w_base * current.scale_x;
                        let object_height = h_base * current.scale_y;

                        let object_center_x = current.x + (0.5 - anchor_x) * object_width;
                        let object_center_y = current.y + (0.5 - anchor_y) * object_height;

                        // Step 2: Zoom the object's center toward/from canvas center
                        // IMPORTANT: Use effective_factor, not factor, to avoid decentering when scale is capped
                        let new_object_center_x = canvas_center_x
                            + (object_center_x - canvas_center_x) * effective_factor;
                        let new_object_center_y = canvas_center_y
                            + (object_center_y - canvas_center_y) * effective_factor;

                        // Step 3: Calculate new anchor position
                        let new_object_width = w_base * new_state.scale_x;
                        let new_object_height = h_base * new_state.scale_y;

                        new_state.x = new_object_center_x - (0.5 - anchor_x) * new_object_width;
                        new_state.y = new_object_center_y - (0.5 - anchor_y) * new_object_height;

                        debug!("OBS scale zoom: {:.3} * {:.3} (eff={:.3}) = {:.3} align=({:.1},{:.1}) center {:.1},{:.1} → {:.1},{:.1} anchor {:.1},{:.1} → {:.1},{:.1}",
                            current.scale_x, factor, effective_factor, new_state.scale_x,
                            anchor_x, anchor_y,
                            object_center_x, object_center_y, new_object_center_x, new_object_center_y,
                            current.x, current.y, new_state.x, new_state.y);
                    } else {
                        // Fallback: if we don't have dimensions, just scale position directly
                        // Use effective_factor to avoid decentering when scale is capped
                        new_state.x =
                            canvas_center_x + (current.x - canvas_center_x) * effective_factor;
                        new_state.y =
                            canvas_center_y + (current.y - canvas_center_y) * effective_factor;

                        debug!("OBS scale zoom (fallback): {:.3} * {:.3} (eff={:.3}) = {:.3} pos {:.1},{:.1} → {:.1},{:.1}",
                            current.scale_x, factor, effective_factor, new_state.scale_x,
                            current.x, current.y, new_state.x, new_state.y);
                    }
                } else {
                    // No dimensions available, use fallback
                    // Use effective_factor to avoid decentering when scale is capped
                    let effective_factor_x = new_state.scale_x / current.scale_x;
                    let effective_factor_y = new_state.scale_y / current.scale_y;
                    let effective_factor = effective_factor_x.min(effective_factor_y);

                    new_state.x =
                        canvas_center_x + (current.x - canvas_center_x) * effective_factor;
                    new_state.y =
                        canvas_center_y + (current.y - canvas_center_y) * effective_factor;

                    debug!("OBS scale zoom (no dims): {:.3} * {:.3} (eff={:.3}) = {:.3} pos {:.1},{:.1} → {:.1},{:.1}",
                        current.scale_x, factor, effective_factor, new_state.scale_x,
                        current.x, current.y, new_state.x, new_state.y);
                }
            }
        }

        // Send update to OBS
        let guard = self.client.read().await;
        let client = guard.as_ref().context("OBS client not connected")?;

        // Build transform conditionally based on what changed
        let mut transform = obws::requests::scene_items::SceneItemTransform::default();

        // Include position if changed (pan, tilt, or zoom with position adjustment)
        if dx.is_some() || dy.is_some() || ds.is_some() {
            transform.position = Some(obws::requests::scene_items::Position {
                x: Some(new_state.x as f32),
                y: Some(new_state.y as f32),
                ..Default::default()
            });
        }

        // For scale changes: include EITHER bounds OR scale (not both!)
        if ds.is_some() {
            if let (Some(bw), Some(bh)) = (new_state.bounds_width, new_state.bounds_height) {
                if bw > 0.0 && bh > 0.0 {
                    // Use bounds-based transform (for camera sources)
                    transform.bounds = Some(obws::requests::scene_items::Bounds {
                        width: Some(bw as f32),
                        height: Some(bh as f32),
                        ..Default::default()
                    });
                    debug!("OBS sending BOUNDS transform: {}×{}", bw, bh);
                } else {
                    // Bounds exist but are zero - fall back to scale
                    transform.scale = Some(obws::requests::scene_items::Scale {
                        x: Some(new_state.scale_x as f32),
                        y: Some(new_state.scale_y as f32),
                        ..Default::default()
                    });
                }
            } else {
                // No bounds - use scale-based transform (for image sources)
                transform.scale = Some(obws::requests::scene_items::Scale {
                    x: Some(new_state.scale_x as f32),
                    y: Some(new_state.scale_y as f32),
                    ..Default::default()
                });
                debug!(
                    "OBS sending SCALE transform: {:.3}×{:.3}",
                    new_state.scale_x, new_state.scale_y
                );
            }
        }

        let result = client
            .scene_items()
            .set_transform(obws::requests::scene_items::SetTransform {
                scene: scene_name,
                item_id,
                transform,
            })
            .await;

        match result {
            Ok(_) => {
                if let (Some(bw), Some(bh)) = (new_state.bounds_width, new_state.bounds_height) {
                    if bw > 0.0 && bh > 0.0 {
                        debug!("OBS set_transform SUCCESS: '{}' pos=({:.1},{:.1}) bounds=({:.0}×{:.0})",
                            cache_key, new_state.x, new_state.y, bw, bh);
                    } else {
                        debug!(
                            "OBS set_transform SUCCESS: '{}' pos=({:.1},{:.1}) scale=({:.3},{:.3})",
                            cache_key,
                            new_state.x,
                            new_state.y,
                            new_state.scale_x,
                            new_state.scale_y
                        );
                    }
                } else {
                    debug!(
                        "OBS set_transform SUCCESS: '{}' pos=({:.1},{:.1}) scale=({:.3},{:.3})",
                        cache_key, new_state.x, new_state.y, new_state.scale_x, new_state.scale_y
                    );
                }
                // Update cache
                self.transform_cache.write().insert(cache_key, new_state);
                Ok(())
            },
            Err(e) => {
                warn!("OBS set_transform FAILED: '{}' error: {}", cache_key, e);
                Err(e).context("Failed to set scene item transform")
            },
        }?;

        Ok(())
    }

    /// Reset camera transform to default (centered, no zoom)
    ///
    /// # Arguments
    /// * `scene_name` - The OBS scene containing the source
    /// * `source_name` - The source to reset
    /// * `mode` - Reset mode: "position" (center), "zoom" (scale 1.0), or "both"
    pub async fn reset_transform(
        &self,
        scene_name: &str,
        source_name: &str,
        mode: &str,
    ) -> Result<()> {
        let item_id = self.resolve_item_id(scene_name, source_name).await?;

        // Get canvas dimensions for centering
        let (canvas_width, canvas_height) = self.get_canvas_dimensions().await?;
        let center_x = canvas_width / 2.0;
        let center_y = canvas_height / 2.0;

        // Read current transform to preserve what we don't reset
        let current = self.read_transform(scene_name, item_id).await?;

        let cache_key = self.cache_key(scene_name, source_name);
        let mut new_state = current.clone();

        let reset_position = mode == "position" || mode == "both";
        let reset_zoom = mode == "zoom" || mode == "both";

        // Get source base dimensions for centering calculations
        let (source_width, source_height) =
            if let (Some(w), Some(h)) = (current.width, current.height) {
                (w, h)
            } else {
                (canvas_width, canvas_height) // Fallback to canvas size
            };

        // Determine if using bounds-based or scale-based transform
        let use_bounds = if let (Some(bw), Some(bh)) = (current.bounds_width, current.bounds_height)
        {
            bw > 0.0 && bh > 0.0
        } else {
            false
        };

        if reset_zoom {
            if use_bounds {
                // Reset bounds to canvas dimensions
                new_state.bounds_width = Some(canvas_width);
                new_state.bounds_height = Some(canvas_height);
            } else {
                // Reset scale to 1.0
                new_state.scale_x = 1.0;
                new_state.scale_y = 1.0;
            }
        }

        if reset_position {
            // Calculate the object dimensions after zoom reset
            let (obj_width, obj_height) = if use_bounds {
                (
                    new_state.bounds_width.unwrap_or(canvas_width),
                    new_state.bounds_height.unwrap_or(canvas_height),
                )
            } else {
                (
                    source_width * new_state.scale_x,
                    source_height * new_state.scale_y,
                )
            };

            // Compute anchor from alignment
            let (anchor_x, anchor_y) = compute_anchor_from_alignment(current.alignment);

            // Position so object is centered on canvas
            // Object center should be at canvas center
            // anchor_pos = object_center - (0.5 - anchor) * object_size
            new_state.x = center_x - (0.5 - anchor_x) * obj_width;
            new_state.y = center_y - (0.5 - anchor_y) * obj_height;
        }

        // Apply the transform
        let guard = self.client.read().await;
        let client = guard.as_ref().context("OBS client not connected")?;

        let mut transform = obws::requests::scene_items::SceneItemTransform::default();

        if reset_position {
            transform.position = Some(obws::requests::scene_items::Position {
                x: Some(new_state.x as f32),
                y: Some(new_state.y as f32),
                ..Default::default()
            });
        }

        if reset_zoom {
            if use_bounds {
                transform.bounds = Some(obws::requests::scene_items::Bounds {
                    width: Some(new_state.bounds_width.unwrap() as f32),
                    height: Some(new_state.bounds_height.unwrap() as f32),
                    ..Default::default()
                });
            } else {
                transform.scale = Some(obws::requests::scene_items::Scale {
                    x: Some(new_state.scale_x as f32),
                    y: Some(new_state.scale_y as f32),
                    ..Default::default()
                });
            }
        }

        client
            .scene_items()
            .set_transform(obws::requests::scene_items::SetTransform {
                scene: scene_name,
                item_id,
                transform,
            })
            .await
            .context("Failed to reset scene item transform")?;

        // Update cache
        self.transform_cache
            .write()
            .insert(cache_key.clone(), new_state);

        info!(
            "OBS reset_transform: scene='{}' source='{}' mode='{}' -> centered at ({:.1},{:.1})",
            scene_name, source_name, mode, center_x, center_y
        );

        Ok(())
    }
}
