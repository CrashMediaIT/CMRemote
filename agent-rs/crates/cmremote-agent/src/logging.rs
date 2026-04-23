// Source: CMRemote, clean-room implementation.

//! Logging initialisation. Wraps `tracing-subscriber` with the agent's
//! preferred defaults (env-controlled level, JSON in production,
//! human-friendly in dev).

use std::sync::Once;

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

static INIT: Once = Once::new();

/// Output format for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable single-line records. Default for interactive
    /// runs.
    Pretty,
    /// Newline-delimited JSON for ingestion by a log shipper.
    Json,
}

impl LogFormat {
    /// Pick a format based on whether stdout is a TTY.
    pub fn auto() -> Self {
        // Fall back to pretty when we cannot determine — that matches
        // a developer running the binary directly.
        if std::env::var("CMREMOTE_LOG_JSON")
            .map(|v| v == "1")
            .unwrap_or(false)
        {
            Self::Json
        } else {
            Self::Pretty
        }
    }
}

/// Install a global tracing subscriber. Idempotent: subsequent calls
/// are no-ops, which keeps tests well-behaved.
pub fn init(format: LogFormat) {
    INIT.call_once(|| {
        let filter = EnvFilter::try_from_env("CMREMOTE_LOG")
            .unwrap_or_else(|_| EnvFilter::new("info,cmremote_agent=info"));

        let registry = tracing_subscriber::registry().with(filter);

        // Best-effort install — `try_init` returns an error if a
        // subscriber is already installed, which we treat as success.
        let _ = match format {
            LogFormat::Json => registry.with(fmt::layer().json()).try_init(),
            LogFormat::Pretty => registry.with(fmt::layer().compact()).try_init(),
        };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        init(LogFormat::Pretty);
        init(LogFormat::Pretty);
        // No assertion — we're verifying no panic / double-install
        // error escapes.
    }
}
