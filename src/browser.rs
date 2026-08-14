use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    process::Stdio,
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};
use uuid::Uuid;

use crate::{
    event::{EventSink, Provenance, RawEvent},
    storage::ArtifactStore,
};

#[derive(Clone, Debug)]
pub struct BrowserObserverOptions {
    pub command: Vec<String>,
    pub endpoint: String,
    pub trace_path: PathBuf,
    pub sensor_artifacts_dir: PathBuf,
    pub duration_ms: u64,
}

impl BrowserObserverOptions {
    pub fn playwright(
        script: PathBuf,
        endpoint: String,
        trace_path: PathBuf,
        sensor_artifacts_dir: PathBuf,
        duration_ms: u64,
    ) -> Self {
        Self {
            command: vec!["node".into(), script.display().to_string()],
            endpoint,
            trace_path,
            sensor_artifacts_dir,
            duration_ms,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserObserverResult {
    pub event_count: u64,
    pub trace_artifact_ref: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotCorrelation {
    pub display_x: u32,
    pub display_y: u32,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub host_width: u32,
    pub host_height: u32,
    pub anchor_count: usize,
    pub exact_pixel_ratio: f64,
    pub mean_absolute_channel_error: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserFailureDiagnosis {
    pub code: String,
    pub summary: String,
    pub click_event_id: Uuid,
    pub supporting_event_ids: Vec<Uuid>,
    pub artifact_refs: Vec<String>,
    pub causal_certainty: Value,
}

/// Derive a narrow diagnosis from independently recorded input, display, and
/// browser observations. This deliberately recognizes only a complete pattern;
/// it does not turn temporal proximity alone into a general causality claim.
pub fn diagnose_double_submit_failure(
    events: &[RawEvent],
    click_event_id: Option<Uuid>,
) -> Result<BrowserFailureDiagnosis> {
    let mut ordered = events.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|event| (event.host_monotonic_ns, event.id));
    let click = match click_event_id {
        Some(id) => ordered
            .iter()
            .copied()
            .find(|event| event.id == id)
            .with_context(|| format!("click event {id} is not in the timeline"))?,
        None => ordered
            .iter()
            .rev()
            .copied()
            .find(|event| event.kind == "pointer.down")
            .context("timeline contains no pointer-down event")?,
    };
    ensure!(
        click.kind == "pointer.down",
        "selected event is not pointer-down"
    );
    let next_click_ns = ordered
        .iter()
        .copied()
        .find(|event| {
            event.kind == "pointer.down" && event.host_monotonic_ns > click.host_monotonic_ns
        })
        .map(|event| event.host_monotonic_ns)
        .unwrap_or(u64::MAX);
    let interval = ordered
        .into_iter()
        .filter(|event| {
            event.host_monotonic_ns >= click.host_monotonic_ns
                && event.host_monotonic_ns < next_click_ns
        })
        .collect::<Vec<_>>();
    let pointer_up = interval
        .iter()
        .copied()
        .find(|event| event.kind == "pointer.up")
        .context("click has no subsequent pointer-up")?;
    let display_during_click = interval
        .iter()
        .copied()
        .find(|event| {
            matches!(event.kind.as_str(), "display.scanout" | "display.update")
                && event.host_monotonic_ns <= pointer_up.host_monotonic_ns
        })
        .context("click has no observed display event while the pointer was held")?;

    let requests = interval
        .iter()
        .copied()
        .filter(|event| {
            event.kind == "browser.network.request"
                && event.payload.get("method").and_then(Value::as_str) == Some("POST")
        })
        .collect::<Vec<_>>();
    ensure!(
        requests.len() == 2,
        "expected exactly two POST requests after one click, observed {}",
        requests.len()
    );
    ensure!(
        requests[0].payload.get("method") == requests[1].payload.get("method")
            && requests[0].payload.get("url") == requests[1].payload.get("url")
            && requests[0].payload.get("postData") == requests[1].payload.get("postData"),
        "the two POST requests are not identical"
    );
    let request_url = requests[0]
        .payload
        .get("url")
        .and_then(Value::as_str)
        .context("POST request has no URL")?;
    let responses = interval
        .iter()
        .copied()
        .filter(|event| {
            event.kind == "browser.network.response"
                && event.payload.get("url").and_then(Value::as_str) == Some(request_url)
        })
        .collect::<Vec<_>>();
    ensure!(
        responses.len() == 2
            && responses
                .iter()
                .all(|event| event.payload.get("status").and_then(Value::as_u64) == Some(501)),
        "the duplicate POSTs were not both observed returning 501"
    );
    let console = interval
        .iter()
        .copied()
        .find(|event| {
            event.kind == "browser.console.message"
                && event
                    .payload
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.contains("Save failed") && text.contains("501"))
        })
        .context("no console observation ties the 501 responses to the Save failure")?;
    let mutation = interval
        .iter()
        .copied()
        .find(|event| {
            event.kind == "browser.dom.mutation"
                && event
                    .payload
                    .get("count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0
        })
        .context("no DOM mutation was observed after the click")?;
    let snapshot = interval
        .iter()
        .rev()
        .copied()
        .find(|event| {
            event.kind == "browser.page.snapshot"
                && event
                    .payload
                    .get("dom")
                    .is_some_and(|dom| dom.to_string().contains("Save failed"))
        })
        .context("no browser snapshot contains the visible Save failed state")?;
    let correlation = events.iter().find(|event| {
        event.kind == "browser.coordinate_correlation"
            && event
                .payload
                .get("browserSnapshotEventId")
                .and_then(Value::as_str)
                == Some(snapshot.id.to_string().as_str())
    });
    ensure!(
        correlation.is_some(),
        "the failure snapshot has no display-coordinate correlation"
    );

    let mut supporting_event_ids = vec![
        click.id,
        pointer_up.id,
        display_during_click.id,
        requests[0].id,
        requests[1].id,
        responses[0].id,
        responses[1].id,
        console.id,
        mutation.id,
        snapshot.id,
    ];
    supporting_event_ids.push(correlation.expect("checked above").id);
    let artifact_refs = supporting_event_ids
        .iter()
        .filter_map(|id| events.iter().find(|event| event.id == *id))
        .flat_map(|event| event.artifact_refs.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(BrowserFailureDiagnosis {
        code: "duplicate_submit_with_unsupported_post".into(),
        summary: format!(
            "one physical click was followed by two identical POST requests to {request_url}; both returned 501 and the correlated UI state reported Save failed"
        ),
        click_event_id: click.id,
        supporting_event_ids,
        artifact_refs,
        causal_certainty: json!({
            "singlePhysicalClick": "directly_observed",
            "duplicateClientDispatch": "strong_single_action_temporal_evidence",
            "serverRejectedPost": "directly_observed",
            "visibleFailure": "browser_snapshot_correlated_to_host_framebuffer",
            "limitation": "the event sequence supports the diagnosis but does not by itself identify a source-code line"
        }),
    })
}

pub fn correlate_viewport_png(
    host_png: &[u8],
    viewport_png: &[u8],
) -> Result<ScreenshotCorrelation> {
    let host = image::load_from_memory(host_png)
        .context("decode host framebuffer PNG")?
        .to_rgb8();
    let viewport = image::load_from_memory(viewport_png)
        .context("decode browser viewport PNG")?
        .to_rgb8();
    ensure!(
        viewport.width() <= host.width() && viewport.height() <= host.height(),
        "browser viewport is larger than host framebuffer"
    );

    let mut frequencies = HashMap::<[u8; 3], usize>::new();
    for pixel in viewport.pixels() {
        *frequencies.entry(pixel.0).or_default() += 1;
    }
    let background = frequencies
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(color, _)| color)
        .context("browser viewport is empty")?;
    let mut distinct = Vec::new();
    for y in 0..viewport.height() {
        for x in 0..viewport.width() {
            let color = viewport.get_pixel(x, y).0;
            if color != background {
                distinct.push((x, y, color));
            }
        }
    }
    if distinct.len() < 16 {
        for y in (0..viewport.height()).step_by((viewport.height() / 8).max(1) as usize) {
            for x in (0..viewport.width()).step_by((viewport.width() / 8).max(1) as usize) {
                distinct.push((x, y, viewport.get_pixel(x, y).0));
            }
        }
    }
    let stride = distinct.len().div_ceil(512).max(1);
    let anchors = distinct
        .into_iter()
        .step_by(stride)
        .take(512)
        .collect::<Vec<_>>();
    ensure!(
        !anchors.is_empty(),
        "could not select browser screenshot anchors"
    );

    let mut candidates = Vec::new();
    for display_y in 0..=host.height() - viewport.height() {
        for display_x in 0..=host.width() - viewport.width() {
            let error = anchors
                .iter()
                .map(|(x, y, expected)| {
                    color_error(host.get_pixel(display_x + x, display_y + y).0, *expected)
                })
                .sum::<u64>();
            candidates.push((error, display_x, display_y));
        }
    }
    candidates.sort_unstable();
    candidates.truncate(16);
    let (_, display_x, display_y) = candidates
        .into_iter()
        .min_by_key(|(_, display_x, display_y)| {
            sampled_region_error(&host, &viewport, *display_x, *display_y, 3)
        })
        .context("no coordinate-correlation candidate")?;

    let mut exact = 0_u64;
    let mut absolute_error = 0_u64;
    let pixel_count = u64::from(viewport.width()) * u64::from(viewport.height());
    for y in 0..viewport.height() {
        for x in 0..viewport.width() {
            let actual = host.get_pixel(display_x + x, display_y + y).0;
            let expected = viewport.get_pixel(x, y).0;
            if actual == expected {
                exact += 1;
            }
            absolute_error += color_error(actual, expected);
        }
    }
    Ok(ScreenshotCorrelation {
        display_x,
        display_y,
        viewport_width: viewport.width(),
        viewport_height: viewport.height(),
        host_width: host.width(),
        host_height: host.height(),
        anchor_count: anchors.len(),
        exact_pixel_ratio: exact as f64 / pixel_count as f64,
        mean_absolute_channel_error: absolute_error as f64 / (pixel_count * 3) as f64,
    })
}

fn sampled_region_error(
    host: &image::RgbImage,
    viewport: &image::RgbImage,
    display_x: u32,
    display_y: u32,
    step: usize,
) -> u64 {
    let mut error = 0_u64;
    for y in (0..viewport.height()).step_by(step) {
        for x in (0..viewport.width()).step_by(step) {
            error += color_error(
                host.get_pixel(display_x + x, display_y + y).0,
                viewport.get_pixel(x, y).0,
            );
        }
    }
    error
}

fn color_error(actual: [u8; 3], expected: [u8; 3]) -> u64 {
    actual
        .into_iter()
        .zip(expected)
        .map(|(actual, expected)| u64::from(actual.abs_diff(expected)))
        .sum()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserSensorEvent {
    source: String,
    kind: String,
    #[serde(default)]
    source_timestamp: Option<Value>,
    payload: Value,
    #[serde(default)]
    artifacts: Vec<BrowserSensorArtifact>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserSensorArtifact {
    path: PathBuf,
    role: String,
    mime_type: String,
}

pub async fn run_browser_observer(
    session_id: Uuid,
    sink: Arc<dyn EventSink>,
    artifacts: Arc<ArtifactStore>,
    options: BrowserObserverOptions,
) -> Result<BrowserObserverResult> {
    ensure!(
        !options.command.is_empty(),
        "browser observer command is empty"
    );
    if let Some(parent) = options.trace_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(&options.sensor_artifacts_dir)?;
    let sensor_artifacts_dir = options.sensor_artifacts_dir.canonicalize()?;
    ensure!(!options.trace_path.exists(), "browser trace already exists");
    let mut command = Command::new(&options.command[0]);
    command
        .args(&options.command[1..])
        .args(["--endpoint", &options.endpoint, "--trace"])
        .arg(&options.trace_path)
        .arg("--artifacts-dir")
        .arg(&sensor_artifacts_dir)
        .arg("--duration-ms")
        .arg(options.duration_ms.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .context("launch Playwright browser observer")?;
    let stdout = child
        .stdout
        .take()
        .context("browser observer has no stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("browser observer has no stderr")?;
    let stderr_task = {
        let sink = sink.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut captured = Vec::new();
            while let Some(line) = lines.next_line().await? {
                sink.record(RawEvent::observed(
                    session_id,
                    "browser",
                    "browser.observer.stderr",
                    json!({"line": line}),
                ))?;
                captured.push(line);
            }
            Ok::<Vec<String>, anyhow::Error>(captured)
        })
    };
    let mut event_count = 0_u64;
    let mut lines = BufReader::new(stdout).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let sensor: BrowserSensorEvent =
            serde_json::from_str(&line).context("decode browser observer JSONL")?;
        ensure!(!sensor.source.is_empty(), "browser event source is empty");
        ensure!(!sensor.kind.is_empty(), "browser event kind is empty");
        let mut event =
            RawEvent::observed(session_id, &sensor.source, &sensor.kind, sensor.payload);
        event.source_timestamp = sensor.source_timestamp;
        event.source_sequence = Some(event_count);
        for artifact in sensor.artifacts {
            let path = artifact.path.canonicalize()?;
            ensure!(
                path.starts_with(&sensor_artifacts_dir) && path.is_file(),
                "browser sensor artifact escaped its owned directory"
            );
            let artifact_ref = artifacts.put(&std::fs::read(&path)?)?;
            event.artifact_refs.push(artifact_ref);
            if let Some(payload) = event.payload.as_object_mut() {
                payload
                    .entry("sensorArtifacts")
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .context("sensorArtifacts payload is not an array")?
                    .push(json!({
                        "role": artifact.role,
                        "mimeType": artifact.mime_type,
                    }));
            }
            std::fs::remove_file(path)?;
        }
        sink.record(event)?;
        event_count += 1;
    }
    let status = child.wait().await?;
    let stderr_lines = stderr_task
        .await
        .context("join browser stderr recorder")??;
    ensure!(
        status.success(),
        "browser observer failed with {status}: {}",
        stderr_lines.join("\n")
    );
    ensure!(
        options.trace_path.is_file(),
        "browser observer produced no trace"
    );
    let trace_artifact_ref = artifacts.put(&std::fs::read(&options.trace_path)?)?;
    let mut completed = RawEvent::observed(
        session_id,
        "browser",
        "browser.trace.stored",
        json!({
            "tracePath": options.trace_path,
            "eventCount": event_count,
        }),
    );
    completed.provenance = Provenance::Observed;
    completed.source_sequence = Some(event_count);
    completed.artifact_refs.push(trace_artifact_ref.clone());
    sink.record(completed)?;
    Ok(BrowserObserverResult {
        event_count: event_count + 1,
        trace_artifact_ref,
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::event::MemoryEventSink;

    #[tokio::test]
    async fn records_sensor_jsonl_and_content_addresses_trace() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("fake-observer.sh");
        std::fs::write(
            &script,
            r##"#!/bin/sh
trace=""
artifacts=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--trace" ]; then trace=$2; shift 2
  elif [ "$1" = "--artifacts-dir" ]; then artifacts=$2; shift 2
  else shift; fi
done
printf '%s' 'viewport bytes' > "$artifacts/viewport.png"
printf '%s\n' "{\"source\":\"browser\",\"kind\":\"browser.navigation\",\"sourceTimestamp\":{\"clock\":1},\"payload\":{\"url\":\"https://example.invalid\"},\"artifacts\":[{\"path\":\"$artifacts/viewport.png\",\"role\":\"browser.viewport\",\"mimeType\":\"image/png\"}]}"
printf '%s' 'trace bytes' > "$trace"
"##,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let trace = temp.path().join("trace.zip");
        let sink = Arc::new(MemoryEventSink::default());
        let artifacts = Arc::new(ArtifactStore::new(temp.path().join("artifacts")).unwrap());
        let result = run_browser_observer(
            Uuid::new_v4(),
            sink.clone(),
            artifacts.clone(),
            BrowserObserverOptions {
                command: vec![script.display().to_string()],
                endpoint: "http://127.0.0.1:9222".into(),
                trace_path: trace,
                sensor_artifacts_dir: temp.path().join("sensor-artifacts"),
                duration_ms: 1,
            },
        )
        .await
        .unwrap();
        assert_eq!(result.event_count, 2);
        assert_eq!(
            artifacts.read(&result.trace_artifact_ref).unwrap(),
            b"trace bytes"
        );
        let events = sink.events();
        assert_eq!(events[0].kind, "browser.navigation");
        assert_eq!(events[0].source_timestamp, Some(json!({"clock": 1})));
        assert_eq!(events[0].artifact_refs.len(), 1);
        assert_eq!(events[1].artifact_refs, vec![result.trace_artifact_ref]);
    }

    #[tokio::test]
    async fn surfaces_observer_stderr_on_failure() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("failed-observer.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\necho 'CDP transport reset' >&2\nexit 7\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let sink = Arc::new(MemoryEventSink::default());
        let error = run_browser_observer(
            Uuid::new_v4(),
            sink.clone(),
            Arc::new(ArtifactStore::new(temp.path().join("artifacts")).unwrap()),
            BrowserObserverOptions {
                command: vec![script.display().to_string()],
                endpoint: "http://127.0.0.1:9223".into(),
                trace_path: temp.path().join("trace.zip"),
                sensor_artifacts_dir: temp.path().join("sensor-artifacts"),
                duration_ms: 1,
            },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("CDP transport reset"));
        assert_eq!(sink.events()[0].kind, "browser.observer.stderr");
    }

    #[test]
    fn recovers_exact_viewport_offset_inside_host_frame() {
        let mut host = image::RgbImage::from_pixel(30, 20, image::Rgb([12, 20, 30]));
        let mut viewport = image::RgbImage::from_pixel(9, 7, image::Rgb([245, 245, 245]));
        for index in 0..7 {
            viewport.put_pixel(index + 1, index, image::Rgb([index as u8 * 19, 40, 210]));
        }
        image::imageops::replace(&mut host, &viewport, 13, 8);
        let mut host_png = Vec::new();
        let mut viewport_png = Vec::new();
        host.write_to(
            &mut std::io::Cursor::new(&mut host_png),
            image::ImageFormat::Png,
        )
        .unwrap();
        viewport
            .write_to(
                &mut std::io::Cursor::new(&mut viewport_png),
                image::ImageFormat::Png,
            )
            .unwrap();
        let correlation = correlate_viewport_png(&host_png, &viewport_png).unwrap();
        assert_eq!((correlation.display_x, correlation.display_y), (13, 8));
        assert_eq!(correlation.exact_pixel_ratio, 1.0);
        assert_eq!(correlation.mean_absolute_channel_error, 0.0);
    }

    fn event(session: Uuid, at: u64, source: &str, kind: &str, payload: Value) -> RawEvent {
        RawEvent::observed_at(session, at, source, kind, payload)
    }

    fn double_submit_fixture() -> Vec<RawEvent> {
        let session = Uuid::new_v4();
        let down = event(
            session,
            10,
            "input",
            "pointer.down",
            json!({"button":"left"}),
        );
        let up = event(session, 30, "input", "pointer.up", json!({"button":"left"}));
        let display = event(session, 20, "display", "display.update", json!({}));
        let request_payload = json!({
            "method":"POST", "url":"http://fixture/api/document", "postData":"{\"x\":1}"
        });
        let first_request = event(
            session,
            40,
            "browser",
            "browser.network.request",
            request_payload.clone(),
        );
        let second_request = event(
            session,
            41,
            "browser",
            "browser.network.request",
            request_payload,
        );
        let first_response = event(
            session,
            50,
            "browser",
            "browser.network.response",
            json!({"url":"http://fixture/api/document", "status":501}),
        );
        let second_response = event(
            session,
            51,
            "browser",
            "browser.network.response",
            json!({"url":"http://fixture/api/document", "status":501}),
        );
        let console = event(
            session,
            60,
            "browser",
            "browser.console.message",
            json!({"type":"error", "text":"Save failed [501, 501]"}),
        );
        let mutation = event(
            session,
            61,
            "browser",
            "browser.dom.mutation",
            json!({"count":2, "records":[]}),
        );
        let mut snapshot = event(
            session,
            70,
            "browser",
            "browser.page.snapshot",
            json!({"dom":{"strings":["Save failed"]}}),
        );
        snapshot.artifact_refs.push("sha256:viewport".into());
        let mut correlation = event(
            session,
            80,
            "browser",
            "browser.coordinate_correlation",
            json!({"browserSnapshotEventId":snapshot.id}),
        );
        correlation.artifact_refs.push("sha256:frame".into());
        vec![
            down,
            display,
            up,
            first_request,
            second_request,
            first_response,
            second_response,
            console,
            mutation,
            snapshot,
            correlation,
        ]
    }

    #[test]
    fn derives_double_submit_diagnosis_only_from_complete_observed_pattern() {
        let events = double_submit_fixture();
        let diagnosis = diagnose_double_submit_failure(&events, None).unwrap();
        assert_eq!(diagnosis.code, "duplicate_submit_with_unsupported_post");
        assert_eq!(diagnosis.supporting_event_ids.len(), 11);
        assert_eq!(
            diagnosis.artifact_refs,
            vec!["sha256:frame", "sha256:viewport"]
        );
    }

    #[test]
    fn refuses_diagnosis_when_only_one_post_was_observed() {
        let mut events = double_submit_fixture();
        let second_request = events
            .iter()
            .filter(|event| event.kind == "browser.network.request")
            .nth(1)
            .unwrap()
            .id;
        events.retain(|event| event.id != second_request);
        let error = diagnose_double_submit_failure(&events, None).unwrap_err();
        assert!(error.to_string().contains("exactly two POST requests"));
    }
}
