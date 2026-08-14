use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    event::{Provenance, RawEvent},
    experience::ExperienceStore,
    storage::ArtifactStore,
    temporal::{TemporalAnalysis, TemporalObservation},
};

pub const DEFAULT_VLM_PROMPT: &str = "Describe the visible difference between these frames.\n\nWhat appeared or disappeared?\nDid any element become clipped or obscured?\nDid relative alignment change?\nDid an object visibly jump?\nDo not decide whether the behavior is correct.";

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandVlmConfig {
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    pub model: String,
    pub model_version: String,
}

impl CommandVlmConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.program.as_os_str().is_empty(),
            "VLM adapter program must not be empty"
        );
        ensure!(!self.model.trim().is_empty(), "VLM model must not be empty");
        ensure!(
            !self.model_version.trim().is_empty(),
            "VLM model version must not be empty"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VlmInputFrame {
    pub role: String,
    pub host_monotonic_ns: u64,
    pub source_event_id: Uuid,
    pub frame_sha256: String,
    pub artifact_ref: String,
    pub artifact_path: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VlmFrameReference {
    pub role: String,
    pub host_monotonic_ns: u64,
    pub source_event_id: Uuid,
    pub frame_sha256: String,
    pub artifact_ref: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VlmRequest {
    pub protocol_version: u32,
    pub model: String,
    pub model_version: String,
    pub prompt: String,
    pub frames: Vec<VlmInputFrame>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VlmAdapterResponse {
    pub output: Value,
}

pub trait VlmAdapter {
    fn model(&self) -> &str;
    fn model_version(&self) -> &str;
    fn observe(&self, request: &VlmRequest) -> Result<VlmAdapterResponse>;
}

pub struct CommandVlmAdapter {
    config: CommandVlmConfig,
}

impl CommandVlmAdapter {
    pub fn new(config: CommandVlmConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self { config })
    }
}

impl VlmAdapter for CommandVlmAdapter {
    fn model(&self) -> &str {
        &self.config.model
    }

    fn model_version(&self) -> &str {
        &self.config.model_version
    }

    fn observe(&self, request: &VlmRequest) -> Result<VlmAdapterResponse> {
        let mut child = Command::new(&self.config.program)
            .args(&self.config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "start VLM adapter program {}",
                    self.config.program.display()
                )
            })?;
        child
            .stdin
            .take()
            .context("VLM adapter stdin was not piped")?
            .write_all(&serde_json::to_vec(request)?)?;
        let output = child.wait_with_output()?;
        ensure!(
            output.status.success(),
            "VLM adapter exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        serde_json::from_slice(&output.stdout).context("parse VLM adapter JSON response")
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInterpretedObservation {
    pub model: String,
    pub model_version: String,
    pub prompt: String,
    pub temporal_analysis_event_id: Uuid,
    pub trigger: TemporalObservation,
    pub input_frames: Vec<VlmFrameReference>,
    pub output: Value,
}

pub fn observe_temporal_event(
    store: &ExperienceStore,
    artifacts: &ArtifactStore,
    temporal_event: &RawEvent,
    observation_index: Option<usize>,
    prompt: &str,
    adapter: &dyn VlmAdapter,
) -> Result<(RawEvent, ModelInterpretedObservation)> {
    ensure!(
        temporal_event.kind == "perception.temporal.analysis",
        "VLM trigger must be a temporal analysis event"
    );
    ensure!(
        temporal_event.provenance == Provenance::Derived,
        "VLM trigger must retain derived provenance"
    );
    ensure!(!prompt.trim().is_empty(), "VLM prompt must not be empty");
    let analysis: TemporalAnalysis = serde_json::from_value(temporal_event.payload.clone())?;
    let trigger = select_trigger(&analysis, observation_index)?.clone();
    let before_ns = trigger.start_ns.saturating_sub(1);
    let during_ns = trigger.start_ns + (trigger.end_ns - trigger.start_ns) / 2;
    let frame_times = [
        ("before", before_ns),
        ("during", during_ns),
        ("after", trigger.end_ns),
    ];
    let mut frames = Vec::with_capacity(frame_times.len());
    for (role, timestamp) in frame_times {
        let frame = store.frame(timestamp, None)?;
        frames.push(VlmInputFrame {
            role: role.to_owned(),
            host_monotonic_ns: timestamp,
            source_event_id: frame.event_id,
            frame_sha256: frame.frame_sha256,
            artifact_path: artifacts.path(&frame.artifact_ref)?,
            artifact_ref: frame.artifact_ref,
        });
    }
    let request = VlmRequest {
        protocol_version: 1,
        model: adapter.model().to_owned(),
        model_version: adapter.model_version().to_owned(),
        prompt: prompt.to_owned(),
        frames: frames.clone(),
    };
    let response = adapter.observe(&request)?;
    let input_frames = frames
        .iter()
        .map(|frame| VlmFrameReference {
            role: frame.role.clone(),
            host_monotonic_ns: frame.host_monotonic_ns,
            source_event_id: frame.source_event_id,
            frame_sha256: frame.frame_sha256.clone(),
            artifact_ref: frame.artifact_ref.clone(),
        })
        .collect();
    let observation = ModelInterpretedObservation {
        model: request.model,
        model_version: request.model_version,
        prompt: request.prompt,
        temporal_analysis_event_id: temporal_event.id,
        trigger,
        input_frames,
        output: response.output,
    };
    let mut event = RawEvent::observed(
        temporal_event.session_id,
        "perception",
        "perception.vlm.observation",
        serde_json::to_value(&observation)?,
    );
    event.provenance = Provenance::ModelInterpreted;
    event.artifact_refs = observation
        .input_frames
        .iter()
        .map(|frame| frame.artifact_ref.clone())
        .collect();
    Ok((event, observation))
}

fn select_trigger(
    analysis: &TemporalAnalysis,
    observation_index: Option<usize>,
) -> Result<&TemporalObservation> {
    let selected = match observation_index {
        Some(index) => analysis
            .observations
            .get(index)
            .with_context(|| format!("temporal observation index {index} is out of range"))?,
        None => analysis
            .observations
            .iter()
            .find(|observation| is_interesting(observation))
            .context("temporal analysis contains no VLM-worthy observation")?,
    };
    ensure!(
        is_interesting(selected),
        "temporal observation {} does not warrant a VLM call",
        selected.kind
    );
    Ok(selected)
}

fn is_interesting(observation: &TemporalObservation) -> bool {
    matches!(
        observation.kind.as_str(),
        "display.region_updated_repeatedly"
            | "display.state_reverted"
            | "display.pixel_translation_detected"
            | "display.large_changed_region"
            | "input.multiple_visual_states_while_pointer_down"
            | "network.response_before_visual_response"
    ) || (observation.kind == "input.visual_response"
        && observation.payload.get("delayed").and_then(Value::as_bool) == Some(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        event::EventSink,
        framebuffer::{Framebuffer, PIXMAN_X8R8G8B8, Rect},
        timeline::TimelineStore,
    };
    use serde_json::json;

    fn analysis(kinds: &[&str]) -> TemporalAnalysis {
        TemporalAnalysis {
            start_ns: 1,
            end_ns: 10,
            config: Default::default(),
            display_change_facts: Vec::new(),
            observations: kinds
                .iter()
                .enumerate()
                .map(|(index, kind)| TemporalObservation {
                    kind: (*kind).to_owned(),
                    start_ns: index as u64 + 2,
                    end_ns: index as u64 + 3,
                    supporting_event_ids: Vec::new(),
                    payload: json!({}),
                })
                .collect(),
        }
    }

    #[test]
    fn trigger_filter_skips_directly_explained_cursor_and_no_response_events() {
        let analysis = analysis(&[
            "display.cursor_only_change",
            "input.pointer_move_without_display_response",
            "display.pixel_translation_detected",
        ]);
        assert_eq!(
            select_trigger(&analysis, None).unwrap().kind,
            "display.pixel_translation_detected"
        );
        assert!(select_trigger(&analysis, Some(0)).is_err());
    }

    #[test]
    fn trigger_filter_allows_only_delayed_visual_responses() {
        let mut analysis = analysis(&["input.visual_response", "input.visual_response"]);
        analysis.observations[0].payload = json!({"delayed": false});
        analysis.observations[1].payload = json!({"delayed": true});
        assert_eq!(select_trigger(&analysis, None).unwrap().start_ns, 3);
        assert!(select_trigger(&analysis, Some(0)).is_err());
    }

    #[test]
    fn adapter_config_requires_stable_model_identity() {
        let config = CommandVlmConfig {
            program: "adapter".into(),
            args: Vec::new(),
            model: "".into(),
            model_version: "2026-08-14".into(),
        };
        assert!(config.validate().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn command_adapter_uses_json_stdio_protocol_without_a_shell_wrapper() {
        let adapter = CommandVlmAdapter::new(CommandVlmConfig {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "cat >/dev/null; printf '%s' '{\"output\":{\"summary\":\"observed\"}}'".into(),
            ],
            model: "fixture-vlm".into(),
            model_version: "v1".into(),
        })
        .unwrap();
        let response = adapter
            .observe(&VlmRequest {
                protocol_version: 1,
                model: adapter.model().to_owned(),
                model_version: adapter.model_version().to_owned(),
                prompt: DEFAULT_VLM_PROMPT.to_owned(),
                frames: Vec::new(),
            })
            .unwrap();
        assert_eq!(response.output["summary"], "observed");
    }

    struct FakeAdapter;

    impl VlmAdapter for FakeAdapter {
        fn model(&self) -> &str {
            "fixture-vlm"
        }

        fn model_version(&self) -> &str {
            "v1"
        }

        fn observe(&self, request: &VlmRequest) -> Result<VlmAdapterResponse> {
            assert_eq!(request.frames.len(), 3);
            assert!(
                request
                    .frames
                    .iter()
                    .all(|frame| frame.artifact_path.is_file())
            );
            Ok(VlmAdapterResponse {
                output: json!({"description": "the marked object moved right"}),
            })
        }
    }

    #[test]
    fn model_observation_retains_raw_frames_and_separate_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let artifact_root = temp.path().join("artifacts");
        let artifacts = ArtifactStore::new(&artifact_root).unwrap();
        let timeline_path = temp.path().join("timeline.sqlite3");
        let timeline = TimelineStore::open(&timeline_path).unwrap();
        let session = Uuid::new_v4();
        let initial = vec![0_u8; 16];
        let first_update = vec![1_u8, 2, 3, 4];
        let second_update = vec![5_u8, 6, 7, 8];
        let initial_frame = Framebuffer::from_scanout(2, 2, 8, PIXMAN_X8R8G8B8, &initial).unwrap();
        let mut first_frame = initial_frame.clone();
        first_frame
            .apply_update(
                Rect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                4,
                PIXMAN_X8R8G8B8,
                &first_update,
            )
            .unwrap();
        let mut final_frame = first_frame.clone();
        final_frame
            .apply_update(
                Rect {
                    x: 1,
                    y: 1,
                    width: 1,
                    height: 1,
                },
                4,
                PIXMAN_X8R8G8B8,
                &second_update,
            )
            .unwrap();
        let mut scanout = RawEvent::observed_at(
            session,
            10,
            "display",
            "display.scanout",
            json!({
                "width": 2, "height": 2, "stride": 8,
                "pixmanFormat": PIXMAN_X8R8G8B8,
                "frameSha256": initial_frame.sha256()
            }),
        );
        scanout.artifact_refs.push(artifacts.put(&initial).unwrap());
        timeline.record(scanout).unwrap();
        for (timestamp, x, y, bytes, hash) in [
            (20, 0, 0, &first_update, first_frame.sha256()),
            (30, 1, 1, &second_update, final_frame.sha256()),
        ] {
            let mut update = RawEvent::observed_at(
                session,
                timestamp,
                "display",
                "display.update",
                json!({
                    "rect": {"x": x, "y": y, "width": 1, "height": 1},
                    "stride": 4, "pixmanFormat": PIXMAN_X8R8G8B8,
                    "frameSha256": hash
                }),
            );
            update.artifact_refs.push(artifacts.put(bytes).unwrap());
            timeline.record(update).unwrap();
        }
        let store = ExperienceStore::open(session, timeline_path, artifact_root).unwrap();
        let mut temporal_analysis = analysis(&["display.pixel_translation_detected"]);
        temporal_analysis.observations[0].start_ns = 20;
        temporal_analysis.observations[0].end_ns = 30;
        let mut temporal_event = RawEvent::observed_at(
            session,
            40,
            "perception",
            "perception.temporal.analysis",
            serde_json::to_value(temporal_analysis).unwrap(),
        );
        temporal_event.provenance = Provenance::Derived;
        let original = temporal_event.clone();

        let (event, observation) = observe_temporal_event(
            &store,
            &artifacts,
            &temporal_event,
            None,
            DEFAULT_VLM_PROMPT,
            &FakeAdapter,
        )
        .unwrap();

        assert_eq!(temporal_event.payload, original.payload);
        assert_eq!(temporal_event.provenance, Provenance::Derived);
        assert_eq!(event.provenance, Provenance::ModelInterpreted);
        assert_eq!(event.kind, "perception.vlm.observation");
        assert_eq!(event.artifact_refs.len(), 3);
        assert_eq!(observation.temporal_analysis_event_id, temporal_event.id);
        assert_eq!(observation.model, "fixture-vlm");
        assert_eq!(observation.model_version, "v1");
    }
}
