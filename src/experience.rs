use std::{collections::BTreeSet, path::Path};

use anyhow::{Context, Result, ensure};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    event::RawEvent,
    framebuffer::{Framebuffer, Rect},
    storage::ArtifactStore,
    timeline::TimelineStore,
};

pub struct ExperienceStore {
    session_id: Uuid,
    timeline: TimelineStore,
    artifacts: ArtifactStore,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredFrame {
    pub event_id: Uuid,
    pub host_monotonic_ns: u64,
    pub width: u32,
    pub height: u32,
    pub frame_sha256: String,
    pub artifact_ref: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Replay {
    pub session_id: Uuid,
    pub start_ns: u64,
    pub end_ns: u64,
    pub input_events: Vec<RawEvent>,
    pub significant_display_events: Vec<RawEvent>,
    pub keyframes: Vec<StoredFrame>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOnlyFrame {
    pub event_id: Uuid,
    pub host_monotonic_ns: u64,
    pub width: u32,
    pub height: u32,
    pub frame_sha256: String,
    #[serde(skip)]
    pub png: Vec<u8>,
}

impl ExperienceStore {
    pub fn open(
        session_id: Uuid,
        timeline_path: impl AsRef<Path>,
        artifact_root: impl AsRef<Path>,
    ) -> Result<Self> {
        Ok(Self {
            session_id,
            timeline: TimelineStore::open(timeline_path)?,
            artifacts: ArtifactStore::new(artifact_root)?,
        })
    }

    pub fn open_read_only(
        session_id: Uuid,
        timeline_path: impl AsRef<Path>,
        artifact_root: impl AsRef<Path>,
    ) -> Result<Self> {
        Ok(Self {
            session_id,
            timeline: TimelineStore::open_read_only(timeline_path)?,
            artifacts: ArtifactStore::open_read_only(artifact_root)?,
        })
    }

    pub fn latest_ns(&self) -> Result<Option<u64>> {
        self.timeline.latest_ns(self.session_id)
    }

    pub fn event(&self, id: Uuid) -> Result<Option<RawEvent>> {
        self.timeline.event(id)
    }

    pub fn history(
        &self,
        start_ns: Option<u64>,
        end_ns: Option<u64>,
        sources: &[String],
    ) -> Result<Vec<RawEvent>> {
        let mut events = self.timeline.range(self.session_id, start_ns, end_ns)?;
        if !sources.is_empty() {
            events.retain(|event| sources.iter().any(|source| source == &event.source));
        }
        Ok(events)
    }

    pub fn frame(&self, at_ns: u64, output: Option<&Path>) -> Result<StoredFrame> {
        let events = self.timeline.all(self.session_id)?;
        let reconstructed = reconstruct_frame(&events, &self.artifacts, at_ns)?;
        if let Some(path) = output {
            reconstructed.framebuffer.save_png(path)?;
        }
        let png = reconstructed.framebuffer.png_bytes()?;
        let artifact_ref = self.artifacts.put(&png)?;
        Ok(StoredFrame {
            event_id: reconstructed.event_id,
            host_monotonic_ns: reconstructed.host_monotonic_ns,
            width: reconstructed.framebuffer.width(),
            height: reconstructed.framebuffer.height(),
            frame_sha256: reconstructed.framebuffer.sha256(),
            artifact_ref,
        })
    }

    /// Reconstruct a framebuffer without inserting a derived artifact into the
    /// supervisor-owned store. The read-only WebUI uses this path so viewing a
    /// run cannot mutate its evidence.
    pub fn frame_read_only(&self, at_ns: u64) -> Result<ReadOnlyFrame> {
        let events = self.timeline.all(self.session_id)?;
        let reconstructed = reconstruct_frame(&events, &self.artifacts, at_ns)?;
        Ok(ReadOnlyFrame {
            event_id: reconstructed.event_id,
            host_monotonic_ns: reconstructed.host_monotonic_ns,
            width: reconstructed.framebuffer.width(),
            height: reconstructed.framebuffer.height(),
            frame_sha256: reconstructed.framebuffer.sha256(),
            png: reconstructed.framebuffer.png_bytes()?,
        })
    }

    pub fn replay(&self, start_ns: u64, end_ns: u64) -> Result<Replay> {
        ensure!(start_ns <= end_ns, "replay start must not exceed end");
        let all = self.timeline.all(self.session_id)?;
        let interval = all
            .iter()
            .filter(|event| {
                event.host_monotonic_ns >= start_ns && event.host_monotonic_ns <= end_ns
            })
            .cloned()
            .collect::<Vec<_>>();
        let input_events = interval
            .iter()
            .filter(|event| event.source == "input")
            .cloned()
            .collect::<Vec<_>>();
        let display_events = interval
            .iter()
            .filter(|event| matches!(event.kind.as_str(), "display.scanout" | "display.update"))
            .cloned()
            .collect::<Vec<_>>();

        let mut frame_times = BTreeSet::new();
        if let Some(first) = display_events.first() {
            frame_times.insert(first.host_monotonic_ns);
        }
        for input in input_events.iter().filter(|event| {
            matches!(
                event.kind.as_str(),
                "pointer.down" | "pointer.up" | "key.up"
            )
        }) {
            if let Some(response) = display_events
                .iter()
                .find(|display| display.host_monotonic_ns > input.host_monotonic_ns)
            {
                frame_times.insert(response.host_monotonic_ns);
            }
        }
        if let Some(last) = display_events.last() {
            frame_times.insert(last.host_monotonic_ns);
        }
        let frame_times = compact_times(frame_times.into_iter().collect(), 12);
        let keyframes = frame_times
            .into_iter()
            .map(|timestamp| self.frame(timestamp, None))
            .collect::<Result<Vec<_>>>()?;
        let significant_ids = keyframes
            .iter()
            .map(|frame| frame.event_id)
            .collect::<BTreeSet<_>>();
        let significant_display_events = display_events
            .into_iter()
            .filter(|event| event.kind == "display.scanout" || significant_ids.contains(&event.id))
            .collect();

        Ok(Replay {
            session_id: self.session_id,
            start_ns,
            end_ns,
            input_events,
            significant_display_events,
            keyframes,
        })
    }
}

fn compact_times(times: Vec<u64>, limit: usize) -> Vec<u64> {
    if times.len() <= limit {
        return times;
    }
    (0..limit)
        .map(|index| {
            let source_index = index * (times.len() - 1) / (limit - 1);
            times[source_index]
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[derive(Clone, Debug)]
pub struct ReconstructedFrame {
    pub event_id: Uuid,
    pub host_monotonic_ns: u64,
    pub framebuffer: Framebuffer,
}

pub fn reconstruct_frame(
    events: &[RawEvent],
    artifacts: &ArtifactStore,
    at_ns: u64,
) -> Result<ReconstructedFrame> {
    let mut ordered = events
        .iter()
        .filter(|event| {
            event.host_monotonic_ns <= at_ns
                && matches!(event.kind.as_str(), "display.scanout" | "display.update")
        })
        .collect::<Vec<_>>();
    ordered.sort_by_key(|event| (event.host_monotonic_ns, event.id));

    let mut frame = None;
    let mut last_event = None;
    for event in ordered {
        let artifact_ref = event
            .artifact_refs
            .first()
            .with_context(|| format!("{} has no raw framebuffer artifact", event.kind))?;
        let bytes = artifacts.read(artifact_ref)?;
        match event.kind.as_str() {
            "display.scanout" => {
                frame = Some(Framebuffer::from_scanout(
                    u32_field(event, "width")?,
                    u32_field(event, "height")?,
                    u32_field(event, "stride")?,
                    u32_field(event, "pixmanFormat")?,
                    &bytes,
                )?);
            }
            "display.update" => {
                let current = frame
                    .as_mut()
                    .context("display update has no preceding scanout")?;
                let rect = event
                    .payload
                    .get("rect")
                    .context("display update has no rect")?;
                current.apply_update(
                    Rect {
                        x: i32_value(rect, "x")?,
                        y: i32_value(rect, "y")?,
                        width: i32_value(rect, "width")?,
                        height: i32_value(rect, "height")?,
                    },
                    u32_field(event, "stride")?,
                    u32_field(event, "pixmanFormat")?,
                    &bytes,
                )?;
            }
            _ => unreachable!(),
        }
        let current = frame.as_ref().context("frame reconstruction is empty")?;
        if let Some(expected) = event
            .payload
            .get("frameSha256")
            .and_then(|value| value.as_str())
        {
            ensure!(
                current.sha256() == expected,
                "reconstructed frame hash differs at event {}",
                event.id
            );
        }
        last_event = Some(event);
    }

    let event = last_event.context("no reconstructable framebuffer at requested time")?;
    Ok(ReconstructedFrame {
        event_id: event.id,
        host_monotonic_ns: event.host_monotonic_ns,
        framebuffer: frame.context("frame reconstruction is empty")?,
    })
}

fn u32_field(event: &RawEvent, name: &str) -> Result<u32> {
    let value = event
        .payload
        .get(name)
        .and_then(|value| value.as_u64())
        .with_context(|| format!("{} has no unsigned {name}", event.kind))?;
    u32::try_from(value).with_context(|| format!("{name} exceeds u32"))
}

fn i32_value(value: &serde_json::Value, name: &str) -> Result<i32> {
    let value = value
        .get(name)
        .and_then(|value| value.as_i64())
        .with_context(|| format!("rect has no signed {name}"))?;
    i32::try_from(value).with_context(|| format!("rect {name} exceeds i32"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventSink;
    use serde_json::json;

    #[test]
    fn reconstructs_scanout_and_update_from_immutable_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(temp.path().join("artifacts")).unwrap();
        let session = Uuid::new_v4();
        let initial = vec![0_u8; 16];
        let update = vec![1_u8, 2, 3, 4];
        let initial_frame =
            Framebuffer::from_scanout(2, 2, 8, crate::framebuffer::PIXMAN_X8R8G8B8, &initial)
                .unwrap();
        let mut final_frame = initial_frame.clone();
        final_frame
            .apply_update(
                Rect {
                    x: 1,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                4,
                crate::framebuffer::PIXMAN_X8R8G8B8,
                &update,
            )
            .unwrap();

        let mut scanout = RawEvent::observed_at(
            session,
            10,
            "display",
            "display.scanout",
            json!({
                "width": 2, "height": 2, "stride": 8,
                "pixmanFormat": crate::framebuffer::PIXMAN_X8R8G8B8,
                "frameSha256": initial_frame.sha256()
            }),
        );
        scanout.artifact_refs.push(artifacts.put(&initial).unwrap());
        let mut changed = RawEvent::observed_at(
            session,
            20,
            "display",
            "display.update",
            json!({
                "rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "stride": 4, "pixmanFormat": crate::framebuffer::PIXMAN_X8R8G8B8,
                "frameSha256": final_frame.sha256()
            }),
        );
        changed.artifact_refs.push(artifacts.put(&update).unwrap());

        let reconstructed = reconstruct_frame(&[changed, scanout], &artifacts, 20).unwrap();
        assert_eq!(reconstructed.framebuffer, final_frame);
        assert_eq!(reconstructed.host_monotonic_ns, 20);
    }

    #[test]
    fn replay_works_after_recording_has_finished_without_repeating_actions() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(temp.path().join("artifacts")).unwrap();
        let timeline_path = temp.path().join("timeline.sqlite3");
        let timeline = TimelineStore::open(&timeline_path).unwrap();
        let session = Uuid::new_v4();
        let initial = vec![0_u8; 16];
        let update = vec![9_u8, 8, 7, 6];
        let initial_frame =
            Framebuffer::from_scanout(2, 2, 8, crate::framebuffer::PIXMAN_X8R8G8B8, &initial)
                .unwrap();
        let mut final_frame = initial_frame.clone();
        final_frame
            .apply_update(
                Rect {
                    x: 0,
                    y: 1,
                    width: 1,
                    height: 1,
                },
                4,
                crate::framebuffer::PIXMAN_X8R8G8B8,
                &update,
            )
            .unwrap();
        let mut scanout = RawEvent::observed_at(
            session,
            1_000,
            "display",
            "display.scanout",
            json!({
                "width": 2, "height": 2, "stride": 8,
                "pixmanFormat": crate::framebuffer::PIXMAN_X8R8G8B8,
                "frameSha256": initial_frame.sha256()
            }),
        );
        scanout.artifact_refs.push(artifacts.put(&initial).unwrap());
        let down = RawEvent::observed_at(
            session,
            2_000,
            "input",
            "pointer.down",
            json!({"actionId": Uuid::new_v4()}),
        );
        let mut changed = RawEvent::observed_at(
            session,
            3_000,
            "display",
            "display.update",
            json!({
                "rect": {"x": 0, "y": 1, "width": 1, "height": 1},
                "stride": 4, "pixmanFormat": crate::framebuffer::PIXMAN_X8R8G8B8,
                "frameSha256": final_frame.sha256()
            }),
        );
        changed.artifact_refs.push(artifacts.put(&update).unwrap());
        let up = RawEvent::observed_at(
            session,
            4_000,
            "input",
            "pointer.up",
            json!({"actionId": Uuid::new_v4()}),
        );
        for event in [scanout, down, changed, up] {
            timeline.record(event).unwrap();
        }
        drop(timeline);
        drop(artifacts);

        let store =
            ExperienceStore::open(session, &timeline_path, temp.path().join("artifacts")).unwrap();
        let replay = store.replay(1_000, 4_000).unwrap();
        assert_eq!(replay.input_events.len(), 2);
        assert_eq!(replay.keyframes.len(), 2);
        assert_eq!(
            replay.keyframes.last().unwrap().frame_sha256,
            final_frame.sha256()
        );
        assert_eq!(
            store.history(None, None, &["input".into()]).unwrap().len(),
            2
        );
    }
}
