//! Command-line interface and REPL
//!
//! Provides stdin command processing for runtime debugging.
//! The `state` command dumps MidiStateEntry values from the StateActor.

use crate::state::StateActorHandle;
use crate::state::{AppKey, MidiStatus, MidiValue};

/// Process a single REPL command line.
///
/// Returns `true` if the application should shut down.
pub async fn process_command(line: &str, state_actor: &StateActorHandle) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    match trimmed {
        "exit" | "quit" => return true,
        "$" => {
            // Clear terminal (ANSI escape)
            print!("\x1B[2J\x1B[H");
            use std::io::Write;
            let _ = std::io::stdout().flush();
        },
        "help" | "?" => {
            println!("Commands:");
            println!("  state              Dump all app states");
            println!(
                "  state <app>        Dump state for app (voicemeeter, qlc, obs, midi-bridge)"
            );
            println!("  state <app> <type> Filter by status type (cc, pb, note, sysex)");
            println!("  $                  Clear terminal");
            println!("  exit / quit        Shutdown");
        },
        _ => {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            match parts[0] {
                "state" => handle_state_command(&parts[1..], state_actor).await,
                other => println!(
                    "Unknown command: '{}'. Type 'help' for available commands.",
                    other
                ),
            }
        },
    }

    false
}

/// Handle the `state` command with optional app and status filters.
async fn handle_state_command(args: &[&str], state_actor: &StateActorHandle) {
    let app_filter = args.first().and_then(|s| AppKey::from_str(s));
    let status_filter = args.get(1).and_then(|s| parse_status_filter(s));

    // Validate app name if provided but not recognized
    if let Some(name) = args.first() {
        if app_filter.is_none() {
            println!(
                "Unknown app: '{}'. Valid: voicemeeter, qlc, obs, midi-bridge",
                name
            );
            return;
        }
    }

    // Validate status filter if provided but not recognized
    if let Some(name) = args.get(1) {
        if status_filter.is_none() {
            println!("Unknown status: '{}'. Valid: cc, pb, note, sysex", name);
            return;
        }
    }

    let apps = match app_filter {
        Some(app) => vec![app],
        None => AppKey::all().to_vec(),
    };

    let states = state_actor.list_states_for_apps(apps.clone()).await;

    let mut any_output = false;
    for app in &apps {
        let mut entries = match states.get(app) {
            Some(e) if !e.is_empty() => e.clone(),
            _ => continue,
        };

        // Apply status filter
        if let Some(status) = &status_filter {
            entries.retain(|e| &e.addr.status == status);
            if entries.is_empty() {
                continue;
            }
        }

        // Sort: status, then channel, then data1
        entries.sort_by(|a, b| {
            let status_ord = format!("{:?}", a.addr.status).cmp(&format!("{:?}", b.addr.status));
            status_ord
                .then(a.addr.channel.cmp(&b.addr.channel))
                .then(a.addr.data1.cmp(&b.addr.data1))
        });

        println!("=== {} ({} entries) ===", app, entries.len());
        for entry in &entries {
            let status = format!("{:>5}", format!("{}", entry.addr.status).to_uppercase());
            let ch = entry
                .addr
                .channel
                .map(|c| format!("{}", c))
                .unwrap_or("-".into());
            let d1 = entry
                .addr
                .data1
                .map(|d| format!("{}", d))
                .unwrap_or("-".into());
            let value = format_value(&entry.value);
            let known = if entry.known { "known" } else { "unknown" };
            let stale = if entry.stale { " stale" } else { "" };
            let port = &entry.addr.port_id;

            println!(
                "  {} ch={:<2} data1={:<3} value={:<6} {}{}  port={}",
                status, ch, d1, value, known, stale, port
            );
        }
        any_output = true;
    }

    if !any_output {
        println!("(no state entries)");
    }
}

fn parse_status_filter(s: &str) -> Option<MidiStatus> {
    match s.to_lowercase().as_str() {
        "cc" => Some(MidiStatus::CC),
        "pb" => Some(MidiStatus::PB),
        "note" => Some(MidiStatus::Note),
        "sysex" => Some(MidiStatus::SysEx),
        _ => None,
    }
}

fn format_value(value: &MidiValue) -> String {
    match value {
        MidiValue::Number(n) => n.to_string(),
        MidiValue::Text(t) => format!("\"{}\"", t),
        MidiValue::Binary(b) => format!("[{}B]", b.len()),
    }
}
