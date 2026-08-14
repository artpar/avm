use std::collections::{BTreeSet, VecDeque};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    event::RawEvent,
    framebuffer::{Framebuffer, Rect},
    storage::ArtifactStore,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TemporalConfig {
    pub response_window_ms: u64,
    pub delayed_response_ms: u64,
    pub repeated_region_window_ms: u64,
    pub repeated_region_min_updates: usize,
    pub flicker_window_ms: u64,
    pub large_changed_screen_ratio: f64,
    pub motion_min_distance_px: f64,
    pub motion_min_match_ratio: f64,
    pub component_min_pixels: u64,
}

impl Default for TemporalConfig {
    fn default() -> Self {
        Self {
            response_window_ms: 1_000,
            delayed_response_ms: 200,
            repeated_region_window_ms: 300,
            repeated_region_min_updates: 3,
            flicker_window_ms: 300,
            large_changed_screen_ratio: 0.20,
            motion_min_distance_px: 24.0,
            motion_min_match_ratio: 0.70,
            component_min_pixels: 4,
        }
    }
}

impl TemporalConfig {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.response_window_ms > 0,
            "response window must be positive"
        );
        ensure!(
            self.delayed_response_ms <= self.response_window_ms,
            "delayed response threshold exceeds response window"
        );
        ensure!(
            self.repeated_region_min_updates >= 2,
            "repeated region minimum must be at least two"
        );
        ensure!(
            (0.0..=1.0).contains(&self.large_changed_screen_ratio),
            "large changed-screen ratio must be between zero and one"
        );
        ensure!(
            (0.0..=1.0).contains(&self.motion_min_match_ratio),
            "motion match ratio must be between zero and one"
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PixelBounds {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayUpdateFact {
    pub event_id: Uuid,
    pub host_monotonic_ns: u64,
    pub announced_rect: PixelBounds,
    pub changed_bounds: Option<PixelBounds>,
    pub changed_pixels: u64,
    pub announced_pixels: u64,
    pub screen_pixels: u64,
    pub changed_ratio_in_announced_rect: f64,
    pub changed_ratio_of_screen: f64,
    pub resulting_frame_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemporalObservation {
    pub kind: String,
    pub start_ns: u64,
    pub end_ns: u64,
    pub supporting_event_ids: Vec<Uuid>,
    pub payload: Value,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemporalAnalysis {
    pub start_ns: u64,
    pub end_ns: u64,
    pub config: TemporalConfig,
    pub display_update_facts: Vec<DisplayUpdateFact>,
    pub observations: Vec<TemporalObservation>,
}

#[derive(Clone)]
struct FrameState {
    event_id: Uuid,
    timestamp: u64,
    hash: String,
}

#[derive(Clone, Debug)]
struct Component {
    bounds: PixelBounds,
    pixels: Vec<(u32, u32)>,
}

struct Difference {
    bounds: Option<PixelBounds>,
    changed_pixels: u64,
    components: Vec<Component>,
}

pub fn analyze_temporal(
    events: &[RawEvent],
    artifacts: &ArtifactStore,
    start_ns: u64,
    end_ns: u64,
    config: TemporalConfig,
) -> Result<TemporalAnalysis> {
    ensure!(start_ns <= end_ns, "temporal analysis start exceeds end");
    config.validate()?;
    let mut ordered = events
        .iter()
        .filter(|event| event.host_monotonic_ns <= end_ns)
        .collect::<Vec<_>>();
    ordered.sort_by_key(|event| (event.host_monotonic_ns, event.id));

    let mut frame: Option<Framebuffer> = None;
    let mut states = Vec::new();
    let mut facts = Vec::new();
    let mut observations = Vec::new();

    for event in &ordered {
        match event.kind.as_str() {
            "display.scanout" => {
                let bytes = artifacts.read(first_artifact(event)?)?;
                let next = Framebuffer::from_scanout(
                    u32_field(event, "width")?,
                    u32_field(event, "height")?,
                    u32_field(event, "stride")?,
                    u32_field(event, "pixmanFormat")?,
                    &bytes,
                )?;
                states.push(FrameState {
                    event_id: event.id,
                    timestamp: event.host_monotonic_ns,
                    hash: next.sha256(),
                });
                frame = Some(next);
            }
            "display.update" => {
                let before = frame
                    .as_ref()
                    .context("display update has no preceding scanout")?
                    .clone();
                let rect = event_rect(event)?;
                let bytes = artifacts.read(first_artifact(event)?)?;
                let current = frame.as_mut().expect("frame checked above");
                current.apply_update(
                    Rect {
                        x: rect.x as i32,
                        y: rect.y as i32,
                        width: rect.width as i32,
                        height: rect.height as i32,
                    },
                    u32_field(event, "stride")?,
                    u32_field(event, "pixmanFormat")?,
                    &bytes,
                )?;
                let hash = current.sha256();
                states.push(FrameState {
                    event_id: event.id,
                    timestamp: event.host_monotonic_ns,
                    hash: hash.clone(),
                });
                if event.host_monotonic_ns < start_ns {
                    continue;
                }
                let difference = pixel_difference(&before, current)?;
                let announced_pixels = u64::from(rect.width) * u64::from(rect.height);
                let screen_pixels = u64::from(current.width()) * u64::from(current.height());
                let fact = DisplayUpdateFact {
                    event_id: event.id,
                    host_monotonic_ns: event.host_monotonic_ns,
                    announced_rect: rect,
                    changed_bounds: difference.bounds,
                    changed_pixels: difference.changed_pixels,
                    announced_pixels,
                    screen_pixels,
                    changed_ratio_in_announced_rect: ratio(
                        difference.changed_pixels,
                        announced_pixels,
                    ),
                    changed_ratio_of_screen: ratio(difference.changed_pixels, screen_pixels),
                    resulting_frame_sha256: hash,
                };
                if fact.changed_ratio_of_screen >= config.large_changed_screen_ratio {
                    observations.push(observation(
                        "display.large_region_changed",
                        event.host_monotonic_ns,
                        event.host_monotonic_ns,
                        vec![event.id],
                        json!({
                            "changedPixels": fact.changed_pixels,
                            "screenPixels": fact.screen_pixels,
                            "changedRatioOfScreen": fact.changed_ratio_of_screen,
                            "changedBounds": fact.changed_bounds,
                        }),
                    ));
                }
                observations.extend(motion_observations(
                    event,
                    &before,
                    current,
                    &difference.components,
                    &config,
                ));
                facts.push(fact);
            }
            _ => {}
        }
    }

    derive_action_responses(
        &ordered,
        &facts,
        start_ns,
        end_ns,
        &config,
        &mut observations,
    );
    derive_repeated_regions(&facts, &config, &mut observations);
    derive_state_reversions(&states, start_ns, end_ns, &config, &mut observations);
    derive_multiple_states(&ordered, &states, start_ns, end_ns, &mut observations);
    derive_network_display_order(&ordered, &facts, start_ns, &config, &mut observations);
    observations.sort_by_key(|item| (item.start_ns, item.end_ns, item.kind.clone()));

    Ok(TemporalAnalysis {
        start_ns,
        end_ns,
        config,
        display_update_facts: facts,
        observations,
    })
}

fn derive_action_responses(
    events: &[&RawEvent],
    facts: &[DisplayUpdateFact],
    start_ns: u64,
    end_ns: u64,
    config: &TemporalConfig,
    observations: &mut Vec<TemporalObservation>,
) {
    let window_ns = config.response_window_ms.saturating_mul(1_000_000);
    for input in events.iter().copied().filter(|event| {
        event.source == "input"
            && event.host_monotonic_ns >= start_ns
            && event.host_monotonic_ns <= end_ns
    }) {
        let deadline = input.host_monotonic_ns.saturating_add(window_ns);
        let response = facts.iter().find(|fact| {
            fact.changed_pixels > 0
                && fact.host_monotonic_ns > input.host_monotonic_ns
                && fact.host_monotonic_ns <= deadline
        });
        match response {
            Some(response) => {
                let latency_ns = response.host_monotonic_ns - input.host_monotonic_ns;
                let latency_ms = latency_ns as f64 / 1_000_000.0;
                observations.push(observation(
                    "input.visual_response",
                    input.host_monotonic_ns,
                    response.host_monotonic_ns,
                    vec![input.id, response.event_id],
                    json!({
                        "actionKind": input.kind,
                        "latencyNs": latency_ns,
                        "latencyMs": latency_ms,
                        "delayed": latency_ns >= config.delayed_response_ms * 1_000_000,
                        "changedBounds": response.changed_bounds,
                    }),
                ));
            }
            None if end_ns >= deadline => observations.push(observation(
                if input.kind == "pointer.move" {
                    "input.pointer_move_without_display_response"
                } else {
                    "input.no_display_response"
                },
                input.host_monotonic_ns,
                deadline,
                vec![input.id],
                json!({
                    "actionKind": input.kind,
                    "waitedMs": config.response_window_ms,
                }),
            )),
            None => {}
        }
    }
}

fn derive_repeated_regions(
    facts: &[DisplayUpdateFact],
    config: &TemporalConfig,
    observations: &mut Vec<TemporalObservation>,
) {
    let window_ns = config.repeated_region_window_ms.saturating_mul(1_000_000);
    let mut index = 0;
    while index < facts.len() {
        let mut end = index + 1;
        let mut union = facts[index].announced_rect;
        while end < facts.len()
            && facts[end].host_monotonic_ns - facts[end - 1].host_monotonic_ns <= window_ns
            && intersection_over_union(union, facts[end].announced_rect) >= 0.50
        {
            union = union_bounds(union, facts[end].announced_rect);
            end += 1;
        }
        if end - index >= config.repeated_region_min_updates {
            observations.push(observation(
                "display.region_updated_repeatedly",
                facts[index].host_monotonic_ns,
                facts[end - 1].host_monotonic_ns,
                facts[index..end].iter().map(|fact| fact.event_id).collect(),
                json!({
                    "updateCount": end - index,
                    "region": union,
                    "maximumInterUpdateMs": config.repeated_region_window_ms,
                }),
            ));
        }
        index = end;
    }
}

fn derive_state_reversions(
    states: &[FrameState],
    start_ns: u64,
    end_ns: u64,
    config: &TemporalConfig,
    observations: &mut Vec<TemporalObservation>,
) {
    let window_ns = config.flicker_window_ms.saturating_mul(1_000_000);
    for triple in states.windows(3) {
        let [before, transient, restored] = triple else {
            unreachable!()
        };
        if restored.timestamp < start_ns || restored.timestamp > end_ns {
            continue;
        }
        if before.hash == restored.hash
            && before.hash != transient.hash
            && restored.timestamp - before.timestamp <= window_ns
        {
            observations.push(observation(
                "display.state_reverted",
                before.timestamp,
                restored.timestamp,
                vec![before.event_id, transient.event_id, restored.event_id],
                json!({
                    "transientDurationNs": restored.timestamp - transient.timestamp,
                    "pattern": "A-B-A",
                    "restoredFrameSha256": restored.hash,
                    "transientFrameSha256": transient.hash,
                }),
            ));
        }
    }
}

fn derive_multiple_states(
    events: &[&RawEvent],
    states: &[FrameState],
    start_ns: u64,
    end_ns: u64,
    observations: &mut Vec<TemporalObservation>,
) {
    let inputs = events
        .iter()
        .copied()
        .filter(|event| event.source == "input")
        .collect::<Vec<_>>();
    for (index, down) in inputs.iter().enumerate().filter(|(_, event)| {
        event.kind == "pointer.down"
            && event.host_monotonic_ns >= start_ns
            && event.host_monotonic_ns <= end_ns
    }) {
        let Some(up) = inputs[index + 1..]
            .iter()
            .find(|event| event.kind == "pointer.up")
        else {
            continue;
        };
        let during = states
            .iter()
            .filter(|state| {
                state.timestamp > down.host_monotonic_ns && state.timestamp < up.host_monotonic_ns
            })
            .collect::<Vec<_>>();
        let distinct = during
            .iter()
            .map(|state| state.hash.as_str())
            .collect::<BTreeSet<_>>();
        if distinct.len() >= 2 {
            let mut ids = vec![down.id];
            ids.extend(during.iter().map(|state| state.event_id));
            ids.push(up.id);
            observations.push(observation(
                "input.multiple_visual_states_while_pointer_down",
                down.host_monotonic_ns,
                up.host_monotonic_ns,
                ids,
                json!({
                    "distinctFrameCount": distinct.len(),
                    "displayEventCount": during.len(),
                }),
            ));
        }
    }
}

fn derive_network_display_order(
    events: &[&RawEvent],
    facts: &[DisplayUpdateFact],
    start_ns: u64,
    config: &TemporalConfig,
    observations: &mut Vec<TemporalObservation>,
) {
    let window_ns = config.response_window_ms.saturating_mul(1_000_000);
    for response in events.iter().copied().filter(|event| {
        event.host_monotonic_ns >= start_ns && event.kind == "browser.network.response"
    }) {
        if let Some(display) = facts.iter().find(|fact| {
            fact.changed_pixels > 0
                && fact.host_monotonic_ns > response.host_monotonic_ns
                && fact.host_monotonic_ns <= response.host_monotonic_ns.saturating_add(window_ns)
        }) {
            observations.push(observation(
                "display.visual_response_after_network_response",
                response.host_monotonic_ns,
                display.host_monotonic_ns,
                vec![response.id, display.event_id],
                json!({
                    "latencyNs": display.host_monotonic_ns - response.host_monotonic_ns,
                    "networkStatus": response.payload.get("status"),
                    "changedBounds": display.changed_bounds,
                }),
            ));
        }
    }
}

fn motion_observations(
    event: &RawEvent,
    before: &Framebuffer,
    after: &Framebuffer,
    components: &[Component],
    config: &TemporalConfig,
) -> Vec<TemporalObservation> {
    let components = components
        .iter()
        .filter(|component| component.pixels.len() as u64 >= config.component_min_pixels)
        .collect::<Vec<_>>();
    let mut best: Option<(usize, usize, f64, i64, i64)> = None;
    for first in 0..components.len() {
        for second in first + 1..components.len() {
            let a = &components[first];
            let b = &components[second];
            let dx = i64::from(b.bounds.x) - i64::from(a.bounds.x);
            let dy = i64::from(b.bounds.y) - i64::from(a.bounds.y);
            let distance = ((dx * dx + dy * dy) as f64).sqrt();
            if distance < config.motion_min_distance_px {
                continue;
            }
            let forward = translation_match(before, after, &a.pixels, dx, dy);
            let reverse = translation_match(before, after, &b.pixels, -dx, -dy);
            let (from, to, score, move_x, move_y) = if forward >= reverse {
                (first, second, forward, dx, dy)
            } else {
                (second, first, reverse, -dx, -dy)
            };
            if score >= config.motion_min_match_ratio
                && best.as_ref().is_none_or(|candidate| score > candidate.2)
            {
                best = Some((from, to, score, move_x, move_y));
            }
        }
    }
    best.into_iter()
        .map(|(from, to, score, dx, dy)| {
            observation(
                "display.pixel_translation_detected",
                event.host_monotonic_ns,
                event.host_monotonic_ns,
                vec![event.id],
                json!({
                    "fromBounds": components[from].bounds,
                    "toBounds": components[to].bounds,
                    "deltaX": dx,
                    "deltaY": dy,
                    "distancePx": ((dx * dx + dy * dy) as f64).sqrt(),
                    "matchRatio": score,
                    "method": "exact_pixel_translation_between_changed_components",
                }),
            )
        })
        .collect()
}

fn pixel_difference(before: &Framebuffer, after: &Framebuffer) -> Result<Difference> {
    ensure!(
        before.width() == after.width() && before.height() == after.height(),
        "frame dimensions changed without a scanout"
    );
    let width = before.width() as usize;
    let height = before.height() as usize;
    let mut changed = vec![false; width * height];
    let mut changed_pixels = 0_u64;
    let mut minimum_x = u32::MAX;
    let mut minimum_y = u32::MAX;
    let mut maximum_x = 0_u32;
    let mut maximum_y = 0_u32;
    for y in 0..height {
        for x in 0..width {
            if pixel(before, x as u32, y as u32) != pixel(after, x as u32, y as u32) {
                changed[y * width + x] = true;
                changed_pixels += 1;
                minimum_x = minimum_x.min(x as u32);
                minimum_y = minimum_y.min(y as u32);
                maximum_x = maximum_x.max(x as u32);
                maximum_y = maximum_y.max(y as u32);
            }
        }
    }
    let bounds = (changed_pixels > 0).then_some(PixelBounds {
        x: minimum_x,
        y: minimum_y,
        width: maximum_x - minimum_x + 1,
        height: maximum_y - minimum_y + 1,
    });
    let components = connected_components(&changed, width, height);
    Ok(Difference {
        bounds,
        changed_pixels,
        components,
    })
}

fn connected_components(mask: &[bool], width: usize, height: usize) -> Vec<Component> {
    let mut visited = vec![false; mask.len()];
    let mut components = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || visited[start] {
            continue;
        }
        let mut queue = VecDeque::from([start]);
        visited[start] = true;
        let mut pixels = Vec::new();
        while let Some(index) = queue.pop_front() {
            let x = index % width;
            let y = index / width;
            pixels.push((x as u32, y as u32));
            for (next_x, next_y) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if next_x >= width || next_y >= height {
                    continue;
                }
                let next = next_y * width + next_x;
                if mask[next] && !visited[next] {
                    visited[next] = true;
                    queue.push_back(next);
                }
            }
        }
        let minimum_x = pixels.iter().map(|(x, _)| *x).min().unwrap();
        let maximum_x = pixels.iter().map(|(x, _)| *x).max().unwrap();
        let minimum_y = pixels.iter().map(|(_, y)| *y).min().unwrap();
        let maximum_y = pixels.iter().map(|(_, y)| *y).max().unwrap();
        components.push(Component {
            bounds: PixelBounds {
                x: minimum_x,
                y: minimum_y,
                width: maximum_x - minimum_x + 1,
                height: maximum_y - minimum_y + 1,
            },
            pixels,
        });
    }
    components
}

fn translation_match(
    before: &Framebuffer,
    after: &Framebuffer,
    source: &[(u32, u32)],
    dx: i64,
    dy: i64,
) -> f64 {
    let mut comparable = 0_u64;
    let mut matched = 0_u64;
    for (x, y) in source {
        let target_x = i64::from(*x) + dx;
        let target_y = i64::from(*y) + dy;
        if target_x < 0
            || target_y < 0
            || target_x >= i64::from(after.width())
            || target_y >= i64::from(after.height())
        {
            continue;
        }
        comparable += 1;
        if pixel(before, *x, *y) == pixel(after, target_x as u32, target_y as u32) {
            matched += 1;
        }
    }
    ratio(matched, comparable)
}

fn pixel(frame: &Framebuffer, x: u32, y: u32) -> &[u8] {
    let start = y as usize * frame.stride() as usize + x as usize * 4;
    &frame.bytes()[start..start + 4]
}

fn event_rect(event: &RawEvent) -> Result<PixelBounds> {
    let rect = event
        .payload
        .get("rect")
        .context("display update has no rectangle")?;
    let value = |name: &str| -> Result<u32> {
        let signed = rect
            .get(name)
            .and_then(Value::as_i64)
            .with_context(|| format!("display update rectangle has no signed {name}"))?;
        u32::try_from(signed)
            .with_context(|| format!("display update rectangle {name} is negative"))
    };
    let bounds = PixelBounds {
        x: value("x")?,
        y: value("y")?,
        width: value("width")?,
        height: value("height")?,
    };
    ensure!(
        bounds.width > 0 && bounds.height > 0,
        "empty display update rectangle"
    );
    Ok(bounds)
}

fn first_artifact(event: &RawEvent) -> Result<&str> {
    event
        .artifact_refs
        .first()
        .map(String::as_str)
        .with_context(|| format!("{} has no framebuffer artifact", event.kind))
}

fn u32_field(event: &RawEvent, name: &str) -> Result<u32> {
    let value = event
        .payload
        .get(name)
        .and_then(Value::as_u64)
        .with_context(|| format!("{} has no unsigned {name}", event.kind))?;
    u32::try_from(value).with_context(|| format!("{name} exceeds u32"))
}

fn observation(
    kind: &str,
    start_ns: u64,
    end_ns: u64,
    supporting_event_ids: Vec<Uuid>,
    payload: Value,
) -> TemporalObservation {
    TemporalObservation {
        kind: kind.into(),
        start_ns,
        end_ns,
        supporting_event_ids,
        payload,
    }
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn intersection_over_union(a: PixelBounds, b: PixelBounds) -> f64 {
    let left = a.x.max(b.x);
    let top = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    if right <= left || bottom <= top {
        return 0.0;
    }
    let intersection = u64::from(right - left) * u64::from(bottom - top);
    let area_a = u64::from(a.width) * u64::from(a.height);
    let area_b = u64::from(b.width) * u64::from(b.height);
    ratio(intersection, area_a + area_b - intersection)
}

fn union_bounds(a: PixelBounds, b: PixelBounds) -> PixelBounds {
    let left = a.x.min(b.x);
    let top = a.y.min(b.y);
    let right = (a.x + a.width).max(b.x + b.width);
    let bottom = (a.y + a.height).max(b.y + b.height);
    PixelBounds {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framebuffer::PIXMAN_X8R8G8B8;

    fn scanout(session: Uuid, artifacts: &ArtifactStore, bytes: &[u8]) -> RawEvent {
        let frame = Framebuffer::from_scanout(40, 10, 160, PIXMAN_X8R8G8B8, bytes).unwrap();
        let mut event = RawEvent::observed_at(
            session,
            0,
            "display",
            "display.scanout",
            json!({
                "width": 40, "height": 10, "stride": 160,
                "pixmanFormat": PIXMAN_X8R8G8B8,
                "frameSha256": frame.sha256(),
            }),
        );
        event.artifact_refs.push(artifacts.put(bytes).unwrap());
        event
    }

    fn update(
        session: Uuid,
        artifacts: &ArtifactStore,
        timestamp: u64,
        x: i32,
        width: i32,
        pixels: &[u8],
    ) -> RawEvent {
        let mut event = RawEvent::observed_at(
            session,
            timestamp,
            "display",
            "display.update",
            json!({
                "rect": {"x": x, "y": 2, "width": width, "height": 2},
                "stride": width * 4, "pixmanFormat": PIXMAN_X8R8G8B8,
            }),
        );
        event.artifact_refs.push(artifacts.put(pixels).unwrap());
        event
    }

    #[test]
    fn derives_generic_delay_reversion_repetition_motion_and_no_response_facts() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(temp.path()).unwrap();
        let session = Uuid::new_v4();
        let mut initial = vec![0_u8; 40 * 10 * 4];
        for y in 2..4 {
            for x in 2..6 {
                let offset = (y * 40 + x) * 4;
                initial[offset..offset + 4].copy_from_slice(&[255, 255, 255, 0]);
            }
        }
        let mut moved_row = vec![0_u8; 32 * 2 * 4];
        for y in 0..2 {
            for x in 26..30 {
                let offset = (y * 32 + x) * 4;
                moved_row[offset..offset + 4].copy_from_slice(&[255, 255, 255, 0]);
            }
        }
        let cleared_row = vec![0_u8; 32 * 2 * 4];
        let input = RawEvent::observed_at(session, 1_000_000, "input", "pointer.down", json!({}));
        let network = RawEvent::observed_at(
            session,
            249_000_000,
            "network",
            "browser.network.response",
            json!({"status": 200}),
        );
        let moved = update(session, &artifacts, 251_000_000, 2, 32, &moved_row);
        let restored = update(session, &artifacts, 280_000_000, 2, 32, &cleared_row);
        let moved_again = update(session, &artifacts, 310_000_000, 2, 32, &moved_row);
        let up = RawEvent::observed_at(session, 350_000_000, "input", "pointer.up", json!({}));
        let no_response =
            RawEvent::observed_at(session, 500_000_000, "input", "pointer.move", json!({}));
        let analysis = analyze_temporal(
            &[
                scanout(session, &artifacts, &initial),
                input,
                network,
                moved,
                restored,
                moved_again,
                up,
                no_response,
            ],
            &artifacts,
            0,
            1_600_000_000,
            TemporalConfig::default(),
        )
        .unwrap();
        let kinds = analysis
            .observations
            .iter()
            .map(|item| item.kind.as_str())
            .collect::<BTreeSet<_>>();
        assert!(kinds.contains("input.visual_response"));
        assert!(analysis.observations.iter().any(|item| {
            item.kind == "input.visual_response" && item.payload["delayed"] == true
        }));
        assert!(kinds.contains("display.region_updated_repeatedly"));
        assert!(kinds.contains("display.state_reverted"));
        assert!(kinds.contains("display.pixel_translation_detected"));
        assert!(kinds.contains("input.multiple_visual_states_while_pointer_down"));
        assert!(kinds.contains("input.pointer_move_without_display_response"));
        assert!(kinds.contains("display.visual_response_after_network_response"));
    }
}
