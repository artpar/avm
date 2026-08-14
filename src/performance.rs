use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, ensure};
use serde::Serialize;
use serde_json::Value;

use crate::event::RawEvent;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageUsage {
    pub file_count: u64,
    pub byte_count: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencySummary {
    pub count: usize,
    pub minimum_ns: u64,
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub maximum_ns: u64,
    pub mean_ns: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessPhase {
    pub qemu_cpu_percent: f64,
    pub guest_vcpu_host_cpu_percent: f64,
    pub supervisor_cpu_percent: f64,
    pub qemu_rss_bytes_end: u64,
    pub supervisor_rss_bytes_end: u64,
    pub qemu_read_bytes: u64,
    pub qemu_write_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMeasurement {
    pub phase_duration_ms: u64,
    pub baseline: ProcessPhase,
    pub instrumented: ProcessPhase,
    pub qemu_cpu_percent_difference: f64,
    pub guest_vcpu_host_cpu_percent_difference: f64,
    pub supervisor_cpu_percent_difference: f64,
    pub event_count_growth: u64,
    pub event_bytes_growth: u64,
    pub event_bytes_per_second: f64,
    pub artifact_file_growth: u64,
    pub artifact_bytes_growth: u64,
    pub artifact_bytes_per_second: f64,
    pub note: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceReport {
    pub start_ns: u64,
    pub end_ns: u64,
    pub event_count: usize,
    pub serialized_event_bytes: u64,
    pub events_by_source: BTreeMap<String, usize>,
    pub events_by_kind: BTreeMap<String, usize>,
    pub artifact_storage: StorageUsage,
    pub display_processing_latency: Option<LatencySummary>,
    pub input_action_duration: Option<LatencySummary>,
    pub audio_callback_processing: Option<LatencySummary>,
    pub vlm_call_count: usize,
    pub audio_interpretation_call_count: usize,
    pub model_input_tokens: u64,
    pub model_output_tokens: u64,
    pub resource_measurement: Option<ResourceMeasurement>,
}

pub fn build_report(
    events: &[RawEvent],
    artifact_root: &Path,
    start_ns: u64,
    end_ns: u64,
    resource_measurement: Option<ResourceMeasurement>,
) -> Result<PerformanceReport> {
    ensure!(start_ns <= end_ns, "performance interval is invalid");
    let events = events
        .iter()
        .filter(|event| event.host_monotonic_ns >= start_ns && event.host_monotonic_ns <= end_ns)
        .collect::<Vec<_>>();
    let mut events_by_source = BTreeMap::new();
    let mut events_by_kind = BTreeMap::new();
    let mut display_latencies = Vec::new();
    let mut input_latencies = Vec::new();
    let mut audio_latencies = Vec::new();
    let mut model_input_tokens = 0;
    let mut model_output_tokens = 0;
    for event in &events {
        *events_by_source.entry(event.source.clone()).or_insert(0) += 1;
        *events_by_kind.entry(event.kind.clone()).or_insert(0) += 1;
        if matches!(event.kind.as_str(), "display.scanout" | "display.update") {
            if let Some(value) = event
                .payload
                .get("processingLatencyNs")
                .and_then(Value::as_u64)
            {
                display_latencies.push(value);
            }
        }
        if event.kind == "input.action.completed" {
            if let Some(value) = event
                .payload
                .get("actionDurationNs")
                .and_then(Value::as_u64)
            {
                input_latencies.push(value);
            }
        }
        if event.kind == "audio.raw.interval" {
            if let Some(value) = event
                .payload
                .get("maxCallbackProcessingNs")
                .and_then(Value::as_u64)
            {
                audio_latencies.push(value);
            }
        }
        if matches!(
            event.kind.as_str(),
            "perception.vlm.observation" | "perception.audio.interpretation"
        ) {
            model_input_tokens += recursive_token_sum(&event.payload, "inputTokens");
            model_output_tokens += recursive_token_sum(&event.payload, "outputTokens");
        }
    }
    let serialized_event_bytes = events.iter().try_fold(0_u64, |total, event| {
        let length = serde_json::to_vec(event)?.len() as u64 + 1;
        Ok::<_, serde_json::Error>(total.saturating_add(length))
    })?;
    Ok(PerformanceReport {
        start_ns,
        end_ns,
        event_count: events.len(),
        serialized_event_bytes,
        events_by_source,
        events_by_kind,
        artifact_storage: tree_usage(artifact_root)?,
        display_processing_latency: latency_summary(display_latencies),
        input_action_duration: latency_summary(input_latencies),
        audio_callback_processing: latency_summary(audio_latencies),
        vlm_call_count: events
            .iter()
            .filter(|event| event.kind == "perception.vlm.observation")
            .count(),
        audio_interpretation_call_count: events
            .iter()
            .filter(|event| event.kind == "perception.audio.interpretation")
            .count(),
        model_input_tokens,
        model_output_tokens,
        resource_measurement,
    })
}

fn latency_summary(mut values: Vec<u64>) -> Option<LatencySummary> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let sum = values
        .iter()
        .fold(0_u128, |total, value| total + *value as u128);
    Some(LatencySummary {
        count: values.len(),
        minimum_ns: values[0],
        p50_ns: percentile(&values, 50),
        p95_ns: percentile(&values, 95),
        maximum_ns: *values.last().expect("non-empty latency values"),
        mean_ns: (sum / values.len() as u128) as u64,
    })
}

fn percentile(values: &[u64], percentile: usize) -> u64 {
    let index = (percentile * values.len()).div_ceil(100).saturating_sub(1);
    values[index.min(values.len() - 1)]
}

fn recursive_token_sum(value: &Value, key: &str) -> u64 {
    match value {
        Value::Object(object) => object
            .iter()
            .map(|(name, value)| {
                if name == key {
                    value.as_u64().unwrap_or(0)
                } else {
                    recursive_token_sum(value, key)
                }
            })
            .sum(),
        Value::Array(values) => values
            .iter()
            .map(|value| recursive_token_sum(value, key))
            .sum(),
        _ => 0,
    }
}

pub fn tree_usage(root: &Path) -> Result<StorageUsage> {
    if !root.exists() {
        return Ok(StorageUsage::default());
    }
    let mut usage = StorageUsage::default();
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(&path)
            .with_context(|| format!("read storage directory {}", path.display()))?
        {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                usage.file_count += 1;
                usage.byte_count = usage.byte_count.saturating_add(metadata.len());
            }
        }
    }
    Ok(usage)
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug)]
pub struct ProcessSnapshot {
    ticks: u64,
    vcpu_ticks: u64,
    rss_bytes: u64,
    read_bytes: u64,
    write_bytes: u64,
}

#[cfg(target_os = "linux")]
impl ProcessSnapshot {
    pub fn capture(pid: u32) -> Result<Self> {
        let process = std::path::PathBuf::from(format!("/proc/{pid}"));
        let (ticks, rss_pages) = stat_values(&std::fs::read_to_string(process.join("stat"))?)?;
        let mut vcpu_ticks = 0_u64;
        for entry in std::fs::read_dir(process.join("task"))? {
            let task = entry?.path();
            let name = std::fs::read_to_string(task.join("comm")).unwrap_or_default();
            if name.starts_with("CPU ") {
                let (task_ticks, _) = stat_values(&std::fs::read_to_string(task.join("stat"))?)?;
                vcpu_ticks = vcpu_ticks.saturating_add(task_ticks);
            }
        }
        let io = std::fs::read_to_string(process.join("io"))?;
        Ok(Self {
            ticks,
            vcpu_ticks,
            rss_bytes: rss_pages.saturating_mul(page_size()),
            read_bytes: io_value(&io, "read_bytes:"),
            write_bytes: io_value(&io, "write_bytes:"),
        })
    }

    pub fn phase(
        &self,
        end: &Self,
        duration: std::time::Duration,
        supervisor: (&Self, &Self),
    ) -> ProcessPhase {
        let seconds = duration.as_secs_f64();
        ProcessPhase {
            qemu_cpu_percent: cpu_percent(self.ticks, end.ticks, seconds),
            guest_vcpu_host_cpu_percent: cpu_percent(self.vcpu_ticks, end.vcpu_ticks, seconds),
            supervisor_cpu_percent: cpu_percent(supervisor.0.ticks, supervisor.1.ticks, seconds),
            qemu_rss_bytes_end: end.rss_bytes,
            supervisor_rss_bytes_end: supervisor.1.rss_bytes,
            qemu_read_bytes: end.read_bytes.saturating_sub(self.read_bytes),
            qemu_write_bytes: end.write_bytes.saturating_sub(self.write_bytes),
        }
    }
}

#[cfg(target_os = "linux")]
fn stat_values(stat: &str) -> Result<(u64, u64)> {
    let close = stat
        .rfind(')')
        .context("process stat has no command terminator")?;
    let fields = stat[close + 1..].split_whitespace().collect::<Vec<_>>();
    ensure!(fields.len() > 21, "process stat is incomplete");
    let user = fields[11].parse::<u64>()?;
    let system = fields[12].parse::<u64>()?;
    let rss = fields[21].parse::<u64>()?;
    Ok((user.saturating_add(system), rss))
}

#[cfg(target_os = "linux")]
fn io_value(io: &str, name: &str) -> u64 {
    io.lines()
        .find_map(|line| line.strip_prefix(name)?.trim().parse().ok())
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn cpu_percent(start: u64, end: u64, seconds: f64) -> f64 {
    end.saturating_sub(start) as f64 / clock_ticks_per_second() as f64 / seconds * 100.0
}

#[cfg(target_os = "linux")]
fn clock_ticks_per_second() -> u64 {
    unsafe { libc::sysconf(libc::_SC_CLK_TCK) as u64 }
}

#[cfg(target_os = "linux")]
fn page_size() -> u64 {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Provenance;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn summarizes_volume_latency_storage_and_model_usage() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("artifact"), b"1234").unwrap();
        let session = Uuid::new_v4();
        let mut events = vec![
            RawEvent::observed_at(
                session,
                10,
                "display",
                "display.update",
                json!({"processingLatencyNs": 20}),
            ),
            RawEvent::observed_at(
                session,
                11,
                "input",
                "input.action.completed",
                json!({"actionDurationNs": 40}),
            ),
            RawEvent::observed_at(
                session,
                12,
                "audio",
                "audio.raw.interval",
                json!({"maxCallbackProcessingNs": 30}),
            ),
        ];
        let mut model = RawEvent::observed_at(
            session,
            13,
            "perception",
            "perception.vlm.observation",
            json!({"output":{"usage":{"inputTokens":100,"outputTokens":25}}}),
        );
        model.provenance = Provenance::ModelInterpreted;
        events.push(model);
        let report = build_report(&events, temp.path(), 10, 13, None).unwrap();
        assert_eq!(report.event_count, 4);
        assert_eq!(report.artifact_storage.byte_count, 4);
        assert_eq!(report.display_processing_latency.unwrap().p95_ns, 20);
        assert_eq!(report.model_input_tokens, 100);
        assert_eq!(report.model_output_tokens, 25);
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        assert_eq!(percentile(&[1, 2, 3, 4, 5], 50), 3);
        assert_eq!(percentile(&[1, 2, 3, 4, 5], 95), 5);
    }
}
