//! 7-segment timecode display renderer.
//!
//! Owns the state of an effect (static text, marquee scrolling, blink,
//! spinner, pulse, progress bar) and produces the next frame for the
//! 12-character timecode display on the X-Touch.
//!
//! Used by the seven-segment ticker task in `app.rs`, which polls
//! `render(now)` on a 50ms interval and forwards new frames to the main
//! loop for SysEx delivery (since `XTouchDriver` is `!Sync`).

use std::time::{Duration, Instant};

use crate::config::{SevenSegmentConfig, SpinnerFramesSpec};

/// Width of the X-Touch timecode display, in characters.
///
/// Matches the `center_to_length(text, 12)` in `XTouchDriver::set_seven_segment_text`.
pub const DISPLAY_WIDTH: usize = 12;

/// Default spinner frames if the user omits them.
const DEFAULT_SPINNER_FRAMES: &[&str] = &["/", "-", "\\", "|"];

/// Default speed for marquee scrolling (ms per step).
const DEFAULT_MARQUEE_SPEED_MS: u64 = 200;

/// Default blink period (full cycle: on + off).
const DEFAULT_BLINK_PERIOD_MS: u64 = 800;

/// Default pulse period (slower than blink, breath-style).
const DEFAULT_PULSE_PERIOD_MS: u64 = 1500;

/// Default spinner frame interval.
const DEFAULT_SPINNER_SPEED_MS: u64 = 150;

/// Default progress-bar width (digits used by the bar itself).
const DEFAULT_PROGRESS_WIDTH: usize = 8;

/// A rendering effect with its internal animation state.
#[derive(Debug, Clone)]
pub enum SevenSegmentEffect {
    Static {
        text: String,
        emitted: bool,
    },
    Marquee {
        text: String,
        speed_ms: u64,
        last_offset: Option<usize>,
    },
    Blink {
        text: String,
        period_ms: u64,
        last_phase: Option<bool>,
    },
    Spinner {
        prefix: String,
        frames: Vec<String>,
        speed_ms: u64,
        last_index: Option<usize>,
    },
    Pulse {
        text: String,
        period_ms: u64,
        last_phase: Option<bool>,
    },
    Progress {
        value: f32,
        width: usize,
        prefix: String,
        last_filled: Option<usize>,
    },
}

/// Renderer holding the effect and its start time.
#[derive(Debug)]
pub struct SevenSegmentRenderer {
    effect: SevenSegmentEffect,
    started_at: Instant,
}

impl SevenSegmentRenderer {
    /// Build a renderer from optional config + the page name fallback.
    ///
    /// When `config` is `None`, the renderer shows the page name (or `(none)`
    /// if the page name is empty) as a static frame — preserving the
    /// pre-feature behavior for pages that haven't opted in.
    pub fn from_config(config: Option<&SevenSegmentConfig>, page_name: &str, now: Instant) -> Self {
        let effect = match config {
            None => SevenSegmentEffect::Static {
                text: if page_name.is_empty() {
                    "(none)".to_string()
                } else {
                    page_name.to_string()
                },
                emitted: false,
            },
            Some(cfg) => build_effect(cfg),
        };
        Self {
            effect,
            started_at: now,
        }
    }

    /// Compute the frame for `now`, returning `Some(text)` only when the
    /// visible frame actually changed since the previous call.
    ///
    /// The first call after construction always returns `Some`, so the
    /// display gets painted on page change without a perceptible delay.
    pub fn render(&mut self, now: Instant) -> Option<String> {
        let elapsed = now.saturating_duration_since(self.started_at);
        match &mut self.effect {
            SevenSegmentEffect::Static { text, emitted } => {
                if *emitted {
                    None
                } else {
                    *emitted = true;
                    Some(sanitize(text))
                }
            },
            SevenSegmentEffect::Marquee {
                text,
                speed_ms,
                last_offset,
            } => {
                let sanitized = sanitize(text);
                let frame = marquee_frame(&sanitized, elapsed, *speed_ms);
                let offset = marquee_offset(&sanitized, elapsed, *speed_ms);
                if last_offset.is_none_or(|prev| prev != offset) {
                    *last_offset = Some(offset);
                    Some(frame)
                } else {
                    None
                }
            },
            SevenSegmentEffect::Blink {
                text,
                period_ms,
                last_phase,
            } => {
                let phase = blink_phase(elapsed, *period_ms);
                if last_phase.is_none_or(|prev| prev != phase) {
                    *last_phase = Some(phase);
                    Some(if phase { sanitize(text) } else { String::new() })
                } else {
                    None
                }
            },
            SevenSegmentEffect::Spinner {
                prefix,
                frames,
                speed_ms,
                last_index,
            } => {
                let idx = spinner_index(elapsed, *speed_ms, frames.len());
                if last_index.is_none_or(|prev| prev != idx) {
                    *last_index = Some(idx);
                    Some(sanitize(&format!("{}{}", prefix, frames[idx])))
                } else {
                    None
                }
            },
            SevenSegmentEffect::Pulse {
                text,
                period_ms,
                last_phase,
            } => {
                // Same logic as Blink, distinct enum variant to allow
                // tuning defaults differently (slower period).
                let phase = blink_phase(elapsed, *period_ms);
                if last_phase.is_none_or(|prev| prev != phase) {
                    *last_phase = Some(phase);
                    Some(if phase { sanitize(text) } else { String::new() })
                } else {
                    None
                }
            },
            SevenSegmentEffect::Progress {
                value,
                width,
                prefix,
                last_filled,
            } => {
                let filled = progress_filled(*value, *width);
                if last_filled.is_none_or(|prev| prev != filled) {
                    *last_filled = Some(filled);
                    Some(progress_frame(prefix, filled, *width))
                } else {
                    None
                }
            },
        }
    }
}

/// Replace 7-segment-unrenderable characters with `_` to keep length stable
/// and truncate to the display width.
///
/// The unrenderable set matches `XTouchDriver::seven_seg_for_char`: M, W,
/// K, X map to `_`. V is rendered as a U-shape and passes through. All
/// other chars pass through untouched so the existing SysEx encoder
/// handles them.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'M' | 'm' | 'W' | 'w' | 'K' | 'k' | 'X' | 'x' => '_',
            other => other,
        })
        .take(DISPLAY_WIDTH)
        .collect()
}

fn build_effect(cfg: &SevenSegmentConfig) -> SevenSegmentEffect {
    match cfg {
        SevenSegmentConfig::Static { text } => SevenSegmentEffect::Static {
            text: text.clone(),
            emitted: false,
        },
        SevenSegmentConfig::Marquee { text, speed_ms } => SevenSegmentEffect::Marquee {
            text: text.clone(),
            speed_ms: speed_ms.unwrap_or(DEFAULT_MARQUEE_SPEED_MS).max(1),
            last_offset: None,
        },
        SevenSegmentConfig::Blink { text, period_ms } => SevenSegmentEffect::Blink {
            text: text.clone(),
            period_ms: period_ms.unwrap_or(DEFAULT_BLINK_PERIOD_MS).max(2),
            last_phase: None,
        },
        SevenSegmentConfig::Spinner {
            prefix,
            frames,
            speed_ms,
        } => {
            let frames_vec = match frames {
                Some(SpinnerFramesSpec::List(v)) if !v.is_empty() => v.clone(),
                _ => DEFAULT_SPINNER_FRAMES
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            };
            SevenSegmentEffect::Spinner {
                prefix: prefix.clone().unwrap_or_default(),
                frames: frames_vec,
                speed_ms: speed_ms.unwrap_or(DEFAULT_SPINNER_SPEED_MS).max(1),
                last_index: None,
            }
        },
        SevenSegmentConfig::Pulse { text, period_ms } => SevenSegmentEffect::Pulse {
            text: text.clone(),
            period_ms: period_ms.unwrap_or(DEFAULT_PULSE_PERIOD_MS).max(2),
            last_phase: None,
        },
        SevenSegmentConfig::Progress {
            value,
            width,
            prefix,
        } => {
            let width = width.unwrap_or(DEFAULT_PROGRESS_WIDTH).min(DISPLAY_WIDTH);
            SevenSegmentEffect::Progress {
                value: value.clamp(0.0, 1.0),
                width,
                prefix: prefix.clone().unwrap_or_default(),
                last_filled: None,
            }
        },
    }
}

fn marquee_offset(sanitized: &str, elapsed: Duration, speed_ms: u64) -> usize {
    // Pad with trailing spaces so the marquee scrolls off the right edge
    // before wrapping. Length includes display width of padding.
    let padded_len = sanitized.chars().count() + DISPLAY_WIDTH;
    if padded_len == 0 {
        return 0;
    }
    let steps = elapsed.as_millis() as u64 / speed_ms.max(1);
    (steps as usize) % padded_len
}

fn marquee_frame(sanitized: &str, elapsed: Duration, speed_ms: u64) -> String {
    let padded: Vec<char> = sanitized
        .chars()
        .chain(std::iter::repeat_n(' ', DISPLAY_WIDTH))
        .collect();
    if padded.is_empty() {
        return String::new();
    }
    let offset = marquee_offset(sanitized, elapsed, speed_ms);
    padded
        .iter()
        .cycle()
        .skip(offset)
        .take(DISPLAY_WIDTH)
        .collect()
}

fn blink_phase(elapsed: Duration, period_ms: u64) -> bool {
    // True = on, false = off. Half-period each.
    let half = period_ms.max(2) / 2;
    let cycle = (elapsed.as_millis() as u64) / half.max(1);
    cycle.is_multiple_of(2)
}

fn spinner_index(elapsed: Duration, speed_ms: u64, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let steps = elapsed.as_millis() as u64 / speed_ms.max(1);
    (steps as usize) % n
}

fn progress_filled(value: f32, width: usize) -> usize {
    if width == 0 {
        return 0;
    }
    let v = value.clamp(0.0, 1.0);
    (v * width as f32).round() as usize
}

fn progress_frame(prefix: &str, filled: usize, width: usize) -> String {
    let filled = filled.min(width);
    let bar: String = std::iter::repeat_n('=', filled)
        .chain(std::iter::repeat_n('-', width - filled))
        .collect();
    sanitize(&format!("{}{}", prefix, bar))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SevenSegmentConfig, SpinnerFramesSpec};

    fn at(now: Instant, ms: u64) -> Instant {
        now + Duration::from_millis(ms)
    }

    #[test]
    fn sanitize_preserves_renderables() {
        assert_eq!(sanitize("OBS LIVE"), "OBS LIVE");
        assert_eq!(sanitize("REC 042"), "REC 042");
    }

    #[test]
    fn sanitize_replaces_unrenderables() {
        // K and M have no honest 7-segment rendering.
        assert_eq!(sanitize("MIKE"), "_I_E");
        assert_eq!(sanitize("WORK"), "_OR_");
    }

    #[test]
    fn sanitize_truncates_to_display_width() {
        let long = "ABCDEFGHIJKLMNOP";
        assert_eq!(sanitize(long).chars().count(), DISPLAY_WIDTH);
    }

    #[test]
    fn static_emits_once_then_none() {
        let now = Instant::now();
        let cfg = SevenSegmentConfig::Static {
            text: "OBS LIVE".to_string(),
        };
        let mut r = SevenSegmentRenderer::from_config(Some(&cfg), "", now);
        assert_eq!(r.render(now).as_deref(), Some("OBS LIVE"));
        assert_eq!(r.render(at(now, 10)), None);
        assert_eq!(r.render(at(now, 100)), None);
    }

    #[test]
    fn fallback_to_page_name_when_no_config() {
        let now = Instant::now();
        let mut r = SevenSegmentRenderer::from_config(None, "Page 1", now);
        assert_eq!(r.render(now).as_deref(), Some("Page 1"));
    }

    #[test]
    fn marquee_scrolls_one_step_per_speed_ms() {
        let now = Instant::now();
        let cfg = SevenSegmentConfig::Marquee {
            text: "ABCDEFGHIJKL".to_string(),
            speed_ms: Some(200),
        };
        let mut r = SevenSegmentRenderer::from_config(Some(&cfg), "", now);
        let f0 = r.render(now).unwrap();
        // Same instant -> no second frame
        assert!(r.render(now).is_none());
        // After 200ms -> frame shifts by 1
        let f1 = r.render(at(now, 200)).unwrap();
        assert_ne!(f0, f1);
        assert_eq!(f0.chars().count(), DISPLAY_WIDTH);
        assert_eq!(f1.chars().count(), DISPLAY_WIDTH);
    }

    #[test]
    fn blink_alternates_on_period() {
        let now = Instant::now();
        let cfg = SevenSegmentConfig::Blink {
            text: "ON".to_string(),
            period_ms: Some(800),
        };
        let mut r = SevenSegmentRenderer::from_config(Some(&cfg), "", now);
        assert_eq!(r.render(now).as_deref(), Some("ON"));
        assert_eq!(r.render(at(now, 100)), None);
        // Half-period crosses to "off"
        assert_eq!(r.render(at(now, 400)).as_deref(), Some(""));
        // Full period back to "on"
        assert_eq!(r.render(at(now, 800)).as_deref(), Some("ON"));
    }

    #[test]
    fn spinner_cycles_through_frames() {
        let now = Instant::now();
        let cfg = SevenSegmentConfig::Spinner {
            prefix: Some("LOAD ".to_string()),
            frames: Some(SpinnerFramesSpec::List(vec![
                "/".to_string(),
                "-".to_string(),
                "\\".to_string(),
                "|".to_string(),
            ])),
            speed_ms: Some(150),
        };
        let mut r = SevenSegmentRenderer::from_config(Some(&cfg), "", now);
        assert_eq!(r.render(now).as_deref(), Some("LOAD /"));
        assert_eq!(r.render(at(now, 150)).as_deref(), Some("LOAD -"));
        assert_eq!(r.render(at(now, 300)).as_deref(), Some("LOAD \\"));
        assert_eq!(r.render(at(now, 450)).as_deref(), Some("LOAD |"));
        // Wraps
        assert_eq!(r.render(at(now, 600)).as_deref(), Some("LOAD /"));
    }

    #[test]
    fn spinner_uses_default_frames_when_unspecified() {
        let now = Instant::now();
        let cfg = SevenSegmentConfig::Spinner {
            prefix: None,
            frames: None,
            speed_ms: Some(100),
        };
        let mut r = SevenSegmentRenderer::from_config(Some(&cfg), "", now);
        assert_eq!(r.render(now).as_deref(), Some("/"));
    }

    #[test]
    fn progress_renders_bar() {
        let now = Instant::now();
        let cfg = SevenSegmentConfig::Progress {
            value: 0.5,
            width: Some(4),
            prefix: None,
        };
        let mut r = SevenSegmentRenderer::from_config(Some(&cfg), "", now);
        // 50% of 4 = 2 filled, 2 empty -> "==--"
        assert_eq!(r.render(now).as_deref(), Some("==--"));
    }

    #[test]
    fn progress_clamps_value() {
        let now = Instant::now();
        let cfg = SevenSegmentConfig::Progress {
            value: 1.7,
            width: Some(3),
            prefix: None,
        };
        let mut r = SevenSegmentRenderer::from_config(Some(&cfg), "", now);
        assert_eq!(r.render(now).as_deref(), Some("==="));
    }

    #[test]
    fn pulse_alternates_like_blink() {
        let now = Instant::now();
        let cfg = SevenSegmentConfig::Pulse {
            text: "LIVE".to_string(),
            period_ms: Some(1500),
        };
        let mut r = SevenSegmentRenderer::from_config(Some(&cfg), "", now);
        assert_eq!(r.render(now).as_deref(), Some("LIVE"));
        // Half-period -> off
        assert_eq!(r.render(at(now, 750)).as_deref(), Some(""));
    }
}
