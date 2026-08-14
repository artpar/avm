use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{event::RawEvent, storage::ArtifactStore};

const MAX_BATCH_BYTES: usize = 64 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 1024 * 1024;
const MAX_RECORDS: usize = 100_000;

#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum RuntimeRecord {
    ApplicationLog {
        timestamp_unix_nano: Option<String>,
        severity_text: Option<String>,
        body: Value,
        trace_id: Option<String>,
        span_id: Option<String>,
        #[serde(default)]
        attributes: Value,
    },
    Span {
        trace_id: String,
        span_id: String,
        parent_span_id: Option<String>,
        name: String,
        start_time_unix_nano: String,
        end_time_unix_nano: String,
        #[serde(default)]
        status: Value,
        #[serde(default)]
        attributes: Value,
    },
    ProcessStatus {
        process_id: u32,
        state: ProcessState,
        exit_code: Option<i32>,
        signal: Option<i32>,
        executable: Option<String>,
    },
    ProfileSample {
        timestamp_unix_nano: String,
        process_id: u32,
        thread_id: Option<u32>,
        stack: Vec<String>,
        weight: Option<u64>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProcessState {
    Running,
    Exited,
    Signaled,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeImportResult {
    pub event_count: usize,
    pub artifact_ref: String,
    pub kinds: Vec<String>,
}

pub fn import_runtime_jsonl(
    session_id: Uuid,
    bytes: &[u8],
    artifacts: &ArtifactStore,
) -> Result<(Vec<RawEvent>, RuntimeImportResult)> {
    ensure!(!bytes.is_empty(), "runtime telemetry batch is empty");
    ensure!(
        bytes.len() <= MAX_BATCH_BYTES,
        "runtime telemetry batch exceeds 64 MiB"
    );
    let mut records = Vec::new();
    for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        ensure!(
            line.len() <= MAX_LINE_BYTES,
            "runtime telemetry line {} exceeds one MiB",
            index + 1
        );
        ensure!(
            records.len() < MAX_RECORDS,
            "runtime telemetry batch exceeds {MAX_RECORDS} records"
        );
        let record = serde_json::from_slice::<RuntimeRecord>(line)
            .with_context(|| format!("decode runtime telemetry line {}", index + 1))?;
        validate_record(&record)
            .with_context(|| format!("runtime telemetry line {}", index + 1))?;
        records.push(record);
    }
    ensure!(
        !records.is_empty(),
        "runtime telemetry batch has no records"
    );

    let artifact_ref = artifacts.put(bytes)?;
    let mut kinds = Vec::new();
    let events = records
        .into_iter()
        .enumerate()
        .map(|(sequence, record)| {
            let (kind, source_timestamp, payload) = normalize(record);
            if !kinds.iter().any(|existing| existing == kind) {
                kinds.push(kind.to_owned());
            }
            let mut event = RawEvent::observed(session_id, "runtime", kind, payload);
            event.source_timestamp = source_timestamp;
            event.source_sequence = Some(sequence as u64);
            event.artifact_refs.push(artifact_ref.clone());
            event
        })
        .collect::<Vec<_>>();
    let result = RuntimeImportResult {
        event_count: events.len(),
        artifact_ref,
        kinds,
    };
    Ok((events, result))
}

fn validate_record(record: &RuntimeRecord) -> Result<()> {
    match record {
        RuntimeRecord::ApplicationLog {
            timestamp_unix_nano,
            trace_id,
            span_id,
            ..
        } => {
            if let Some(timestamp) = timestamp_unix_nano {
                parse_timestamp(timestamp)?;
            }
            match (trace_id, span_id) {
                (Some(trace_id), Some(span_id)) => {
                    validate_hex_id(trace_id, 32, "traceId")?;
                    validate_hex_id(span_id, 16, "spanId")?;
                }
                (None, None) => {}
                _ => anyhow::bail!("application log traceId and spanId must appear together"),
            }
        }
        RuntimeRecord::Span {
            trace_id,
            span_id,
            parent_span_id,
            name,
            start_time_unix_nano,
            end_time_unix_nano,
            ..
        } => {
            validate_hex_id(trace_id, 32, "traceId")?;
            validate_hex_id(span_id, 16, "spanId")?;
            if let Some(parent) = parent_span_id {
                validate_hex_id(parent, 16, "parentSpanId")?;
                ensure!(parent != span_id, "span cannot be its own parent");
            }
            ensure!(!name.trim().is_empty(), "span name is empty");
            let start = parse_timestamp(start_time_unix_nano)?;
            let end = parse_timestamp(end_time_unix_nano)?;
            ensure!(end >= start, "span ends before it starts");
        }
        RuntimeRecord::ProcessStatus {
            state,
            exit_code,
            signal,
            ..
        } => match state {
            ProcessState::Running => ensure!(
                exit_code.is_none() && signal.is_none(),
                "running process cannot have exitCode or signal"
            ),
            ProcessState::Exited => ensure!(
                exit_code.is_some() && signal.is_none(),
                "exited process requires exitCode and no signal"
            ),
            ProcessState::Signaled => ensure!(
                signal.is_some() && exit_code.is_none(),
                "signaled process requires signal and no exitCode"
            ),
        },
        RuntimeRecord::ProfileSample {
            timestamp_unix_nano,
            stack,
            ..
        } => {
            parse_timestamp(timestamp_unix_nano)?;
            ensure!(!stack.is_empty(), "profile sample stack is empty");
            ensure!(
                stack.len() <= 1024,
                "profile sample stack exceeds 1024 frames"
            );
        }
    }
    Ok(())
}

fn normalize(record: RuntimeRecord) -> (&'static str, Option<Value>, Value) {
    match record {
        RuntimeRecord::ApplicationLog {
            timestamp_unix_nano,
            severity_text,
            body,
            trace_id,
            span_id,
            attributes,
        } => (
            "runtime.application.log",
            timestamp_unix_nano
                .as_ref()
                .map(|timestamp| json!({"unixNano": timestamp})),
            json!({
                "severityText": severity_text,
                "body": body,
                "traceId": trace_id,
                "spanId": span_id,
                "attributes": attributes,
                "traceAssociation": trace_id.as_ref().map(|_| json!({
                    "basis": "instrumented_trace_context",
                    "certainty": "declared_by_instrumentation"
                }))
            }),
        ),
        RuntimeRecord::Span {
            trace_id,
            span_id,
            parent_span_id,
            name,
            start_time_unix_nano,
            end_time_unix_nano,
            status,
            attributes,
        } => (
            "runtime.span",
            Some(json!({
                "startUnixNano": start_time_unix_nano,
                "endUnixNano": end_time_unix_nano
            })),
            json!({
                "traceId": trace_id,
                "spanId": span_id,
                "parentSpanId": parent_span_id,
                "name": name,
                "status": status,
                "attributes": attributes,
                "parentRelation": parent_span_id.as_ref().map(|_| json!({
                    "basis": "instrumented_parent_span_id",
                    "certainty": "declared_by_instrumentation"
                }))
            }),
        ),
        RuntimeRecord::ProcessStatus {
            process_id,
            state,
            exit_code,
            signal,
            executable,
        } => (
            "runtime.process.status",
            None,
            json!({
                "processId": process_id,
                "state": state,
                "exitCode": exit_code,
                "signal": signal,
                "executable": executable
            }),
        ),
        RuntimeRecord::ProfileSample {
            timestamp_unix_nano,
            process_id,
            thread_id,
            stack,
            weight,
        } => (
            "runtime.profile.sample",
            Some(json!({"unixNano": timestamp_unix_nano})),
            json!({
                "processId": process_id,
                "threadId": thread_id,
                "stack": stack,
                "weight": weight.unwrap_or(1)
            }),
        ),
    }
}

fn parse_timestamp(value: &str) -> Result<u128> {
    value
        .parse::<u128>()
        .with_context(|| format!("invalid Unix nanosecond timestamp {value:?}"))
}

fn validate_hex_id(value: &str, length: usize, name: &str) -> Result<()> {
    ensure!(
        value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{name} must be exactly {length} hexadecimal characters"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_bounded_runtime_records_with_explicit_causal_basis() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(temp.path()).unwrap();
        let input = concat!(
            "{\"kind\":\"span\",\"traceId\":\"0123456789abcdef0123456789abcdef\",",
            "\"spanId\":\"0123456789abcdef\",\"parentSpanId\":\"fedcba9876543210\",",
            "\"name\":\"POST /cards\",\"startTimeUnixNano\":\"100\",",
            "\"endTimeUnixNano\":\"200\"}\n",
            "{\"kind\":\"application_log\",\"timestampUnixNano\":\"150\",",
            "\"body\":\"saved\",\"traceId\":\"0123456789abcdef0123456789abcdef\",",
            "\"spanId\":\"0123456789abcdef\"}\n"
        );
        let (events, result) =
            import_runtime_jsonl(Uuid::new_v4(), input.as_bytes(), &artifacts).unwrap();
        assert_eq!(result.event_count, 2);
        assert_eq!(events[0].kind, "runtime.span");
        assert_eq!(
            events[0].payload["parentRelation"]["certainty"],
            "declared_by_instrumentation"
        );
        assert_eq!(events[1].source_sequence, Some(1));
        assert_eq!(
            artifacts.read(&result.artifact_ref).unwrap(),
            input.as_bytes()
        );
    }

    #[test]
    fn validates_the_entire_batch_before_storing_it() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(temp.path()).unwrap();
        let input = concat!(
            "{\"kind\":\"process_status\",\"processId\":7,\"state\":\"running\"}\n",
            "{\"kind\":\"span\",\"traceId\":\"bad\",\"spanId\":\"bad\",",
            "\"name\":\"broken\",\"startTimeUnixNano\":\"2\",",
            "\"endTimeUnixNano\":\"1\"}\n"
        );
        let error = import_runtime_jsonl(Uuid::new_v4(), input.as_bytes(), &artifacts).unwrap_err();
        assert!(error.to_string().contains("line 2"));
        assert_eq!(
            std::fs::read_dir(temp.path().join("sha256"))
                .unwrap()
                .count(),
            0
        );
    }
}
