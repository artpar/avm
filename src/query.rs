use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    event::{Provenance, RawEvent},
    experience::{ExperienceStore, StoredFrame},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ExperienceQuery {
    AroundEvent {
        event_id: Uuid,
        #[serde(default = "default_before_ms")]
        before_ms: u64,
        #[serde(default = "default_after_ms")]
        after_ms: u64,
    },
    NetworkFrames {
        event_id: Uuid,
    },
    VisibleWhilePointerDown {
        event_id: Uuid,
    },
    EvidenceSinceFingerprint {
        repository_fingerprint: String,
    },
    BeforeConsoleException {
        event_id: Uuid,
        #[serde(default = "default_console_before_ms")]
        before_ms: u64,
    },
    RicherVisualEvidence {
        event_id: Uuid,
        #[serde(default = "default_before_ms")]
        before_ms: u64,
        #[serde(default = "default_after_ms")]
        after_ms: u64,
        #[serde(default = "default_frame_limit")]
        max_frames: usize,
    },
}

fn default_before_ms() -> u64 {
    500
}

fn default_after_ms() -> u64 {
    2_000
}

fn default_console_before_ms() -> u64 {
    2_000
}

fn default_frame_limit() -> usize {
    12
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryInterval {
    pub start_ns: u64,
    pub end_ns: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperienceQueryResult {
    pub query: ExperienceQuery,
    pub relation: Value,
    pub interval: QueryInterval,
    pub observed_events: Vec<RawEvent>,
    pub derived_events: Vec<RawEvent>,
    pub model_interpretations: Vec<RawEvent>,
    pub agent_claims: Vec<RawEvent>,
    pub frames: Vec<StoredFrame>,
    pub artifact_refs: Vec<String>,
}

pub fn execute_query(
    store: &ExperienceStore,
    query: ExperienceQuery,
) -> Result<ExperienceQueryResult> {
    match &query {
        ExperienceQuery::AroundEvent {
            event_id,
            before_ms,
            after_ms,
        } => around_event(store, query.clone(), *event_id, *before_ms, *after_ms, 12),
        ExperienceQuery::NetworkFrames { event_id } => {
            network_frames(store, query.clone(), *event_id)
        }
        ExperienceQuery::VisibleWhilePointerDown { event_id } => {
            visible_while_pointer_down(store, query.clone(), *event_id)
        }
        ExperienceQuery::EvidenceSinceFingerprint {
            repository_fingerprint,
        } => evidence_since_fingerprint(store, query.clone(), repository_fingerprint),
        ExperienceQuery::BeforeConsoleException {
            event_id,
            before_ms,
        } => before_console_exception(store, query.clone(), *event_id, *before_ms),
        ExperienceQuery::RicherVisualEvidence {
            event_id,
            before_ms,
            after_ms,
            max_frames,
        } => around_event(
            store,
            query.clone(),
            *event_id,
            *before_ms,
            *after_ms,
            *max_frames,
        ),
    }
}

fn around_event(
    store: &ExperienceStore,
    query: ExperienceQuery,
    event_id: Uuid,
    before_ms: u64,
    after_ms: u64,
    frame_limit: usize,
) -> Result<ExperienceQueryResult> {
    ensure!(frame_limit > 0, "frame limit must be positive");
    let anchor = required_event(store, event_id)?;
    let interval = QueryInterval {
        start_ns: anchor
            .host_monotonic_ns
            .saturating_sub(ms_to_ns(before_ms)?),
        end_ns: anchor.host_monotonic_ns.saturating_add(ms_to_ns(after_ms)?),
    };
    let events = store.history(Some(interval.start_ns), Some(interval.end_ns), &[])?;
    let frames = replay_frames(store, &interval, frame_limit)?;
    build_result(
        query,
        json!({
            "type": "bounded_temporal_context",
            "anchorEventId": event_id,
            "beforeMs": before_ms,
            "afterMs": after_ms,
        }),
        interval,
        events,
        frames,
    )
}

fn network_frames(
    store: &ExperienceStore,
    query: ExperienceQuery,
    event_id: Uuid,
) -> Result<ExperienceQueryResult> {
    let anchor = required_event(store, event_id)?;
    ensure!(
        matches!(
            anchor.kind.as_str(),
            "browser.network.request" | "browser.network.response"
        ),
        "selected event is not a browser network request or response"
    );
    let url = anchor
        .payload
        .get("url")
        .and_then(Value::as_str)
        .context("network event has no URL")?;
    let all = store.history(None, None, &[])?;
    let peer = if anchor.kind == "browser.network.request" {
        all.iter().find(|event| {
            event.host_monotonic_ns >= anchor.host_monotonic_ns
                && event.kind == "browser.network.response"
                && event.payload.get("url").and_then(Value::as_str) == Some(url)
        })
    } else {
        all.iter().rev().find(|event| {
            event.host_monotonic_ns <= anchor.host_monotonic_ns
                && event.kind == "browser.network.request"
                && event.payload.get("url").and_then(Value::as_str) == Some(url)
        })
    };
    let pair_start = peer
        .map(|event| event.host_monotonic_ns.min(anchor.host_monotonic_ns))
        .unwrap_or(anchor.host_monotonic_ns);
    let pair_end = peer
        .map(|event| event.host_monotonic_ns.max(anchor.host_monotonic_ns))
        .unwrap_or(anchor.host_monotonic_ns);
    let interval = QueryInterval {
        start_ns: pair_start.saturating_sub(ms_to_ns(250)?),
        end_ns: pair_end.saturating_add(ms_to_ns(500)?),
    };
    let events = store.history(Some(interval.start_ns), Some(interval.end_ns), &[])?;
    let frame_times = [pair_start, pair_end];
    let frames = frames_at(store, &frame_times, frame_times.len())?;
    build_result(
        query,
        json!({
            "type": "temporal_network_association",
            "anchorEventId": event_id,
            "peerEventId": peer.map(|event| event.id),
            "url": url,
            "limitation": "request and response are paired by URL and order because the browser sensor does not yet expose a transport request identifier",
        }),
        interval,
        events,
        frames,
    )
}

fn visible_while_pointer_down(
    store: &ExperienceStore,
    query: ExperienceQuery,
    event_id: Uuid,
) -> Result<ExperienceQueryResult> {
    let down = required_event(store, event_id)?;
    ensure!(
        down.kind == "pointer.down",
        "selected event is not pointer-down"
    );
    let all = store.history(Some(down.host_monotonic_ns), None, &[])?;
    let up = all
        .iter()
        .skip(1)
        .take_while(|event| event.kind != "pointer.down")
        .find(|event| event.kind == "pointer.up")
        .context("pointer-down has no subsequent pointer-up before the next pointer-down")?;
    let interval = QueryInterval {
        start_ns: down.host_monotonic_ns,
        end_ns: up.host_monotonic_ns,
    };
    let mut events = store.history(Some(interval.start_ns), Some(interval.end_ns), &[])?;
    let later_derived = store
        .history(Some(interval.end_ns), None, &["perception".to_owned()])?
        .into_iter()
        .filter(|event| temporal_payload_overlaps(event, &interval))
        .collect::<Vec<_>>();
    events.extend(later_derived);
    let mut times = vec![interval.start_ns, interval.end_ns];
    times.extend(
        events
            .iter()
            .filter(|event| matches!(event.kind.as_str(), "display.scanout" | "display.update"))
            .map(|event| event.host_monotonic_ns),
    );
    let frames = frames_at(store, &times, 12)?;
    build_result(
        query,
        json!({
            "type": "pointer_hold_interval",
            "pointerDownEventId": down.id,
            "pointerUpEventId": up.id,
        }),
        interval,
        events,
        frames,
    )
}

fn evidence_since_fingerprint(
    store: &ExperienceStore,
    query: ExperienceQuery,
    fingerprint: &str,
) -> Result<ExperienceQueryResult> {
    ensure!(
        !fingerprint.trim().is_empty(),
        "fingerprint must not be empty"
    );
    let all = store.history(None, None, &[])?;
    let anchor = all
        .iter()
        .find(|event| event.repository_fingerprint.as_deref() == Some(fingerprint))
        .with_context(|| format!("repository fingerprint {fingerprint} is not in the timeline"))?;
    let anchor_id = anchor.id;
    let anchor_ns = anchor.host_monotonic_ns;
    let end_ns = all
        .last()
        .map(|event| event.host_monotonic_ns)
        .unwrap_or(anchor_ns);
    let interval = QueryInterval {
        start_ns: anchor_ns,
        end_ns,
    };
    let events = all
        .into_iter()
        .filter(|event| {
            event.host_monotonic_ns >= interval.start_ns
                && (event.source == "evidence" || event.kind.starts_with("evidence."))
        })
        .collect();
    build_result(
        query,
        json!({
            "type": "evidence_since_repository_state",
            "repositoryFingerprint": fingerprint,
            "anchorEventId": anchor_id,
        }),
        interval,
        events,
        Vec::new(),
    )
}

fn before_console_exception(
    store: &ExperienceStore,
    query: ExperienceQuery,
    event_id: Uuid,
    before_ms: u64,
) -> Result<ExperienceQueryResult> {
    let exception = required_event(store, event_id)?;
    ensure!(
        matches!(
            exception.kind.as_str(),
            "browser.javascript.exception" | "browser.console.message"
        ),
        "selected event is not a console message or JavaScript exception"
    );
    let interval = QueryInterval {
        start_ns: exception
            .host_monotonic_ns
            .saturating_sub(ms_to_ns(before_ms)?),
        end_ns: exception.host_monotonic_ns,
    };
    let events = store.history(Some(interval.start_ns), Some(interval.end_ns), &[])?;
    let frames = replay_frames(store, &interval, 8)?;
    build_result(
        query,
        json!({
            "type": "pre_exception_context",
            "exceptionEventId": exception.id,
            "beforeMs": before_ms,
        }),
        interval,
        events,
        frames,
    )
}

fn required_event(store: &ExperienceStore, event_id: Uuid) -> Result<RawEvent> {
    store
        .event(event_id)?
        .with_context(|| format!("event {event_id} is not in the timeline"))
}

fn replay_frames(
    store: &ExperienceStore,
    interval: &QueryInterval,
    limit: usize,
) -> Result<Vec<StoredFrame>> {
    let replay = store.replay(interval.start_ns, interval.end_ns)?;
    let times = replay
        .keyframes
        .iter()
        .map(|frame| frame.host_monotonic_ns)
        .collect::<Vec<_>>();
    frames_at(store, &times, limit)
}

fn frames_at(store: &ExperienceStore, times: &[u64], limit: usize) -> Result<Vec<StoredFrame>> {
    ensure!(limit > 0, "frame limit must be positive");
    let unique = times.iter().copied().collect::<BTreeSet<_>>();
    let selected = compact(unique.into_iter().collect(), limit);
    let frames = selected
        .into_iter()
        .filter_map(|timestamp| match store.frame(timestamp, None) {
            Ok(frame) => Some(Ok(frame)),
            Err(error) if error.to_string().contains("no reconstructable framebuffer") => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(frames.into_iter().fold(Vec::new(), |mut compact, frame| {
        if compact
            .last()
            .is_none_or(|previous: &StoredFrame| previous.frame_sha256 != frame.frame_sha256)
        {
            compact.push(frame);
        }
        compact
    }))
}

fn compact<T: Clone>(items: Vec<T>, limit: usize) -> Vec<T> {
    if items.len() <= limit {
        return items;
    }
    if limit == 1 {
        return vec![items[items.len() - 1].clone()];
    }
    (0..limit)
        .map(|index| items[index * (items.len() - 1) / (limit - 1)].clone())
        .collect()
}

fn temporal_payload_overlaps(event: &RawEvent, interval: &QueryInterval) -> bool {
    event.kind == "perception.temporal.analysis"
        && event
            .payload
            .get("observations")
            .and_then(Value::as_array)
            .is_some_and(|observations| {
                observations.iter().any(|observation| {
                    let start = observation.get("startNs").and_then(Value::as_u64);
                    let end = observation.get("endNs").and_then(Value::as_u64);
                    matches!((start, end), (Some(start), Some(end)) if start <= interval.end_ns && end >= interval.start_ns)
                })
            })
}

fn build_result(
    query: ExperienceQuery,
    relation: Value,
    interval: QueryInterval,
    events: Vec<RawEvent>,
    frames: Vec<StoredFrame>,
) -> Result<ExperienceQueryResult> {
    ensure!(
        interval.start_ns <= interval.end_ns,
        "query interval is invalid"
    );
    let mut observed_events = Vec::new();
    let mut derived_events = Vec::new();
    let mut model_interpretations = Vec::new();
    let mut agent_claims = Vec::new();
    let mut artifact_refs = BTreeSet::new();
    for event in events {
        artifact_refs.extend(event.artifact_refs.iter().cloned());
        match event.provenance {
            Provenance::Observed => observed_events.push(event),
            Provenance::Derived => derived_events.push(event),
            Provenance::ModelInterpreted => model_interpretations.push(event),
            Provenance::AgentClaim => agent_claims.push(event),
        }
    }
    artifact_refs.extend(frames.iter().map(|frame| frame.artifact_ref.clone()));
    Ok(ExperienceQueryResult {
        query,
        relation,
        interval,
        observed_events,
        derived_events,
        model_interpretations,
        agent_claims,
        frames,
        artifact_refs: artifact_refs.into_iter().collect(),
    })
}

fn ms_to_ns(milliseconds: u64) -> Result<u64> {
    milliseconds
        .checked_mul(1_000_000)
        .context("query duration exceeds nanosecond range")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::EventSink,
        framebuffer::{Framebuffer, PIXMAN_X8R8G8B8},
        storage::ArtifactStore,
        timeline::TimelineStore,
    };

    struct Fixture {
        _temp: tempfile::TempDir,
        store: ExperienceStore,
        down_id: Uuid,
        request_id: Uuid,
        exception_id: Uuid,
    }

    fn fixture() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let artifact_root = temp.path().join("artifacts");
        let artifacts = ArtifactStore::new(&artifact_root).unwrap();
        let timeline_path = temp.path().join("timeline.sqlite3");
        let timeline = TimelineStore::open(&timeline_path).unwrap();
        let session = Uuid::new_v4();
        let first_bytes = vec![0_u8; 16];
        let second_bytes = vec![9_u8; 16];
        for (timestamp, bytes) in [(10, &first_bytes), (25, &second_bytes)] {
            let frame = Framebuffer::from_scanout(2, 2, 8, PIXMAN_X8R8G8B8, bytes).unwrap();
            let mut event = RawEvent::observed_at(
                session,
                timestamp,
                "display",
                "display.scanout",
                json!({
                    "width": 2, "height": 2, "stride": 8,
                    "pixmanFormat": PIXMAN_X8R8G8B8,
                    "frameSha256": frame.sha256()
                }),
            );
            event.artifact_refs.push(artifacts.put(bytes).unwrap());
            timeline.record(event).unwrap();
        }
        let mut down = RawEvent::observed_at(
            session,
            20,
            "input",
            "pointer.down",
            json!({"x": 1, "y": 1}),
        );
        down.repository_fingerprint = Some("repo-a".into());
        let down_id = down.id;
        timeline.record(down).unwrap();
        timeline
            .record(RawEvent::observed_at(
                session,
                30,
                "input",
                "pointer.up",
                json!({"x": 1, "y": 1}),
            ))
            .unwrap();
        let request = RawEvent::observed_at(
            session,
            40,
            "network",
            "browser.network.request",
            json!({"url": "https://fixture.invalid/api", "method": "POST"}),
        );
        let request_id = request.id;
        timeline.record(request).unwrap();
        timeline
            .record(RawEvent::observed_at(
                session,
                50,
                "network",
                "browser.network.response",
                json!({"url": "https://fixture.invalid/api", "status": 200}),
            ))
            .unwrap();
        let exception = RawEvent::observed_at(
            session,
            60,
            "console",
            "browser.javascript.exception",
            json!({"message": "fixture failure"}),
        );
        let exception_id = exception.id;
        timeline.record(exception).unwrap();
        let mut evidence = RawEvent::observed_at(
            session,
            70,
            "evidence",
            "evidence.command.completed",
            json!({"exitCode": 0}),
        );
        evidence.repository_fingerprint = Some("repo-a".into());
        timeline.record(evidence).unwrap();
        let mut temporal = RawEvent::observed_at(
            session,
            80,
            "perception",
            "perception.temporal.analysis",
            json!({"observations": [{
                "kind": "input.multiple_visual_states_while_pointer_down",
                "startNs": 20,
                "endNs": 30
            }]}),
        );
        temporal.provenance = Provenance::Derived;
        timeline.record(temporal).unwrap();
        let store = ExperienceStore::open(session, timeline_path, artifact_root).unwrap();
        Fixture {
            _temp: temp,
            store,
            down_id,
            request_id,
            exception_id,
        }
    }

    #[test]
    fn compact_keeps_endpoints_and_requested_limit() {
        assert_eq!(compact((0..20).collect(), 3), vec![0, 9, 19]);
        assert_eq!(compact(vec![1, 2], 3), vec![1, 2]);
        assert_eq!(compact(vec![1, 2], 1), vec![2]);
    }

    #[test]
    fn query_json_uses_explicit_variant_and_defaults() {
        let event_id = Uuid::new_v4();
        let query: ExperienceQuery = serde_json::from_value(json!({
            "kind": "aroundEvent",
            "eventId": event_id
        }))
        .unwrap();
        match query {
            ExperienceQuery::AroundEvent {
                event_id: parsed,
                before_ms,
                after_ms,
            } => {
                assert_eq!(parsed, event_id);
                assert_eq!(before_ms, 500);
                assert_eq!(after_ms, 2_000);
            }
            _ => panic!("wrong query variant"),
        }
    }

    #[test]
    fn cross_source_queries_return_direct_evidence_before_derived_context() {
        let fixture = fixture();
        let around = execute_query(
            &fixture.store,
            ExperienceQuery::AroundEvent {
                event_id: fixture.down_id,
                before_ms: 1,
                after_ms: 1,
            },
        )
        .unwrap();
        assert!(
            around
                .observed_events
                .iter()
                .any(|event| event.id == fixture.down_id)
        );
        assert!(!around.frames.is_empty());

        let network = execute_query(
            &fixture.store,
            ExperienceQuery::NetworkFrames {
                event_id: fixture.request_id,
            },
        )
        .unwrap();
        assert_eq!(network.relation["url"], "https://fixture.invalid/api");
        assert_eq!(network.frames.len(), 1);

        let held = execute_query(
            &fixture.store,
            ExperienceQuery::VisibleWhilePointerDown {
                event_id: fixture.down_id,
            },
        )
        .unwrap();
        assert_eq!(held.interval.start_ns, 20);
        assert_eq!(held.interval.end_ns, 30);
        assert!(
            held.derived_events
                .iter()
                .any(|event| event.kind == "perception.temporal.analysis")
        );
        assert_eq!(held.frames.len(), 2);

        let evidence = execute_query(
            &fixture.store,
            ExperienceQuery::EvidenceSinceFingerprint {
                repository_fingerprint: "repo-a".into(),
            },
        )
        .unwrap();
        assert_eq!(evidence.observed_events.len(), 1);
        assert_eq!(
            evidence.observed_events[0].kind,
            "evidence.command.completed"
        );

        let before_exception = execute_query(
            &fixture.store,
            ExperienceQuery::BeforeConsoleException {
                event_id: fixture.exception_id,
                before_ms: 1,
            },
        )
        .unwrap();
        assert_eq!(
            before_exception.observed_events.last().unwrap().id,
            fixture.exception_id
        );

        let richer = execute_query(
            &fixture.store,
            ExperienceQuery::RicherVisualEvidence {
                event_id: fixture.down_id,
                before_ms: 1,
                after_ms: 1,
                max_frames: 1,
            },
        )
        .unwrap();
        assert_eq!(richer.frames.len(), 1);
        assert!(
            richer
                .artifact_refs
                .contains(&richer.frames[0].artifact_ref)
        );
    }
}
