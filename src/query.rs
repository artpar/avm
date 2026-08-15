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
    BrowserElementUnderPointer {
        event_id: Uuid,
    },
    LastDialog {
        text: Option<String>,
    },
    RuntimeTrace {
        event_id: Uuid,
        #[serde(default = "default_before_ms")]
        before_ms: u64,
        #[serde(default = "default_after_ms")]
        after_ms: u64,
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
        ExperienceQuery::BrowserElementUnderPointer { event_id } => {
            browser_element_under_pointer(store, query.clone(), *event_id)
        }
        ExperienceQuery::LastDialog { text } => last_dialog(store, query.clone(), text.as_deref()),
        ExperienceQuery::RuntimeTrace {
            event_id,
            before_ms,
            after_ms,
        } => runtime_trace(store, query.clone(), *event_id, *before_ms, *after_ms),
    }
}

fn runtime_trace(
    store: &ExperienceStore,
    query: ExperienceQuery,
    event_id: Uuid,
    before_ms: u64,
    after_ms: u64,
) -> Result<ExperienceQueryResult> {
    let anchor = required_event(store, event_id)?;
    ensure!(
        anchor.source == "runtime",
        "selected event is not runtime telemetry"
    );
    let trace_id = anchor
        .payload
        .get("traceId")
        .and_then(Value::as_str)
        .context("runtime event has no traceId")?;
    ensure!(
        trace_id.len() == 32 && trace_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "runtime event traceId is invalid"
    );
    let all = store.history(None, None, &[])?;
    let trace_members = all
        .iter()
        .filter(|event| {
            event.source == "runtime"
                && event.payload.get("traceId").and_then(Value::as_str) == Some(trace_id)
        })
        .collect::<Vec<_>>();
    ensure!(!trace_members.is_empty(), "runtime trace has no members");
    let trace_start = trace_members
        .iter()
        .map(|event| event.host_monotonic_ns)
        .min()
        .context("runtime trace has no start")?;
    let trace_end = trace_members
        .iter()
        .map(|event| event.host_monotonic_ns)
        .max()
        .context("runtime trace has no end")?;
    let interval = QueryInterval {
        start_ns: trace_start.saturating_sub(ms_to_ns(before_ms)?),
        end_ns: trace_end.saturating_add(ms_to_ns(after_ms)?),
    };
    let events = interval_events(store, &interval)?;
    let frames = replay_frames(store, &interval, 12)?;
    build_result(
        query,
        json!({
            "type": "instrumented_trace_context",
            "anchorEventId": event_id,
            "traceId": trace_id,
            "traceMemberEventIds": trace_members.iter().map(|event| event.id).collect::<Vec<_>>(),
            "traceMembershipBasis": "exact_instrumented_trace_id",
            "traceMembershipCertainty": "declared_by_instrumentation",
            "temporalNeighborsAreCausal": false,
            "note": "Events without this trace ID are included only as temporal context."
        }),
        interval,
        events,
        frames,
    )
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
    let events = interval_events(store, &interval)?;
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
    let events = interval_events(store, &interval)?;
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
    let events = interval_events(store, &interval)?;
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
    let events = interval_events(store, &interval)?;
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

fn browser_element_under_pointer(
    store: &ExperienceStore,
    query: ExperienceQuery,
    event_id: Uuid,
) -> Result<ExperienceQueryResult> {
    let input = required_event(store, event_id)?;
    ensure!(
        matches!(
            input.kind.as_str(),
            "pointer.move" | "pointer.down" | "pointer.up"
        ),
        "selected event is not a pointer event"
    );
    let all = store.history(None, None, &[])?;
    let (display_x, display_y, coordinate_event) = pointer_coordinates(&input, &all)?;
    let mut correlations = all
        .iter()
        .filter(|event| event.kind == "browser.coordinate_correlation")
        .filter_map(|correlation| {
            let snapshot_id = correlation
                .payload
                .get("browserSnapshotEventId")
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())?;
            let snapshot = all.iter().find(|event| event.id == snapshot_id)?;
            Some((
                snapshot.host_monotonic_ns.abs_diff(input.host_monotonic_ns),
                correlation,
                snapshot,
            ))
        })
        .collect::<Vec<_>>();
    correlations.sort_by_key(|(distance, correlation, _)| (*distance, correlation.id));
    let (distance_ns, correlation, snapshot) = correlations
        .first()
        .copied()
        .context("timeline contains no browser snapshot with framebuffer correlation")?;
    let mapping = correlation
        .payload
        .get("correlation")
        .context("browser correlation event has no correlation payload")?;
    let origin_x = numeric_field(mapping, "displayX")?;
    let origin_y = numeric_field(mapping, "displayY")?;
    let viewport_width = numeric_field(mapping, "viewportWidth")?;
    let viewport_height = numeric_field(mapping, "viewportHeight")?;
    ensure!(
        display_x >= origin_x && display_y >= origin_y,
        "pointer is outside the correlated browser viewport"
    );
    let inner_width = snapshot
        .payload
        .pointer("/state/windowMetrics/innerWidth")
        .and_then(Value::as_f64)
        .unwrap_or(viewport_width);
    let inner_height = snapshot
        .payload
        .pointer("/state/windowMetrics/innerHeight")
        .and_then(Value::as_f64)
        .unwrap_or(viewport_height);
    ensure!(
        viewport_width > 0.0 && viewport_height > 0.0,
        "correlated browser viewport is empty"
    );
    let viewport_x = (display_x - origin_x) * inner_width / viewport_width;
    let viewport_y = (display_y - origin_y) * inner_height / viewport_height;
    let element = hit_test_snapshot(snapshot, viewport_x, viewport_y)?;
    let interval = QueryInterval {
        start_ns: snapshot
            .host_monotonic_ns
            .min(input.host_monotonic_ns)
            .min(coordinate_event.host_monotonic_ns),
        end_ns: snapshot.host_monotonic_ns.max(input.host_monotonic_ns),
    };
    let mut events = vec![snapshot.clone(), correlation.clone()];
    if coordinate_event.id != input.id {
        events.push(coordinate_event.clone());
    }
    events.push(input.clone());
    let frames = frames_at(store, &[input.host_monotonic_ns], 1)?;
    build_result(
        query,
        json!({
            "type": "browser_snapshot_hit_test",
            "pointerEventId": input.id,
            "coordinateSourceEventId": coordinate_event.id,
            "browserSnapshotEventId": snapshot.id,
            "coordinateCorrelationEventId": correlation.id,
            "snapshotDistanceNs": distance_ns,
            "displayPoint": {"x": display_x, "y": display_y},
            "viewportCssPoint": {"x": viewport_x, "y": viewport_y},
            "element": element,
            "limitation": "element identity is derived from the nearest correlated snapshot, not a live DOM hit-test at input dispatch time",
        }),
        interval,
        events,
        frames,
    )
}

fn last_dialog(
    store: &ExperienceStore,
    query: ExperienceQuery,
    requested_text: Option<&str>,
) -> Result<ExperienceQueryResult> {
    let all = store.history(None, None, &[])?;
    let (snapshot, dialog) = all
        .iter()
        .rev()
        .filter(|event| event.kind == "browser.page.snapshot")
        .find_map(|snapshot| {
            accessibility_dialog(snapshot, requested_text).map(|dialog| (snapshot, dialog))
        })
        .context("no matching dialog appears in a browser accessibility snapshot")?;
    let down = all.iter().rev().find(|event| {
        event.kind == "pointer.down" && event.host_monotonic_ns <= snapshot.host_monotonic_ns
    });
    let start_ns = down
        .map(|event| event.host_monotonic_ns)
        .unwrap_or(snapshot.host_monotonic_ns);
    let interval = QueryInterval {
        start_ns,
        end_ns: snapshot.host_monotonic_ns,
    };
    let events = interval_events(store, &interval)?;
    let frames = replay_frames(store, &interval, 12)?;
    build_result(
        query,
        json!({
            "type": "last_accessibility_dialog_interaction",
            "browserSnapshotEventId": snapshot.id,
            "precedingPointerDownEventId": down.map(|event| event.id),
            "dialog": dialog,
            "requestedText": requested_text,
        }),
        interval,
        events,
        frames,
    )
}

fn hit_test_snapshot(snapshot: &RawEvent, viewport_x: f64, viewport_y: f64) -> Result<Value> {
    let strings = snapshot
        .payload
        .pointer("/dom/strings")
        .and_then(Value::as_array)
        .context("browser snapshot has no DOM string table")?;
    let documents = snapshot
        .payload
        .pointer("/dom/documents")
        .and_then(Value::as_array)
        .context("browser snapshot has no DOM documents")?;
    let mut candidates = Vec::new();
    for (document_index, document) in documents.iter().enumerate() {
        let scroll_x = document
            .get("scrollOffsetX")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let scroll_y = document
            .get("scrollOffsetY")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let document_x = viewport_x + scroll_x;
        let document_y = viewport_y + scroll_y;
        let layout = document
            .get("layout")
            .context("DOM document has no layout table")?;
        let node_indexes = value_array(layout, "nodeIndex")?;
        let bounds = value_array(layout, "bounds")?;
        let styles = value_array(layout, "styles")?;
        let paint_orders = layout.get("paintOrders").and_then(Value::as_array);
        let nodes = document
            .get("nodes")
            .context("DOM document has no node table")?;
        for (layout_index, bounds_value) in bounds.iter().enumerate() {
            let Some(rect) = rectangle(bounds_value) else {
                continue;
            };
            if rect[2] <= 0.0
                || rect[3] <= 0.0
                || document_x < rect[0]
                || document_y < rect[1]
                || document_x > rect[0] + rect[2]
                || document_y > rect[1] + rect[3]
            {
                continue;
            }
            let Some(node_index) = node_indexes
                .get(layout_index)
                .and_then(Value::as_u64)
                .map(|index| index as usize)
            else {
                continue;
            };
            if indexed_u64(nodes, "nodeType", node_index) != Some(1)
                || !visible_style(styles.get(layout_index), strings)
            {
                continue;
            }
            let paint_order = paint_orders
                .and_then(|orders| orders.get(layout_index))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            candidates.push((
                paint_order,
                rect[2] * rect[3],
                document_index,
                node_index,
                rect,
                nodes,
            ));
        }
    }
    candidates.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.total_cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.3.cmp(&right.3))
    });
    let (paint_order, _, document_index, node_index, bounds, nodes) = candidates
        .first()
        .context("no visible DOM element contains the correlated pointer point")?;
    let backend_node_id = indexed_u64(nodes, "backendNodeId", *node_index);
    let node_name = indexed_string(nodes, "nodeName", *node_index, strings);
    let attributes = indexed_attributes(nodes, *node_index, strings);
    let accessibility = backend_node_id.and_then(|backend_id| {
        snapshot
            .payload
            .pointer("/accessibility/nodes")
            .and_then(Value::as_array)
            .and_then(|nodes| {
                nodes.iter().find(|node| {
                    node.get("backendDOMNodeId").and_then(Value::as_u64) == Some(backend_id)
                })
            })
            .map(|node| {
                json!({
                    "role": node.pointer("/role/value"),
                    "name": node.pointer("/name/value"),
                })
            })
    });
    Ok(json!({
        "documentIndex": document_index,
        "nodeIndex": node_index,
        "backendNodeId": backend_node_id,
        "nodeName": node_name,
        "attributes": attributes,
        "bounds": {"x": bounds[0], "y": bounds[1], "width": bounds[2], "height": bounds[3]},
        "paintOrder": paint_order,
        "accessibility": accessibility,
    }))
}

fn accessibility_dialog(snapshot: &RawEvent, requested_text: Option<&str>) -> Option<Value> {
    snapshot
        .payload
        .pointer("/accessibility/nodes")?
        .as_array()?
        .iter()
        .find_map(|node| {
            let role = node.pointer("/role/value")?.as_str()?;
            if !matches!(role, "dialog" | "alertdialog") {
                return None;
            }
            let name = node
                .pointer("/name/value")
                .and_then(Value::as_str)
                .unwrap_or("");
            if requested_text.is_some_and(|text| !name.contains(text)) {
                return None;
            }
            Some(json!({
                "role": role,
                "name": name,
                "backendNodeId": node.get("backendDOMNodeId"),
            }))
        })
}

fn value_array<'a>(value: &'a Value, field: &str) -> Result<&'a Vec<Value>> {
    value
        .get(field)
        .and_then(Value::as_array)
        .with_context(|| format!("DOM layout has no {field} array"))
}

fn rectangle(value: &Value) -> Option<[f64; 4]> {
    let values = value.as_array()?;
    Some([
        values.first()?.as_f64()?,
        values.get(1)?.as_f64()?,
        values.get(2)?.as_f64()?,
        values.get(3)?.as_f64()?,
    ])
}

fn visible_style(style: Option<&Value>, strings: &[Value]) -> bool {
    let Some(indexes) = style.and_then(Value::as_array) else {
        return true;
    };
    let values = indexes
        .iter()
        .filter_map(Value::as_u64)
        .filter_map(|index| strings.get(index as usize))
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    values.first().is_none_or(|value| *value != "none")
        && values.get(1).is_none_or(|value| *value != "hidden")
        && values.get(2).is_none_or(|value| *value != "0")
        && values.get(3).is_none_or(|value| *value != "none")
}

fn indexed_u64(nodes: &Value, field: &str, index: usize) -> Option<u64> {
    nodes.get(field)?.as_array()?.get(index)?.as_u64()
}

fn indexed_string(nodes: &Value, field: &str, index: usize, strings: &[Value]) -> Option<String> {
    let string_index = indexed_u64(nodes, field, index)? as usize;
    strings.get(string_index)?.as_str().map(ToOwned::to_owned)
}

fn indexed_attributes(nodes: &Value, index: usize, strings: &[Value]) -> Value {
    let indexes = nodes
        .get("attributes")
        .and_then(Value::as_array)
        .and_then(|attributes| attributes.get(index))
        .and_then(Value::as_array);
    let mut object = serde_json::Map::new();
    if let Some(indexes) = indexes {
        for pair in indexes.chunks(2) {
            let Some(name) = pair
                .first()
                .and_then(Value::as_u64)
                .and_then(|index| strings.get(index as usize))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let value = pair
                .get(1)
                .and_then(Value::as_u64)
                .and_then(|index| strings.get(index as usize))
                .and_then(Value::as_str)
                .unwrap_or("");
            object.insert(name.to_owned(), Value::String(value.to_owned()));
        }
    }
    Value::Object(object)
}

fn numeric_field(value: &Value, field: &str) -> Result<f64> {
    value
        .get(field)
        .and_then(Value::as_f64)
        .with_context(|| format!("payload has no numeric {field}"))
}

fn direct_pointer_coordinates(event: &RawEvent) -> Option<(f64, f64)> {
    let coordinates = event.payload.get("detail").unwrap_or(&event.payload);
    Some((
        coordinates.get("x")?.as_f64()?,
        coordinates.get("y")?.as_f64()?,
    ))
}

fn pointer_coordinates<'a>(
    input: &'a RawEvent,
    events: &'a [RawEvent],
) -> Result<(f64, f64, &'a RawEvent)> {
    if let Some((x, y)) = direct_pointer_coordinates(input) {
        return Ok((x, y, input));
    }

    let action_id = input.payload.get("actionId").and_then(Value::as_str);
    let preceding_moves = events.iter().filter(|event| {
        event.source == "input"
            && event.kind == "pointer.move"
            && event.host_monotonic_ns <= input.host_monotonic_ns
            && direct_pointer_coordinates(event).is_some()
    });
    let coordinate_event = action_id
        .and_then(|action_id| {
            preceding_moves
                .clone()
                .filter(|event| {
                    event.payload.get("actionId").and_then(Value::as_str) == Some(action_id)
                })
                .max_by_key(|event| (event.host_monotonic_ns, event.id))
        })
        .or_else(|| preceding_moves.max_by_key(|event| (event.host_monotonic_ns, event.id)))
        .context("pointer event has no coordinates and no preceding pointer move")?;
    let (x, y) = direct_pointer_coordinates(coordinate_event)
        .context("coordinate source event has no pointer coordinates")?;
    Ok((x, y, coordinate_event))
}

fn required_event(store: &ExperienceStore, event_id: Uuid) -> Result<RawEvent> {
    store
        .event(event_id)?
        .with_context(|| format!("event {event_id} is not in the timeline"))
}

fn interval_events(store: &ExperienceStore, interval: &QueryInterval) -> Result<Vec<RawEvent>> {
    let mut events = store.history(Some(interval.start_ns), Some(interval.end_ns), &[])?;
    let existing = events.iter().map(|event| event.id).collect::<BTreeSet<_>>();
    events.extend(
        store
            .history(None, None, &["perception".to_owned()])?
            .into_iter()
            .filter(|event| !existing.contains(&event.id))
            .filter(|event| interpreted_payload_overlaps(event, interval)),
    );
    events.sort_by_key(|event| (event.host_monotonic_ns, event.id));
    Ok(events)
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

fn interpreted_payload_overlaps(event: &RawEvent, interval: &QueryInterval) -> bool {
    if event.kind == "perception.vlm.observation" {
        let start = event
            .payload
            .pointer("/trigger/startNs")
            .and_then(Value::as_u64);
        let end = event
            .payload
            .pointer("/trigger/endNs")
            .and_then(Value::as_u64);
        return matches!((start, end), (Some(start), Some(end)) if start <= interval.end_ns && end >= interval.start_ns);
    }
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
        move_id: Uuid,
        down_id: Uuid,
        up_id: Uuid,
        request_id: Uuid,
        exception_id: Uuid,
        runtime_span_id: Uuid,
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
        let mut browser_snapshot = RawEvent::observed_at(
            session,
            15,
            "browser",
            "browser.page.snapshot",
            json!({
                "state": {"windowMetrics": {"innerWidth": 100, "innerHeight": 100}},
                "dom": {
                    "strings": ["#document", "HTML", "BODY", "BUTTON", "id", "save", "block", "visible", "1", "auto"],
                    "documents": [{
                        "scrollOffsetX": 0,
                        "scrollOffsetY": 0,
                        "nodes": {
                            "parentIndex": [-1, 0, 1, 2],
                            "nodeType": [9, 1, 1, 1],
                            "nodeName": [0, 1, 2, 3],
                            "backendNodeId": [1, 2, 3, 4],
                            "attributes": [[], [], [], [4, 5]]
                        },
                        "layout": {
                            "nodeIndex": [1, 2, 3],
                            "bounds": [[0, 0, 100, 100], [0, 0, 100, 100], [0, 0, 20, 20]],
                            "styles": [[6, 7, 8, 9], [6, 7, 8, 9], [6, 7, 8, 9]],
                            "paintOrders": [0, 1, 2]
                        }
                    }]
                },
                "accessibility": {"nodes": [{
                    "backendDOMNodeId": 4,
                    "role": {"value": "button"},
                    "name": {"value": "Save"}
                }]}
            }),
        );
        browser_snapshot
            .artifact_refs
            .push(artifacts.put(b"browser viewport fixture").unwrap());
        let browser_snapshot_id = browser_snapshot.id;
        timeline.record(browser_snapshot).unwrap();
        let mut correlation = RawEvent::observed_at(
            session,
            18,
            "browser",
            "browser.coordinate_correlation",
            json!({
                "browserSnapshotEventId": browser_snapshot_id,
                "correlation": {
                    "displayX": 0,
                    "displayY": 0,
                    "viewportWidth": 100,
                    "viewportHeight": 100
                }
            }),
        );
        correlation.provenance = Provenance::Derived;
        timeline.record(correlation).unwrap();
        let action_id = Uuid::new_v4();
        let pointer_move = RawEvent::observed_at(
            session,
            19,
            "input",
            "pointer.move",
            json!({"actionId": action_id, "detail": {"x": 1, "y": 1}}),
        );
        let move_id = pointer_move.id;
        timeline.record(pointer_move).unwrap();
        let mut down = RawEvent::observed_at(
            session,
            20,
            "input",
            "pointer.down",
            json!({"actionId": action_id, "detail": {"button": "Left"}}),
        );
        down.repository_fingerprint = Some("repo-a".into());
        let down_id = down.id;
        timeline.record(down).unwrap();
        let pointer_up = RawEvent::observed_at(
            session,
            30,
            "input",
            "pointer.up",
            json!({"actionId": action_id, "detail": {"button": "Left"}}),
        );
        let up_id = pointer_up.id;
        timeline.record(pointer_up).unwrap();
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
        let runtime_span = RawEvent::observed_at(
            session,
            42,
            "runtime",
            "runtime.span",
            json!({
                "traceId": "0123456789abcdef0123456789abcdef",
                "spanId": "0123456789abcdef",
                "name": "POST /api"
            }),
        );
        let runtime_span_id = runtime_span.id;
        timeline.record(runtime_span).unwrap();
        timeline
            .record(RawEvent::observed_at(
                session,
                48,
                "runtime",
                "runtime.application.log",
                json!({
                    "traceId": "0123456789abcdef0123456789abcdef",
                    "spanId": "0123456789abcdef",
                    "body": "saved"
                }),
            ))
            .unwrap();
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
        let mut dialog_snapshot = RawEvent::observed_at(
            session,
            90,
            "browser",
            "browser.page.snapshot",
            json!({
                "accessibility": {"nodes": [{
                    "backendDOMNodeId": 9,
                    "role": {"value": "dialog"},
                    "name": {"value": "Confirm deletion"}
                }]}
            }),
        );
        dialog_snapshot
            .artifact_refs
            .push(artifacts.put(b"dialog viewport fixture").unwrap());
        timeline.record(dialog_snapshot).unwrap();
        let store = ExperienceStore::open(session, timeline_path, artifact_root).unwrap();
        Fixture {
            _temp: temp,
            store,
            move_id,
            down_id,
            up_id,
            request_id,
            exception_id,
            runtime_span_id,
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
        assert!(
            around
                .derived_events
                .iter()
                .any(|event| event.kind == "perception.temporal.analysis")
        );

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

        for (event_id, expected_distance) in [
            (fixture.move_id, 4),
            (fixture.down_id, 5),
            (fixture.up_id, 15),
        ] {
            let element = execute_query(
                &fixture.store,
                ExperienceQuery::BrowserElementUnderPointer { event_id },
            )
            .unwrap();
            assert_eq!(element.relation["element"]["nodeName"], "BUTTON");
            assert_eq!(element.relation["element"]["accessibility"]["name"], "Save");
            assert_eq!(element.relation["snapshotDistanceNs"], expected_distance);
            assert_eq!(
                element.relation["coordinateSourceEventId"],
                fixture.move_id.to_string()
            );
        }

        let dialog = execute_query(
            &fixture.store,
            ExperienceQuery::LastDialog {
                text: Some("deletion".into()),
            },
        )
        .unwrap();
        assert_eq!(dialog.relation["dialog"]["role"], "dialog");
        assert_eq!(dialog.relation["dialog"]["name"], "Confirm deletion");
        assert_eq!(dialog.interval.start_ns, 20);
        assert_eq!(dialog.interval.end_ns, 90);

        let runtime = execute_query(
            &fixture.store,
            ExperienceQuery::RuntimeTrace {
                event_id: fixture.runtime_span_id,
                before_ms: 0,
                after_ms: 0,
            },
        )
        .unwrap();
        assert_eq!(
            runtime.relation["traceMemberEventIds"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(runtime.relation["temporalNeighborsAreCausal"], false);
        assert_eq!(
            runtime
                .observed_events
                .iter()
                .filter(|event| event.source == "runtime")
                .count(),
            2
        );
    }
}
