use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, ensure};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    event::{EventSink, RawEvent, monotonic_ns},
    fingerprint::repository_fingerprint,
    timeline::ExperienceEventSink,
    vm::RunConfig,
};

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

    pub fn publish_workspace(&self, session_id: Uuid, sink: &dyn EventSink) -> Result<String> {
        let transfer_id = Uuid::new_v4();
        let transfers = self.state_dir.join("transfers");
        std::fs::create_dir_all(&transfers)?;
        let archive = transfers.join(format!("{transfer_id}.tar"));
        let file_list = transfers.join(format!("{transfer_id}.files"));
        sink.record(RawEvent::observed(
            session_id,
            "transport",
            "remote.workspace_publish.started",
            json!({"channelId": self.id, "transferId": transfer_id}),
        ))?;
        let (source_fingerprint, file_count) =
            create_git_workspace_archive(&self.local_candidate, &archive, &file_list)?;
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
        ])?;
        ensure!(
            applied.status.success(),
            "remote apply failed: {}",
            String::from_utf8_lossy(&applied.stderr)
        );
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
            }),
        ))?;
        Ok(fingerprint)
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
        .args(["-C"])
        .arg(repository)
        .args(["--null", "--files-from"])
        .arg(file_list)
        .args(["--no-recursion", "-cf"])
        .arg(archive)
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

pub fn apply_transfer(run: &Path, transfer_id: Uuid, expected_sha256: &str) -> Result<String> {
    ensure!(
        expected_sha256.len() == 64 && expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid SHA-256 digest"
    );
    let config = load_run(run)?;
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
    let unpack = Command::new("tar")
        .args(["--no-same-owner", "--no-same-permissions", "-xf"])
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .status()?;
    ensure!(unpack.success(), "tar extraction failed with {unpack}");
    let sync = Command::new("rsync")
        .args(["--archive", "--delete", "--exclude=.git"])
        .arg(format!("{}/", staging.display()))
        .arg(format!("{}/", config.candidate_workspace.display()))
        .status()?;
    ensure!(sync.success(), "rsync failed with {sync}");
    std::fs::remove_dir_all(&staging)?;
    std::fs::remove_file(&archive)?;
    let fingerprint = repository_fingerprint(&config.candidate_workspace)?;
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
        }),
    ))?;
    Ok(fingerprint)
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

fn file_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()))
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
