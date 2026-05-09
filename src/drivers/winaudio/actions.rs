//! Action parameter parsing for the WinAudio driver.
//!
//! Session-targeted actions accept a single string parameter of the form
//! `"pinned:<N>"` or `"discovered:<N>"` where `N` is a 1-based slot for
//! pinned apps and a 0-based index for discovery slots. The slot index is
//! resolved at runtime by the driver's mapping layer to a concrete
//! Windows audio session.

use anyhow::{anyhow, Result};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTarget {
    /// Pinned slot, 1..=8 (matches the fader number declared in YAML).
    Pinned(u8),
    /// Discovery slot, 0-based, used to index into the FIFO list of
    /// auto-discovered sessions that aren't pinned.
    Discovered(u8),
}

pub fn parse_session_target(params: &[Value]) -> Result<SessionTarget> {
    let raw = params
        .first()
        .ok_or_else(|| {
            anyhow!("session action requires a target parameter (pinned:N or discovered:N)")
        })?
        .as_str()
        .ok_or_else(|| anyhow!("session target must be a string (pinned:N or discovered:N)"))?;

    let (kind, idx) = raw
        .split_once(':')
        .ok_or_else(|| anyhow!("session target '{}' missing ':' separator", raw))?;

    let n: u8 = idx
        .trim()
        .parse()
        .map_err(|_| anyhow!("session target '{}': index '{}' is not a u8", raw, idx))?;

    match kind.trim() {
        "pinned" => {
            if !(1..=8).contains(&n) {
                return Err(anyhow!("pinned slot {} must be in 1..=8", n));
            }
            Ok(SessionTarget::Pinned(n))
        },
        "discovered" => {
            if n >= 8 {
                return Err(anyhow!("discovered slot {} must be < 8", n));
            }
            Ok(SessionTarget::Discovered(n))
        },
        other => Err(anyhow!(
            "unknown session target kind '{}': expected 'pinned' or 'discovered'",
            other
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_pinned() {
        let p = parse_session_target(&[json!("pinned:3")]).unwrap();
        assert_eq!(p, SessionTarget::Pinned(3));
    }

    #[test]
    fn parses_discovered() {
        let p = parse_session_target(&[json!("discovered:2")]).unwrap();
        assert_eq!(p, SessionTarget::Discovered(2));
    }

    #[test]
    fn rejects_missing_separator() {
        assert!(parse_session_target(&[json!("pinned3")]).is_err());
    }

    #[test]
    fn rejects_bad_kind() {
        assert!(parse_session_target(&[json!("foo:1")]).is_err());
    }

    #[test]
    fn rejects_pinned_out_of_range() {
        assert!(parse_session_target(&[json!("pinned:9")]).is_err());
        assert!(parse_session_target(&[json!("pinned:0")]).is_err());
    }

    #[test]
    fn rejects_missing_param() {
        assert!(parse_session_target(&[]).is_err());
    }
}
