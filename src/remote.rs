use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    coordination::RunLock,
    event::{EventSink, RawEvent, monotonic_ns},
    fingerprint::repository_fingerprint,
    guest_command::active_command_ids,
    integrity::file_sha256,
    timeline::ExperienceEventSink,
    vm::RunConfig,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicationManifest {
    transfer_id: Uuid,
    generation_id: String,
    previous_generation: String,
    archive_sha256: String,
    publication_fingerprint: String,
    repository_fingerprint: String,
    source_repository_fingerprint: String,
    entries: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicationReceipt {
    pub channel_id: Uuid,
    pub run_id: Uuid,
    pub outcome: String,
    pub source_repository_fingerprint: String,
    pub materialized_repository_fingerprint: String,
    pub transfer_id: Option<Uuid>,
    pub forced: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEndpoint {
    pub purpose: String,
    pub guest_port: u16,
    pub remote_port: u16,
    pub local_port: u16,
    pub url: String,
    pub ready: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConnection {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub run_id: Uuid,
    pub pid: u32,
    pub created_at: String,
    pub log: PathBuf,
    pub endpoints: Vec<RemoteEndpoint>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteChannelConfig {
    pub id: Uuid,
    pub run_id: Uuid,
    pub project: String,
    pub zone: String,
    pub instance: String,
    pub remote_run: PathBuf,
    pub remote_candidate: PathBuf,
    pub remote_avm: PathBuf,
    pub local_candidate: PathBuf,
    pub state_dir: PathBuf,
}

impl RemoteChannelConfig {
    pub fn create(
        local_candidate: &Path,
        state_root: &Path,
        project: String,
        zone: String,
        instance: String,
        remote_run: PathBuf,
        remote_avm: PathBuf,
    ) -> Result<Self> {
        validate_endpoint(&project, &zone, &instance)?;
        ensure!(remote_run.is_absolute(), "remote run path must be absolute");
        ensure!(remote_avm.is_absolute(), "remote AVM path must be absolute");
        let local_candidate = local_candidate
            .canonicalize()
            .context("canonicalize local candidate")?;
        ensure!(
            local_candidate.is_dir(),
            "local candidate must be a directory"
        );
        std::fs::create_dir_all(state_root)?;
        let state_root = state_root
            .canonicalize()
            .context("canonicalize state root")?;
        ensure!(
            !state_root.starts_with(&local_candidate),
            "remote channel state must be outside the local candidate"
        );
        let remote: RunConfig = serde_json::from_slice(&ssh_output(
            &project,
            &zone,
            &instance,
            &format!("cat -- {}", shell_quote(&remote_run)),
        )?)
        .context("decode remote run configuration")?;
        ensure!(
            remote.state_dir.join("run.json") == remote_run,
            "remote run path does not match its state directory"
        );
        let id = Uuid::new_v4();
        let config = Self {
            id,
            run_id: remote.id,
            project,
            zone,
            instance,
            remote_run,
            remote_candidate: remote.candidate_workspace,
            remote_avm,
            local_candidate,
            state_dir: state_root.join(id.to_string()),
        };
        config.save()?;
        Ok(config)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let path = if path.is_dir() {
            path.join("channel.json")
        } else {
            path.to_owned()
        };
        let config: Self = serde_json::from_slice(&std::fs::read(&path)?)
            .with_context(|| format!("decode remote channel {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        self.validate()?;
        std::fs::create_dir_all(&self.state_dir)?;
        std::fs::write(
            self.state_dir.join("channel.json"),
            serde_json::to_vec_pretty(self)?,
        )?;
        Ok(())
    }

    pub fn config_path(&self) -> PathBuf {
        self.state_dir.join("channel.json")
    }

    pub fn validate(&self) -> Result<()> {
        validate_endpoint(&self.project, &self.zone, &self.instance)?;
        ensure!(
            self.remote_run.is_absolute(),
            "remote run path must be absolute"
        );
        ensure!(
            self.remote_candidate.is_absolute(),
            "remote candidate must be absolute"
        );
        ensure!(
            self.remote_avm.is_absolute(),
            "remote AVM path must be absolute"
        );
        let candidate = self.local_candidate.canonicalize()?;
        let state = self
            .state_dir
            .canonicalize()
            .unwrap_or_else(|_| self.state_dir.clone());
        ensure!(
            !state.starts_with(candidate),
            "remote channel state must be outside the local candidate"
        );
        Ok(())
    }

    pub fn connect_event_sink(&self) -> Result<Arc<dyn EventSink>> {
        Ok(Arc::new(RemoteRelaySink::connect(self.clone())?))
    }

    pub fn publish_workspace(
        &self,
        session_id: Uuid,
        sink: &dyn EventSink,
        force: bool,
    ) -> Result<PublicationReceipt> {
        let source_fingerprint = repository_fingerprint(&self.local_candidate)?;
        if !force {
            let status: Value = self.remote_json(&[
                "publication-status".into(),
                "--run".into(),
                self.remote_run.display().to_string(),
            ])?;
            if publication_is_unchanged(&status, self.run_id, &source_fingerprint) {
                let materialized = status["repositoryFingerprint"]
                    .as_str()
                    .context("publication status omitted repository fingerprint")?
                    .to_owned();
                sink.record(RawEvent::observed(
                    session_id,
                    "transport",
                    "remote.workspace_publish.unchanged",
                    json!({
                        "channelId": self.id,
                        "runId": self.run_id,
                        "repositoryFingerprint": source_fingerprint,
                    }),
                ))?;
                return Ok(PublicationReceipt {
                    channel_id: self.id,
                    run_id: self.run_id,
                    outcome: "unchanged".into(),
                    source_repository_fingerprint: source_fingerprint,
                    materialized_repository_fingerprint: materialized,
                    transfer_id: None,
                    forced: false,
                });
            }
        }
        let transfer_id = Uuid::new_v4();
        let transfers = self.state_dir.join("transfers");
        std::fs::create_dir_all(&transfers)?;
        let archive = transfers.join(format!("{transfer_id}.tar"));
        let file_list = transfers.join(format!("{transfer_id}.files"));
        sink.record(RawEvent::observed(
            session_id,
            "transport",
            "remote.workspace_publish.started",
            json!({"channelId": self.id, "transferId": transfer_id, "forced": force}),
        ))?;
        let result = (|| -> Result<PublicationReceipt> {
            let (archived_fingerprint, file_count) =
                create_git_workspace_archive(&self.local_candidate, &archive, &file_list)?;
            ensure!(
                archived_fingerprint == source_fingerprint,
                "source changed while publication archive was created; retry"
            );
            let digest = file_sha256(&archive)?;
            let prepare = self.remote_command(&[
                "prepare-transfer".into(),
                "--run".into(),
                self.remote_run.display().to_string(),
                "--transfer-id".into(),
                transfer_id.to_string(),
            ])?;
            let remote_archive = String::from_utf8(prepare.stdout)?;
            let remote_archive = remote_archive.trim();
            ensure!(!remote_archive.is_empty(), "remote transfer path is empty");
            let copy = Command::new("gcloud")
                .args([
                    "compute",
                    "scp",
                    "--quiet",
                    "--project",
                    &self.project,
                    "--zone",
                    &self.zone,
                ])
                .arg(&archive)
                .arg(format!("{}:{remote_archive}", self.instance))
                .status()
                .context("upload workspace archive")?;
            ensure!(copy.success(), "gcloud scp failed with {copy}");
            let applied = self.remote_command(&[
                "apply-transfer".into(),
                "--run".into(),
                self.remote_run.display().to_string(),
                "--transfer-id".into(),
                transfer_id.to_string(),
                "--sha256".into(),
                digest.clone(),
                "--source-fingerprint".into(),
                source_fingerprint.clone(),
            ])?;
            std::fs::remove_file(&archive)?;
            let fingerprint = String::from_utf8(applied.stdout)?.trim().to_owned();
            sink.record(RawEvent::observed(
                session_id,
                "transport",
                "remote.workspace_publish.completed",
                json!({
                    "channelId": self.id,
                    "transferId": transfer_id,
                    "archiveSha256": digest,
                    "fileCount": file_count,
                    "sourceRepositoryFingerprint": source_fingerprint,
                    "materializedRepositoryFingerprint": fingerprint,
                    "forced": force,
                }),
            ))?;
            Ok(PublicationReceipt {
                channel_id: self.id,
                run_id: self.run_id,
                outcome: "published".into(),
                source_repository_fingerprint: source_fingerprint.clone(),
                materialized_repository_fingerprint: fingerprint,
                transfer_id: Some(transfer_id),
                forced: force,
            })
        })();
        if let Err(error) = &result {
            let _ = sink.record(RawEvent::observed(
                session_id,
                "transport",
                "remote.workspace_publish.failed",
                json!({
                    "channelId": self.id,
                    "transferId": transfer_id,
                    "message": error.to_string(),
                    "localArchiveRetained": archive.is_file(),
                }),
            ));
        }
        result
    }

    fn remote_command(&self, arguments: &[String]) -> Result<std::process::Output> {
        let mut command = shell_quote(&self.remote_avm);
        for argument in arguments {
            command.push(' ');
            command.push_str(&shell_quote(Path::new(argument)));
        }
        let output = ssh_command(&self.project, &self.zone, &self.instance, &command)
            .output()
            .context("run remote AVM command")?;
        ensure!(
            output.status.success(),
            "remote AVM command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(output)
    }

    pub fn doctor_report(&self) -> Result<Vec<u8>> {
        let output = self.remote_command(&[
            "doctor-host".into(),
            "--run".into(),
            self.remote_run.display().to_string(),
        ])?;
        Ok(output.stdout)
    }

    pub fn rollback_publication(&self, transfer_id: Uuid) -> Result<String> {
        let output = self.remote_command(&[
            "rollback-transfer".into(),
            "--run".into(),
            self.remote_run.display().to_string(),
            "--transfer-id".into(),
            transfer_id.to_string(),
        ])?;
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    }

    pub fn connect(&self, browser: bool, web_ui_ports: &[u16]) -> Result<RemoteConnection> {
        ensure!(
            browser || !web_ui_ports.is_empty(),
            "request at least one remote endpoint"
        );
        let remote: RunConfig = serde_json::from_slice(&ssh_output(
            &self.project,
            &self.zone,
            &self.instance,
            &format!("cat -- {}", shell_quote(&self.remote_run)),
        )?)?;
        ensure!(
            remote.id == self.run_id,
            "remote channel run identity changed"
        );
        self.remote_command(&[
            "exec-list".into(),
            "--run".into(),
            self.remote_run.display().to_string(),
        ])?;

        let id = Uuid::new_v4();
        let mut guest_ports = Vec::new();
        if browser {
            guest_ports.push(("browser".to_owned(), 9222));
        }
        for port in web_ui_ports {
            ensure!(*port > 0, "Web UI port must be nonzero");
            guest_ports.push((format!("web-ui-{port}"), *port));
        }
        guest_ports.sort();
        guest_ports.dedup();
        let remote_start = 30_000 + u16::from(id.as_bytes()[0]) * 32;
        let mut endpoints = Vec::new();
        for (index, (purpose, guest_port)) in guest_ports.into_iter().enumerate() {
            let local_port = available_local_port()?;
            let remote_port = remote_start
                .checked_add(index as u16)
                .context("too many requested endpoints")?;
            endpoints.push(RemoteEndpoint {
                purpose,
                guest_port,
                remote_port,
                local_port,
                url: format!("http://127.0.0.1:{local_port}"),
                ready: false,
            });
        }

        let connection_dir = self.state_dir.join("connections").join(id.to_string());
        std::fs::create_dir_all(&connection_dir)?;
        let log = connection_dir.join("tunnel.log");
        let log_file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&log)?;
        let known_hosts = remote.state_dir.join("guest_known_hosts");
        let mut nested = vec![
            "env".into(),
            format!("AVM_CONNECTION_ID={id}"),
            "ssh".into(),
            "-N".into(),
            "-T".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ExitOnForwardFailure=yes".into(),
            "-o".into(),
            "StrictHostKeyChecking=yes".into(),
            "-o".into(),
            format!("UserKnownHostsFile={}", known_hosts.display()),
            "-i".into(),
            remote.guest_ssh_private_key.display().to_string(),
            "-p".into(),
            remote.guest_ssh_port.to_string(),
        ];
        for endpoint in &endpoints {
            nested.extend([
                "-L".into(),
                format!(
                    "127.0.0.1:{}:127.0.0.1:{}",
                    endpoint.remote_port, endpoint.guest_port
                ),
            ]);
        }
        nested.push(format!("{}@127.0.0.1", remote.guest_ssh_user));
        let nested = nested
            .iter()
            .map(|argument| shell_quote(Path::new(argument)))
            .collect::<Vec<_>>()
            .join(" ");
        let mut command = Command::new("gcloud");
        command.args([
            "compute",
            "ssh",
            "--quiet",
            "--project",
            &self.project,
            "--zone",
            &self.zone,
        ]);
        command.arg("--ssh-flag=-o=ExitOnForwardFailure=yes");
        for endpoint in &endpoints {
            command.arg(format!(
                "--ssh-flag=-L={}:127.0.0.1:{}",
                endpoint.local_port, endpoint.remote_port
            ));
        }
        let child = command
            .arg(&self.instance)
            .arg("--command")
            .arg(nested)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file.try_clone()?))
            .stderr(Stdio::from(log_file))
            .process_group(0)
            .spawn()
            .context("start owned remote development tunnel")?;
        let mut connection = RemoteConnection {
            id,
            channel_id: self.id,
            run_id: self.run_id,
            pid: child.id(),
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
            log,
            endpoints,
        };
        write_json_atomic(&connection_dir.join("connection.json"), &connection)?;
        wait_for_connection(&mut connection, Duration::from_secs(15))?;
        write_json_atomic(&connection_dir.join("connection.json"), &connection)?;
        Ok(connection)
    }

    pub fn connection_status(&self, path: &Path) -> Result<Value> {
        let connection = load_connection(path)?;
        ensure!(
            connection.channel_id == self.id,
            "connection belongs to another channel"
        );
        let owned = owned_connection_process(&connection);
        let outer_host_reachable =
            ssh_output(&self.project, &self.zone, &self.instance, "true").is_ok();
        let guest_ssh_reachable = outer_host_reachable
            && self
                .remote_command(&[
                    "exec-list".into(),
                    "--run".into(),
                    self.remote_run.display().to_string(),
                ])
                .is_ok();
        let endpoints = connection
            .endpoints
            .iter()
            .map(|endpoint| {
                let ready = owned && endpoint_ready(endpoint);
                json!({
                    "purpose": endpoint.purpose,
                    "url": endpoint.url,
                    "localPort": endpoint.local_port,
                    "guestPort": endpoint.guest_port,
                    "ready": ready,
                    "failureLayer": if !outer_host_reachable { Some("outer-host") } else if !guest_ssh_reachable { Some("guest-ssh") } else if !owned { Some("forwarding-process") } else if !ready && endpoint.purpose == "browser" { Some("guest-cdp") } else if !ready { Some("guest-web-ui") } else { None },
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "connectionId": connection.id,
            "channelId": connection.channel_id,
            "runId": connection.run_id,
            "processOwned": owned,
            "outerHostReachable": outer_host_reachable,
            "guestSshReachable": guest_ssh_reachable,
            "pid": connection.pid,
            "endpoints": endpoints,
            "log": connection.log,
        }))
    }

    pub fn stop_connection(&self, path: &Path) -> Result<Value> {
        let connection = load_connection(path)?;
        ensure!(
            connection.channel_id == self.id,
            "connection belongs to another channel"
        );
        let stopped = if owned_connection_process(&connection) {
            // SAFETY: ownership is verified by the unguessable connection ID in
            // the recorded process command before signalling its process group.
            unsafe { libc::kill(-(connection.pid as i32), libc::SIGTERM) };
            let deadline = Instant::now() + Duration::from_secs(3);
            while owned_connection_process(&connection) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            if owned_connection_process(&connection) {
                // SAFETY: ownership was checked again after the grace period.
                unsafe { libc::kill(-(connection.pid as i32), libc::SIGKILL) };
            }
            true
        } else {
            false
        };
        Ok(json!({
            "connectionId": connection.id,
            "channelId": connection.channel_id,
            "stopped": stopped,
        }))
    }

    pub fn guest_exec_start(
        &self,
        cwd: &str,
        argv: &[String],
        idempotency_key: Option<&str>,
    ) -> Result<Value> {
        let mut arguments = vec![
            "exec".into(),
            "--run".into(),
            self.remote_run.display().to_string(),
            "--cwd".into(),
            cwd.into(),
            "--detach".into(),
        ];
        if let Some(key) = idempotency_key {
            arguments.extend(["--idempotency-key".into(), key.into()]);
        }
        arguments.push("--".into());
        arguments.extend(argv.iter().cloned());
        self.remote_json(&arguments)
    }

    pub fn guest_exec_operation(
        &self,
        operation: &str,
        command_id: Option<Uuid>,
        option: Option<(&str, u64)>,
    ) -> Result<Value> {
        ensure!(
            matches!(
                operation,
                "exec-list" | "exec-status" | "exec-wait" | "exec-attach" | "exec-cancel"
            ),
            "unsupported remote guest command operation"
        );
        let mut arguments = vec![
            operation.into(),
            "--run".into(),
            self.remote_run.display().to_string(),
        ];
        if let Some((name, value)) = option {
            arguments.extend([format!("--{name}"), value.to_string()]);
        }
        if let Some(command_id) = command_id {
            arguments.push(command_id.to_string());
        }
        self.remote_json(&arguments)
    }

    fn remote_json(&self, arguments: &[String]) -> Result<Value> {
        let output = self.remote_command(arguments)?;
        serde_json::from_slice(&output.stdout).context("decode remote AVM JSON response")
    }
}

fn publication_is_unchanged(status: &Value, run_id: Uuid, source_fingerprint: &str) -> bool {
    status["runId"] == run_id.to_string()
        && status["active"] == true
        && status["sourceRepositoryFingerprint"] == source_fingerprint
}

fn available_local_port() -> Result<u16> {
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

fn load_connection(path: &Path) -> Result<RemoteConnection> {
    let path = if path.is_dir() {
        path.join("connection.json")
    } else {
        path.to_owned()
    };
    serde_json::from_slice(&std::fs::read(&path)?)
        .with_context(|| format!("decode remote connection {}", path.display()))
}

fn owned_connection_process(connection: &RemoteConnection) -> bool {
    let output = Command::new("ps")
        .args(["-p", &connection.pid.to_string(), "-o", "command="])
        .output();
    output.is_ok_and(|output| {
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains(&connection.id.to_string())
    })
}

fn wait_for_connection(connection: &mut RemoteConnection, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        ensure!(
            owned_connection_process(connection),
            "remote tunnel exited before its endpoints became ready; inspect {}",
            connection.log.display()
        );
        let mut all_ready = true;
        for endpoint in &mut connection.endpoints {
            endpoint.ready = endpoint_ready(endpoint);
            all_ready &= endpoint.ready;
        }
        if all_ready {
            return Ok(());
        }
        if Instant::now() >= deadline {
            // SAFETY: the connection process is still ownership-verified above.
            unsafe { libc::kill(-(connection.pid as i32), libc::SIGTERM) };
            bail!(
                "remote tunnel readiness timed out; inspect {}",
                connection.log.display()
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn endpoint_ready(endpoint: &RemoteEndpoint) -> bool {
    if endpoint.purpose != "browser" {
        return TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], endpoint.local_port)),
            Duration::from_millis(300),
        )
        .is_ok();
    }
    let Ok(mut stream) = TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], endpoint.local_port)),
        Duration::from_millis(300),
    ) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    if stream
        .write_all(b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut response = String::new();
    stream.read_to_string(&mut response).is_ok()
        && response.starts_with("HTTP/1.1 200")
        && response.contains("webSocketDebuggerUrl")
}

fn create_git_workspace_archive(
    repository: &Path,
    archive: &Path,
    file_list: &Path,
) -> Result<(String, usize)> {
    let before = repository_fingerprint(repository)?;
    let file_count = write_git_transfer_list(repository, file_list)?;
    let status = Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        // Keep every option before the file-list operand. BSD tar tolerates
        // positional options here, while GNU tar rejects them; AVM development
        // must produce the same bounded archive on both hosts.
        .args(["--null", "--no-recursion", "-C"])
        .arg(repository)
        .arg("-cf")
        .arg(archive)
        .arg("--files-from")
        .arg(file_list)
        .status()
        .context("create workspace transfer archive");
    let _ = std::fs::remove_file(file_list);
    let status = status?;
    ensure!(status.success(), "tar failed with {status}");
    let after = repository_fingerprint(repository)?;
    if before != after {
        let _ = std::fs::remove_file(archive);
    }
    ensure!(
        before == after,
        "candidate changed while the remote transfer archive was created"
    );
    Ok((before, file_count))
}

fn write_git_transfer_list(repository: &Path, output: &Path) -> Result<usize> {
    let top_level = git_output(repository, &["rev-parse", "--show-toplevel"])
        .context("remote publishing requires a Git worktree")?;
    let top_level = PathBuf::from(String::from_utf8(top_level)?.trim()).canonicalize()?;
    ensure!(
        top_level == repository.canonicalize()?,
        "remote candidate must be the root of its Git worktree"
    );
    let staged = git_output(repository, &["ls-files", "--stage", "-z"])?;
    ensure!(
        !staged
            .split(|byte| *byte == 0)
            .any(|entry| entry.starts_with(b"160000 ")),
        "remote publishing does not yet support Git submodules"
    );
    let listed = git_output(
        repository,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;
    let mut writer = BufWriter::new(File::create(output)?);
    let mut count = 0;
    for relative in listed
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        #[cfg(unix)]
        let relative_path = {
            use std::os::unix::ffi::OsStrExt;
            Path::new(std::ffi::OsStr::from_bytes(relative))
        };
        #[cfg(not(unix))]
        let relative_path = Path::new(std::str::from_utf8(relative)?);
        ensure!(
            !relative_path.is_absolute()
                && !relative_path.components().any(|component| {
                    matches!(
                        component,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                }),
            "Git returned a path outside the candidate"
        );
        if std::fs::symlink_metadata(repository.join(relative_path)).is_ok() {
            writer.write_all(relative)?;
            writer.write_all(&[0])?;
            count += 1;
        }
    }
    writer.flush()?;
    Ok(count)
}

fn git_output(repository: &Path, arguments: &[&str]) -> Result<Vec<u8>> {
    let listed = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .with_context(|| format!("run git {}", arguments.join(" ")))?;
    ensure!(
        listed.status.success(),
        "git {} failed: {}",
        arguments.join(" "),
        String::from_utf8_lossy(&listed.stderr).trim()
    );
    Ok(listed.stdout)
}

pub struct TeeEventSink {
    first: Arc<dyn EventSink>,
    second: Arc<dyn EventSink>,
}

impl TeeEventSink {
    pub fn new(first: Arc<dyn EventSink>, second: Arc<dyn EventSink>) -> Self {
        Self { first, second }
    }
}

impl EventSink for TeeEventSink {
    fn record(&self, event: RawEvent) -> Result<()> {
        self.first.record(event.clone())?;
        self.second.record(event)
    }
}

struct RelayProcess {
    _child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_sequence: u64,
}

pub struct RemoteRelaySink {
    channel: RemoteChannelConfig,
    process: Mutex<RelayProcess>,
}

impl RemoteRelaySink {
    fn connect(channel: RemoteChannelConfig) -> Result<Self> {
        let command = format!(
            "{} event-relay --run {}",
            shell_quote(&channel.remote_avm),
            shell_quote(&channel.remote_run)
        );
        let mut child = ssh_command(&channel.project, &channel.zone, &channel.instance, &command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("start remote event relay")?;
        let stdin = BufWriter::new(child.stdin.take().context("event relay has no stdin")?);
        let mut stdout = BufReader::new(child.stdout.take().context("event relay has no stdout")?);
        let mut ready = String::new();
        stdout.read_line(&mut ready)?;
        let ready: Value = serde_json::from_str(&ready).context("decode relay ready message")?;
        ensure!(
            ready["ready"] == true,
            "remote event relay did not become ready"
        );
        ensure!(
            ready["runId"] == channel.run_id.to_string(),
            "remote event relay opened a different run"
        );
        Ok(Self {
            channel,
            process: Mutex::new(RelayProcess {
                _child: child,
                stdin,
                stdout,
                next_sequence: 0,
            }),
        })
    }
}

impl EventSink for RemoteRelaySink {
    fn record(&self, mut event: RawEvent) -> Result<()> {
        if event.repository_fingerprint.is_none() {
            event.repository_fingerprint =
                Some(repository_fingerprint(&self.channel.local_candidate)?);
        }
        let mut process = self.process.lock().expect("remote relay mutex poisoned");
        let sequence = process.next_sequence;
        process.next_sequence += 1;
        serde_json::to_writer(
            &mut process.stdin,
            &json!({"sequence": sequence, "event": event}),
        )?;
        process.stdin.write_all(b"\n")?;
        process.stdin.flush()?;
        let mut line = String::new();
        process.stdout.read_line(&mut line)?;
        ensure!(
            !line.is_empty(),
            "remote event relay closed before acknowledging event"
        );
        let acknowledgement: Value = serde_json::from_str(&line)?;
        ensure!(
            acknowledgement["sequence"] == sequence,
            "relay acknowledgement out of order"
        );
        ensure!(
            acknowledgement["stored"] == true,
            "remote relay rejected event"
        );
        Ok(())
    }
}

pub fn serve_event_relay(run: &Path) -> Result<()> {
    let config = RunConfig::load(if run.is_dir() {
        run.join("run.json")
    } else {
        run.to_owned()
    })?;
    let paths = config.paths();
    let sink = ExperienceEventSink::open_dynamic(
        paths.timeline,
        paths.events,
        &config.candidate_workspace,
    )?;
    println!("{}", json!({"ready": true, "runId": config.id}));
    std::io::stdout().flush()?;
    for line in std::io::stdin().lock().lines() {
        let envelope: Value = serde_json::from_str(&line?)?;
        let sequence = envelope["sequence"]
            .as_u64()
            .context("relay event has no sequence")?;
        let mut event: RawEvent = serde_json::from_value(envelope["event"].clone())?;
        let source_time = json!({
            "transport": "avm-ssh-jsonl-v1",
            "sourceSessionId": event.session_id,
            "sourceHostMonotonicNs": event.host_monotonic_ns,
            "sourceWallClockTime": event.wall_clock_time,
        });
        event.session_id = config.id;
        event.host_monotonic_ns = monotonic_ns();
        event.wall_clock_time = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
        event.source_timestamp = Some(source_time);
        let event_id = event.id;
        let remote_time = event.host_monotonic_ns;
        sink.record(event)?;
        println!(
            "{}",
            json!({
                "sequence": sequence,
                "stored": true,
                "eventId": event_id,
                "remoteHostMonotonicNs": remote_time,
            })
        );
        std::io::stdout().flush()?;
    }
    Ok(())
}

pub fn prepare_transfer(run: &Path, transfer_id: Uuid) -> Result<PathBuf> {
    let config = load_run(run)?;
    let incoming = config.state_dir.join("transport").join("incoming");
    std::fs::create_dir_all(&incoming)?;
    let archive = incoming.join(format!("{transfer_id}.tar"));
    ensure!(!archive.exists(), "transfer already exists");
    Ok(archive)
}

pub fn apply_transfer(
    run: &Path,
    transfer_id: Uuid,
    expected_sha256: &str,
    source_repository_fingerprint: &str,
) -> Result<String> {
    ensure!(
        expected_sha256.len() == 64 && expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid SHA-256 digest"
    );
    ensure!(
        source_repository_fingerprint
            .strip_prefix("sha256:")
            .is_some_and(
                |digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            ),
        "invalid source repository fingerprint"
    );
    let config = load_run(run)?;
    let _lock = RunLock::exclusive(&config.state_dir, "workspace publication")?;
    let active = active_command_ids(&config)?;
    ensure!(
        active.is_empty(),
        "workspace publication is blocked by active guest commands: {}",
        active.join(", ")
    );
    let transport = config.state_dir.join("transport");
    let archive = transport
        .join("incoming")
        .join(format!("{transfer_id}.tar"));
    ensure!(archive.is_file(), "transfer archive is missing");
    ensure!(
        file_sha256(&archive)? == expected_sha256.to_ascii_lowercase(),
        "transfer digest mismatch"
    );
    let staging = transport.join("staging").join(transfer_id.to_string());
    ensure!(!staging.exists(), "transfer staging path already exists");
    std::fs::create_dir_all(&staging)?;
    extract_publication_archive(&archive, &staging)?;

    let entries = publication_entries(&staging)?;
    for state in &config.guest_state_paths {
        ensure!(
            std::fs::symlink_metadata(staging.join(state)).is_err(),
            "publication path conflicts with declared guest state: {}",
            state.display()
        );
    }
    for entry in &entries {
        let entry = Path::new(entry);
        for state in &config.guest_state_paths {
            ensure!(
                !entry.starts_with(state) && !state.starts_with(entry),
                "publication path conflicts with declared guest state: {}",
                entry.display()
            );
        }
    }
    let publication_fingerprint = publication_fingerprint(&staging, &entries)?;
    let active_manifest_path = transport.join("active-publication.json");
    let previous_generation = current_generation(&config)?;
    let generation_id = transfer_id.to_string();
    let generation = config
        .workspace_root
        .join("generations")
        .join(&generation_id);
    ensure!(
        !generation.exists(),
        "publication generation already exists"
    );
    std::fs::rename(&staging, &generation)?;
    install_state_links(&config, &generation)?;
    let fingerprint = repository_fingerprint(&generation)?;
    let manifest = PublicationManifest {
        transfer_id,
        generation_id: generation_id.clone(),
        previous_generation: previous_generation.clone(),
        archive_sha256: expected_sha256.to_ascii_lowercase(),
        publication_fingerprint: publication_fingerprint.clone(),
        repository_fingerprint: fingerprint.clone(),
        source_repository_fingerprint: source_repository_fingerprint.to_owned(),
        entries,
    };
    let journal = transport.join(format!("activation-{transfer_id}.json"));
    write_json_atomic(
        &journal,
        &json!({
            "transferId": transfer_id,
            "phase": "activation_pending",
            "previousGeneration": previous_generation,
            "generationId": generation_id,
            "newFingerprint": fingerprint,
        }),
    )?;
    sync_tree(&generation)?;
    activate_generation(&config, &generation_id)?;
    let manifests = transport.join("publication-manifests");
    std::fs::create_dir_all(&manifests)?;
    write_json_atomic(&manifests.join(format!("{generation_id}.json")), &manifest)?;
    write_json_atomic(&active_manifest_path, &manifest)?;
    write_json_atomic(
        &journal,
        &json!({
            "transferId": transfer_id,
            "phase": "active",
            "previousGeneration": previous_generation,
            "generationId": generation_id,
            "repositoryFingerprint": fingerprint,
            "publicationFingerprint": publication_fingerprint,
        }),
    )?;
    let paths = config.paths();
    let sink = ExperienceEventSink::open_dynamic(
        paths.timeline,
        paths.events,
        &config.candidate_workspace,
    )?;
    sink.record(RawEvent::observed(
        config.id,
        "transport",
        "remote.workspace_applied",
        json!({
            "transferId": transfer_id,
            "archiveSha256": expected_sha256,
            "repositoryFingerprint": fingerprint,
            "publicationFingerprint": manifest.publication_fingerprint,
            "previousGeneration": manifest.previous_generation,
            "generationId": manifest.generation_id,
            "activation": "atomic_symlink",
            "fileCount": manifest.entries.len(),
        }),
    ))?;
    std::fs::remove_file(&archive)?;
    Ok(fingerprint)
}

pub fn publication_status(run: &Path) -> Result<Value> {
    let config = load_run(run)?;
    let active_manifest = config.state_dir.join("transport/active-publication.json");
    if !active_manifest.is_file() {
        return Ok(json!({"runId": config.id, "active": false}));
    }
    let manifest: PublicationManifest = serde_json::from_slice(&std::fs::read(active_manifest)?)?;
    let active = current_generation(&config)? == manifest.generation_id;
    Ok(json!({
        "runId": config.id,
        "hostBootId": config.host_boot_id,
        "active": active,
        "generationId": manifest.generation_id,
        "sourceRepositoryFingerprint": manifest.source_repository_fingerprint,
        "repositoryFingerprint": manifest.repository_fingerprint,
        "transferId": manifest.transfer_id,
    }))
}

pub fn rollback_transfer(run: &Path, transfer_id: Uuid) -> Result<String> {
    let config = load_run(run)?;
    let _lock = RunLock::exclusive(&config.state_dir, "publication rollback")?;
    let active = active_command_ids(&config)?;
    ensure!(
        active.is_empty(),
        "publication rollback is blocked by active guest commands: {}",
        active.join(", ")
    );
    let transport = config.state_dir.join("transport");
    let journal: Value = serde_json::from_slice(&std::fs::read(
        transport.join(format!("activation-{transfer_id}.json")),
    )?)?;
    let previous = journal["previousGeneration"]
        .as_str()
        .context("activation journal has no previous generation")?;
    let published = journal["generationId"]
        .as_str()
        .context("activation journal has no published generation")?;
    ensure!(
        current_generation(&config)? == published,
        "rollback target is not the active publication"
    );
    ensure!(
        config
            .workspace_root
            .join("generations")
            .join(previous)
            .is_dir(),
        "retained previous generation is missing"
    );
    activate_generation(&config, previous)?;
    let active_manifest = transport.join("active-publication.json");
    let previous_manifest = transport
        .join("publication-manifests")
        .join(format!("{previous}.json"));
    if previous_manifest.is_file() {
        let value: PublicationManifest =
            serde_json::from_slice(&std::fs::read(&previous_manifest)?)?;
        write_json_atomic(&active_manifest, &value)?;
    } else if active_manifest.exists() {
        std::fs::remove_file(&active_manifest)?;
        sync_directory(&transport)?;
    }
    let fingerprint = repository_fingerprint(&config.candidate_workspace)?;
    write_json_atomic(
        &transport.join(format!("activation-{transfer_id}.json")),
        &json!({
            "transferId": transfer_id,
            "phase": "rolled_back",
            "repositoryFingerprint": fingerprint,
            "generationId": published,
            "previousGeneration": previous,
            "activeGeneration": previous,
        }),
    )?;
    let paths = config.paths();
    ExperienceEventSink::open_dynamic(paths.timeline, paths.events, &config.candidate_workspace)?
        .record(RawEvent::observed(
        config.id,
        "transport",
        "remote.workspace_rolled_back",
        json!({
            "transferId": transfer_id,
            "repositoryFingerprint": fingerprint,
            "generationId": previous,
        }),
    ))?;
    Ok(fingerprint)
}

fn current_generation(config: &RunConfig) -> Result<String> {
    let target = std::fs::read_link(&config.candidate_workspace)
        .context("read current workspace generation")?;
    target
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .context("current workspace link has no generation name")
}

fn install_state_links(config: &RunConfig, generation: &Path) -> Result<()> {
    use std::os::unix::fs::symlink;

    for relative in &config.guest_state_paths {
        let link = generation.join(relative);
        ensure!(
            std::fs::symlink_metadata(&link).is_err(),
            "generation conflicts with declared guest state: {}",
            relative.display()
        );
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut target = PathBuf::new();
        for _ in 0..=relative.components().count() {
            target.push("..");
        }
        target.push("state");
        target.push(relative);
        symlink(target, link)?;
    }
    Ok(())
}

fn activate_generation(config: &RunConfig, generation_id: &str) -> Result<()> {
    use std::os::unix::fs::symlink;

    ensure!(
        generation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "invalid generation identifier"
    );
    let generation = config
        .workspace_root
        .join("generations")
        .join(generation_id);
    ensure!(generation.is_dir(), "workspace generation is missing");
    let temporary = config
        .workspace_root
        .join(format!(".current-{}.tmp", Uuid::new_v4()));
    symlink(Path::new("generations").join(generation_id), &temporary)?;
    std::fs::rename(&temporary, &config.candidate_workspace)?;
    sync_directory(&config.workspace_root)
}

fn extract_publication_archive(archive: &Path, staging: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(File::open(archive)?);
    archive.set_preserve_permissions(false);
    archive.set_preserve_ownerships(false);
    archive.set_unpack_xattrs(false);
    let mut paths = std::collections::BTreeSet::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        ensure!(
            !path.as_os_str().is_empty()
                && path
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_))),
            "archive contains an unsafe path"
        );
        ensure!(
            paths.insert(path.clone()),
            "archive contains a duplicate path"
        );
        let kind = entry.header().entry_type();
        ensure!(
            kind.is_file() || kind.is_dir() || kind.is_symlink(),
            "archive contains an unsupported entry type"
        );
        if kind.is_symlink() {
            let target = entry
                .link_name()?
                .context("archive symlink has no target")?;
            ensure!(
                !target.is_absolute()
                    && target.components().all(|component| !matches!(
                        component,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )),
                "archive symlink escapes the publication"
            );
        }
        ensure!(
            entry.unpack_in(staging)?,
            "archive entry escaped the staging directory"
        );
    }
    Ok(())
}

fn publication_entries(root: &Path) -> Result<Vec<String>> {
    fn visit(root: &Path, directory: &Path, entries: &mut Vec<String>) -> Result<()> {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                visit(root, &path, entries)?;
            } else {
                let relative = path.strip_prefix(root)?;
                ensure!(
                    relative
                        .components()
                        .all(|component| matches!(component, std::path::Component::Normal(_))),
                    "publication entry escaped staging"
                );
                let encoded = relative
                    .to_str()
                    .context("publication paths must be UTF-8")?
                    .to_owned();
                entries.push(encoded);
            }
        }
        Ok(())
    }
    let mut entries = Vec::new();
    visit(root, root, &mut entries)?;
    entries.sort();
    entries.dedup();
    Ok(entries)
}

fn publication_fingerprint(root: &Path, entries: &[String]) -> Result<String> {
    #[cfg(unix)]
    use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
    let mut digest = Sha256::new();
    digest.update(b"avm-publication\0");
    for relative in entries {
        let path = root.join(relative);
        let metadata = std::fs::symlink_metadata(&path)?;
        digest.update(relative.as_bytes());
        digest.update(b"\0");
        if metadata.file_type().is_symlink() {
            digest.update(b"symlink\0");
            let target = std::fs::read_link(&path)?;
            #[cfg(unix)]
            digest.update(target.as_os_str().as_bytes());
            #[cfg(not(unix))]
            digest.update(target.to_string_lossy().as_bytes());
        } else if metadata.is_file() {
            digest.update(b"file\0");
            #[cfg(unix)]
            digest.update(metadata.permissions().mode().to_le_bytes());
            digest.update(file_sha256(&path)?.as_bytes());
        } else {
            bail!("unsupported publication entry type: {relative}");
        }
        digest.update(b"\0");
    }
    Ok(format!("sha256:{}", hex::encode(digest.finalize())))
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("JSON state path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".tmp-{}", Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(value)?;
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        sync_directory(parent)
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn sync_tree(root: &Path) -> Result<()> {
    fn visit(path: &Path) -> Result<()> {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let child = entry.path();
            let metadata = std::fs::symlink_metadata(&child)?;
            if metadata.is_dir() {
                visit(&child)?;
            } else if metadata.is_file() {
                File::open(&child)?.sync_all()?;
            }
        }
        sync_directory(path)
    }
    visit(root)
}

fn load_run(path: &Path) -> Result<RunConfig> {
    let config = RunConfig::load(if path.is_dir() {
        path.join("run.json")
    } else {
        path.to_owned()
    })?;
    config.ensure_current_host_boot()?;
    Ok(config)
}

fn validate_endpoint(project: &str, zone: &str, instance: &str) -> Result<()> {
    for (name, value) in [("project", project), ("zone", zone), ("instance", instance)] {
        ensure!(!value.is_empty(), "{name} must not be empty");
        ensure!(
            value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
            "{name} contains unsupported characters"
        );
    }
    Ok(())
}

fn ssh_command(project: &str, zone: &str, instance: &str, remote_command: &str) -> Command {
    let mut command = Command::new("gcloud");
    command.args([
        "compute",
        "ssh",
        instance,
        "--quiet",
        "--project",
        project,
        "--zone",
        zone,
        "--command",
        remote_command,
    ]);
    command
}

fn ssh_output(project: &str, zone: &str, instance: &str, remote_command: &str) -> Result<Vec<u8>> {
    let output = ssh_command(project, zone, instance, remote_command)
        .output()
        .context("run gcloud compute ssh")?;
    ensure!(
        output.status.success(),
        "remote command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_validation_rejects_shell_metacharacters() {
        assert!(validate_endpoint("project", "zone-a", "instance_1").is_ok());
        assert!(validate_endpoint("project; touch nope", "zone-a", "instance").is_err());
    }

    #[test]
    fn shell_quote_handles_spaces_and_apostrophes() {
        assert_eq!(shell_quote(Path::new("/tmp/a b'c")), "'/tmp/a b'\\''c'");
    }

    #[test]
    fn transfer_archive_contains_only_git_visible_files() {
        let repository = tempfile::tempdir().unwrap();
        git(repository.path(), &["init", "-q"]);
        git(
            repository.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(repository.path(), &["config", "user.name", "AVM Test"]);
        std::fs::write(
            repository.path().join(".gitignore"),
            ".local/\nnode_modules/\n",
        )
        .unwrap();
        std::fs::write(repository.path().join("tracked.txt"), "tracked\n").unwrap();
        std::fs::write(repository.path().join("-leading.txt"), "leading\n").unwrap();
        std::fs::create_dir(repository.path().join(".local")).unwrap();
        std::fs::write(
            repository.path().join(".local/secrets.env"),
            "TOKEN=secret\n",
        )
        .unwrap();
        std::fs::create_dir(repository.path().join("node_modules")).unwrap();
        std::fs::write(repository.path().join("node_modules/large.js"), "ignored\n").unwrap();
        git(
            repository.path(),
            &["add", "--", ".gitignore", "tracked.txt"],
        );
        git(repository.path(), &["commit", "-qm", "base"]);

        let transfer = tempfile::tempdir().unwrap();
        let archive = transfer.path().join("candidate.tar");
        let file_list = transfer.path().join("candidate.files");
        let (_fingerprint, count) =
            create_git_workspace_archive(repository.path(), &archive, &file_list).unwrap();
        assert_eq!(count, 3);
        let listing = Command::new("tar")
            .args(["-tf"])
            .arg(&archive)
            .output()
            .unwrap();
        assert!(listing.status.success());
        let listing = String::from_utf8(listing.stdout).unwrap();
        assert!(listing.lines().any(|path| path == ".gitignore"));
        assert!(listing.lines().any(|path| path == "tracked.txt"));
        assert!(listing.lines().any(|path| path == "-leading.txt"));
        assert!(!listing.contains("secrets.env"));
        assert!(!listing.contains("node_modules"));
        assert!(!file_list.exists());
    }

    #[test]
    fn transfer_archive_fails_closed_outside_a_git_root() {
        let repository = tempfile::tempdir().unwrap();
        let transfer = tempfile::tempdir().unwrap();
        let error = create_git_workspace_archive(
            repository.path(),
            &transfer.path().join("candidate.tar"),
            &transfer.path().join("candidate.files"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("Git worktree"));
    }

    #[test]
    fn publication_entries_are_sorted_and_relative() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("z.txt"), "z").unwrap();
        std::fs::write(root.path().join("src/a.txt"), "a").unwrap();
        assert_eq!(
            publication_entries(root.path()).unwrap(),
            vec!["src/a.txt".to_owned(), "z.txt".to_owned()]
        );
    }

    #[test]
    fn publication_fingerprint_binds_paths_modes_and_content() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), "one").unwrap();
        let entries = publication_entries(root.path()).unwrap();
        let first = publication_fingerprint(root.path(), &entries).unwrap();
        std::fs::write(root.path().join("a.txt"), "two").unwrap();
        let second = publication_fingerprint(root.path(), &entries).unwrap();
        assert_ne!(first, second);
        assert!(first.starts_with("sha256:"));
    }

    #[test]
    fn unchanged_publication_requires_the_same_active_run_and_source() {
        let run_id = Uuid::new_v4();
        let status = json!({
            "runId": run_id,
            "active": true,
            "sourceRepositoryFingerprint": "sha256:source",
        });
        assert!(publication_is_unchanged(&status, run_id, "sha256:source"));
        assert!(!publication_is_unchanged(
            &status,
            Uuid::new_v4(),
            "sha256:source"
        ));
        assert!(!publication_is_unchanged(&status, run_id, "sha256:changed"));
        let failed = json!({
            "runId": run_id,
            "active": false,
            "sourceRepositoryFingerprint": "sha256:source",
        });
        assert!(!publication_is_unchanged(&failed, run_id, "sha256:source"));
    }

    #[test]
    fn browser_endpoint_requires_cdp_metadata() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 256];
            let _ = stream.read(&mut request);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 42\r\n\r\n{\"webSocketDebuggerUrl\":\"ws://example\"}")
                .unwrap();
        });
        let endpoint = RemoteEndpoint {
            purpose: "browser".into(),
            guest_port: 9222,
            remote_port: 31000,
            local_port: port,
            url: format!("http://127.0.0.1:{port}"),
            ready: false,
        };
        assert!(endpoint_ready(&endpoint));
        server.join().unwrap();
    }

    #[test]
    fn archive_extraction_rejects_escaping_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("transfer.tar");
        let file = File::create(&archive_path).unwrap();
        let mut builder = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_path("escape").unwrap();
        header.set_link_name("../../outside").unwrap();
        header.set_cksum();
        builder.append(&header, std::io::empty()).unwrap();
        builder.finish().unwrap();
        let staging = temp.path().join("staging");
        std::fs::create_dir(&staging).unwrap();
        assert!(extract_publication_archive(&archive_path, &staging).is_err());
        assert!(!staging.join("escape").exists());
    }

    #[cfg(unix)]
    #[test]
    fn generation_activation_keeps_a_stable_root_and_external_state() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let bootstrap = workspace.join("generations/bootstrap");
        let next = workspace.join("generations/next");
        std::fs::create_dir_all(&bootstrap).unwrap();
        std::fs::create_dir_all(&next).unwrap();
        std::fs::create_dir_all(workspace.join("state/cache")).unwrap();
        std::fs::write(workspace.join("state/cache/value"), "preserved").unwrap();
        symlink("generations/bootstrap", workspace.join("current")).unwrap();
        let config = RunConfig {
            id: Uuid::nil(),
            base_image: temp.path().join("base.qcow2"),
            workspace_root: workspace.clone(),
            candidate_workspace: workspace.join("current"),
            guest_state_paths: vec![PathBuf::from("cache")],
            state_dir: temp.path().join("run"),
            memory_mib: 2048,
            cpus: 2,
            width: 1280,
            height: 720,
            host_boot_id: None,
            guest_ssh_port: 2222,
            guest_ssh_user: "avm".into(),
            guest_ssh_private_key: temp.path().join("guest-key"),
            guest_ssh_host_public_key: temp.path().join("host-key.pub"),
        };

        install_state_links(&config, &next).unwrap();
        activate_generation(&config, "next").unwrap();

        assert_eq!(current_generation(&config).unwrap(), "next");
        assert_eq!(
            std::fs::read_to_string(config.candidate_workspace.join("cache/value")).unwrap(),
            "preserved"
        );
        assert!(bootstrap.is_dir());
    }

    fn git(repository: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success(), "git {}", arguments.join(" "));
    }
}
