use std::{
    collections::BTreeMap,
    io::Write,
    os::unix::net::UnixStream,
    path::Path,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
use zbus::{Connection, proxy, zvariant::Fd};

use crate::{
    event::{EventSink, Provenance, RawEvent, monotonic_ns},
    storage::ArtifactStore,
};

const MAX_PCM_BYTES_PER_STREAM: usize = 64 * 1024 * 1024;
pub const DEFAULT_TRANSCRIPTION_PROMPT: &str = "Transcribe only intelligible speech in this PCM interval. Preserve uncertainty and do not infer missing words. Do not decide whether the software behavior is correct.";
pub const DEFAULT_AUDIO_EVENT_PROMPT: &str = "Describe the audible events in this PCM interval, including silence, tones, clicks, alarms, music, or speech. Preserve uncertainty and do not decide whether the software behavior is correct.";

#[proxy(default_service = "org.qemu", interface = "org.qemu.Display1.Audio")]
trait QemuAudio {
    fn register_out_listener(&self, listener: Fd<'_>) -> zbus::Result<()>;
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PcmFormat {
    bits: u8,
    is_signed: bool,
    is_float: bool,
    frequency_hz: u32,
    channel_count: u8,
    bytes_per_frame: u32,
    bytes_per_second: u32,
    big_endian: bool,
}

struct StreamCapture {
    format: PcmFormat,
    enabled: bool,
    muted: bool,
    volume: Vec<u8>,
    started_ns: Option<u64>,
    ended_ns: Option<u64>,
    chunk_count: u64,
    received_bytes: u64,
    callback_processing_ns: u64,
    max_callback_processing_ns: u64,
    pcm: Vec<u8>,
    truncated: bool,
    finished: bool,
}

#[derive(Default)]
struct AudioState {
    streams: BTreeMap<u64, StreamCapture>,
    protocol_errors: Vec<String>,
}

#[derive(Clone)]
struct AudioOutListener {
    state: Arc<Mutex<AudioState>>,
}

#[zbus::interface(name = "org.qemu.Display1.AudioOutListener", spawn = false)]
impl AudioOutListener {
    // QEMU owns this D-Bus method signature.
    #[allow(clippy::too_many_arguments)]
    async fn init(
        &mut self,
        id: u64,
        bits: u8,
        is_signed: bool,
        is_float: bool,
        frequency_hz: u32,
        channel_count: u8,
        bytes_per_frame: u32,
        bytes_per_second: u32,
        big_endian: bool,
    ) {
        let mut state = self.state.lock().expect("audio mutex poisoned");
        if bits == 0
            || frequency_hz == 0
            || channel_count == 0
            || bytes_per_frame == 0
            || bytes_per_second == 0
        {
            state
                .protocol_errors
                .push(format!("stream {id} supplied an invalid PCM format"));
            return;
        }
        state.streams.insert(
            id,
            StreamCapture {
                format: PcmFormat {
                    bits,
                    is_signed,
                    is_float,
                    frequency_hz,
                    channel_count,
                    bytes_per_frame,
                    bytes_per_second,
                    big_endian,
                },
                enabled: false,
                muted: false,
                volume: Vec::new(),
                started_ns: None,
                ended_ns: None,
                chunk_count: 0,
                received_bytes: 0,
                callback_processing_ns: 0,
                max_callback_processing_ns: 0,
                pcm: Vec::new(),
                truncated: false,
                finished: false,
            },
        );
    }

    async fn fini(&mut self, id: u64) {
        if let Some(stream) = self
            .state
            .lock()
            .expect("audio mutex poisoned")
            .streams
            .get_mut(&id)
        {
            stream.finished = true;
        }
    }

    async fn set_enabled(&mut self, id: u64, enabled: bool) {
        if let Some(stream) = self
            .state
            .lock()
            .expect("audio mutex poisoned")
            .streams
            .get_mut(&id)
        {
            stream.enabled = enabled;
        }
    }

    async fn set_volume(&mut self, id: u64, muted: bool, volume: Vec<u8>) {
        if let Some(stream) = self
            .state
            .lock()
            .expect("audio mutex poisoned")
            .streams
            .get_mut(&id)
        {
            stream.muted = muted;
            stream.volume = volume;
        }
    }

    async fn write(&mut self, id: u64, data: Vec<u8>) {
        let timestamp = monotonic_ns();
        let mut state = self.state.lock().expect("audio mutex poisoned");
        let Some(stream) = state.streams.get_mut(&id) else {
            state
                .protocol_errors
                .push(format!("received PCM for unknown stream {id}"));
            return;
        };
        stream.started_ns.get_or_insert(timestamp);
        stream.ended_ns = Some(timestamp);
        stream.chunk_count += 1;
        stream.received_bytes = stream.received_bytes.saturating_add(data.len() as u64);
        let remaining = MAX_PCM_BYTES_PER_STREAM.saturating_sub(stream.pcm.len());
        let retained = remaining.min(data.len());
        stream.pcm.extend_from_slice(&data[..retained]);
        stream.truncated |= retained < data.len();
        let processing_ns = monotonic_ns().saturating_sub(timestamp);
        stream.callback_processing_ns = stream.callback_processing_ns.saturating_add(processing_ns);
        stream.max_callback_processing_ns = stream.max_callback_processing_ns.max(processing_ns);
    }

    #[zbus(property)]
    fn interfaces(&self) -> Vec<String> {
        Vec::new()
    }
}

pub struct AudioCapture {
    _bus: Connection,
    _listener_connection: Connection,
    session_id: Uuid,
    sink: Arc<dyn EventSink>,
    artifacts: Arc<ArtifactStore>,
    state: Arc<Mutex<AudioState>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioCaptureResult {
    pub stream_count: usize,
    pub interval_count: usize,
    pub received_bytes: u64,
    pub retained_bytes: u64,
    pub truncated_stream_count: usize,
    pub protocol_errors: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioInterpretationKind {
    Transcription,
    AudioEvent,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandAudioAdapterConfig {
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    pub model: String,
    pub model_version: String,
    pub kind: AudioInterpretationKind,
}

impl CommandAudioAdapterConfig {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.program.as_os_str().is_empty(),
            "audio adapter program must not be empty"
        );
        anyhow::ensure!(
            !self.model.trim().is_empty(),
            "audio adapter model must not be empty"
        );
        anyhow::ensure!(
            !self.model_version.trim().is_empty(),
            "audio adapter model version must not be empty"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInterpretationRequest {
    pub protocol_version: u32,
    pub model: String,
    pub model_version: String,
    pub kind: AudioInterpretationKind,
    pub prompt: String,
    pub raw_interval_event_id: Uuid,
    pub artifact_ref: String,
    pub artifact_path: PathBuf,
    pub interval: Value,
    pub format: Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AudioAdapterResponse {
    pub output: Value,
}

pub trait AudioAdapter {
    fn model(&self) -> &str;
    fn model_version(&self) -> &str;
    fn kind(&self) -> AudioInterpretationKind;
    fn interpret(&self, request: &AudioInterpretationRequest) -> Result<AudioAdapterResponse>;
}

pub struct CommandAudioAdapter {
    config: CommandAudioAdapterConfig,
}

impl CommandAudioAdapter {
    pub fn new(config: CommandAudioAdapterConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self { config })
    }
}

impl AudioAdapter for CommandAudioAdapter {
    fn model(&self) -> &str {
        &self.config.model
    }

    fn model_version(&self) -> &str {
        &self.config.model_version
    }

    fn kind(&self) -> AudioInterpretationKind {
        self.config.kind.clone()
    }

    fn interpret(&self, request: &AudioInterpretationRequest) -> Result<AudioAdapterResponse> {
        let mut child = Command::new(&self.config.program)
            .args(&self.config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "start audio adapter program {}",
                    self.config.program.display()
                )
            })?;
        child
            .stdin
            .take()
            .context("audio adapter stdin was not piped")?
            .write_all(&serde_json::to_vec(request)?)?;
        let output = child.wait_with_output()?;
        anyhow::ensure!(
            output.status.success(),
            "audio adapter exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        serde_json::from_slice(&output.stdout).context("parse audio adapter JSON response")
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInterpretation {
    pub model: String,
    pub model_version: String,
    pub kind: AudioInterpretationKind,
    pub prompt: String,
    pub raw_interval_event_id: Uuid,
    pub artifact_ref: String,
    pub output: Value,
}

pub fn interpret_audio_event(
    artifacts: &ArtifactStore,
    raw_event: &RawEvent,
    prompt: &str,
    adapter: &dyn AudioAdapter,
) -> Result<(RawEvent, AudioInterpretation)> {
    anyhow::ensure!(
        raw_event.kind == "audio.raw.interval",
        "audio interpretation requires a raw audio interval event"
    );
    anyhow::ensure!(
        raw_event.provenance == Provenance::Observed,
        "raw audio interval must retain observed provenance"
    );
    anyhow::ensure!(!prompt.trim().is_empty(), "audio prompt must not be empty");
    let artifact_ref = raw_event
        .artifact_refs
        .first()
        .context("raw audio interval has no PCM artifact")?
        .clone();
    artifacts.read(&artifact_ref)?;
    let request = AudioInterpretationRequest {
        protocol_version: 1,
        model: adapter.model().to_owned(),
        model_version: adapter.model_version().to_owned(),
        kind: adapter.kind(),
        prompt: prompt.to_owned(),
        raw_interval_event_id: raw_event.id,
        artifact_path: artifacts.path(&artifact_ref)?,
        artifact_ref: artifact_ref.clone(),
        interval: json!({
            "startNs": raw_event.payload.get("startNs"),
            "endNs": raw_event.payload.get("endNs")
        }),
        format: raw_event
            .payload
            .get("format")
            .cloned()
            .context("raw audio interval has no PCM format")?,
    };
    let response = adapter.interpret(&request)?;
    let interpretation = AudioInterpretation {
        model: request.model,
        model_version: request.model_version,
        kind: request.kind,
        prompt: request.prompt,
        raw_interval_event_id: request.raw_interval_event_id,
        artifact_ref: request.artifact_ref,
        output: response.output,
    };
    let mut event = RawEvent::observed(
        raw_event.session_id,
        "perception",
        "perception.audio.interpretation",
        serde_json::to_value(&interpretation)?,
    );
    event.provenance = Provenance::ModelInterpreted;
    event.artifact_refs.push(artifact_ref);
    Ok((event, interpretation))
}

impl AudioCapture {
    pub async fn connect(
        bus_socket: &Path,
        session_id: Uuid,
        sink: Arc<dyn EventSink>,
        artifacts: Arc<ArtifactStore>,
    ) -> Result<Self> {
        let address = format!("unix:path={}", bus_socket.display());
        let bus = zbus::connection::Builder::address(address.as_str())?
            .build()
            .await
            .context("connect private display bus for audio")?;
        let audio = QemuAudioProxy::builder(&bus)
            .path("/org/qemu/Display1/Audio")?
            .build()
            .await?;
        let state = Arc::new(Mutex::new(AudioState::default()));
        let listener = AudioOutListener {
            state: state.clone(),
        };
        let (qemu_end, listener_end) = UnixStream::pair()?;
        audio
            .register_out_listener(Fd::from(&qemu_end))
            .await
            .context("register QEMU audio output listener")?;
        let listener_connection = zbus::connection::Builder::async_io_unix_stream(listener_end)
            .p2p()
            .serve_at("/org/qemu/Display1/AudioOutListener", listener)?
            .build()
            .await
            .context("serve QEMU audio output listener")?;
        Ok(Self {
            _bus: bus,
            _listener_connection: listener_connection,
            session_id,
            sink,
            artifacts,
            state,
        })
    }

    pub async fn observe(self, duration: Duration) -> Result<AudioCaptureResult> {
        tokio::time::sleep(duration).await;
        self.finish()
    }

    fn finish(self) -> Result<AudioCaptureResult> {
        let mut state = self.state.lock().expect("audio mutex poisoned");
        finalize_audio(
            self.session_id,
            self.sink.as_ref(),
            self.artifacts.as_ref(),
            &mut state,
        )
    }
}

fn finalize_audio(
    session_id: Uuid,
    sink: &dyn EventSink,
    artifacts: &ArtifactStore,
    state: &mut AudioState,
) -> Result<AudioCaptureResult> {
    let mut interval_count = 0;
    let mut received_bytes = 0_u64;
    let mut retained_bytes = 0_u64;
    let mut truncated_stream_count = 0;
    for (stream_id, stream) in &state.streams {
        received_bytes = received_bytes.saturating_add(stream.received_bytes);
        retained_bytes = retained_bytes.saturating_add(stream.pcm.len() as u64);
        truncated_stream_count += usize::from(stream.truncated);
        let (Some(start_ns), Some(end_ns)) = (stream.started_ns, stream.ended_ns) else {
            continue;
        };
        let artifact_ref = artifacts.put(&stream.pcm)?;
        let duration_ms = if stream.format.bytes_per_second == 0 {
            None
        } else {
            Some(stream.pcm.len() as f64 * 1000.0 / stream.format.bytes_per_second as f64)
        };
        let mut interval = RawEvent::observed_at(
            session_id,
            start_ns,
            "audio",
            "audio.raw.interval",
            json!({
                "streamId": stream_id,
                "startNs": start_ns,
                "endNs": end_ns,
                "format": stream.format,
                "chunkCount": stream.chunk_count,
                "receivedByteLength": stream.received_bytes,
                "retainedByteLength": stream.pcm.len(),
                "callbackProcessingNs": stream.callback_processing_ns,
                "maxCallbackProcessingNs": stream.max_callback_processing_ns,
                "durationMsFromRetainedPcm": duration_ms,
                "truncated": stream.truncated,
                "enabledAtEnd": stream.enabled,
                "mutedAtEnd": stream.muted,
                "volumeAtEnd": stream.volume,
                "finished": stream.finished
            }),
        );
        interval.artifact_refs.push(artifact_ref.clone());
        let interval_id = interval.id;
        sink.record(interval)?;

        let waveform = waveform_metadata(&stream.format, &stream.pcm);
        let mut waveform_event = RawEvent::observed_at(
            session_id,
            end_ns,
            "audio",
            "audio.waveform.metadata",
            json!({
                "streamId": stream_id,
                "rawIntervalEventId": interval_id,
                "rawArtifactRef": artifact_ref,
                "startNs": start_ns,
                "endNs": end_ns,
                "sampleFrameCount": stream.pcm.len() as u64 / stream.format.bytes_per_frame as u64,
                "peakNormalized": waveform.peak,
                "rmsNormalized": waveform.rms,
                "amplitudeCalculation": waveform.calculation
            }),
        );
        waveform_event.provenance = Provenance::Derived;
        sink.record(waveform_event)?;
        interval_count += 1;
    }
    Ok(AudioCaptureResult {
        stream_count: state.streams.len(),
        interval_count,
        received_bytes,
        retained_bytes,
        truncated_stream_count,
        protocol_errors: std::mem::take(&mut state.protocol_errors),
    })
}

struct WaveformMetadata {
    peak: Option<f64>,
    rms: Option<f64>,
    calculation: &'static str,
}

fn waveform_metadata(format: &PcmFormat, pcm: &[u8]) -> WaveformMetadata {
    if format.bits == 16 && format.is_signed && !format.is_float {
        let samples = pcm
            .chunks_exact(2)
            .map(|chunk| {
                let bytes = [chunk[0], chunk[1]];
                (if format.big_endian {
                    i16::from_be_bytes(bytes)
                } else {
                    i16::from_le_bytes(bytes)
                }) as f64
                    / 32768.0
            })
            .collect::<Vec<_>>();
        return amplitudes(&samples, "signed_16_bit_pcm");
    }
    if format.bits == 8 && !format.is_signed && !format.is_float {
        let samples = pcm
            .iter()
            .map(|sample| (*sample as f64 - 128.0) / 128.0)
            .collect::<Vec<_>>();
        return amplitudes(&samples, "unsigned_8_bit_pcm");
    }
    WaveformMetadata {
        peak: None,
        rms: None,
        calculation: "unsupported_pcm_amplitude_format",
    }
}

fn amplitudes(samples: &[f64], calculation: &'static str) -> WaveformMetadata {
    if samples.is_empty() {
        return WaveformMetadata {
            peak: Some(0.0),
            rms: Some(0.0),
            calculation,
        };
    }
    let peak = samples
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0, f64::max);
    let mean_square =
        samples.iter().map(|sample| sample * sample).sum::<f64>() / samples.len() as f64;
    WaveformMetadata {
        peak: Some(peak),
        rms: Some(mean_square.sqrt()),
        calculation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::MemoryEventSink;

    fn signed_16_format() -> PcmFormat {
        PcmFormat {
            bits: 16,
            is_signed: true,
            is_float: false,
            frequency_hz: 48_000,
            channel_count: 1,
            bytes_per_frame: 2,
            bytes_per_second: 96_000,
            big_endian: false,
        }
    }

    #[test]
    fn computes_normalized_pcm_waveform_metadata() {
        let pcm = [0_i16, i16::MAX, i16::MIN]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let waveform = waveform_metadata(&signed_16_format(), &pcm);
        assert_eq!(waveform.peak, Some(1.0));
        assert!(waveform.rms.unwrap() > 0.81);
        assert_eq!(waveform.calculation, "signed_16_bit_pcm");
    }

    #[test]
    fn finalizes_one_raw_interval_and_separate_derived_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = Arc::new(ArtifactStore::new(temp.path()).unwrap());
        let sink = Arc::new(MemoryEventSink::default());
        let state = Arc::new(Mutex::new(AudioState::default()));
        state.lock().unwrap().streams.insert(
            9,
            StreamCapture {
                format: signed_16_format(),
                enabled: true,
                muted: false,
                volume: vec![255],
                started_ns: Some(100),
                ended_ns: Some(200),
                chunk_count: 2,
                received_bytes: 4,
                callback_processing_ns: 40,
                max_callback_processing_ns: 30,
                pcm: vec![0, 0, 255, 127],
                truncated: false,
                finished: true,
            },
        );
        let result = finalize_audio(
            Uuid::new_v4(),
            sink.as_ref(),
            artifacts.as_ref(),
            &mut state.lock().unwrap(),
        )
        .unwrap();
        assert_eq!(result.interval_count, 1);
        let events = sink.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, "audio.raw.interval");
        assert_eq!(events[0].artifact_refs.len(), 1);
        assert_eq!(events[1].kind, "audio.waveform.metadata");
        assert_eq!(events[1].provenance, Provenance::Derived);
    }

    struct FakeAudioAdapter;

    impl AudioAdapter for FakeAudioAdapter {
        fn model(&self) -> &str {
            "fixture/audio"
        }

        fn model_version(&self) -> &str {
            "v1"
        }

        fn kind(&self) -> AudioInterpretationKind {
            AudioInterpretationKind::AudioEvent
        }

        fn interpret(&self, request: &AudioInterpretationRequest) -> Result<AudioAdapterResponse> {
            assert!(request.artifact_path.is_file());
            Ok(AudioAdapterResponse {
                output: json!({"event": "tone"}),
            })
        }
    }

    #[test]
    fn keeps_optional_audio_interpretation_separate_from_raw_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(temp.path()).unwrap();
        let artifact_ref = artifacts.put(&[0, 0, 255, 127]).unwrap();
        let mut raw = RawEvent::observed(
            Uuid::new_v4(),
            "audio",
            "audio.raw.interval",
            json!({
                "startNs": 100,
                "endNs": 200,
                "format": signed_16_format()
            }),
        );
        raw.artifact_refs.push(artifact_ref.clone());
        let (event, interpretation) = interpret_audio_event(
            &artifacts,
            &raw,
            DEFAULT_AUDIO_EVENT_PROMPT,
            &FakeAudioAdapter,
        )
        .unwrap();
        assert_eq!(event.provenance, Provenance::ModelInterpreted);
        assert_eq!(event.artifact_refs, vec![artifact_ref]);
        assert_eq!(interpretation.output["event"], "tone");
        assert!(event.payload.get("artifactPath").is_none());
    }
}
