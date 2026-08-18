use std::{io::Write, process::Stdio};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    coordination::RunLock,
    vm::{RunConfig, VmController},
};

pub struct GuestCommandClient {
    config: RunConfig,
}

impl GuestCommandClient {
    pub fn new(config: RunConfig) -> Result<Self> {
        config.ensure_current_host_boot()?;
        ensure!(
            VmController::new(config.clone()).is_running(),
            "VM is not running"
        );
        Ok(Self { config })
    }

    pub fn start(
        &self,
        cwd: &str,
        argv: Vec<String>,
        idempotency_key: Option<String>,
    ) -> Result<Value> {
        ensure!(!argv.is_empty(), "guest command argv is empty");
        let _lock = RunLock::exclusive(&self.config.state_dir, "guest command start")?;
        self.call(json!({
            "operation": "start",
            "commandId": Uuid::new_v4(),
            "cwd": cwd,
            "argv": argv,
            "idempotencyKey": idempotency_key,
        }))
    }

    pub fn list(&self) -> Result<Value> {
        self.call(json!({"operation": "list"}))
    }

    pub fn status(&self, command_id: Uuid) -> Result<Value> {
        self.call(json!({"operation": "status", "commandId": command_id}))
    }

    pub fn wait(&self, command_id: Uuid, timeout_ms: u64) -> Result<Value> {
        self.call(json!({"operation": "wait", "commandId": command_id, "timeoutMs": timeout_ms}))
    }

    pub fn attach(&self, command_id: Uuid) -> Result<Value> {
        self.call(json!({"operation": "attach", "commandId": command_id}))
    }

    pub fn cancel(&self, command_id: Uuid, grace_ms: u64) -> Result<Value> {
        let _lock = RunLock::exclusive(&self.config.state_dir, "guest command cancellation")?;
        self.call(json!({"operation": "cancel", "commandId": command_id, "graceMs": grace_ms}))
    }

    pub fn health(&self) -> Result<Value> {
        self.call(json!({"operation": "health"}))
    }

    fn call(&self, request: Value) -> Result<Value> {
        let identity = &self.config.guest_ssh_private_key;
        let host_public_key = &self.config.guest_ssh_host_public_key;
        ensure!(
            identity.is_file(),
            "guest SSH identity is missing at {}",
            identity.display()
        );
        ensure!(
            host_public_key.is_file(),
            "trusted guest SSH host key is missing at {}",
            host_public_key.display()
        );
        let known_hosts = self.config.state_dir.join("guest_known_hosts");
        prepare_known_hosts(host_public_key, &known_hosts, self.config.guest_ssh_port)?;

        let mut child = std::process::Command::new("ssh")
            .args([
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "IdentitiesOnly=yes",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
            ])
            .arg(format!("UserKnownHostsFile={}", known_hosts.display()))
            .arg("-i")
            .arg(identity)
            .arg("-p")
            .arg(self.config.guest_ssh_port.to_string())
            .arg(format!("{}@127.0.0.1", self.config.guest_ssh_user))
            .arg("sudo -n /usr/local/bin/avm-command-agent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("launch verified guest command transport")?;
        serde_json::to_writer(
            child
                .stdin
                .as_mut()
                .context("guest transport has no stdin")?,
            &request,
        )?;
        child.stdin.as_mut().unwrap().write_all(b"\n")?;
        drop(child.stdin.take());
        let output = child.wait_with_output()?;
        let response: Value = serde_json::from_slice(&output.stdout).with_context(|| {
            format!(
                "decode guest command response: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })?;
        if !output.status.success() {
            bail!(
                "guest command agent rejected request: {}",
                response["error"].as_str().unwrap_or("unknown agent error")
            );
        }
        ensure!(
            response.get("error").is_none(),
            "guest command agent rejected request: {}",
            response["error"]
        );
        Ok(response)
    }
}

fn prepare_known_hosts(
    public_key: &std::path::Path,
    output: &std::path::Path,
    port: u16,
) -> Result<()> {
    let key = std::fs::read_to_string(public_key)?;
    let mut fields = key.split_whitespace();
    let kind = fields.next().context("guest host key has no type")?;
    let body = fields.next().context("guest host key has no body")?;
    ensure!(kind == "ssh-ed25519", "guest host key must be Ed25519");
    let expected = format!("[127.0.0.1]:{port} {kind} {body}\n");
    if std::fs::read_to_string(output).ok().as_deref() != Some(&expected) {
        std::fs::write(output, expected)?;
    }
    Ok(())
}

pub fn active_command_ids(config: &RunConfig) -> Result<Vec<String>> {
    if !VmController::new(config.clone()).is_running() {
        return Ok(Vec::new());
    }
    let response = GuestCommandClient::new(config.clone())?.list()?;
    Ok(response["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|command| {
            command["state"]
                .as_str()
                .is_some_and(|state| matches!(state, "accepted" | "starting" | "running"))
        })
        .filter_map(|command| command["commandId"].as_str().map(str::to_owned))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_pinned_known_hosts_entry() {
        let temp = tempfile::tempdir().unwrap();
        let key = temp.path().join("host.pub");
        let known = temp.path().join("known_hosts");
        std::fs::write(&key, "ssh-ed25519 AAAATEST comment\n").unwrap();
        prepare_known_hosts(&key, &known, 2222).unwrap();
        assert_eq!(
            std::fs::read_to_string(known).unwrap(),
            "[127.0.0.1]:2222 ssh-ed25519 AAAATEST\n"
        );
    }
}
