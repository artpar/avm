use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::sleep;
use uuid::Uuid;

use crate::qmp::QmpClient;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    pub id: Uuid,
    pub base_image: PathBuf,
    pub candidate_workspace: PathBuf,
    pub state_dir: PathBuf,
    pub memory_mib: u32,
    pub cpus: u8,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub host_boot_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RunPaths {
    pub config: PathBuf,
    pub overlay: PathBuf,
    pub qmp_socket: PathBuf,
    pub display_socket: PathBuf,
    pub dbus_pid: PathBuf,
    pub dbus_log: PathBuf,
    pub virtiofs_socket: PathBuf,
    pub qemu_pid: PathBuf,
    pub virtiofs_pid: PathBuf,
    pub qemu_log: PathBuf,
    pub virtiofs_log: PathBuf,
    pub events: PathBuf,
    pub timeline: PathBuf,
    pub artifacts: PathBuf,
    pub clean_snapshot: PathBuf,
}

impl RunConfig {
    pub fn new(base_image: &Path, candidate_workspace: &Path, state_root: &Path) -> Result<Self> {
        let base_image = base_image
            .canonicalize()
            .context("canonicalize base image")?;
        let candidate_workspace = candidate_workspace
            .canonicalize()
            .context("canonicalize candidate workspace")?;
        ensure!(base_image.is_file(), "base image must be a regular file");
        ensure!(
            candidate_workspace.is_dir(),
            "candidate workspace must be a directory"
        );
        ensure!(
            !base_image.starts_with(&candidate_workspace),
            "base image must be outside the candidate workspace"
        );
        let state_root = canonicalize_future_path(state_root)?;
        ensure!(
            !state_root.starts_with(&candidate_workspace),
            "state root must be outside the candidate workspace"
        );
        let id = Uuid::new_v4();
        Ok(Self {
            id,
            base_image,
            candidate_workspace,
            state_dir: state_root.join(id.to_string()),
            memory_mib: 4096,
            cpus: 4,
            width: 1280,
            height: 720,
            host_boot_id: current_host_boot_id()?,
        })
    }

    pub fn paths(&self) -> RunPaths {
        RunPaths {
            config: self.state_dir.join("run.json"),
            overlay: self.state_dir.join("overlay.qcow2"),
            qmp_socket: self.state_dir.join("qmp.sock"),
            display_socket: self.state_dir.join("display.sock"),
            dbus_pid: self.state_dir.join("dbus.pid"),
            dbus_log: self.state_dir.join("dbus.log"),
            virtiofs_socket: self.state_dir.join("virtiofs.sock"),
            qemu_pid: self.state_dir.join("qemu.pid"),
            virtiofs_pid: self.state_dir.join("virtiofs.pid"),
            qemu_log: self.state_dir.join("qemu.log"),
            virtiofs_log: self.state_dir.join("virtiofs.log"),
            events: self.state_dir.join("events.jsonl"),
            timeline: self.state_dir.join("timeline.sqlite3"),
            artifacts: self.state_dir.join("artifacts"),
            clean_snapshot: self.state_dir.join("clean.snapshot"),
        }
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(&self.state_dir)?;
        std::fs::write(self.paths().config, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = std::fs::read(path.as_ref())?;
        serde_json::from_slice(&bytes).context("decode run configuration")
    }

    pub fn ensure_current_host_boot(&self) -> Result<()> {
        if let (Some(expected), Some(current)) = (&self.host_boot_id, current_host_boot_id()?) {
            ensure!(
                expected == &current,
                "run belongs to Linux boot {expected}, current boot is {current}; create a new run before recording more evidence"
            );
        }
        Ok(())
    }
}

pub struct VmController {
    config: RunConfig,
}

impl VmController {
    pub fn new(config: RunConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &RunConfig {
        &self.config
    }

    pub fn create_overlay(&self) -> Result<()> {
        let paths = self.config.paths();
        ensure!(!paths.overlay.exists(), "run overlay already exists");
        self.config.save()?;
        let output = Command::new("qemu-img")
            .args(["create", "-f", "qcow2", "-F", "qcow2", "-b"])
            .arg(&self.config.base_image)
            .arg(&paths.overlay)
            .output()
            .context("launch qemu-img")?;
        ensure!(
            output.status.success(),
            "qemu-img failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(())
    }

    pub async fn start(&self) -> Result<()> {
        self.config.ensure_current_host_boot()?;
        require_linux()?;
        let paths = self.config.paths();
        ensure!(
            paths.overlay.is_file(),
            "run overlay does not exist; call create-run first"
        );
        ensure!(!self.is_running(), "VM is already running");
        remove_stale_sockets(&paths)?;

        if let Err(error) = self.launch(&paths).await {
            let cleanup = self.stop().await;
            return match cleanup {
                Ok(()) => Err(error),
                Err(cleanup_error) => {
                    Err(error.context(format!("startup cleanup also failed: {cleanup_error:#}")))
                }
            };
        }
        Ok(())
    }

    async fn launch(&self, paths: &RunPaths) -> Result<()> {
        let dbus_log = log_file(&paths.dbus_log)?;
        let dbus = Command::new("dbus-daemon")
            .arg("--session")
            .arg("--nofork")
            .arg(format!(
                "--address=unix:path={}",
                paths.display_socket.display()
            ))
            .stdout(Stdio::from(dbus_log.try_clone()?))
            .stderr(Stdio::from(dbus_log))
            .spawn()
            .context("launch private dbus-daemon")?;
        std::fs::write(&paths.dbus_pid, dbus.id().to_string())?;
        wait_for_path(&paths.display_socket, Duration::from_secs(10)).await?;

        let virtiofs_log = log_file(&paths.virtiofs_log)?;
        let virtiofs = Command::new(virtiofsd_binary())
            .arg(format!("--socket-path={}", paths.virtiofs_socket.display()))
            .arg(format!(
                "--shared-dir={}",
                self.config.candidate_workspace.display()
            ))
            .arg("--sandbox=namespace")
            .stdout(Stdio::from(virtiofs_log.try_clone()?))
            .stderr(Stdio::from(virtiofs_log))
            .spawn()
            .context("launch virtiofsd")?;
        std::fs::write(&paths.virtiofs_pid, virtiofs.id().to_string())?;

        wait_for_path(&paths.virtiofs_socket, Duration::from_secs(10)).await?;
        ensure!(
            process_exists(virtiofs.id()),
            "virtiofsd exited during startup; see {}",
            paths.virtiofs_log.display()
        );
        let qemu_log = log_file(&paths.qemu_log)?;
        let args = self.qemu_args();
        let qemu = Command::new("qemu-system-x86_64")
            .args(&args)
            .stdout(Stdio::from(qemu_log.try_clone()?))
            .stderr(Stdio::from(qemu_log))
            .spawn()
            .context("launch qemu-system-x86_64")?;
        std::fs::write(&paths.qemu_pid, qemu.id().to_string())?;
        wait_for_path(&paths.qmp_socket, Duration::from_secs(10)).await?;
        let mut qmp = QmpClient::connect(&paths.qmp_socket).await?;
        qmp.execute("query-status", None).await?;
        if !paths.clean_snapshot.exists() {
            qmp.snapshot_save("avm-clean-save", "avm-clean", "os", &["os"])
                .await?;
            std::fs::write(&paths.clean_snapshot, b"avm-clean\n")?;
        } else {
            qmp.snapshot_load("avm-clean-start-load", "avm-clean", "os", &["os"])
                .await?;
        }
        qmp.execute(
            "device_add",
            Some(json!({
                "driver": "vhost-user-fs-pci",
                "id": "candidate-fs",
                "chardev": "charfs",
                "tag": "candidate",
                "bus": "fs-root-port",
            })),
        )
        .await?;
        qmp.execute("cont", None).await?;
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        let paths = self.config.paths();
        if paths.qmp_socket.exists() {
            if let Ok(mut qmp) = QmpClient::connect(&paths.qmp_socket).await {
                let _ = qmp.execute("quit", None).await;
            }
        }
        let qemu_result = wait_for_process_exit(&paths.qemu_pid, Duration::from_secs(10)).await;
        let virtiofs_result = terminate_pid_file(&paths.virtiofs_pid);
        let dbus_result = terminate_pid_file(&paths.dbus_pid);
        qemu_result?;
        virtiofs_result?;
        dbus_result?;
        Ok(())
    }

    pub async fn reset(&self) -> Result<()> {
        self.config.ensure_current_host_boot()?;
        require_linux()?;
        self.stop().await?;
        self.start().await
    }

    pub async fn restore_checkpoint_in_place(&self) -> Result<()> {
        self.config.ensure_current_host_boot()?;
        require_linux()?;
        let paths = self.config.paths();
        ensure!(self.is_running(), "VM is not running");
        ensure!(
            paths.clean_snapshot.is_file(),
            "clean VM snapshot marker is missing"
        );
        let mut qmp = QmpClient::connect(&paths.qmp_socket).await?;
        qmp.snapshot_load("avm-clean-load", "avm-clean", "os", &["os"])
            .await?;
        qmp.execute("cont", None).await?;
        Ok(())
    }

    pub async fn checkpoint(&self) -> Result<()> {
        self.config.ensure_current_host_boot()?;
        require_linux()?;
        let paths = self.config.paths();
        ensure!(self.is_running(), "VM is not running");
        ensure!(
            !paths.clean_snapshot.exists(),
            "clean VM snapshot already exists"
        );
        let mut qmp = QmpClient::connect(&paths.qmp_socket).await?;
        qmp.snapshot_save("avm-clean-save", "avm-clean", "os", &["os"])
            .await?;
        std::fs::write(&paths.clean_snapshot, b"avm-clean\n")?;
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        read_pid(&self.config.paths().qemu_pid).is_some_and(process_exists)
    }

    pub fn qemu_args(&self) -> Vec<String> {
        let paths = self.config.paths();
        vec![
            "-name".into(),
            format!("avm-{}", self.config.id),
            "-machine".into(),
            "q35,accel=kvm".into(),
            "-cpu".into(),
            "host".into(),
            "-m".into(),
            self.config.memory_mib.to_string(),
            "-object".into(),
            format!(
                "memory-backend-memfd,id=guestmem,size={}M,share=on",
                self.config.memory_mib
            ),
            "-numa".into(),
            "node,memdev=guestmem".into(),
            "-smp".into(),
            self.config.cpus.to_string(),
            "-nodefaults".into(),
            "-device".into(),
            "virtio-vga".into(),
            "-device".into(),
            "qemu-xhci".into(),
            "-device".into(),
            "usb-kbd".into(),
            "-device".into(),
            "usb-tablet".into(),
            "-blockdev".into(),
            format!(
                "driver=qcow2,node-name=os,file.driver=file,file.filename={}",
                paths.overlay.display()
            ),
            "-device".into(),
            "virtio-blk-pci,drive=os".into(),
            "-chardev".into(),
            format!("socket,id=charfs,path={}", paths.virtiofs_socket.display()),
            "-device".into(),
            "pcie-root-port,id=fs-root-port".into(),
            "-netdev".into(),
            "user,id=net0,hostfwd=tcp:127.0.0.1:2222-:22,hostfwd=tcp:127.0.0.1:9222-:9222".into(),
            "-device".into(),
            "virtio-net-pci,netdev=net0".into(),
            "-qmp".into(),
            format!("unix:{},server=on,wait=off", paths.qmp_socket.display()),
            "-display".into(),
            format!(
                "dbus,addr=unix:path={},gl=off",
                paths.display_socket.display()
            ),
            "-audiodev".into(),
            "none,id=noaudio".into(),
            "-boot".into(),
            "order=c".into(),
            "-no-reboot".into(),
            "-S".into(),
        ]
    }
}

fn require_linux() -> Result<()> {
    if cfg!(target_os = "linux") {
        Ok(())
    } else {
        bail!("QEMU/KVM runs require a Linux host")
    }
}

fn virtiofsd_binary() -> PathBuf {
    for path in ["/usr/libexec/virtiofsd", "/usr/lib/qemu/virtiofsd"] {
        if Path::new(path).is_file() {
            return PathBuf::from(path);
        }
    }
    PathBuf::from("virtiofsd")
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn canonicalize_future_path(path: &Path) -> Result<PathBuf> {
    let absolute = absolute_path(path)?;
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let name = ancestor
            .file_name()
            .context("path has no existing ancestor")?;
        missing.push(name.to_owned());
        ancestor = ancestor.parent().context("path has no existing ancestor")?;
    }
    let mut canonical = ancestor.canonicalize()?;
    for component in missing.iter().rev() {
        canonical.push(component);
    }
    Ok(canonical)
}

fn current_host_boot_id() -> Result<Option<String>> {
    #[cfg(target_os = "linux")]
    {
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .context("read Linux boot ID")?;
        return Ok(Some(boot_id.trim().to_owned()));
    }
    #[cfg(not(target_os = "linux"))]
    Ok(None)
}

fn log_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open log {}", path.display()))
}

fn remove_stale_sockets(paths: &RunPaths) -> Result<()> {
    for path in [
        &paths.qmp_socket,
        &paths.display_socket,
        &paths.virtiofs_socket,
    ] {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

async fn wait_for_path(path: &Path, timeout: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if path.exists() {
            return Ok(());
        }
        sleep(Duration::from_millis(25)).await;
    }
    bail!("timed out waiting for {}", path.display())
}

async fn wait_for_process_exit(pid_file: &Path, timeout: Duration) -> Result<()> {
    let Some(pid) = read_pid(pid_file) else {
        return Ok(());
    };
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if !process_exists(pid) {
            let _ = std::fs::remove_file(pid_file);
            return Ok(());
        }
        sleep(Duration::from_millis(50)).await;
    }
    bail!("QEMU process {pid} did not exit")
}

fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn process_exists(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        if stat
            .rsplit_once(") ")
            .and_then(|(_, rest)| rest.chars().next())
            == Some('Z')
        {
            return false;
        }
    }
    // SAFETY: kill(pid, 0) performs no mutation and only probes process existence.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn terminate_pid_file(path: &Path) -> Result<()> {
    if let Some(pid) = read_pid(path) {
        // SAFETY: pid was written from the child we spawned for this run.
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    }
    let _ = std::fs::remove_file(path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qemu_contract_uses_kvm_qmp_dbus_overlay_and_virtiofs() {
        let config = RunConfig {
            id: Uuid::nil(),
            base_image: "/images/base.qcow2".into(),
            candidate_workspace: "/candidate".into(),
            state_dir: "/outside/run".into(),
            memory_mib: 2048,
            cpus: 2,
            width: 1280,
            height: 720,
            host_boot_id: None,
        };
        let args = VmController::new(config).qemu_args().join(" ");
        assert!(args.contains("q35,accel=kvm"));
        assert!(args.contains("driver=qcow2,node-name=os"));
        assert!(args.contains("-qmp unix:/outside/run/qmp.sock,server=on,wait=off"));
        assert!(args.contains("-display dbus,addr=unix:path=/outside/run/display.sock,gl=off"));
        assert!(args.contains("socket,id=charfs,path=/outside/run/virtiofs.sock"));
        assert!(args.contains("pcie-root-port,id=fs-root-port"));
        assert!(!args.contains("vhost-user-fs-pci"));
        assert!(args.contains("memory-backend-memfd,id=guestmem,size=2048M,share=on"));
        assert!(args.contains("-numa node,memdev=guestmem"));
        assert!(args.contains("usb-tablet"));
        assert!(args.ends_with("-S"));
    }

    #[test]
    fn state_inside_candidate_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("base.qcow2");
        std::fs::write(&base, b"not a real image").unwrap();
        let error = RunConfig::new(&base, temp.path(), &temp.path().join("state")).unwrap_err();
        assert!(error.to_string().contains("outside"));
    }

    #[test]
    fn base_image_inside_candidate_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("base.qcow2");
        std::fs::write(&base, b"not a real image").unwrap();
        let external_state = tempfile::tempdir().unwrap();
        let error = RunConfig::new(&base, temp.path(), external_state.path()).unwrap_err();
        assert!(error.to_string().contains("base image must be outside"));
    }

    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn start_rejects_non_linux_hosts_before_launching_processes() {
        let config = RunConfig {
            id: Uuid::nil(),
            base_image: "/images/base.qcow2".into(),
            candidate_workspace: "/candidate".into(),
            state_dir: "/outside/run".into(),
            memory_mib: 2048,
            cpus: 2,
            width: 1280,
            height: 720,
            host_boot_id: None,
        };
        let error = VmController::new(config).start().await.unwrap_err();
        assert_eq!(error.to_string(), "QEMU/KVM runs require a Linux host");
    }
}
