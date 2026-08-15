use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    net::SocketAddr,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path as RoutePath, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    event::RawEvent, experience::ExperienceStore, storage::ArtifactStore, timeline::TimelineStore,
    vm::RunConfig,
};

const INDEX: &str = include_str!("../webui/index.html");
const STYLES: &str = include_str!("../webui/styles.css");
const APP: &str = include_str!("../webui/app.js");
const MAX_EVENTS: usize = 20_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectorEndpoint {
    pub url: String,
    pub address: String,
    pub pid: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct InspectorProcess {
    pid: u32,
    instance_token: Uuid,
}

#[derive(Clone)]
struct WebState {
    run: RunConfig,
    root: std::path::PathBuf,
    timeline: Arc<TimelineStore>,
    artifacts: Arc<ArtifactStore>,
}

#[derive(Debug)]
struct WebError(anyhow::Error);

impl<E> From<E> for WebError
where
    E: Into<anyhow::Error>,
{
    fn from(error: E) -> Self {
        Self(error.into())
    }
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let request_id = Uuid::new_v4();
        let detail = format!("{:#}", self.0);
        eprintln!(
            "{}",
            serde_json::json!({
                "level": "error",
                "component": "webui",
                "requestId": request_id,
                "code": "inspector_request_failed",
                "detail": detail,
            })
        );
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "code": "inspector_request_failed",
                "message": "The requested evidence could not be read or verified.",
                "detail": detail,
                "requestId": request_id,
                "recoverable": true,
            })),
        )
            .into_response()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RunSummary {
    run_id: Uuid,
    width: u32,
    height: u32,
    event_count: usize,
    artifact_count: usize,
    source_counts: BTreeMap<String, usize>,
    source_coverage: BTreeMap<String, SourceCoverage>,
    provenance_counts: BTreeMap<String, usize>,
    start_ns: Option<u64>,
    end_ns: Option<u64>,
    started_at: Option<String>,
    ended_at: Option<String>,
    repository_fingerprints: Vec<String>,
    read_only: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceCoverage {
    start_ns: u64,
    end_ns: u64,
    count: usize,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsQuery {
    start_ns: Option<u64>,
    end_ns: Option<u64>,
    source: Option<String>,
    provenance: Option<String>,
    query: Option<String>,
    limit: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventsResponse {
    events: Vec<RawEvent>,
    relations: Vec<EventRelation>,
    total_before_limit: usize,
    truncated: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
struct EventRelation {
    from_event_id: Uuid,
    to_event_id: Uuid,
    basis: &'static str,
    label: String,
}

#[derive(Deserialize)]
struct OverviewQuery {
    buckets: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Overview {
    start_ns: Option<u64>,
    end_ns: Option<u64>,
    bucket_width_ns: u64,
    buckets: Vec<OverviewBucket>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OverviewBucket {
    start_ns: u64,
    end_ns: u64,
    count: usize,
    source_counts: BTreeMap<String, usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrameAtQuery {
    time_ns: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameAvailability {
    available: bool,
    state: &'static str,
    message: String,
    selected_ns: u64,
    frame_ns: Option<u64>,
    frame_event_id: Option<Uuid>,
    capture_start_ns: Option<u64>,
    capture_end_ns: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DisplayStateAtTime {
    NotCollected,
    BeforeCapture,
    Available,
    Disabled,
}

#[derive(Clone, Copy, Debug)]
struct DisplayContext<'a> {
    state: DisplayStateAtTime,
    capture_start_ns: Option<u64>,
    capture_end_ns: Option<u64>,
    frame_event: Option<&'a RawEvent>,
}

pub async fn start_inspector(config: &RunConfig) -> Result<InspectorEndpoint> {
    stop_inspector(config)?;
    let instance_token = Uuid::new_v4();
    let executable = std::env::current_exe().context("resolve AVM executable")?;
    let log_path = config.state_dir.join("webui.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let mut child = Command::new(executable)
        .arg("web-serve")
        .arg("--run")
        .arg(config.paths().config)
        .arg("--instance-token")
        .arg(instance_token.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()
        .context("start AVM WebUI")?;
    let process = InspectorProcess {
        pid: child.id(),
        instance_token,
    };
    if let Err(error) = write_process_record(&config.state_dir, &process) {
        cleanup_failed_start(&mut child, &config.state_dir)?;
        return Err(error.context("record AVM WebUI process identity"));
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        if let Some(endpoint) = inspector_endpoint(config) {
            if endpoint.pid == child.id() {
                return Ok(endpoint);
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                cleanup_inspector_files(&config.state_dir);
                anyhow::bail!("AVM WebUI exited during startup with {status}");
            }
            Ok(None) => {}
            Err(error) => {
                cleanup_failed_start(&mut child, &config.state_dir)?;
                return Err(error).context("poll AVM WebUI startup");
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    cleanup_failed_start(&mut child, &config.state_dir)?;
    anyhow::bail!(
        "AVM WebUI did not become ready; inspect {}",
        log_path.display()
    )
}

pub fn inspector_endpoint(config: &RunConfig) -> Option<InspectorEndpoint> {
    let process = read_process_record(&config.state_dir)?;
    let endpoint: InspectorEndpoint =
        serde_json::from_slice(&std::fs::read(config.state_dir.join("webui.json")).ok()?).ok()?;
    (endpoint.pid == process.pid && process_matches(&process)).then_some(endpoint)
}

pub fn stop_inspector(config: &RunConfig) -> Result<()> {
    stop_recorded_inspector(&config.state_dir);
    Ok(())
}

fn stop_recorded_inspector(state_dir: &Path) {
    if let Some(process) = read_process_record(state_dir) {
        if process_matches(&process) {
            // SAFETY: the non-reusable token in this process's command line matches
            // the supervisor-owned record, so a recycled PID cannot reach this call.
            unsafe { libc::kill(process.pid as i32, libc::SIGTERM) };
        }
    }
    cleanup_inspector_files(state_dir);
}

pub async fn serve_inspector(run: &Path, _instance_token: Uuid) -> Result<()> {
    let run_path = if run.is_dir() {
        run.join("run.json")
    } else {
        run.to_owned()
    };
    let config = RunConfig::load(&run_path)?;
    let root = run_path
        .parent()
        .context("run configuration has no parent directory")?
        .to_owned();
    let timeline = root.join("timeline.sqlite3");
    let artifacts = root.join("artifacts");
    ensure!(timeline.is_file(), "run has no timeline database");
    let state = WebState {
        run: config,
        root: root.clone(),
        timeline: Arc::new(TimelineStore::open_read_only(timeline)?),
        artifacts: Arc::new(ArtifactStore::open_read_only(artifacts)?),
    };
    let address: SocketAddr = "127.0.0.1:0".parse().expect("static loopback address");
    let state_dir = root;
    let app = Router::new()
        .route("/", get(index))
        .route("/styles.css", get(styles))
        .route("/app.js", get(script))
        .route("/api/run", get(run_summary))
        .route("/api/events", get(events))
        .route("/api/overview", get(overview))
        .route("/api/artifacts/{digest}", get(artifact))
        .route("/api/frames/{event_id}", get(frame))
        .route("/api/frame-status", get(frame_status))
        .route("/api/frame-at", get(frame_at))
        .fallback(get(index))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(address).await?;
    let address = listener.local_addr()?;
    let endpoint = InspectorEndpoint {
        url: format!("http://{address}"),
        address: address.to_string(),
        pid: std::process::id(),
    };
    let endpoint_path = state_dir.join("webui.json");
    let temporary_path = state_dir.join("webui.json.tmp");
    std::fs::write(&temporary_path, serde_json::to_vec_pretty(&endpoint)?)?;
    std::fs::rename(temporary_path, endpoint_path)?;
    println!("AVM read-only inspector: {}", endpoint.url);
    axum::serve(listener, app)
        .await
        .context("serve AVM WebUI")?;
    Ok(())
}

fn process_record_path(state_dir: &Path) -> std::path::PathBuf {
    state_dir.join("webui.pid")
}

fn write_process_record(state_dir: &Path, process: &InspectorProcess) -> Result<()> {
    let path = process_record_path(state_dir);
    let temporary = state_dir.join("webui.pid.tmp");
    std::fs::write(&temporary, serde_json::to_vec(process)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

fn read_process_record(state_dir: &Path) -> Option<InspectorProcess> {
    serde_json::from_slice(&std::fs::read(process_record_path(state_dir)).ok()?).ok()
}

fn process_matches(process: &InspectorProcess) -> bool {
    let token = process.instance_token.to_string();
    #[cfg(target_os = "linux")]
    {
        let Ok(command_line) = std::fs::read(format!("/proc/{}/cmdline", process.pid)) else {
            return false;
        };
        command_line
            .split(|byte| *byte == 0)
            .any(|argument| argument == token.as_bytes())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let Ok(output) = Command::new("ps")
            .args(["-p", &process.pid.to_string(), "-o", "command="])
            .output()
        else {
            return false;
        };
        output.status.success()
            && String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .any(|argument| argument == token)
    }
}

fn cleanup_failed_start(child: &mut std::process::Child, state_dir: &Path) -> Result<()> {
    if child.try_wait()?.is_none() {
        child
            .kill()
            .context("terminate AVM WebUI after failed startup")?;
        child
            .wait()
            .context("reap AVM WebUI after failed startup")?;
    }
    cleanup_inspector_files(state_dir);
    Ok(())
}

fn cleanup_inspector_files(state_dir: &Path) {
    let _ = std::fs::remove_file(process_record_path(state_dir));
    let _ = std::fs::remove_file(state_dir.join("webui.pid.tmp"));
    let _ = std::fs::remove_file(state_dir.join("webui.json"));
    let _ = std::fs::remove_file(state_dir.join("webui.json.tmp"));
}

async fn index() -> Response {
    static_response(INDEX, "text/html; charset=utf-8")
}

async fn styles() -> Response {
    static_response(STYLES, "text/css; charset=utf-8")
}

async fn script() -> Response {
    static_response(APP, "text/javascript; charset=utf-8")
}

fn static_response(content: &'static str, content_type: &'static str) -> Response {
    let mut response = Response::new(Body::from(content));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn run_summary(State(state): State<WebState>) -> Result<Json<RunSummary>, WebError> {
    let events = state.timeline.all(state.run.id)?;
    let mut source_counts = BTreeMap::new();
    let mut source_coverage = BTreeMap::<String, SourceCoverage>::new();
    let mut provenance_counts = BTreeMap::new();
    let mut artifacts = BTreeSet::new();
    let mut fingerprints = BTreeSet::new();
    for event in &events {
        *source_counts.entry(event.source.clone()).or_insert(0) += 1;
        source_coverage
            .entry(event.source.clone())
            .and_modify(|coverage| {
                coverage.start_ns = coverage.start_ns.min(event.host_monotonic_ns);
                coverage.end_ns = coverage.end_ns.max(event.host_monotonic_ns);
                coverage.count += 1;
            })
            .or_insert(SourceCoverage {
                start_ns: event.host_monotonic_ns,
                end_ns: event.host_monotonic_ns,
                count: 1,
            });
        *provenance_counts
            .entry(provenance_name(event).to_owned())
            .or_insert(0) += 1;
        artifacts.extend(event.artifact_refs.iter().cloned());
        if let Some(fingerprint) = &event.repository_fingerprint {
            fingerprints.insert(fingerprint.clone());
        }
    }
    Ok(Json(RunSummary {
        run_id: state.run.id,
        width: state.run.width,
        height: state.run.height,
        event_count: events.len(),
        artifact_count: artifacts.len(),
        source_counts,
        source_coverage,
        provenance_counts,
        start_ns: events.first().map(|event| event.host_monotonic_ns),
        end_ns: events.last().map(|event| event.host_monotonic_ns),
        started_at: events.first().map(|event| event.wall_clock_time.clone()),
        ended_at: events.last().map(|event| event.wall_clock_time.clone()),
        repository_fingerprints: fingerprints.into_iter().collect(),
        read_only: true,
    }))
}

async fn events(
    State(state): State<WebState>,
    Query(query): Query<EventsQuery>,
) -> Result<Json<EventsResponse>, WebError> {
    let mut events = state
        .timeline
        .range(state.run.id, query.start_ns, query.end_ns)?;
    if let Some(sources) = query.source {
        let sources = sources.split(',').collect::<BTreeSet<_>>();
        events.retain(|event| sources.contains(event.source.as_str()));
    }
    if let Some(provenance) = query.provenance {
        let values = provenance.split(',').collect::<BTreeSet<_>>();
        events.retain(|event| values.contains(provenance_name(event)));
    }
    if let Some(needle) = query.query.filter(|value| !value.trim().is_empty()) {
        let needle = needle.to_lowercase();
        events.retain(|event| event_search_text(event).contains(&needle));
    }
    let total_before_limit = events.len();
    let limit = query.limit.unwrap_or(5_000).clamp(1, MAX_EVENTS);
    events.truncate(limit);
    let relations = event_relations(&events);
    Ok(Json(EventsResponse {
        truncated: total_before_limit > events.len(),
        total_before_limit,
        events,
        relations,
    }))
}

async fn overview(
    State(state): State<WebState>,
    Query(query): Query<OverviewQuery>,
) -> Result<Json<Overview>, WebError> {
    let events = state.timeline.all(state.run.id)?;
    let bucket_count = query.buckets.unwrap_or(240).clamp(16, 1_000);
    let Some(first) = events.first() else {
        return Ok(Json(Overview {
            start_ns: None,
            end_ns: None,
            bucket_width_ns: 0,
            buckets: Vec::new(),
        }));
    };
    let start = first.host_monotonic_ns;
    let end = events.last().map_or(start, |event| event.host_monotonic_ns);
    let width = end
        .saturating_sub(start)
        .max(1)
        .div_ceil(bucket_count as u64);
    let mut buckets = (0..bucket_count)
        .map(|index| OverviewBucket {
            start_ns: start.saturating_add(width.saturating_mul(index as u64)),
            end_ns: start.saturating_add(width.saturating_mul(index as u64 + 1)),
            count: 0,
            source_counts: BTreeMap::new(),
        })
        .collect::<Vec<_>>();
    for event in &events {
        let index = ((event.host_monotonic_ns.saturating_sub(start) / width) as usize)
            .min(bucket_count - 1);
        buckets[index].count += 1;
        *buckets[index]
            .source_counts
            .entry(event.source.clone())
            .or_insert(0) += 1;
    }
    Ok(Json(Overview {
        start_ns: Some(start),
        end_ns: Some(end),
        bucket_width_ns: width,
        buckets,
    }))
}

async fn artifact(
    State(state): State<WebState>,
    RoutePath(digest): RoutePath<String>,
) -> Result<Response, WebError> {
    let reference = format!("sha256:{digest}");
    let bytes = state.artifacts.read(&reference)?;
    let mime = sniff_mime(&bytes);
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    Ok(response)
}

async fn frame(
    State(state): State<WebState>,
    RoutePath(event_id): RoutePath<Uuid>,
) -> Result<Response, WebError> {
    let event = state
        .timeline
        .event(event_id)?
        .context("frame anchor event does not exist")?;
    if event.session_id != state.run.id {
        return Err(WebError(anyhow::anyhow!("event belongs to another run")));
    }
    let experience = ExperienceStore::open_read_only(
        state.run.id,
        state.root.join("timeline.sqlite3"),
        state.root.join("artifacts"),
    )?;
    let frame = experience.frame_read_only(event.host_monotonic_ns)?;
    let mut response = Response::new(Body::from(frame.png));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    response.headers_mut().insert(
        "x-avm-frame-sha256",
        HeaderValue::from_str(&frame.frame_sha256)?,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn frame_status(
    State(state): State<WebState>,
    Query(query): Query<FrameAtQuery>,
) -> Result<Json<FrameAvailability>, WebError> {
    let events = state.timeline.all(state.run.id)?;
    let context = display_context(&events, query.time_ns);

    let Some(frame_event) = context.frame_event else {
        let (state_name, message) = match context.state {
            DisplayStateAtTime::NotCollected => (
                "not_collected",
                "No reconstructable display evidence was collected for this run.",
            ),
            DisplayStateAtTime::BeforeCapture => (
                "before_capture",
                "No display evidence exists at this time; display collection begins later.",
            ),
            DisplayStateAtTime::Disabled => (
                "display_disabled",
                "Display capture is disabled at this time; no framebuffer state persists.",
            ),
            DisplayStateAtTime::Available => unreachable!("available display state has a frame"),
        };
        return Ok(Json(FrameAvailability {
            available: false,
            state: state_name,
            message: message.into(),
            selected_ns: query.time_ns,
            frame_ns: None,
            frame_event_id: None,
            capture_start_ns: context.capture_start_ns,
            capture_end_ns: context.capture_end_ns,
        }));
    };

    let experience = ExperienceStore::open_read_only(
        state.run.id,
        state.root.join("timeline.sqlite3"),
        state.root.join("artifacts"),
    )?;
    match experience.frame_read_only(query.time_ns) {
        Ok(_) => Ok(Json(FrameAvailability {
            available: true,
            state: "available",
            message: "Display state reconstructed at the selected time.".into(),
            selected_ns: query.time_ns,
            frame_ns: Some(frame_event.host_monotonic_ns),
            frame_event_id: Some(frame_event.id),
            capture_start_ns: context.capture_start_ns,
            capture_end_ns: context.capture_end_ns,
        })),
        Err(error) => Ok(Json(FrameAvailability {
            available: false,
            state: "reconstruction_failed",
            message: format!("Display evidence exists, but reconstruction failed: {error:#}"),
            selected_ns: query.time_ns,
            frame_ns: Some(frame_event.host_monotonic_ns),
            frame_event_id: Some(frame_event.id),
            capture_start_ns: context.capture_start_ns,
            capture_end_ns: context.capture_end_ns,
        })),
    }
}

fn display_context(events: &[RawEvent], time_ns: u64) -> DisplayContext<'_> {
    let mut ordered = events
        .iter()
        .filter(|event| event.source == "display")
        .collect::<Vec<_>>();
    ordered.sort_by_key(|event| (event.host_monotonic_ns, event.id));

    let mut capture_start_ns = None;
    let mut capture_end_ns = None;
    let mut reconstructable = false;
    let mut frame_event = None;
    let mut disabled_at_selection = false;

    for event in ordered {
        match event.kind.as_str() {
            "display.scanout" => {
                capture_start_ns.get_or_insert(event.host_monotonic_ns);
                capture_end_ns = Some(event.host_monotonic_ns);
                reconstructable = true;
                if event.host_monotonic_ns <= time_ns {
                    frame_event = Some(event);
                    disabled_at_selection = false;
                }
            }
            "display.update" if reconstructable => {
                capture_end_ns = Some(event.host_monotonic_ns);
                if event.host_monotonic_ns <= time_ns {
                    frame_event = Some(event);
                }
            }
            "display.disable" => {
                if reconstructable {
                    capture_end_ns = Some(event.host_monotonic_ns);
                    reconstructable = false;
                    if event.host_monotonic_ns <= time_ns {
                        frame_event = None;
                        disabled_at_selection = true;
                    }
                }
            }
            "display.update_rejected"
                if reconstructable
                    && event
                        .payload
                        .get("recovery")
                        .and_then(|value| value.as_str())
                        == Some("awaiting_full_scanout") =>
            {
                capture_end_ns = Some(event.host_monotonic_ns);
                reconstructable = false;
                if event.host_monotonic_ns <= time_ns {
                    frame_event = None;
                    disabled_at_selection = true;
                }
            }
            _ => {}
        }
    }

    let state = if frame_event.is_some() {
        DisplayStateAtTime::Available
    } else if capture_start_ns.is_none() {
        DisplayStateAtTime::NotCollected
    } else if time_ns < capture_start_ns.expect("capture start exists") {
        DisplayStateAtTime::BeforeCapture
    } else if disabled_at_selection {
        DisplayStateAtTime::Disabled
    } else {
        DisplayStateAtTime::BeforeCapture
    };
    DisplayContext {
        state,
        capture_start_ns,
        capture_end_ns,
        frame_event,
    }
}

async fn frame_at(
    State(state): State<WebState>,
    Query(query): Query<FrameAtQuery>,
) -> Result<Response, WebError> {
    let experience = ExperienceStore::open_read_only(
        state.run.id,
        state.root.join("timeline.sqlite3"),
        state.root.join("artifacts"),
    )?;
    let frame = experience.frame_read_only(query.time_ns)?;
    let mut response = Response::new(Body::from(frame.png));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    response.headers_mut().insert(
        "x-avm-frame-sha256",
        HeaderValue::from_str(&frame.frame_sha256)?,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

fn event_relations(events: &[RawEvent]) -> Vec<EventRelation> {
    let ids = events.iter().map(|event| event.id).collect::<BTreeSet<_>>();
    let mut relations = BTreeSet::new();
    let mut artifact_owner: HashMap<&str, Uuid> = HashMap::new();
    for event in events {
        let mut references = BTreeSet::new();
        collect_uuids(&event.payload, &mut references);
        if let Some(timestamp) = &event.source_timestamp {
            collect_uuids(timestamp, &mut references);
        }
        for reference in references.intersection(&ids) {
            if reference != &event.id {
                relations.insert(EventRelation {
                    from_event_id: *reference,
                    to_event_id: event.id,
                    basis: "recorded_reference",
                    label: "recorded event reference".into(),
                });
            }
        }
        for artifact in &event.artifact_refs {
            if let Some(owner) = artifact_owner.insert(artifact, event.id) {
                if owner != event.id {
                    relations.insert(EventRelation {
                        from_event_id: owner,
                        to_event_id: event.id,
                        basis: "shared_artifact",
                        label: artifact.clone(),
                    });
                }
            }
        }
    }
    relations.into_iter().collect()
}

fn collect_uuids(value: &Value, found: &mut BTreeSet<Uuid>) {
    match value {
        Value::String(text) => {
            if let Ok(id) = Uuid::parse_str(text) {
                found.insert(id);
            }
        }
        Value::Array(values) => values.iter().for_each(|value| collect_uuids(value, found)),
        Value::Object(values) => values
            .values()
            .for_each(|value| collect_uuids(value, found)),
        _ => {}
    }
}

fn event_search_text(event: &RawEvent) -> String {
    format!(
        "{} {} {} {} {}",
        event.id,
        event.source,
        event.kind,
        event.repository_fingerprint.as_deref().unwrap_or_default(),
        event.payload
    )
    .to_lowercase()
}

fn provenance_name(event: &RawEvent) -> &'static str {
    use crate::event::Provenance;
    match event.provenance {
        Provenance::Observed => "observed",
        Provenance::Derived => "derived",
        Provenance::ModelInterpreted => "model_interpreted",
        Provenance::AgentClaim => "agent_claim",
    }
}

fn sniff_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"PK\x03\x04") {
        "application/zip"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        "audio/wav"
    } else if std::str::from_utf8(bytes).is_ok() {
        "text/plain; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::RawEvent;
    use serde_json::json;

    #[test]
    fn relations_only_claim_recorded_bases() {
        let session = Uuid::new_v4();
        let first = RawEvent::observed_at(session, 1, "input", "pointer.up", json!({}));
        let mut second = RawEvent::observed_at(
            session,
            2,
            "browser",
            "browser.page.snapshot",
            json!({"inputEventId": first.id}),
        );
        first_artifact(
            &mut second,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let mut third = RawEvent::observed_at(session, 3, "display", "display.update", json!({}));
        first_artifact(
            &mut third,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        let relations = event_relations(&[first.clone(), second.clone(), third.clone()]);
        assert!(relations.iter().any(|edge| {
            edge.from_event_id == first.id
                && edge.to_event_id == second.id
                && edge.basis == "recorded_reference"
        }));
        assert!(relations.iter().any(|edge| {
            edge.from_event_id == second.id
                && edge.to_event_id == third.id
                && edge.basis == "shared_artifact"
        }));
    }

    fn first_artifact(event: &mut RawEvent, reference: &str) {
        event.artifact_refs.push(reference.into());
    }

    #[test]
    fn mime_detection_is_conservative() {
        assert_eq!(sniff_mime(b"\x89PNG\r\n\x1a\nrest"), "image/png");
        assert_eq!(sniff_mime(b"plain text"), "text/plain; charset=utf-8");
        assert_eq!(sniff_mime(&[0, 159, 146, 150]), "application/octet-stream");
    }

    #[test]
    fn display_context_distinguishes_absence_before_capture_and_available_state() {
        let session = Uuid::new_v4();
        let unrelated = RawEvent::observed_at(session, 5, "input", "key.down", json!({}));
        let rejected = RawEvent::observed_at(
            session,
            10,
            "display",
            "display.scanout_dmabuf_unsupported",
            json!({}),
        );
        let first = RawEvent::observed_at(session, 20, "display", "display.scanout", json!({}));
        let update = RawEvent::observed_at(session, 30, "display", "display.update", json!({}));
        let disabled = RawEvent::observed_at(session, 40, "display", "display.disable", json!({}));
        let second = RawEvent::observed_at(session, 60, "display", "display.scanout", json!({}));
        let events = vec![
            unrelated,
            rejected,
            first.clone(),
            update.clone(),
            disabled,
            second.clone(),
        ];

        let context = display_context(&events, 10);
        assert_eq!(
            (context.capture_start_ns, context.capture_end_ns),
            (Some(20), Some(60))
        );
        assert_eq!(context.state, DisplayStateAtTime::BeforeCapture);
        assert!(context.frame_event.is_none());

        let context = display_context(&events, 35);
        assert_eq!(context.state, DisplayStateAtTime::Available);
        assert_eq!(context.frame_event.map(|event| event.id), Some(update.id));

        let context = display_context(&events, 50);
        assert_eq!(context.state, DisplayStateAtTime::Disabled);
        assert!(context.frame_event.is_none());

        let context = display_context(&events, 70);
        assert_eq!(context.state, DisplayStateAtTime::Available);
        assert_eq!(context.frame_event.map(|event| event.id), Some(second.id));
    }

    #[test]
    fn stale_process_record_never_terminates_an_unrelated_process() {
        let state = tempfile::tempdir().unwrap();
        let mut unrelated = Command::new("sleep").arg("30").spawn().unwrap();
        write_process_record(
            state.path(),
            &InspectorProcess {
                pid: unrelated.id(),
                instance_token: Uuid::new_v4(),
            },
        )
        .unwrap();
        std::fs::write(state.path().join("webui.json"), b"stale").unwrap();

        stop_recorded_inspector(state.path());

        assert!(unrelated.try_wait().unwrap().is_none());
        assert!(!process_record_path(state.path()).exists());
        assert!(!state.path().join("webui.json").exists());
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
    }

    #[test]
    fn matching_process_token_allows_only_the_recorded_child_to_stop() {
        let state = tempfile::tempdir().unwrap();
        let token = Uuid::new_v4();
        let mut child = Command::new("sh")
            .args([
                "-c",
                "trap 'exit 0' TERM; while :; do :; done",
                "avm-webui",
                &token.to_string(),
            ])
            .spawn()
            .unwrap();
        let process = InspectorProcess {
            pid: child.id(),
            instance_token: token,
        };
        write_process_record(state.path(), &process).unwrap();
        let matched = (0..100).any(|_| {
            if process_matches(&process) {
                true
            } else {
                std::thread::sleep(std::time::Duration::from_millis(10));
                false
            }
        });
        assert!(matched, "child never published its tokenized command line");

        stop_recorded_inspector(state.path());

        for _ in 0..100 {
            if child.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("recorded inspector did not stop after SIGTERM");
    }

    #[test]
    fn failed_start_cleanup_terminates_and_reaps_the_child() {
        let state = tempfile::tempdir().unwrap();
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        std::fs::write(process_record_path(state.path()), b"process").unwrap();
        std::fs::write(state.path().join("webui.json"), b"endpoint").unwrap();

        cleanup_failed_start(&mut child, state.path()).unwrap();

        assert!(child.try_wait().unwrap().is_some());
        assert!(!process_record_path(state.path()).exists());
        assert!(!state.path().join("webui.json").exists());
    }
}
