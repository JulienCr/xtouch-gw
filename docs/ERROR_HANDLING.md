# Error Handling Strategy - XTouch GW v3

## Overview

XTouch GW uses a layered error handling approach that balances ergonomics with type safety, using Rust's `Result<T, E>` type extensively.

## Error Type Selection

### Application Code (`anyhow::Error`)

Use `anyhow::Result<T>` for application-level code where:
- Errors are typically handled at top-level (main, handlers)
- Rich context is more important than type matching
- Error chains need to be preserved

**Examples:**
- Configuration loading
- CLI command handling
- Main application logic
- Integration glue code

```rust
use anyhow::{Result, Context};

pub async fn load_config(path: &str) -> Result<AppConfig> {
    let contents = fs::read_to_string(path)
        .await
        .with_context(|| format!("Failed to read config: {}", path))?;
    
    let config: AppConfig = serde_yaml::from_str(&contents)
        .context("Failed to parse YAML")?;
    
    config.validate()?;
    Ok(config)
}
```

### Library Code (`thiserror::Error`)

Use `thiserror` for library-level errors where:
- Callers need to match on specific error variants
- Errors represent domain-specific failures
- Type safety is critical

**Examples:**
- MIDI protocol errors
- Driver-specific errors
- State management errors

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MidiError {
    #[error("Invalid MIDI channel: {0} (must be 1-16)")]
    InvalidChannel(u8),
    
    #[error("Port not found: {0}")]
    PortNotFound(String),
    
    #[error("Device disconnected")]
    Disconnected,
    
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
```

## Error Handling Principles

### 1. Never Panic on External Input

**❌ Bad:**
```rust
let channel = data[0]; // Can panic if data is empty
let value = config["key"].as_str().unwrap(); // Panics if missing
```

**✅ Good:**
```rust
let channel = data.first().ok_or(MidiError::InvalidMessage)?;
let value = config.get("key")
    .and_then(|v| v.as_str())
    .ok_or(ConfigError::MissingField("key"))?;
```

### 2. Provide Context at Call Sites

Add context where errors occur, not in library code:

```rust
// Library function - no context
pub fn parse_midi(data: &[u8]) -> Result<MidiMessage> {
    // ...
}

// Application code - adds context
let message = parse_midi(&buffer)
    .with_context(|| format!("Failed to parse MIDI from port {}", port_name))?;
```

### 3. Graceful Degradation

Non-critical failures should not crash the application:

```rust
match reload_config().await {
    Ok(new_config) => {
        info!("Config reloaded successfully");
        self.config = new_config;
    }
    Err(e) => {
        warn!("Failed to reload config, keeping old config: {}", e);
        // Continue with old config
    }
}
```

### 4. Retry with Backoff

For recoverable errors (network, hardware):

```rust
let mut backoff = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

loop {
    match connect_obs().await {
        Ok(client) => return Ok(client),
        Err(e) if e.is_retryable() => {
            warn!("Connection failed, retrying in {:?}: {}", backoff, e);
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
        Err(e) => return Err(e),
    }
}
```

## Error Propagation Patterns

### Option to Result Conversion

```rust
// Convert Option to Result with context
let port = midi_ports.get(&name)
    .ok_or_else(|| anyhow!("MIDI port '{}' not found", name))?;
```

### Result to Option Conversion

```rust
// Log error and return None
match load_optional_resource().await {
    Ok(resource) => Some(resource),
    Err(e) => {
        debug!("Optional resource not available: {}", e);
        None
    }
}
```

### Error Mapping

```rust
// Map library error to domain error
midi_device.send(&message)
    .map_err(|e| DriverError::SendFailed(e.to_string()))?;
```

## Async Error Handling

### Task Spawning

Always handle errors from spawned tasks:

```rust
// ❌ Bad - error lost
tokio::spawn(async move {
    process_events().await.unwrap(); // Can panic in another task!
});

// ✅ Good - error logged
tokio::spawn(async move {
    if let Err(e) = process_events().await {
        error!("Event processing failed: {}", e);
    }
});
```

### Broadcast Errors

Use channels to report errors from background tasks:

```rust
let (error_tx, mut error_rx) = mpsc::channel(10);

tokio::spawn(async move {
    if let Err(e) = worker_task().await {
        let _ = error_tx.send(e).await;
    }
});

// In main loop
tokio::select! {
    Some(error) = error_rx.recv() => {
        error!("Worker task failed: {}", error);
        // Handle or shutdown
    }
}
```

## Validation Errors

Collect multiple validation errors:

```rust
#[derive(Debug)]
pub struct ValidationErrors {
    errors: Vec<String>,
}

impl ValidationErrors {
    pub fn new() -> Self {
        Self { errors: Vec::new() }
    }
    
    pub fn add(&mut self, error: impl Into<String>) {
        self.errors.push(error.into());
    }
    
    pub fn into_result(self) -> Result<()> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!("Validation failed:\n  - {}", self.errors.join("\n  - "))
        }
    }
}
```

## Logging Strategy

### Log Levels

- **ERROR**: Unexpected failures that prevent functionality
- **WARN**: Degraded operation, recoverable failures
- **INFO**: Normal operational messages, state changes
- **DEBUG**: Detailed information for troubleshooting
- **TRACE**: Very detailed, including data dumps

### Error Logging

```rust
// Log at error site with full context
match critical_operation().await {
    Ok(result) => result,
    Err(e) => {
        error!("Critical operation failed: {:?}", e); // Full error chain
        return Err(e);
    }
}

// Or use tracing's error span
async fn process() -> Result<()> {
    let span = tracing::error_span!("process_events");
    async move {
        // errors logged automatically
    }.instrument(span).await
}
```

## Platform-Specific Errors

### Windows MIDI Errors

Handle Windows-specific quirks:

```rust
match midir.connect_output(&port, "xtouch-gw") {
    Err(e) if e.to_string().contains("Access denied") => {
        // Port already in use
        Err(MidiError::PortInUse(port_name))
    }
    Err(e) => Err(MidiError::ConnectionFailed(e.to_string())),
    Ok(conn) => Ok(conn),
}
```

## Testing Error Paths

```rust
#[tokio::test]
async fn test_invalid_config() {
    let result = AppConfig::load("invalid.yaml").await;
    assert!(result.is_err());
    
    let error = result.unwrap_err();
    assert!(error.to_string().contains("Failed to parse YAML"));
}

#[tokio::test]
async fn test_missing_file() {
    let result = AppConfig::load("nonexistent.yaml").await;
    assert!(result.is_err());
    
    let error = result.unwrap_err();
    assert!(error.to_string().contains("Failed to read config"));
}
```

## Summary

| Scenario | Error Type | Strategy |
|----------|-----------|----------|
| Config loading | `anyhow::Result` | Fail fast with context |
| MIDI operations | `thiserror::Error` | Retry with backoff |
| State updates | `anyhow::Result` | Log and continue |
| Driver failures | `thiserror::Error` | Graceful degradation |
| Validation | `anyhow::Result` | Collect all errors |
| Background tasks | `anyhow::Result` | Report via channels |
| Optional features | `Option<T>` | Log at DEBUG level |

## References

- [anyhow documentation](https://docs.rs/anyhow/)
- [thiserror documentation](https://docs.rs/thiserror/)
- [Rust Error Handling](https://doc.rust-lang.org/book/ch09-00-error-handling.html)

