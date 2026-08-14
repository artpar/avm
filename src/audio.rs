use std::{
    collections::BTreeMap,
    os::unix::net::UnixStream,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;
use zbus::{Connection, proxy, zvariant::Fd};

use crate::{
    event::{EventSink, Provenance, RawEvent, monotonic_ns},
    storage::ArtifactStore,
};

const MAX_PCM_BYTES_PER_STREAM: usize = 64 * 1024 * 1024;

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
}
