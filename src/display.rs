use std::{
    os::unix::net::UnixStream,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde_json::json;
use tokio::{sync::Notify, time::timeout};
use uuid::Uuid;
use zbus::{Connection, proxy, zvariant::Fd};

use crate::{
    event::{EventSink, RawEvent, monotonic_ns},
    framebuffer::{Framebuffer, Rect},
    storage::ArtifactStore,
};

#[proxy(default_service = "org.qemu", interface = "org.qemu.Display1.Console")]
trait QemuConsole {
    fn register_listener(&self, listener: Fd<'_>) -> zbus::Result<()>;
}

#[proxy(default_service = "org.qemu", interface = "org.qemu.Display1.Mouse")]
trait QemuMouse {
    fn press(&self, button: u32) -> zbus::Result<()>;
    fn release(&self, button: u32) -> zbus::Result<()>;
    fn set_abs_position(&self, x: u32, y: u32) -> zbus::Result<()>;
    fn rel_motion(&self, dx: i32, dy: i32) -> zbus::Result<()>;
    #[zbus(property)]
    fn is_absolute(&self) -> zbus::Result<bool>;
}

#[proxy(default_service = "org.qemu", interface = "org.qemu.Display1.Keyboard")]
trait QemuKeyboard {
    fn press(&self, keycode: u32) -> zbus::Result<()>;
    fn release(&self, keycode: u32) -> zbus::Result<()>;
}

#[derive(Clone, Copy, Debug)]
#[repr(u32)]
pub enum MouseButton {
    Left = 0,
    Middle = 1,
    Right = 2,
    WheelUp = 3,
    WheelDown = 4,
}

#[derive(Default)]
struct DisplayState {
    framebuffer: Option<Framebuffer>,
    last_scanout_ns: Option<u64>,
    last_display_ns: Option<u64>,
    display_timestamps: Vec<u64>,
    display_callbacks_in_flight: usize,
    last_error: Option<String>,
}

#[derive(Clone)]
struct Listener {
    session_id: Uuid,
    sink: Arc<dyn EventSink>,
    artifacts: Arc<ArtifactStore>,
    state: Arc<Mutex<DisplayState>>,
    changed: Arc<Notify>,
}

impl Listener {
    fn begin_display_callback(&self, timestamp: u64, is_scanout: bool) {
        let mut state = self.state.lock().expect("display mutex poisoned");
        state.display_callbacks_in_flight += 1;
        if is_scanout {
            state.last_scanout_ns = Some(timestamp);
        }
        drop(state);
        self.changed.notify_waiters();
    }

    fn emit(&self, source: &str, kind: &str, payload: serde_json::Value) {
        self.emit_at(monotonic_ns(), source, kind, payload);
    }

    fn emit_at(&self, timestamp: u64, source: &str, kind: &str, payload: serde_json::Value) {
        self.emit_at_with_artifacts(timestamp, source, kind, payload, Vec::new());
    }

    fn emit_at_with_artifacts(
        &self,
        timestamp: u64,
        source: &str,
        kind: &str,
        payload: serde_json::Value,
        artifact_refs: Vec<String>,
    ) {
        let mut event = RawEvent::observed_at(self.session_id, timestamp, source, kind, payload);
        event.artifact_refs = artifact_refs;
        if let Err(error) = self.sink.record(event) {
            self.state
                .lock()
                .expect("display mutex poisoned")
                .last_error = Some(error.to_string());
        }
    }

    fn accept_frame(
        &self,
        timestamp: u64,
        kind: &str,
        payload: serde_json::Value,
        artifact_refs: Vec<String>,
    ) {
        self.emit_at_with_artifacts(timestamp, "display", kind, payload, artifact_refs);
        let mut state = self.state.lock().expect("display mutex poisoned");
        state.last_display_ns = Some(timestamp);
        state.display_timestamps.push(timestamp);
        state.display_callbacks_in_flight -= 1;
        drop(state);
        self.changed.notify_waiters();
    }

    fn reject(&self, timestamp: u64, kind: &str, error: anyhow::Error) {
        self.emit_at(
            timestamp,
            "display",
            kind,
            json!({"error": error.to_string()}),
        );
        self.state
            .lock()
            .expect("display mutex poisoned")
            .last_error = Some(error.to_string());
        self.state
            .lock()
            .expect("display mutex poisoned")
            .display_callbacks_in_flight -= 1;
        self.changed.notify_waiters();
    }

    fn require_full_scanout(
        &self,
        timestamp: u64,
        error: anyhow::Error,
        rect: Rect,
        artifact_ref: String,
    ) {
        self.emit_at_with_artifacts(
            timestamp,
            "display",
            "display.update_rejected",
            json!({
                "error": error.to_string(),
                "rect": {
                    "x": rect.x, "y": rect.y,
                    "width": rect.width, "height": rect.height
                },
                "recovery": "awaiting_full_scanout"
            }),
            vec![artifact_ref],
        );
        let mut state = self.state.lock().expect("display mutex poisoned");
        state.framebuffer = None;
        state.last_scanout_ns = None;
        state.last_error = None;
        state.display_callbacks_in_flight -= 1;
        drop(state);
        self.changed.notify_waiters();
    }
}

#[zbus::interface(name = "org.qemu.Display1.Listener", spawn = false)]
impl Listener {
    async fn scanout(
        &mut self,
        width: u32,
        height: u32,
        stride: u32,
        pixman_format: u32,
        data: Vec<u8>,
    ) {
        let timestamp = monotonic_ns();
        self.begin_display_callback(timestamp, true);
        match Framebuffer::from_scanout(width, height, stride, pixman_format, &data) {
            Ok(frame) => {
                let hash = frame.sha256();
                let artifact = match self.artifacts.put(frame.bytes()) {
                    Ok(artifact) => artifact,
                    Err(error) => {
                        self.reject(timestamp, "display.scanout_rejected", error);
                        return;
                    }
                };
                let mut state = self.state.lock().expect("display mutex poisoned");
                state.framebuffer = Some(frame);
                state.last_error = None;
                drop(state);
                self.accept_frame(
                    timestamp,
                    "display.scanout",
                    json!({
                        "width": width, "height": height, "stride": stride,
                        "pixmanFormat": pixman_format, "frameSha256": hash
                    }),
                    vec![artifact],
                );
            }
            Err(error) => self.reject(timestamp, "display.scanout_rejected", error),
        }
    }

    async fn update(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        stride: u32,
        pixman_format: u32,
        data: Vec<u8>,
    ) {
        let timestamp = monotonic_ns();
        self.begin_display_callback(timestamp, false);
        let rect = Rect {
            x,
            y,
            width,
            height,
        };
        let artifact = (|| -> Result<String> {
            let height = usize::try_from(height).context("negative update height")?;
            let byte_length = (stride as usize)
                .checked_mul(height)
                .context("update artifact byte length overflow")?;
            let bytes = data
                .get(..byte_length)
                .context("update data is shorter than its stride and height")?;
            self.artifacts.put(bytes)
        })();
        let artifact = match artifact {
            Ok(artifact) => artifact,
            Err(error) => {
                self.reject(timestamp, "display.update_rejected", error);
                return;
            }
        };
        let result = {
            let mut state = self.state.lock().expect("display mutex poisoned");
            match state.framebuffer.as_mut() {
                Some(frame) => frame
                    .apply_update(rect, stride, pixman_format, &data)
                    .map(|_| frame.sha256()),
                None => Err(anyhow::anyhow!("update arrived before scanout")),
            }
        };
        match result {
            Ok(hash) => self.accept_frame(
                timestamp,
                "display.update",
                json!({
                    "rect": {"x": x, "y": y, "width": width, "height": height},
                    "stride": stride, "pixmanFormat": pixman_format, "frameSha256": hash
                }),
                vec![artifact],
            ),
            Err(error) => self.require_full_scanout(timestamp, error, rect, artifact),
        }
    }

    #[zbus(name = "ScanoutDMABUF")]
    async fn scanout_dmabuf(
        &mut self,
        _fd: Fd<'_>,
        width: u32,
        height: u32,
        stride: u32,
        fourcc: u32,
        modifier: u64,
        y0_top: bool,
    ) -> zbus::fdo::Result<()> {
        self.emit(
            "display",
            "display.scanout_dmabuf_unsupported",
            json!({
                "width": width, "height": height, "stride": stride, "fourcc": fourcc,
                "modifier": modifier, "y0Top": y0_top
            }),
        );
        Err(zbus::fdo::Error::NotSupported(
            "DMABUF disabled for correctness-first capture".into(),
        ))
    }

    #[zbus(name = "UpdateDMABUF")]
    async fn update_dmabuf(
        &mut self,
        _x: i32,
        _y: i32,
        _width: i32,
        _height: i32,
    ) -> zbus::fdo::Result<()> {
        Err(zbus::fdo::Error::NotSupported(
            "no active DMABUF scanout".into(),
        ))
    }

    async fn disable(&mut self) {
        let mut state = self.state.lock().expect("display mutex poisoned");
        state.framebuffer = None;
        drop(state);
        self.emit("display", "display.disable", json!({}));
    }

    async fn mouse_set(&mut self, x: i32, y: i32, on: i32) {
        self.emit(
            "cursor",
            "cursor.position",
            json!({"x": x, "y": y, "visible": on != 0}),
        );
    }

    async fn cursor_define(
        &mut self,
        width: i32,
        height: i32,
        hot_x: i32,
        hot_y: i32,
        data: Vec<u8>,
    ) {
        match self.artifacts.put(&data) {
            Ok(artifact) => self.emit_at_with_artifacts(
                monotonic_ns(),
                "cursor",
                "cursor.define",
                json!({
                    "width": width, "height": height, "hotX": hot_x, "hotY": hot_y,
                    "byteLength": data.len()
                }),
                vec![artifact],
            ),
            Err(error) => self.emit(
                "cursor",
                "cursor.define_rejected",
                json!({"error": error.to_string()}),
            ),
        }
    }

    #[zbus(property)]
    fn interfaces(&self) -> Vec<String> {
        Vec::new()
    }
}

pub struct HostComputer {
    _bus: Connection,
    _listener_connection: Connection,
    mouse: QemuMouseProxy<'static>,
    keyboard: QemuKeyboardProxy<'static>,
    session_id: Uuid,
    sink: Arc<dyn EventSink>,
    state: Arc<Mutex<DisplayState>>,
    changed: Arc<Notify>,
}

impl HostComputer {
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
            .context("connect private display bus")?;
        let path = "/org/qemu/Display1/Console_0";
        let console = QemuConsoleProxy::builder(&bus).path(path)?.build().await?;
        let mouse = QemuMouseProxy::builder(&bus)
            .path(path)?
            .build()
            .await?
            .to_owned();
        let keyboard = QemuKeyboardProxy::builder(&bus)
            .path(path)?
            .build()
            .await?
            .to_owned();
        let state = Arc::new(Mutex::new(DisplayState::default()));
        let changed = Arc::new(Notify::new());
        let listener = Listener {
            session_id,
            sink: sink.clone(),
            artifacts,
            state: state.clone(),
            changed: changed.clone(),
        };
        let (qemu_end, listener_end) = UnixStream::pair()?;
        console
            .register_listener(Fd::from(&qemu_end))
            .await
            .context("register display listener")?;
        let listener_connection = zbus::connection::Builder::async_io_unix_stream(listener_end)
            .p2p()
            .serve_at("/org/qemu/Display1/Listener", listener)?
            .build()
            .await
            .context("serve display listener")?;
        Ok(Self {
            _bus: bus,
            _listener_connection: listener_connection,
            mouse,
            keyboard,
            session_id,
            sink,
            state,
            changed,
        })
    }

    pub async fn wait_for_frame(&self, duration: Duration) -> Result<()> {
        timeout(duration, async {
            loop {
                let notified = self.changed.notified();
                {
                    let state = self.state.lock().expect("display mutex poisoned");
                    if let Some(error) = &state.last_error {
                        bail!(error.clone());
                    }
                    if state.framebuffer.is_some() {
                        return Ok(());
                    }
                }
                notified.await;
            }
        })
        .await
        .context("timed out waiting for initial framebuffer")?
    }

    pub async fn wait_for_stable_scanout(
        &self,
        duration: Duration,
        quiet_period: Duration,
    ) -> Result<()> {
        self.wait_for_stable_frame_size(duration, quiet_period, 1, 1)
            .await
    }

    pub async fn wait_for_stable_frame_size(
        &self,
        duration: Duration,
        quiet_period: Duration,
        minimum_width: u32,
        minimum_height: u32,
    ) -> Result<()> {
        timeout(duration, async {
            loop {
                let notified = self.changed.notified();
                let remaining = {
                    let state = self.state.lock().expect("display mutex poisoned");
                    if let Some(error) = &state.last_error {
                        bail!(error.clone());
                    }
                    stable_frame_remaining(
                        &state,
                        monotonic_ns(),
                        quiet_period,
                        minimum_width,
                        minimum_height,
                    )
                };
                match remaining {
                    Some(remaining) if remaining.is_zero() => return Ok(()),
                    Some(remaining) => {
                        tokio::select! {
                            _ = notified => {},
                            _ = tokio::time::sleep(remaining) => {},
                        }
                    }
                    None => notified.await,
                }
            }
        })
        .await
        .with_context(|| {
            format!(
                "timed out waiting for stable framebuffer at least {minimum_width}x{minimum_height}"
            )
        })?
    }

    pub async fn wait_for_display_after(&self, after_ns: u64, duration: Duration) -> Result<()> {
        timeout(duration, async {
            loop {
                let notified = self.changed.notified();
                {
                    let state = self.state.lock().expect("display mutex poisoned");
                    if let Some(error) = &state.last_error {
                        bail!(error.clone());
                    }
                    if state
                        .last_display_ns
                        .is_some_and(|timestamp| timestamp > after_ns)
                    {
                        return Ok(());
                    }
                }
                notified.await;
            }
        })
        .await
        .context("timed out waiting for a display update after input")?
    }

    pub fn save_screenshot(&self, path: &Path) -> Result<String> {
        let state = self.state.lock().expect("display mutex poisoned");
        let frame = state
            .framebuffer
            .as_ref()
            .context("no framebuffer received")?;
        frame.save_png(path)?;
        Ok(frame.sha256())
    }

    pub fn display_updates_between(&self, start_ns: u64, end_ns: u64) -> usize {
        self.state
            .lock()
            .expect("display mutex poisoned")
            .display_timestamps
            .iter()
            .filter(|timestamp| **timestamp > start_ns && **timestamp < end_ns)
            .count()
    }

    fn record_input(&self, kind: &str, action_id: Uuid, payload: serde_json::Value) -> Result<u64> {
        let event = RawEvent::observed(
            self.session_id,
            "input",
            kind,
            json!({"actionId": action_id, "detail": payload}),
        );
        let timestamp = event.host_monotonic_ns;
        self.sink.record(event)?;
        Ok(timestamp)
    }

    pub async fn move_pointer(&self, x: u32, y: u32) -> Result<ActionReceipt> {
        let action_id = Uuid::new_v4();
        let started_ns = self.record_input("pointer.move", action_id, json!({"x": x, "y": y}))?;
        if self.mouse.is_absolute().await? {
            self.mouse.set_abs_position(x, y).await?;
        } else {
            bail!("relative pointer is unsupported until its current position is known");
        }
        Ok(ActionReceipt::completed(action_id, started_ns))
    }

    pub async fn mouse_down(&self, button: MouseButton) -> Result<ActionReceipt> {
        let action_id = Uuid::new_v4();
        let started_ns = self.record_input(
            "pointer.down",
            action_id,
            json!({"button": format!("{button:?}")}),
        )?;
        self.mouse.press(button as u32).await?;
        Ok(ActionReceipt::completed(action_id, started_ns))
    }

    pub async fn mouse_up(&self, button: MouseButton) -> Result<ActionReceipt> {
        let action_id = Uuid::new_v4();
        let started_ns = self.record_input(
            "pointer.up",
            action_id,
            json!({"button": format!("{button:?}")}),
        )?;
        self.mouse.release(button as u32).await?;
        Ok(ActionReceipt::completed(action_id, started_ns))
    }

    pub async fn key_down(&self, keycode: u32) -> Result<ActionReceipt> {
        let action_id = Uuid::new_v4();
        let started_ns = self.record_input("key.down", action_id, json!({"keycode": keycode}))?;
        self.keyboard.press(keycode).await?;
        tokio::time::sleep(Duration::from_millis(150)).await;
        Ok(ActionReceipt::completed(action_id, started_ns))
    }

    pub async fn key_up(&self, keycode: u32) -> Result<ActionReceipt> {
        let action_id = Uuid::new_v4();
        let started_ns = self.record_input("key.up", action_id, json!({"keycode": keycode}))?;
        self.keyboard.release(keycode).await?;
        tokio::time::sleep(Duration::from_millis(150)).await;
        Ok(ActionReceipt::completed(action_id, started_ns))
    }

    pub async fn key_press(&self, keycode: u32) -> Result<ActionReceipt> {
        let receipt = self.key_down(keycode).await?;
        self.key_up(keycode).await?;
        Ok(receipt)
    }

    pub async fn type_text(&self, text: &str) -> Result<()> {
        for character in text.chars() {
            for transition in keystroke_sequence(character)
                .with_context(|| format!("no US keyboard mapping for {character:?}"))?
            {
                let (kind, keycode) = match transition {
                    KeyTransition::Down(keycode) => ("key.down", keycode),
                    KeyTransition::Up(keycode) => ("key.up", keycode),
                };
                let action_id = Uuid::new_v4();
                self.record_input(
                    kind,
                    action_id,
                    json!({"keycode": keycode, "character": character.to_string()}),
                )?;
                match transition {
                    KeyTransition::Down(_) => self.keyboard.press(keycode).await?,
                    KeyTransition::Up(_) => self.keyboard.release(keycode).await?,
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionReceipt {
    pub action_id: Uuid,
    pub started_ns: u64,
    pub completed_ns: u64,
}

impl ActionReceipt {
    fn completed(action_id: Uuid, started_ns: u64) -> Self {
        Self {
            action_id,
            started_ns,
            completed_ns: monotonic_ns(),
        }
    }
}

fn us_keycode(c: char) -> Option<(u32, bool)> {
    let lower = c.to_ascii_lowercase();
    let code = match lower {
        'a'..='z' => [
            0x1e, 0x30, 0x2e, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31,
            0x18, 0x19, 0x10, 0x13, 0x1f, 0x14, 0x16, 0x2f, 0x11, 0x2d, 0x15, 0x2c,
        ][(lower as u8 - b'a') as usize],
        '1'..='9' => 0x02 + (lower as u32 - '1' as u32),
        '0' => 0x0b,
        ' ' => 0x39,
        '-' | '_' => 0x0c,
        '=' | '+' => 0x0d,
        '[' | '{' => 0x1a,
        ']' | '}' => 0x1b,
        ';' | ':' => 0x27,
        '\'' | '"' => 0x28,
        '`' | '~' => 0x29,
        '\\' | '|' => 0x2b,
        ',' | '<' => 0x33,
        '.' | '>' => 0x34,
        '/' | '?' => 0x35,
        '\n' => 0x1c,
        _ => return None,
    };
    let shift = c.is_ascii_uppercase()
        || matches!(
            c,
            '_' | '+' | '{' | '}' | ':' | '"' | '~' | '|' | '<' | '>' | '?'
        );
    Some((code, shift))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyTransition {
    Down(u32),
    Up(u32),
}

fn keystroke_sequence(character: char) -> Option<Vec<KeyTransition>> {
    let (keycode, shift) = us_keycode(character)?;
    let mut sequence = Vec::with_capacity(if shift { 4 } else { 2 });
    if shift {
        sequence.push(KeyTransition::Down(0x2a));
    }
    sequence.push(KeyTransition::Down(keycode));
    sequence.push(KeyTransition::Up(keycode));
    if shift {
        sequence.push(KeyTransition::Up(0x2a));
    }
    Some(sequence)
}

fn stable_scanout_remaining(last_ns: u64, now_ns: u64, quiet_period: Duration) -> Duration {
    let quiet_ns = quiet_period.as_nanos().min(u64::MAX as u128) as u64;
    Duration::from_nanos(quiet_ns.saturating_sub(now_ns.saturating_sub(last_ns)))
}

fn stable_frame_remaining(
    state: &DisplayState,
    now_ns: u64,
    quiet_period: Duration,
    minimum_width: u32,
    minimum_height: u32,
) -> Option<Duration> {
    if state.display_callbacks_in_flight > 0 {
        return None;
    }
    let frame = state.framebuffer.as_ref()?;
    if frame.width() < minimum_width || frame.height() < minimum_height {
        return None;
    }
    state
        .last_scanout_ns
        .map(|last| stable_scanout_remaining(last, now_ns, quiet_period))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{event::MemoryEventSink, framebuffer::PIXMAN_X8R8G8B8};
    #[test]
    fn maps_url_characters_to_xt_keyboard_codes() {
        for c in "https://localhost:3000/a-b?x=1".chars() {
            assert!(us_keycode(c).is_some(), "missing mapping for {c:?}");
        }
        assert_eq!(us_keycode('A'), Some((0x1e, true)));
        assert_eq!(us_keycode('/'), Some((0x35, false)));
    }

    #[test]
    fn shifted_text_expands_to_exact_low_level_transitions() {
        assert_eq!(
            keystroke_sequence('A').unwrap(),
            vec![
                KeyTransition::Down(0x2a),
                KeyTransition::Down(0x1e),
                KeyTransition::Up(0x1e),
                KeyTransition::Up(0x2a),
            ]
        );
        assert_eq!(
            keystroke_sequence('a').unwrap(),
            vec![KeyTransition::Down(0x1e), KeyTransition::Up(0x1e)]
        );
    }

    #[test]
    fn scanout_stability_depends_only_on_time_since_latest_full_scanout() {
        let quiet = Duration::from_millis(250);
        assert_eq!(
            stable_scanout_remaining(1_000_000_000, 1_100_000_000, quiet),
            Duration::from_millis(150)
        );
        assert!(stable_scanout_remaining(1_000_000_000, 1_300_000_000, quiet).is_zero());
    }

    #[test]
    fn in_flight_display_work_prevents_stability() {
        let state = DisplayState {
            last_scanout_ns: Some(1_000_000_000),
            display_callbacks_in_flight: 1,
            ..DisplayState::default()
        };
        let remaining = if state.display_callbacks_in_flight > 0 {
            None
        } else {
            state.last_scanout_ns.map(|last| {
                stable_scanout_remaining(last, 2_000_000_000, Duration::from_millis(250))
            })
        };
        assert_eq!(remaining, None);
    }

    #[test]
    fn stable_boot_console_does_not_satisfy_graphical_frame_readiness() {
        let mut state = DisplayState {
            framebuffer: Some(
                Framebuffer::from_scanout(2, 1, 8, PIXMAN_X8R8G8B8, &[0; 8]).unwrap(),
            ),
            last_scanout_ns: Some(1_000_000_000),
            ..DisplayState::default()
        };
        assert_eq!(
            stable_frame_remaining(&state, 2_000_000_000, Duration::from_millis(250), 3, 2),
            None
        );
        state.framebuffer =
            Some(Framebuffer::from_scanout(3, 2, 12, PIXMAN_X8R8G8B8, &[0; 24]).unwrap());
        state.last_scanout_ns = Some(1_500_000_000);
        assert_eq!(
            stable_frame_remaining(&state, 2_000_000_000, Duration::from_millis(250), 3, 2),
            Some(Duration::ZERO)
        );
    }

    #[tokio::test]
    async fn incompatible_update_is_preserved_and_waits_for_replacement_scanout() {
        let temp = tempfile::tempdir().unwrap();
        let sink = Arc::new(MemoryEventSink::default());
        let state = Arc::new(Mutex::new(DisplayState::default()));
        let mut listener = Listener {
            session_id: Uuid::new_v4(),
            sink: sink.clone(),
            artifacts: Arc::new(ArtifactStore::new(temp.path().join("artifacts")).unwrap()),
            state: state.clone(),
            changed: Arc::new(Notify::new()),
        };
        listener
            .scanout(2, 2, 8, PIXMAN_X8R8G8B8, vec![0; 16])
            .await;
        listener
            .update(0, 0, 3, 1, 12, PIXMAN_X8R8G8B8, vec![1; 12])
            .await;
        {
            let state = state.lock().unwrap();
            assert!(state.framebuffer.is_none());
            assert!(state.last_scanout_ns.is_none());
            assert!(state.last_error.is_none());
        }
        let rejected = sink.events().pop().unwrap();
        assert_eq!(rejected.kind, "display.update_rejected");
        assert_eq!(rejected.payload["recovery"], "awaiting_full_scanout");
        assert_eq!(rejected.artifact_refs.len(), 1);

        listener
            .scanout(3, 2, 12, PIXMAN_X8R8G8B8, vec![2; 24])
            .await;
        let state = state.lock().unwrap();
        let frame = state.framebuffer.as_ref().unwrap();
        assert_eq!((frame.width(), frame.height()), (3, 2));
        assert!(state.last_error.is_none());
    }
}
