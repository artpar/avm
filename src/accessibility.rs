use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::event::{EventSink, RawEvent};

const MAX_SENSOR_LINE_BYTES: usize = 1_048_576;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccessibilitySensorEvent {
    kind: String,
    #[serde(default)]
    source_timestamp: Option<Value>,
    payload: Value,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessibilityObserverResult {
    pub event_count: u64,
    pub ready_observed: bool,
}

pub fn observe_accessibility(
    session_id: Uuid,
    socket_path: &Path,
    sink: Arc<dyn EventSink>,
    duration: Duration,
) -> Result<AccessibilityObserverResult> {
    ensure!(
        !duration.is_zero(),
        "accessibility observation duration must be positive"
    );
    let mut stream = UnixStream::connect(socket_path).with_context(|| {
        format!(
            "connect guest accessibility sensor at {}",
            socket_path.display()
        )
    })?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    stream
        .write_all(b"{\"command\":\"observe\",\"protocolVersion\":1}\n")
        .context("request guest accessibility snapshot")?;
    stream
        .flush()
        .context("flush guest accessibility request")?;
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    let mut reader = BufReader::new(stream);
    let deadline = Instant::now() + duration;
    let mut event_count = 0_u64;
    let mut ready_observed = false;
    while Instant::now() < deadline {
        let mut line = Vec::new();
        match reader
            .by_ref()
            .take((MAX_SENSOR_LINE_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
        {
            Ok(0) => break,
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error).context("read guest accessibility sensor"),
        }
        ensure!(
            line.len() <= MAX_SENSOR_LINE_BYTES,
            "guest accessibility sensor line exceeds one MiB"
        );
        let sensor: AccessibilitySensorEvent =
            serde_json::from_slice(&line).context("decode guest accessibility sensor JSON")?;
        ensure!(
            sensor.kind.starts_with("accessibility."),
            "guest accessibility event kind must start with accessibility."
        );
        ready_observed |= sensor.kind == "accessibility.sensor.ready";
        let mut event =
            RawEvent::observed(session_id, "accessibility", &sensor.kind, sensor.payload);
        event.source_timestamp = sensor.source_timestamp;
        sink.record(event)?;
        event_count += 1;
    }
    ensure!(
        ready_observed,
        "guest accessibility sensor closed or timed out before readiness"
    );
    Ok(AccessibilityObserverResult {
        event_count,
        ready_observed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::MemoryEventSink;
    use std::{io::Write, net::Shutdown, os::unix::net::UnixListener, thread};

    #[test]
    fn records_bounded_guest_events_without_granting_store_access() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("accessibility.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let writer = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&request).unwrap(),
                serde_json::json!({"command": "observe", "protocolVersion": 1})
            );
            writeln!(
                stream,
                "{}",
                serde_json::json!({
                    "kind": "accessibility.sensor.ready",
                    "sourceTimestamp": {"guestMonotonicNs": 10},
                    "payload": {"desktopCount": 1}
                })
            )
            .unwrap();
            writeln!(
                stream,
                "{}",
                serde_json::json!({
                    "kind": "accessibility.object.snapshot",
                    "payload": {"role": "button", "name": "Save"}
                })
            )
            .unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
        });
        let sink = Arc::new(MemoryEventSink::default());
        let result = observe_accessibility(
            Uuid::new_v4(),
            &socket,
            sink.clone(),
            Duration::from_secs(1),
        )
        .unwrap();
        writer.join().unwrap();
        assert!(result.ready_observed);
        assert_eq!(result.event_count, 2);
        let events = sink.events();
        assert!(events.iter().all(|event| event.source == "accessibility"));
        assert_eq!(events[1].payload["name"], "Save");
    }

    #[test]
    fn rejects_unscoped_guest_event_kinds() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("accessibility.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let writer = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&request).unwrap(),
                serde_json::json!({"command": "observe", "protocolVersion": 1})
            );
            writeln!(
                stream,
                "{}",
                serde_json::json!({"kind": "evidence.completed", "payload": {}})
            )
            .unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
        });
        let error = observe_accessibility(
            Uuid::new_v4(),
            &socket,
            Arc::new(MemoryEventSink::default()),
            Duration::from_secs(1),
        )
        .unwrap_err();
        writer.join().unwrap();
        assert!(error.to_string().contains("accessibility."));
    }
}
