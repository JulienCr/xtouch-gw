//! 7-segment display renderer for the X-Touch.
//!
//! The X-Touch's right-side digit array is physically split into TWO
//! separate displays:
//!
//! - **Assignment** — 2 digits in a small block.
//! - **Timecode** — 10 digits, framed off from the assignment.
//!
//! Spilling a word across the visible gap looks bad, so this module
//! treats each block as an independent renderer with its own effect
//! (static, marquee, blink, spinner, pulse, progress) and its own
//! internal animation state.
//!
//! Used by the seven-segment ticker task in `app.rs`, which polls
//! `render(now)` on a 50ms interval and forwards new frames to the main
//! loop for delivery (since `XTouchDriver` is `!Sync`).

use std::time::{Duration, Instant};

use crate::config::{SevenSegmentConfig, SevenSegmentEffect, SpinnerFramesSpec};

/// Width of the assignment block (2 leftmost digits, CCs `0x4A..=0x4B`).
pub const ASSIGNMENT_WIDTH: usize = 2;

/// Width of the timecode block (10 rightmost digits, CCs `0x40..=0x49`).
pub const TIMECODE_WIDTH: usize = 10;

const DEFAULT_SPINNER_FRAMES: &[&str] = &["/", "-", "\\", "|"];
const DEFAULT_MARQUEE_SPEED_MS: u64 = 200;
const DEFAULT_BLINK_PERIOD_MS: u64 = 800;
const DEFAULT_PULSE_PERIOD_MS: u64 = 1500;
const DEFAULT_SPINNER_SPEED_MS: u64 = 150;

/// One rendered tick, with optional per-region updates.
///
/// `None` for a field means "no change for this region this tick" so the
/// ticker can avoid resending unchanged frames.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RenderedFrame {
    pub assignment: Option<String>,
    pub timecode: Option<String>,
}

/// Top-level renderer: owns one optional `RegionRenderer` per display.
#[derive(Debug)]
pub struct SevenSegmentRenderer {
    assignment: Option<RegionRenderer>,
    timecode: Option<RegionRenderer>,
}

impl SevenSegmentRenderer {
    /// Build a renderer from optional config + the page name fallback.
    ///
    /// When the config does not provide a timecode effect, the timecode
    /// block falls back to a static frame showing the page name (truncated
    /// to 10 chars). The assignment block stays blank unless explicitly
    /// configured.
    pub fn from_config(config: Option<&SevenSegmentConfig>, page_name: &str, now: Instant) -> Self {
        let assignment = config
            .and_then(|c| c.assignment.as_ref())
            .map(|effect| RegionRenderer::from_effect(effect, ASSIGNMENT_WIDTH, now));

        let timecode = match config.and_then(|c| c.timecode.as_ref()) {
            Some(effect) => Some(RegionRenderer::from_effect(effect, TIMECODE_WIDTH, now)),
            None => Some(RegionRenderer::from_fallback_page_name(page_name, now)),
        };

        Self {
            assignment,
            timecode,
        }
    }

    /// Compute the next frame, returning per-region `Some(text)` only when
    /// the visible content actually changed since the previous call.
    ///
    /// On a freshly constructed renderer, the first call returns the
    /// initial paint for every region that has an effect.
    pub fn render(&mut self, now: Instant) -> RenderedFrame {
        RenderedFrame {
            assignment: self.assignment.as_mut().and_then(|r| r.render(now)),
            timecode: self.timecode.as_mut().and_then(|r| r.render(now)),
        }
    }
}

/// State for one region (either assignment or timecode).
#[derive(Debug)]
struct RegionRenderer {
    state: EffectState,
    started_at: Instant,
    width: usize,
}

impl RegionRenderer {
    fn from_effect(effect: &SevenSegmentEffect, width: usize, now: Instant) -> Self {
        Self {
            state: EffectState::from_effect(effect, width),
            started_at: now,
            width,
        }
    }

    /// Build a region renderer for the "no config" case: show the page
    /// name as static text, truncated to the region's width.
    fn from_fallback_page_name(page_name: &str, now: Instant) -> Self {
        let text = if page_name.is_empty() {
            "(none)".to_string()
        } else {
            page_name.to_string()
        };
        Self {
            state: EffectState::Static {
                text,
                emitted: false,
            },
            started_at: now,
            width: TIMECODE_WIDTH,
        }
    }

    fn render(&mut self, now: Instant) -> Option<String> {
        let elapsed = now.saturating_duration_since(self.started_at);
        self.state.render(elapsed, self.width)
    }
}

#[derive(Debug)]
enum EffectState {
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

impl EffectState {
    fn from_effect(effect: &SevenSegmentEffect, region_width: usize) -> Self {
        match effect {
            SevenSegmentEffect::Static { text } => Self::Static {
                text: text.clone(),
                emitted: false,
            },
            SevenSegmentEffect::Marquee { text, speed_ms } => Self::Marquee {
                text: text.clone(),
                speed_ms: speed_ms.unwrap_or(DEFAULT_MARQUEE_SPEED_MS).max(1),
                last_offset: None,
            },
            SevenSegmentEffect::Blink { text, period_ms } => Self::Blink {
                text: text.clone(),
                period_ms: period_ms.unwrap_or(DEFAULT_BLINK_PERIOD_MS).max(2),
                last_phase: None,
            },
            SevenSegmentEffect::Spinner {
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
                Self::Spinner {
                    prefix: prefix.clone().unwrap_or_default(),
                    frames: frames_vec,
                    speed_ms: speed_ms.unwrap_or(DEFAULT_SPINNER_SPEED_MS).max(1),
                    last_index: None,
                }
            },
            SevenSegmentEffect::Pulse { text, period_ms } => Self::Pulse {
                text: text.clone(),
                period_ms: period_ms.unwrap_or(DEFAULT_PULSE_PERIOD_MS).max(2),
                last_phase: None,
            },
            SevenSegmentEffect::Progress {
                value,
                width,
                prefix,
            } => {
                // Reserve room for the prefix so the bar never overflows
                // the region. If the prefix already fills the region, the
                // bar is squeezed to zero width.
                let prefix_str = prefix.clone().unwrap_or_default();
                let prefix_len = prefix_str.chars().count();
                let max_bar = region_width.saturating_sub(prefix_len);
                let bar_width = width.unwrap_or(max_bar).min(max_bar);
                Self::Progress {
                    value: value.clamp(0.0, 1.0),
                    width: bar_width,
                    prefix: prefix_str,
                    last_filled: None,
                }
            },
        }
    }

    fn render(&mut self, elapsed: Duration, region_width: usize) -> Option<String> {
        match self {
            Self::Static { text, emitted } => {
                if *emitted {
                    None
                } else {
                    *emitted = true;
                    Some(truncate(text, region_width))
                }
            },
            Self::Marquee {
                text,
                speed_ms,
                last_offset,
            } => {
                let offset = marquee_offset(text, elapsed, *speed_ms, region_width);
                if last_offset.is_none_or(|prev| prev != offset) {
                    *last_offset = Some(offset);
                    Some(marquee_frame(text, elapsed, *speed_ms, region_width))
                } else {
                    None
                }
            },
            Self::Blink {
                text,
                period_ms,
                last_phase,
            } => {
                let phase = blink_phase(elapsed, *period_ms);
                if last_phase.is_none_or(|prev| prev != phase) {
                    *last_phase = Some(phase);
                    Some(if phase {
                        truncate(text, region_width)
                    } else {
                        String::new()
                    })
                } else {
                    None
                }
            },
            Self::Spinner {
                prefix,
                frames,
                speed_ms,
                last_index,
            } => {
                let idx = spinner_index(elapsed, *speed_ms, frames.len());
                if last_index.is_none_or(|prev| prev != idx) {
                    *last_index = Some(idx);
                    Some(truncate(
                        &format!("{}{}", prefix, frames[idx]),
                        region_width,
                    ))
                } else {
                    None
                }
            },
            Self::Pulse {
                text,
                period_ms,
                last_phase,
            } => {
                let phase = blink_phase(elapsed, *period_ms);
                if last_phase.is_none_or(|prev| prev != phase) {
                    *last_phase = Some(phase);
                    Some(if phase {
                        truncate(text, region_width)
                    } else {
                        String::new()
                    })
                } else {
                    None
                }
            },
            Self::Progress {
                value,
                width,
                prefix,
                last_filled,
            } => {
                let filled = progress_filled(*value, *width);
                if last_filled.is_none_or(|prev| prev != filled) {
                    *last_filled = Some(filled);
                    Some(truncate(
                        &progress_frame(prefix, filled, *width),
                        region_width,
                    ))
                } else {
                    None
                }
            },
        }
    }
}

/// Truncate to the region width. The X-Touch hardware does its own
/// ASCII→segment decoding (see `XTouchDriver::set_assignment_text` /
/// `set_timecode_text`), so we don't translate characters here.
pub fn truncate(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

fn marquee_offset(text: &str, elapsed: Duration, speed_ms: u64, width: usize) -> usize {
    let padded_len = text.chars().count() + width;
    if padded_len == 0 {
        return 0;
    }
    let steps = elapsed.as_millis() as u64 / speed_ms.max(1);
    (steps as usize) % padded_len
}

fn marquee_frame(text: &str, elapsed: Duration, speed_ms: u64, width: usize) -> String {
    let padded: Vec<char> = text
        .chars()
        .chain(std::iter::repeat_n(' ', width))
        .collect();
    if padded.is_empty() {
        return String::new();
    }
    let offset = marquee_offset(text, elapsed, speed_ms, width);
    padded.iter().cycle().skip(offset).take(width).collect()
}

fn blink_phase(elapsed: Duration, period_ms: u64) -> bool {
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
    format!("{}{}", prefix, bar)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(now: Instant, ms: u64) -> Instant {
        now + Duration::from_millis(ms)
    }

    fn cfg(
        assignment: Option<SevenSegmentEffect>,
        timecode: Option<SevenSegmentEffect>,
    ) -> SevenSegmentConfig {
        SevenSegmentConfig {
            assignment,
            timecode,
        }
    }

    #[test]
    fn truncate_passes_short_text_through() {
        assert_eq!(truncate("OBS LIVE", 10), "OBS LIVE");
    }

    #[test]
    fn truncate_clips_to_width() {
        assert_eq!(truncate("ABCDEFGHIJKLMNOP", 10).chars().count(), 10);
    }

    #[test]
    fn fallback_shows_page_name_on_timecode_only() {
        let now = Instant::now();
        let mut r = SevenSegmentRenderer::from_config(None, "Page 1", now);
        let f = r.render(now);
        assert_eq!(f.assignment, None);
        assert_eq!(f.timecode.as_deref(), Some("Page 1"));
        // Second call: no change
        assert_eq!(r.render(at(now, 10)), RenderedFrame::default());
    }

    #[test]
    fn fallback_truncates_long_page_name() {
        let now = Instant::now();
        let mut r = SevenSegmentRenderer::from_config(None, "A very long page name here", now);
        let f = r.render(now);
        assert_eq!(
            f.timecode.as_deref().unwrap().chars().count(),
            TIMECODE_WIDTH
        );
    }

    #[test]
    fn assignment_static_renders_independently() {
        let now = Instant::now();
        let c = cfg(
            Some(SevenSegmentEffect::Static {
                text: "01".to_string(),
            }),
            None,
        );
        let mut r = SevenSegmentRenderer::from_config(Some(&c), "Page", now);
        let f = r.render(now);
        // Assignment renders the configured static.
        assert_eq!(f.assignment.as_deref(), Some("01"));
        // Timecode falls back to page name.
        assert_eq!(f.timecode.as_deref(), Some("Page"));
    }

    #[test]
    fn marquee_on_timecode_scrolls_within_region_width() {
        let now = Instant::now();
        let c = cfg(
            None,
            Some(SevenSegmentEffect::Marquee {
                text: "RECORDING SESSION".to_string(),
                speed_ms: Some(200),
            }),
        );
        let mut r = SevenSegmentRenderer::from_config(Some(&c), "", now);
        let f0 = r.render(now);
        assert_eq!(f0.assignment, None);
        let frame0 = f0.timecode.unwrap();
        assert_eq!(frame0.chars().count(), TIMECODE_WIDTH);
        // Same instant: no second emission.
        assert_eq!(r.render(now), RenderedFrame::default());
        // After 200ms, timecode advances; assignment still silent.
        let f1 = r.render(at(now, 200));
        assert!(f1.timecode.is_some());
        assert_ne!(f1.timecode.unwrap(), frame0);
        assert_eq!(f1.assignment, None);
    }

    #[test]
    fn blink_alternates_on_period() {
        let now = Instant::now();
        let c = cfg(
            None,
            Some(SevenSegmentEffect::Blink {
                text: "ON".to_string(),
                period_ms: Some(800),
            }),
        );
        let mut r = SevenSegmentRenderer::from_config(Some(&c), "", now);
        assert_eq!(r.render(now).timecode.as_deref(), Some("ON"));
        assert_eq!(r.render(at(now, 100)).timecode, None);
        assert_eq!(r.render(at(now, 400)).timecode.as_deref(), Some(""));
        assert_eq!(r.render(at(now, 800)).timecode.as_deref(), Some("ON"));
    }

    #[test]
    fn spinner_cycles_through_frames() {
        let now = Instant::now();
        let c = cfg(
            None,
            Some(SevenSegmentEffect::Spinner {
                prefix: Some("LOAD ".to_string()),
                frames: Some(SpinnerFramesSpec::List(vec![
                    "/".to_string(),
                    "-".to_string(),
                    "\\".to_string(),
                    "|".to_string(),
                ])),
                speed_ms: Some(150),
            }),
        );
        let mut r = SevenSegmentRenderer::from_config(Some(&c), "", now);
        assert_eq!(r.render(now).timecode.as_deref(), Some("LOAD /"));
        assert_eq!(r.render(at(now, 150)).timecode.as_deref(), Some("LOAD -"));
        assert_eq!(r.render(at(now, 300)).timecode.as_deref(), Some("LOAD \\"));
        assert_eq!(r.render(at(now, 450)).timecode.as_deref(), Some("LOAD |"));
        assert_eq!(r.render(at(now, 600)).timecode.as_deref(), Some("LOAD /"));
    }

    #[test]
    fn progress_renders_bar_in_region() {
        let now = Instant::now();
        let c = cfg(
            None,
            Some(SevenSegmentEffect::Progress {
                value: 0.5,
                width: Some(4),
                prefix: None,
            }),
        );
        let mut r = SevenSegmentRenderer::from_config(Some(&c), "", now);
        // 50% of 4 -> 2 '=' + 2 '-' = "==--"
        assert_eq!(r.render(now).timecode.as_deref(), Some("==--"));
    }

    #[test]
    fn progress_default_width_fits_region_minus_prefix() {
        let now = Instant::now();
        // Prefix "P" (1 char) + bar should fit in TIMECODE_WIDTH (10).
        let c = cfg(
            None,
            Some(SevenSegmentEffect::Progress {
                value: 1.0,
                width: None,
                prefix: Some("P".to_string()),
            }),
        );
        let mut r = SevenSegmentRenderer::from_config(Some(&c), "", now);
        let frame = r.render(now).timecode.unwrap();
        // 1 char prefix + 9 char bar = 10 chars total
        assert_eq!(frame.chars().count(), TIMECODE_WIDTH);
        assert!(frame.starts_with('P'));
    }

    #[test]
    fn both_regions_render_independently() {
        let now = Instant::now();
        let c = cfg(
            Some(SevenSegmentEffect::Blink {
                text: "AB".to_string(),
                period_ms: Some(400),
            }),
            Some(SevenSegmentEffect::Static {
                text: "STEADY".to_string(),
            }),
        );
        let mut r = SevenSegmentRenderer::from_config(Some(&c), "", now);
        let f0 = r.render(now);
        assert_eq!(f0.assignment.as_deref(), Some("AB"));
        assert_eq!(f0.timecode.as_deref(), Some("STEADY"));

        // Timecode is static -> emits once. Assignment keeps blinking.
        let f1 = r.render(at(now, 200));
        assert_eq!(f1.timecode, None);
        assert_eq!(f1.assignment.as_deref(), Some(""));
    }
}
