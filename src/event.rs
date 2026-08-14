use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    os::fd::AsRawFd,
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Observed,
    Derived,
    ModelInterpreted,
    AgentClaim,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RawEvent {
    pub id: Uuid,
    pub session_id: Uuid,
    pub host_monotonic_ns: u64,
    pub wall_clock_time: String,
    pub source: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_timestamp: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sequence: Option<u64>,
    pub payload: Value,
    pub artifact_refs: Vec<String>,
    pub provenance: Provenance,
}

impl RawEvent {
    pub fn observed(session_id: Uuid, source: &str, kind: &str, payload: Value) -> Self {
        Self::observed_at(session_id, monotonic_ns(), source, kind, payload)
    }

    pub fn observed_at(
        session_id: Uuid,
        host_monotonic_ns: u64,
        source: &str,
        kind: &str,
        payload: Value,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            session_id,
            host_monotonic_ns,
            wall_clock_time: Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
            source: source.to_owned(),
            kind: kind.to_owned(),
            repository_fingerprint: None,
            source_timestamp: None,
            source_sequence: None,
            payload,
            artifact_refs: Vec::new(),
            provenance: Provenance::Observed,
        }
    }
}

/// CLOCK_MONOTONIC is deliberately read at the event source. It is not inferred
/// later from wall-clock time, which can jump.
pub fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid writable timespec for the duration of the call.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    assert_eq!(result, 0, "CLOCK_MONOTONIC must be available");
    (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64)
}

pub trait EventSink: Send + Sync {
    fn record(&self, event: RawEvent) -> Result<()>;
}

#[derive(Default)]
pub struct MemoryEventSink(Mutex<Vec<RawEvent>>);

impl MemoryEventSink {
    pub fn events(&self) -> Vec<RawEvent> {
        self.0.lock().expect("event mutex poisoned").clone()
    }
}

impl EventSink for MemoryEventSink {
    fn record(&self, event: RawEvent) -> Result<()> {
        self.0.lock().expect("event mutex poisoned").push(event);
        Ok(())
    }
}

pub struct JsonlEventSink(Mutex<BufWriter<File>>);

impl JsonlEventSink {
    pub fn append(path: impl AsRef<Path>) -> Result<Arc<Self>> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create event directory {}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open event log {}", path.display()))?;
        Ok(Arc::new(Self(Mutex::new(BufWriter::new(file)))))
    }
}

impl EventSink for JsonlEventSink {
    fn record(&self, event: RawEvent) -> Result<()> {
        let mut writer = self.0.lock().expect("event writer mutex poisoned");
        let descriptor = writer.get_ref().as_raw_fd();
        let locked = unsafe { libc::flock(descriptor, libc::LOCK_EX) };
        if locked != 0 {
            return Err(std::io::Error::last_os_error()).context("lock event log");
        }
        let result = (|| -> Result<()> {
            let mut line = serde_json::to_vec(&event)?;
            line.push(b'\n');
            writer.write_all(&line)?;
            writer.flush()?;
            Ok(())
        })();
        let unlocked = unsafe { libc::flock(descriptor, libc::LOCK_UN) };
        if unlocked != 0 && result.is_ok() {
            return Err(std::io::Error::last_os_error()).context("unlock event log");
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_timestamps_do_not_go_backwards() {
        let a = monotonic_ns();
        let b = monotonic_ns();
        assert!(b >= a);
    }

    #[test]
    fn jsonl_sink_persists_complete_envelopes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let sink = JsonlEventSink::append(&path).unwrap();
        let session = Uuid::new_v4();
        sink.record(RawEvent::observed(
            session,
            "input",
            "pointer.down",
            serde_json::json!({"button": "left"}),
        ))
        .unwrap();
        drop(sink);

        let line = std::fs::read_to_string(path).unwrap();
        let event: RawEvent = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(event.session_id, session);
        assert_eq!(event.provenance, Provenance::Observed);
        assert_eq!(event.source, "input");
    }

    #[test]
    fn independent_jsonl_sinks_append_complete_records() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        let first = JsonlEventSink::append(&path).unwrap();
        let second = JsonlEventSink::append(&path).unwrap();
        let session = Uuid::new_v4();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for sequence in 0..100 {
                    first
                        .record(RawEvent::observed(
                            session,
                            "first",
                            "test",
                            serde_json::json!({"sequence": sequence}),
                        ))
                        .unwrap();
                }
            });
            scope.spawn(|| {
                for sequence in 0..100 {
                    second
                        .record(RawEvent::observed(
                            session,
                            "second",
                            "test",
                            serde_json::json!({"sequence": sequence}),
                        ))
                        .unwrap();
                }
            });
        });
        let events = std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<RawEvent>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 200);
        assert_eq!(
            events
                .iter()
                .filter(|event| event.source == "first")
                .count(),
            100
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.source == "second")
                .count(),
            100
        );
    }
}
