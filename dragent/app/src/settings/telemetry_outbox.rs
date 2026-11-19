//! Local telemetry outbox (M4): buffered, metadata-only, opt-in.
//!
//! Events are [`dragent_agent::telemetry`] values — constructible only from
//! scalars and closed enums, so content can never enter the pipeline. The
//! outbox appends one JSON object per line under the app data dir (0600 on
//! unix) and does nothing at all unless `telemetry_enabled` is set.
//!
//! Upload waits for the server ingest endpoint (M3 follow-up):
//! [`Outbox::drain_for_upload`] hands serialized lines to the future
//! uploader and truncates the buffer. No automatic emission points exist
//! yet by design — the first emitter lands with the ingest contract so
//! events never pile up locally forever.

use std::path::PathBuf;

/// Append-only local buffer for control-plane telemetry.
pub struct Outbox {
    path: PathBuf,
    enabled: bool,
}

impl Outbox {
    /// Standard location: `~/.dragent/telemetry.jsonl`.
    pub fn default_path() -> PathBuf {
        dragent_paths::home().join("telemetry.jsonl")
    }

    pub fn new(path: PathBuf, enabled: bool) -> Self {
        Self { path, enabled }
    }

    /// Record one event. No-op (no file touched) when disabled or when the
    /// event cannot serialize — a telemetry path must never fail a turn.
    pub fn record(&self, event: &dragent_agent::telemetry::ExecutionTelemetry) {
        if !self.enabled {
            return;
        }
        let Ok(line) = serde_json::to_string(event) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(&self.path)
                .and_then(|mut f| {
                    use std::io::Write;
                    writeln!(f, "{line}")
                });
        }
        #[cfg(not(unix))]
        {
            use std::io::Write;
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .and_then(|mut f| writeln!(f, "{line}"));
        }
    }

    /// Hand buffered lines to an uploader and truncate. Returns the lines
    /// (empty when disabled or nothing buffered).
    pub fn drain_for_upload(&self) -> Vec<String> {
        if !self.enabled {
            return Vec::new();
        }
        let content = std::fs::read_to_string(&self.path).unwrap_or_default();
        let lines: Vec<String> = content.lines().map(str::to_string).collect();
        let _ = std::fs::write(&self.path, "");
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_agent::telemetry::{ErrorClass, ExecutionPath, ExecutionTelemetry, ProviderSlug};

    fn sample(success: bool) -> ExecutionTelemetry {
        ExecutionTelemetry {
            execution_id: uuid::Uuid::nil(),
            provider: ProviderSlug::OpenAI,
            path: ExecutionPath::Cloud,
            started_at_ms: 10,
            duration_ms: 20,
            success,
            error: (!success).then_some(ErrorClass::RateLimited),
            input_tokens: 30,
            output_tokens: 40,
            tool_calls: 2,
        }
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("dragent-telemetry-test-{name}.jsonl"))
    }

    #[test]
    fn disabled_outbox_touches_nothing() {
        let path = temp_path("disabled");
        let _ = std::fs::remove_file(&path);
        Outbox::new(path.clone(), false).record(&sample(true));
        assert!(!path.exists());
        assert!(Outbox::new(path, false).drain_for_upload().is_empty());
    }

    #[test]
    fn enabled_outbox_round_trips_jsonl() {
        let path = temp_path("enabled");
        let _ = std::fs::remove_file(&path);
        let outbox = Outbox::new(path.clone(), true);
        outbox.record(&sample(true));
        outbox.record(&sample(false));
        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(raw.lines().count(), 2);
        for line in raw.lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(v.get("execution_id").is_some());
        }
        let drained = outbox.drain_for_upload();
        assert_eq!(drained.len(), 2);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 0);
        let _ = std::fs::remove_file(&path);
    }
}
