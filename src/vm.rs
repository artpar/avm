use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::sleep;
use uuid::Uuid;

use crate::{
    coordination::RunLock, guest_command::active_command_ids, integrity::file_sha256,
    qmp::QmpClient,
};

const GUEST_WORKSPACE_UID: u32 = 1000;
const GUEST_WORKSPACE_GID: u32 = 1000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    pub id: Uuid,
    pub base_image: PathBuf,
    pub workspace_root: PathBuf,
    pub candidate_workspace: PathBuf,
    pub guest_state_paths: Vec<PathBuf>,
    pub state_dir: PathBuf,
    pub memory_mib: u32,
    pub cpus: u8,
    pub width: u32,
    pub height: u32,
    pub host_boot_id: Option<String>,
    pub guest_ssh_port: u16,
    pub guest_ssh_user: String,
    pub guest_ssh_private_key: PathBuf,
    pub guest_ssh_host_public_key: PathBuf,
}

#[derive(Clone, Debug)]
pub struct RunPaths {
    pub config: PathBuf,
    pub overlay: PathBuf,
    pub qmp_socket: PathBuf,
    pub display_socket: PathBuf,
    pub accessibility_socket: PathBuf,
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
    pub fn new(
        base_image: &Path,
        candidate_workspace: &Path,
        state_root: &Path,
        guest_state_paths: Vec<PathBuf>,
        guest_ssh_private_key: &Path,
        guest_ssh_host_public_key: &Path,
    ) -> Result<Self> {
        let base_image = base_image
            .canonicalize()
            .context("canonicalize base image")?;
        let candidate_workspace = candidate_workspace
            .canonicalize()
            .context("canonicalize candidate workspace")?;
        ensure!(base_image.is_file(), "base image must be a regular file");
        verify_base_integrity(&base_image)?;
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
        let state_dir = state_root.join(id.to_string());
        let workspace_root = state_dir.join("workspace");
        let guest_state_paths = validate_guest_state_paths(guest_state_paths)?;
        let guest_ssh_private_key = guest_ssh_private_key
            .canonicalize()
            .context("canonicalize guest SSH private key")?;
        let guest_ssh_host_public_key = guest_ssh_host_public_key
            .canonicalize()
            .context("canonicalize guest SSH host public key")?;
        ensure!(
            guest_ssh_private_key.is_file(),
            "guest SSH private key must be a file"
        );
        ensure!(
            guest_ssh_host_public_key.is_file(),
            "guest SSH host public key must be a file"
        );
        Ok(Self {
            id,
            base_image,
            candidate_workspace: workspace_root.join("current"),
            workspace_root,
            guest_state_paths,
            state_dir,
            memory_mib: 4096,
            cpus: 4,
            width: 1280,
            height: 720,
            host_boot_id: current_host_boot_id()?,
            guest_ssh_port: 2222,
            guest_ssh_user: "avm".into(),
            guest_ssh_private_key,
            guest_ssh_host_public_key,
        })
    }

    pub fn paths(&self) -> RunPaths {
        RunPaths {
            config: self.state_dir.join("run.json"),
            overlay: self.state_dir.join("overlay.qcow2"),
            qmp_socket: self.state_dir.join("qmp.sock"),
            display_socket: self.state_dir.join("display.sock"),
            accessibility_socket: self.state_dir.join("accessibility.sock"),
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
        self.validate_contract()?;
        std::fs::create_dir_all(&self.state_dir)?;
        std::fs::write(self.paths().config, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = std::fs::read(path.as_ref())?;
        let config: Self = serde_json::from_slice(&bytes).context("decode run configuration")?;
        config.validate_contract()?;
        Ok(config)
    }

    fn validate_contract(&self) -> Result<()> {
        ensure!(
            self.workspace_root == self.state_dir.join("workspace"),
            "workspace root must be the run-owned workspace directory"
        );
        ensure!(
            self.candidate_workspace == self.workspace_root.join("current"),
            "candidate workspace must be the stable current-generation link"
        );
        ensure!(
            validate_guest_state_paths(self.guest_state_paths.clone())? == self.guest_state_paths,
            "guest state paths must be sorted, unique, and disjoint"
        );
        ensure!(
            self.guest_ssh_port == 2222 && self.guest_ssh_user == "avm",
            "guest command endpoint does not match the image contract"
        );
        ensure!(
            self.base_image.is_absolute()
                && self.state_dir.is_absolute()
                && self.guest_ssh_private_key.is_absolute()
                && self.guest_ssh_host_public_key.is_absolute(),
            "run contract paths must be absolute"
        );
        Ok(())
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

    pub fn create_overlay(&self, source_workspace: &Path) -> Result<()> {
        let _lock = RunLock::exclusive(&self.config.state_dir, "create-run")?;
        let paths = self.config.paths();
        ensure!(!paths.overlay.exists(), "run overlay already exists");
        initialize_workspace(&self.config, source_workspace)?;
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
        let _lock = RunLock::exclusive(&self.config.state_dir, "start")?;
        self.start_unlocked().await
    }

    async fn start_unlocked(&self) -> Result<()> {
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
            let cleanup = self.stop_unlocked().await;
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

        let (host_uid, host_gid) = host_identity();
        ensure!(
            host_uid != 0 && host_gid != 0,
            "AVM must run as a non-root user to map the writable guest workspace safely"
        );
        ensure_candidate_identity(&self.config.workspace_root, host_uid, host_gid)?;
        let virtiofs_log = log_file(&paths.virtiofs_log)?;
        let virtiofs = Command::new(virtiofsd_binary())
            .args(virtiofsd_args(
                paths,
                &self.config.workspace_root,
                host_uid,
                host_gid,
            ))
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
                "id": "workspace-fs",
                "chardev": "charfs",
                "tag": "workspace-root",
                "bus": "fs-root-port",
            })),
        )
        .await?;
        qmp.execute("cont", None).await?;
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        let _lock = RunLock::exclusive(&self.config.state_dir, "stop")?;
        self.ensure_no_active_commands()?;
        self.stop_unlocked().await
    }

    async fn stop_unlocked(&self) -> Result<()> {
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
        let _lock = RunLock::exclusive(&self.config.state_dir, "reset")?;
        self.ensure_no_active_commands()?;
        self.config.ensure_current_host_boot()?;
        require_linux()?;
        self.stop_unlocked().await?;
        self.start_unlocked().await
    }

    pub async fn restore_checkpoint_in_place(&self) -> Result<()> {
        let _lock = RunLock::exclusive(&self.config.state_dir, "restore-checkpoint")?;
        self.ensure_no_active_commands()?;
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
        let _lock = RunLock::exclusive(&self.config.state_dir, "checkpoint")?;
        self.ensure_no_active_commands()?;
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

    pub async fn destroy(self) -> Result<()> {
        let _lock = RunLock::exclusive(&self.config.state_dir, "destroy-run")?;
        self.ensure_no_active_commands()?;
        if self.is_running() {
            self.stop_unlocked().await?;
        }
        let saved = std::fs::canonicalize(self.config.paths().config)?;
        let state = std::fs::canonicalize(&self.config.state_dir)?;
        ensure!(
            saved.parent() == Some(state.as_path()),
            "run.json is not directly inside the state directory"
        );
        ensure!(
            state.file_name().and_then(|name| name.to_str()) == Some(&self.config.id.to_string()),
            "state directory does not match run ID"
        );
        std::fs::remove_dir_all(&state)
            .with_context(|| format!("destroy run {}", state.display()))?;
        Ok(())
    }

    fn ensure_no_active_commands(&self) -> Result<()> {
        let active = active_command_ids(&self.config)?;
        ensure!(
            active.is_empty(),
            "run has active guest commands: {}; wait for or cancel them before changing VM state",
            active.join(", ")
        );
        Ok(())
    }

    pub fn promote_base(&self, output: &Path, confirm_sanitized: bool) -> Result<PromotedBase> {
        self.config.ensure_current_host_boot()?;
        require_linux()?;
        ensure!(confirm_sanitized, "promotion requires --confirm-sanitized");
        let _lock = RunLock::exclusive(&self.config.state_dir, "promote-base")?;
        ensure!(
            !self.is_running(),
            "VM must be stopped before base promotion"
        );
        let overlay = self.config.paths().overlay;
        let metadata = std::fs::symlink_metadata(&overlay).context("run overlay does not exist")?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "run overlay must be a regular non-symlink file"
        );
        ensure!(
            output.extension().and_then(|value| value.to_str()) == Some("qcow2"),
            "promoted base output must end in .qcow2"
        );
        ensure!(!output.exists(), "promoted base output already exists");
        let parent = output
            .parent()
            .context("promoted base output has no parent")?
            .canonicalize()?;
        let output = parent.join(
            output
                .file_name()
                .context("promoted base output has no file name")?,
        );
        ensure!(
            !output.starts_with(&self.config.state_dir),
            "promoted base must be outside the source run state"
        );
        let manifest_path = PathBuf::from(format!("{}.avm.json", output.display()));
        let checksum_path = PathBuf::from(format!("{}.sha256", output.display()));
        ensure!(
            !manifest_path.exists() && !checksum_path.exists(),
            "promoted base sidecar already exists"
        );
        ensure!(
            !output.starts_with(&self.config.candidate_workspace),
            "promoted base must be outside the candidate workspace"
        );

        let chain = qemu_img_backing_chain(&overlay)?;
        ensure!(
            chain.len() >= 2,
            "run overlay has no readable backing image"
        );
        ensure!(
            chain.iter().all(|entry| entry["format"] == "qcow2"),
            "run backing chain contains a non-qcow2 image"
        );
        ensure!(
            chain
                .iter()
                .all(|entry| entry.get("encrypted").and_then(Value::as_bool) != Some(true)),
            "encrypted run images cannot be promoted"
        );
        let configured_base = self.config.base_image.canonicalize()?;
        let chain_contains_base = chain.iter().any(|entry| {
            entry["filename"]
                .as_str()
                .and_then(|path| Path::new(path).canonicalize().ok())
                .as_ref()
                == Some(&configured_base)
        });
        ensure!(
            chain_contains_base,
            "run overlay backing chain does not contain the configured base image"
        );
        qemu_img_success(["check", "-f", "qcow2"], &overlay, None)?;
        let source_info = qemu_img_info(&overlay)?;
        ensure!(source_info["format"] == "qcow2", "run overlay is not qcow2");
        let virtual_size = source_info["virtual-size"]
            .as_u64()
            .context("qemu-img info omitted virtual-size")?;
        let staging = parent.join(format!(".avm-promote-{}.qcow2", Uuid::new_v4()));
        let mut output_created = false;
        let mut manifest_created = false;
        let mut checksum_created = false;
        let result = (|| -> Result<PromotedBase> {
            let converted = Command::new("qemu-img")
                .args([
                    "convert",
                    "-f",
                    "qcow2",
                    "-O",
                    "qcow2",
                    "-o",
                    "lazy_refcounts=on",
                ])
                .arg(&overlay)
                .arg(&staging)
                .output()
                .context("launch qemu-img convert")?;
            ensure!(
                converted.status.success(),
                "qemu-img convert failed: {}",
                String::from_utf8_lossy(&converted.stderr).trim()
            );
            qemu_img_success(["check", "-f", "qcow2"], &staging, None)?;
            let promoted_info = qemu_img_info(&staging)?;
            ensure!(
                promoted_info["format"] == "qcow2",
                "promoted image is not qcow2"
            );
            ensure!(
                promoted_info["virtual-size"].as_u64() == Some(virtual_size),
                "promoted image virtual size changed"
            );
            ensure!(
                promoted_info.get("backing-filename").is_none(),
                "promoted image still has a backing file"
            );
            ensure!(
                promoted_info
                    .get("snapshots")
                    .and_then(Value::as_array)
                    .is_none_or(Vec::is_empty),
                "promoted image retained internal snapshots"
            );
            qemu_img_success(
                ["compare", "-f", "qcow2", "-F", "qcow2"],
                &overlay,
                Some(&staging),
            )?;
            let sha256 = file_sha256(&staging)?;
            File::open(&staging)?.sync_all()?;
            std::fs::hard_link(&staging, &output)
                .context("publish promoted base without replacement")?;
            output_created = true;
            let mut permissions = std::fs::metadata(&output)?.permissions();
            permissions.set_readonly(true);
            std::fs::set_permissions(&output, permissions)?;
            let promoted = PromotedBase {
                source_run_id: self.config.id,
                base_image: output.clone(),
                format: "qcow2".into(),
                virtual_size,
                sha256,
                manifest: manifest_path.clone(),
                checksum_file: checksum_path.clone(),
            };
            write_new_synced(&manifest_path, &serde_json::to_vec_pretty(&promoted)?)?;
            manifest_created = true;
            write_new_synced(
                &checksum_path,
                format!(
                    "{}  {}\n",
                    promoted.sha256,
                    output.file_name().unwrap().to_string_lossy()
                )
                .as_bytes(),
            )?;
            checksum_created = true;
            File::open(&parent)?.sync_all()?;
            Ok(promoted)
        })();
        if result.is_err() {
            if output_created {
                let _ = std::fs::remove_file(&output);
            }
            if manifest_created {
                let _ = std::fs::remove_file(&manifest_path);
            }
            if checksum_created {
                let _ = std::fs::remove_file(&checksum_path);
            }
        }
        let _ = std::fs::remove_file(&staging);
        result
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
            "-device".into(),
            "virtio-serial-pci,id=guest-sensors".into(),
            "-chardev".into(),
            format!(
                "socket,id=accessibility,path={},server=on,wait=off",
                paths.accessibility_socket.display()
            ),
            "-device".into(),
            "virtserialport,bus=guest-sensors.0,chardev=accessibility,name=org.avm.accessibility"
                .into(),
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
                "dbus,addr=unix:path={},gl=off,audiodev=avm-audio",
                paths.display_socket.display()
            ),
            "-audiodev".into(),
            "dbus,id=avm-audio".into(),
            "-device".into(),
            "ich9-intel-hda".into(),
            "-device".into(),
            "hda-duplex,audiodev=avm-audio".into(),
            "-boot".into(),
            "order=c".into(),
            "-no-reboot".into(),
            "-S".into(),
        ]
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromotedBase {
    pub source_run_id: Uuid,
    pub base_image: PathBuf,
    pub format: String,
    pub virtual_size: u64,
    pub sha256: String,
    pub manifest: PathBuf,
    pub checksum_file: PathBuf,
}

fn qemu_img_info(path: &Path) -> Result<Value> {
    let output = Command::new("qemu-img")
        .args(["info", "--output=json"])
        .arg(path)
        .output()
        .context("launch qemu-img info")?;
    ensure!(
        output.status.success(),
        "qemu-img info failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    serde_json::from_slice(&output.stdout).context("decode qemu-img info")
}

fn qemu_img_backing_chain(path: &Path) -> Result<Vec<Value>> {
    let output = Command::new("qemu-img")
        .args(["info", "--output=json", "--backing-chain"])
        .arg(path)
        .output()
        .context("launch qemu-img backing-chain inspection")?;
    ensure!(
        output.status.success(),
        "qemu-img backing-chain inspection failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    serde_json::from_slice(&output.stdout).context("decode qemu-img backing chain")
}

fn qemu_img_success<const N: usize>(
    arguments: [&str; N],
    first: &Path,
    second: Option<&Path>,
) -> Result<()> {
    let mut command = Command::new("qemu-img");
    command.args(arguments).arg(first);
    if let Some(second) = second {
        command.arg(second);
    }
    let output = command.output().context("launch qemu-img")?;
    ensure!(
        output.status.success(),
        "qemu-img failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn verify_base_integrity(base_image: &Path) -> Result<()> {
    let checksum_path = PathBuf::from(format!("{}.sha256", base_image.display()));
    if !checksum_path.is_file() {
        return Ok(());
    }
    let checksum = std::fs::read_to_string(&checksum_path)?;
    let expected = checksum
        .split_whitespace()
        .next()
        .context("base checksum sidecar is empty")?;
    ensure!(
        expected.len() == 64 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "base checksum sidecar is invalid"
    );
    ensure!(
        file_sha256(base_image)? == expected.to_ascii_lowercase(),
        "base image failed SHA-256 verification"
    );
    Ok(())
}

fn validate_guest_state_paths(mut paths: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    paths.sort();
    paths.dedup();
    for path in &paths {
        ensure!(
            !path.as_os_str().is_empty()
                && path
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_))),
            "guest state paths must be non-empty relative paths without '..': {}",
            path.display()
        );
    }
    for pair in paths.windows(2) {
        ensure!(
            !pair[1].starts_with(&pair[0]),
            "guest state paths cannot overlap: {} and {}",
            pair[0].display(),
            pair[1].display()
        );
    }
    Ok(paths)
}

fn initialize_workspace(config: &RunConfig, source: &Path) -> Result<()> {
    use std::os::unix::fs::symlink;

    let source = source
        .canonicalize()
        .context("canonicalize source workspace")?;
    ensure!(source.is_dir(), "source workspace must be a directory");
    ensure!(
        !config.workspace_root.exists(),
        "workspace root already exists"
    );
    let generation = config.workspace_root.join("generations/bootstrap");
    std::fs::create_dir_all(&generation)?;
    std::fs::create_dir_all(config.workspace_root.join("state"))?;
    let copied = Command::new("rsync")
        .args(["--archive", "--exclude=.git"])
        .arg(format!("{}/", source.display()))
        .arg(format!("{}/", generation.display()))
        .status()
        .context("copy source workspace")?;
    ensure!(
        copied.success(),
        "source workspace copy failed with {copied}"
    );
    for relative in &config.guest_state_paths {
        let generation_path = generation.join(relative);
        let state_path = config.workspace_root.join("state").join(relative);
        if let Some(parent) = state_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if generation_path.exists() {
            std::fs::rename(&generation_path, &state_path)?;
        } else {
            std::fs::create_dir_all(&state_path)?;
        }
        if let Some(parent) = generation_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut target = PathBuf::new();
        for _ in 0..=relative.components().count() {
            target.push("..");
        }
        target.push("state");
        target.push(relative);
        symlink(target, generation_path)?;
    }
    symlink("generations/bootstrap", &config.candidate_workspace)?;
    Ok(())
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

fn virtiofsd_args(
    paths: &RunPaths,
    candidate_workspace: &Path,
    host_uid: u32,
    host_gid: u32,
) -> Vec<String> {
    vec![
        format!("--socket-path={}", paths.virtiofs_socket.display()),
        format!("--shared-dir={}", candidate_workspace.display()),
        "--sandbox=namespace".into(),
        format!("--uid-map=:{GUEST_WORKSPACE_UID}:{host_uid}:1:"),
        format!("--gid-map=:{GUEST_WORKSPACE_GID}:{host_gid}:1:"),
    ]
}

#[cfg(unix)]
fn host_identity() -> (u32, u32) {
    // SAFETY: these libc calls only return the current process credentials.
    unsafe { (libc::geteuid(), libc::getegid()) }
}

#[cfg(not(unix))]
fn host_identity() -> (u32, u32) {
    (0, 0)
}

#[cfg(unix)]
fn ensure_candidate_identity(candidate: &Path, host_uid: u32, host_gid: u32) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::metadata(candidate)?;
    ensure!(
        metadata.uid() == host_uid && metadata.gid() == host_gid,
        "candidate must be owned by the AVM host user {host_uid}:{host_gid}; found {}:{}",
        metadata.uid(),
        metadata.gid()
    );
    Ok(())
}

#[cfg(not(unix))]
fn ensure_candidate_identity(_candidate: &Path, _host_uid: u32, _host_gid: u32) -> Result<()> {
    Ok(())
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

pub fn current_host_boot_id() -> Result<Option<String>> {
    #[cfg(target_os = "linux")]
    {
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .context("read Linux boot ID")?;
        Ok(Some(boot_id.trim().to_owned()))
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
        &paths.accessibility_socket,
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
            workspace_root: "/outside/run/workspace".into(),
            candidate_workspace: "/outside/run/workspace/current".into(),
            guest_state_paths: vec![],
            state_dir: "/outside/run".into(),
            memory_mib: 2048,
            cpus: 2,
            width: 1280,
            height: 720,
            host_boot_id: None,
            guest_ssh_port: 2222,
            guest_ssh_user: "avm".into(),
            guest_ssh_private_key: "/keys/guest".into(),
            guest_ssh_host_public_key: "/keys/host.pub".into(),
        };
        let args = VmController::new(config.clone()).qemu_args().join(" ");
        assert!(args.contains("q35,accel=kvm"));
        assert!(args.contains("driver=qcow2,node-name=os"));
        assert!(args.contains("-qmp unix:/outside/run/qmp.sock,server=on,wait=off"));
        assert!(args.contains(
            "-display dbus,addr=unix:path=/outside/run/display.sock,gl=off,audiodev=avm-audio"
        ));
        assert!(args.contains("-audiodev dbus,id=avm-audio"));
        assert!(args.contains("-device ich9-intel-hda"));
        assert!(args.contains("-device hda-duplex,audiodev=avm-audio"));
        assert!(args.contains("socket,id=charfs,path=/outside/run/virtiofs.sock"));
        assert!(args.contains("pcie-root-port,id=fs-root-port"));
        assert!(!args.contains("vhost-user-fs-pci"));
        assert!(args.contains("memory-backend-memfd,id=guestmem,size=2048M,share=on"));
        assert!(args.contains("virtio-serial-pci,id=guest-sensors"));
        assert!(args.contains(
            "socket,id=accessibility,path=/outside/run/accessibility.sock,server=on,wait=off"
        ));
        assert!(args.contains(
            "virtserialport,bus=guest-sensors.0,chardev=accessibility,name=org.avm.accessibility"
        ));
        assert!(args.contains("-numa node,memdev=guestmem"));
        assert!(args.contains("usb-tablet"));
        assert!(args.ends_with("-S"));

        let paths = config.paths();
        let virtiofs = virtiofsd_args(&paths, &config.workspace_root, 1001, 1002);
        assert!(virtiofs.contains(&"--sandbox=namespace".to_owned()));
        assert!(virtiofs.contains(&"--uid-map=:1000:1001:1:".to_owned()));
        assert!(virtiofs.contains(&"--gid-map=:1000:1002:1:".to_owned()));
    }

    #[test]
    fn state_inside_candidate_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("base.qcow2");
        std::fs::write(&base, b"not a real image").unwrap();
        let error = RunConfig::new(
            &base,
            temp.path(),
            &temp.path().join("state"),
            vec![],
            Path::new("/key"),
            Path::new("/host-key"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("outside"));
    }

    #[test]
    fn base_image_inside_candidate_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("base.qcow2");
        std::fs::write(&base, b"not a real image").unwrap();
        let external_state = tempfile::tempdir().unwrap();
        let error = RunConfig::new(
            &base,
            temp.path(),
            external_state.path(),
            vec![],
            Path::new("/key"),
            Path::new("/host-key"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("base image must be outside"));
    }

    #[test]
    fn base_checksum_sidecar_is_enforced() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("base.qcow2");
        std::fs::write(&base, b"base bytes").unwrap();
        std::fs::write(
            format!("{}.sha256", base.display()),
            format!("{}  base.qcow2\n", file_sha256(&base).unwrap()),
        )
        .unwrap();
        verify_base_integrity(&base).unwrap();
        std::fs::write(&base, b"tampered").unwrap();
        assert!(verify_base_integrity(&base).is_err());
    }

    #[test]
    fn run_configuration_rejects_missing_contract_fields() {
        let value = json!({
            "id": Uuid::nil(),
            "baseImage": "/images/base.qcow2",
            "candidateWorkspace": "/candidate",
            "stateDir": "/outside/run",
            "memoryMib": 2048,
            "cpus": 2,
            "width": 1280,
            "height": 720,
            "hostBootId": null
        });
        assert!(serde_json::from_value::<RunConfig>(value).is_err());
    }

    #[test]
    fn guest_state_paths_must_be_relative_and_disjoint() {
        assert!(validate_guest_state_paths(vec![PathBuf::from("../cache")]).is_err());
        assert!(
            validate_guest_state_paths(vec![PathBuf::from("cache"), PathBuf::from("cache/npm")])
                .is_err()
        );
        assert_eq!(
            validate_guest_state_paths(vec![PathBuf::from("tmp"), PathBuf::from("cache")]).unwrap(),
            vec![PathBuf::from("cache"), PathBuf::from("tmp")]
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn start_rejects_non_linux_hosts_before_launching_processes() {
        let config = RunConfig {
            id: Uuid::nil(),
            base_image: "/images/base.qcow2".into(),
            workspace_root: "/outside/run/workspace".into(),
            candidate_workspace: "/outside/run/workspace/current".into(),
            guest_state_paths: vec![],
            state_dir: "/outside/run".into(),
            memory_mib: 2048,
            cpus: 2,
            width: 1280,
            height: 720,
            host_boot_id: None,
            guest_ssh_port: 2222,
            guest_ssh_user: "avm".into(),
            guest_ssh_private_key: "/keys/guest".into(),
            guest_ssh_host_public_key: "/keys/host.pub".into(),
        };
        let error = VmController::new(config).start().await.unwrap_err();
        assert_eq!(error.to_string(), "QEMU/KVM runs require a Linux host");
    }
}
