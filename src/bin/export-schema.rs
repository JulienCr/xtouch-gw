//! Generates a JSON Schema for `AppConfig` so the SPA editor can validate
//! YAML config against the same Rust types the runtime parses.
//!
//! Output: `editor/src/lib/generated/config.schema.json`

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use schemars::schema_for;
use xtouch_gw::config::AppConfig;

fn main() -> ExitCode {
    let schema = schema_for!(AppConfig);

    let json = match serde_json::to_string_pretty(&schema) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to serialize schema: {e}");
            return ExitCode::FAILURE;
        },
    };

    let out_path: PathBuf = ["editor", "src", "lib", "generated", "config.schema.json"]
        .iter()
        .collect();

    if let Some(parent) = out_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!(
                "error: failed to create directory {}: {e}",
                parent.display()
            );
            return ExitCode::FAILURE;
        }
    }

    if let Err(e) = fs::write(&out_path, json) {
        eprintln!("error: failed to write {}: {e}", out_path.display());
        return ExitCode::FAILURE;
    }

    println!("wrote schema to {}", out_path.display());
    ExitCode::SUCCESS
}
