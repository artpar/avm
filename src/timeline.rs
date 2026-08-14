use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use uuid::Uuid;

use crate::{
    event::{EventSink, JsonlEventSink, Provenance, RawEvent},
    fingerprint::repository_fingerprint,
};

pub struct ExperienceEventSink {
    timeline: TimelineStore,
    jsonl: std::sync::Arc<JsonlEventSink>,
    fingerprint: FingerprintMode,
}

enum FingerprintMode {
    Fixed(String),
    Dynamic(PathBuf),
}

impl ExperienceEventSink {
    pub fn open(
        timeline_path: impl AsRef<Path>,
        jsonl_path: impl AsRef<Path>,
        repository: impl AsRef<Path>,
    ) -> Result<Self> {
        Ok(Self {
            timeline: TimelineStore::open(timeline_path)?,
            jsonl: JsonlEventSink::append(jsonl_path)?,
            fingerprint: FingerprintMode::Fixed(repository_fingerprint(repository)?),
        })
    }

    pub fn open_dynamic(
        timeline_path: impl AsRef<Path>,
        jsonl_path: impl AsRef<Path>,
        repository: impl AsRef<Path>,
    ) -> Result<Self> {
        let repository = repository.as_ref().canonicalize()?;
        repository_fingerprint(&repository)?;
        Ok(Self {
            timeline: TimelineStore::open(timeline_path)?,
            jsonl: JsonlEventSink::append(jsonl_path)?,
            fingerprint: FingerprintMode::Dynamic(repository),
        })
    }
}

impl EventSink for ExperienceEventSink {
    fn record(&self, mut event: RawEvent) -> Result<()> {
        if event.repository_fingerprint.is_none() {
            event.repository_fingerprint = Some(match &self.fingerprint {
                FingerprintMode::Fixed(fingerprint) => fingerprint.clone(),
                FingerprintMode::Dynamic(repository) => repository_fingerprint(repository)?,
            });
        }
        self.timeline.record(event.clone())?;
        self.jsonl.record(event)
    }
}

pub struct TimelineStore {
    connection: Mutex<Connection>,
}

impl TimelineStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)
            .with_context(|| format!("open timeline database {}", path.display()))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .context("configure timeline write-lock timeout")?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS experience_events (
               id TEXT PRIMARY KEY NOT NULL,
               session_id TEXT NOT NULL,
               host_monotonic_ns INTEGER NOT NULL,
               wall_clock_time TEXT NOT NULL,
               source TEXT NOT NULL,
               kind TEXT NOT NULL,
               repository_fingerprint TEXT,
               source_timestamp TEXT,
               source_sequence INTEGER,
               payload TEXT NOT NULL,
               artifact_refs TEXT NOT NULL,
               provenance TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS experience_events_time
               ON experience_events(session_id, host_monotonic_ns, id);
             CREATE INDEX IF NOT EXISTS experience_events_source
               ON experience_events(session_id, source, host_monotonic_ns);",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn all(&self, session_id: Uuid) -> Result<Vec<RawEvent>> {
        self.range(session_id, None, None)
    }

    pub fn range(
        &self,
        session_id: Uuid,
        start_ns: Option<u64>,
        end_ns: Option<u64>,
    ) -> Result<Vec<RawEvent>> {
        let start =
            i64::try_from(start_ns.unwrap_or(0)).context("start timestamp exceeds SQLite range")?;
        let end = i64::try_from(end_ns.unwrap_or(i64::MAX as u64))
            .context("end timestamp exceeds SQLite range")?;
        let connection = self.connection.lock().expect("timeline mutex poisoned");
        let mut statement = connection.prepare(
            "SELECT id, session_id, host_monotonic_ns, wall_clock_time, source, kind,
                    repository_fingerprint, source_timestamp, source_sequence, payload,
                    artifact_refs, provenance
             FROM experience_events
             WHERE session_id = ?1 AND host_monotonic_ns >= ?2 AND host_monotonic_ns <= ?3
             ORDER BY host_monotonic_ns ASC, id ASC",
        )?;
        let rows =
            statement.query_map(params![session_id.to_string(), start, end], decode_event)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn event(&self, id: Uuid) -> Result<Option<RawEvent>> {
        let connection = self.connection.lock().expect("timeline mutex poisoned");
        let mut statement = connection.prepare(
            "SELECT id, session_id, host_monotonic_ns, wall_clock_time, source, kind,
                    repository_fingerprint, source_timestamp, source_sequence, payload,
                    artifact_refs, provenance
             FROM experience_events WHERE id = ?1",
        )?;
        let mut rows = statement.query_map([id.to_string()], decode_event)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn latest_ns(&self, session_id: Uuid) -> Result<Option<u64>> {
        let connection = self.connection.lock().expect("timeline mutex poisoned");
        let value: Option<i64> = connection.query_row(
            "SELECT MAX(host_monotonic_ns) FROM experience_events WHERE session_id = ?1",
            [session_id.to_string()],
            |row| row.get(0),
        )?;
        value
            .map(u64::try_from)
            .transpose()
            .context("negative monotonic timestamp in timeline")
    }
}

impl EventSink for TimelineStore {
    fn record(&self, event: RawEvent) -> Result<()> {
        let host_ns = i64::try_from(event.host_monotonic_ns)
            .context("event monotonic timestamp exceeds SQLite range")?;
        let source_sequence = event
            .source_sequence
            .map(i64::try_from)
            .transpose()
            .context("source sequence exceeds SQLite range")?;
        let connection = self.connection.lock().expect("timeline mutex poisoned");
        connection.execute(
            "INSERT INTO experience_events
             (id, session_id, host_monotonic_ns, wall_clock_time, source, kind,
              repository_fingerprint, source_timestamp, source_sequence, payload,
              artifact_refs, provenance)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                event.id.to_string(),
                event.session_id.to_string(),
                host_ns,
                event.wall_clock_time,
                event.source,
                event.kind,
                event.repository_fingerprint,
                event
                    .source_timestamp
                    .map(|value| serde_json::to_string(&value))
                    .transpose()?,
                source_sequence,
                serde_json::to_string(&event.payload)?,
                serde_json::to_string(&event.artifact_refs)?,
                serde_json::to_string(&event.provenance)?,
            ],
        )?;
        Ok(())
    }
}

fn decode_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawEvent> {
    let id: String = row.get(0)?;
    let session_id: String = row.get(1)?;
    let host_ns: i64 = row.get(2)?;
    let source_timestamp: Option<String> = row.get(7)?;
    let source_sequence: Option<i64> = row.get(8)?;
    let payload: String = row.get(9)?;
    let artifact_refs: String = row.get(10)?;
    let provenance: String = row.get(11)?;
    Ok(RawEvent {
        id: Uuid::parse_str(&id).map_err(from_decode_error)?,
        session_id: Uuid::parse_str(&session_id).map_err(from_decode_error)?,
        host_monotonic_ns: u64::try_from(host_ns).map_err(from_decode_error)?,
        wall_clock_time: row.get(3)?,
        source: row.get(4)?,
        kind: row.get(5)?,
        repository_fingerprint: row.get(6)?,
        source_timestamp: source_timestamp
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(from_decode_error)?,
        source_sequence: source_sequence
            .map(u64::try_from)
            .transpose()
            .map_err(from_decode_error)?,
        payload: serde_json::from_str(&payload).map_err(from_decode_error)?,
        artifact_refs: serde_json::from_str(&artifact_refs).map_err(from_decode_error)?,
        provenance: serde_json::from_str::<Provenance>(&provenance).map_err(from_decode_error)?,
    })
}

fn from_decode_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn timeline_round_trips_complete_envelopes_in_monotonic_order() {
        let temp = tempfile::tempdir().unwrap();
        let store = TimelineStore::open(temp.path().join("timeline.sqlite3")).unwrap();
        let session = Uuid::new_v4();
        let mut later = RawEvent::observed_at(session, 20, "input", "pointer.up", json!({}));
        later.repository_fingerprint = Some("sha256:tree".into());
        later.source_timestamp = Some(json!(12.5));
        later.source_sequence = Some(7);
        later.artifact_refs.push("sha256:artifact".into());
        let earlier = RawEvent::observed_at(session, 10, "input", "pointer.down", json!({}));
        store.record(later.clone()).unwrap();
        store.record(earlier.clone()).unwrap();

        let events = store.all(session).unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.host_monotonic_ns)
                .collect::<Vec<_>>(),
            vec![10, 20]
        );
        assert_eq!(events[1], later);
        assert_eq!(store.event(earlier.id).unwrap(), Some(earlier));
    }
}
