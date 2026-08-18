use std::path::Path;

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    fingerprint::repository_fingerprint,
    guest_command::GuestCommandClient,
    qmp::QmpClient,
    remote::RemoteChannelConfig,
    vm::{RunConfig, VmController, current_host_boot_id},
    web::inspector_endpoint,
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
    Skip,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Remediation {
    pub description: String,
    pub command: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorCheck {
    pub id: String,
    pub status: CheckStatus,
    pub required: bool,
    pub summary: String,
    pub evidence: Value,
    pub remediation: Vec<Remediation>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum OverallStatus {
    Ready,
    Degraded,
    Unusable,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub generated_at: String,
    pub overall_status: OverallStatus,
    pub run_id: uuid::Uuid,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub fn exit_code(&self) -> i32 {
        match self.overall_status {
            OverallStatus::Ready => 0,
            OverallStatus::Degraded => 10,
            OverallStatus::Unusable => 20,
        }
    }

    pub fn render_human(&self) -> String {
        let mut lines = vec![format!("AVM doctor: {:?}", self.overall_status).to_uppercase()];
        lines.push(format!("run: {}", self.run_id));
        for check in &self.checks {
            let mark = match check.status {
                CheckStatus::Pass => "PASS",
                CheckStatus::Warn => "WARN",
                CheckStatus::Fail => "FAIL",
                CheckStatus::Skip => "SKIP",
                CheckStatus::Unknown => "UNKNOWN",
            };
            lines.push(format!("[{mark}] {}: {}", check.id, check.summary));
        }
        let remediations = self
            .checks
            .iter()
            .flat_map(|check| &check.remediation)
            .collect::<Vec<_>>();
        if !remediations.is_empty() {
            lines.push("remediation:".into());
            for remediation in remediations {
                lines.push(format!(
                    "- {}: {}",
                    remediation.description,
                    shell_display(&remediation.command)
                ));
            }
        }
        lines.join("\n")
    }
}

pub async fn diagnose_run(run_path: &Path, channel_path: Option<&Path>) -> Result<DoctorReport> {
    let config_path = if run_path.is_dir() {
        run_path.join("run.json")
    } else {
        run_path.to_owned()
    };
    let config = RunConfig::load(&config_path)?;
    let mut checks = Vec::new();
    let current_boot = current_host_boot_id()?;
    checks.push(match (&config.host_boot_id, current_boot) {
        (Some(expected), Some(actual)) if expected == &actual => check(
            "run.host_boot",
            CheckStatus::Pass,
            true,
            "run belongs to the current host boot",
            json!({"bootId": actual}),
            vec![],
        ),
        (Some(expected), Some(actual)) => check(
            "run.host_boot",
            CheckStatus::Fail,
            true,
            "run belongs to a different host boot",
            json!({"expected": expected, "actual": actual}),
            vec![remediation(
                "Create a new run for this boot",
                vec!["avm", "create-run"],
            )],
        ),
        _ => check(
            "run.host_boot",
            CheckStatus::Unknown,
            true,
            "host boot identity is unavailable",
            json!({}),
            vec![],
        ),
    });
    let controller = VmController::new(config.clone());
    let running = controller.is_running();
    checks.push(check(
        "vm.pid",
        if running {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        true,
        if running {
            "VM process is present"
        } else {
            "VM process is not running"
        },
        json!({"running": running}),
        if running {
            vec![]
        } else {
            vec![remediation(
                "Start the run",
                vec!["avm", "start", "--run", &config_path.display().to_string()],
            )]
        },
    ));
    if running {
        let qmp = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let mut client = QmpClient::connect(&config.paths().qmp_socket).await?;
            client.execute("query-status", None).await
        })
        .await;
        checks.push(match qmp {
            Ok(Ok(value)) => check(
                "vm.qmp",
                CheckStatus::Pass,
                true,
                "QMP is responsive",
                value,
                vec![],
            ),
            Ok(Err(error)) => check(
                "vm.qmp",
                CheckStatus::Fail,
                true,
                "QMP connection failed",
                json!({"error": error.to_string()}),
                vec![],
            ),
            Err(_) => check(
                "vm.qmp",
                CheckStatus::Fail,
                true,
                "QMP probe timed out",
                json!({"timeoutMs": 2000}),
                vec![],
            ),
        });
    } else {
        checks.push(check(
            "vm.qmp",
            CheckStatus::Skip,
            true,
            "QMP skipped because the VM is stopped",
            json!({}),
            vec![],
        ));
    }
    checks.push(match repository_fingerprint(&config.candidate_workspace) {
        Ok(fingerprint) => check(
            "guest.workspace.exists",
            CheckStatus::Pass,
            true,
            "candidate workspace is readable",
            json!({"path": config.candidate_workspace, "repositoryFingerprint": fingerprint}),
            vec![],
        ),
        Err(error) => check(
            "guest.workspace.exists",
            CheckStatus::Fail,
            true,
            "candidate workspace is unreadable",
            json!({"error": error.to_string()}),
            vec![],
        ),
    });
    let ssh_configured =
        config.guest_ssh_private_key.is_file() && config.guest_ssh_host_public_key.is_file();
    checks.push(check(
        "guest.ssh.identity",
        if ssh_configured {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        true,
        if ssh_configured {
            "trusted guest host key is configured"
        } else {
            "trusted guest host key is missing"
        },
        json!({"configured": ssh_configured}),
        vec![],
    ));
    if running {
        let address = format!("127.0.0.1:{}", config.guest_ssh_port);
        let reachable = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tokio::net::TcpStream::connect(&address),
        )
        .await;
        checks.push(match reachable {
            Ok(Ok(_)) => check(
                "guest.ssh",
                CheckStatus::Pass,
                true,
                "guest SSH port is reachable",
                json!({"address": address}),
                vec![],
            ),
            Ok(Err(error)) => check(
                "guest.ssh",
                CheckStatus::Fail,
                true,
                "guest SSH port is unreachable",
                json!({"address": address, "error": error.to_string()}),
                vec![],
            ),
            Err(_) => check(
                "guest.ssh",
                CheckStatus::Fail,
                true,
                "guest SSH probe timed out",
                json!({"address": address}),
                vec![],
            ),
        });
    } else {
        checks.push(check(
            "guest.ssh",
            CheckStatus::Skip,
            true,
            "guest SSH skipped because the VM is stopped",
            json!({}),
            vec![],
        ));
    }
    if running && ssh_configured {
        checks.push(
            match GuestCommandClient::new(config.clone()).and_then(|client| client.health()) {
                Ok(evidence) => check(
                    "guest.command_agent",
                    CheckStatus::Pass,
                    true,
                    "command agent accepts the canonical workspace",
                    evidence,
                    vec![],
                ),
                Err(error) => check(
                    "guest.command_agent",
                    CheckStatus::Fail,
                    true,
                    "command agent execution path is unusable",
                    json!({"error": error.to_string()}),
                    vec![remediation(
                        "Rebuild the guest image and create a new run",
                        vec!["vm/image/build-base.sh", "OUTPUT_DIRECTORY"],
                    )],
                ),
            },
        );
    } else {
        checks.push(check(
            "guest.command_agent",
            CheckStatus::Skip,
            true,
            "command-agent probe skipped because the VM or SSH identity is unavailable",
            json!({}),
            vec![],
        ));
    }
    checks.push(match inspector_endpoint(&config) {
        Some(endpoint) => check(
            "webui.inspector",
            CheckStatus::Pass,
            false,
            "inspector is available",
            json!({"url": endpoint.url}),
            vec![],
        ),
        None => check(
            "webui.inspector",
            CheckStatus::Warn,
            false,
            "inspector is not available",
            json!({}),
            vec![],
        ),
    });
    let publication = config.state_dir.join("transport/active-publication.json");
    checks.push(if publication.is_file() {
        let value: Value = serde_json::from_slice(&std::fs::read(&publication)?)?;
        check(
            "publication.materialized",
            CheckStatus::Pass,
            false,
            "an atomic publication manifest is active",
            value,
            vec![],
        )
    } else {
        check(
            "publication.materialized",
            CheckStatus::Unknown,
            false,
            "no atomic publication manifest exists",
            json!({}),
            vec![],
        )
    });
    let state_root = config.workspace_root.join("state");
    let state_boundary_valid = state_root.is_dir()
        && config.workspace_root.join("generations").is_dir()
        && std::fs::symlink_metadata(&config.candidate_workspace)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        && config.guest_state_paths.iter().all(|relative| {
            std::fs::symlink_metadata(state_root.join(relative)).is_ok()
                && std::fs::symlink_metadata(config.candidate_workspace.join(relative))
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
        });
    checks.push(check(
        "guest_state.boundary",
        if state_boundary_valid {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        true,
        if state_boundary_valid {
            "workspace generations and declared guest state are separated"
        } else {
            "workspace generation or guest-state boundary is invalid"
        },
        json!({"paths": config.guest_state_paths, "stateRoot": state_root}),
        vec![],
    ));

    if let Some(channel_path) = channel_path {
        checks.push(match RemoteChannelConfig::load(channel_path) {
            Ok(channel) if channel.run_id == config.id && channel.remote_candidate == config.candidate_workspace => check("channel.run.identity", CheckStatus::Pass, true, "channel targets this run", json!({"channelId": channel.id, "project": channel.project, "zone": channel.zone, "instance": channel.instance}), vec![]),
            Ok(channel) => check("channel.run.identity", CheckStatus::Fail, true, "channel targets a different run or candidate", json!({"channelRunId": channel.run_id, "runId": config.id}), vec![]),
            Err(error) => check("channel.config.valid", CheckStatus::Fail, true, "channel configuration is invalid", json!({"error": error.to_string()}), vec![]),
        });
    }

    let overall_status = aggregate(&checks);
    Ok(DoctorReport {
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        overall_status,
        run_id: config.id,
        checks,
    })
}

pub fn diagnose_remote_channel(run_path: &Path, channel_path: &Path) -> Result<DoctorReport> {
    let channel = RemoteChannelConfig::load(channel_path)?;
    let requested_run = if run_path.is_dir() {
        run_path.join("run.json")
    } else {
        run_path.to_owned()
    };
    let mut report: DoctorReport =
        serde_json::from_slice(&channel.doctor_report()?).context("decode remote doctor report")?;
    report.checks.push(if requested_run == channel.remote_run {
        check(
            "channel.run.path",
            CheckStatus::Pass,
            true,
            "requested remote run matches the channel",
            json!({"remoteRun": channel.remote_run}),
            vec![],
        )
    } else {
        check(
            "channel.run.path",
            CheckStatus::Fail,
            true,
            "requested run does not match the channel remote run",
            json!({"requested": requested_run, "remoteRun": channel.remote_run}),
            vec![],
        )
    });
    report.checks.push(if report.run_id == channel.run_id {
        check(
            "channel.run.identity",
            CheckStatus::Pass,
            true,
            "remote report matches the channel run identity",
            json!({
                "channelId": channel.id,
                "runId": channel.run_id,
                "project": channel.project,
                "zone": channel.zone,
                "instance": channel.instance,
            }),
            vec![],
        )
    } else {
        check(
            "channel.run.identity",
            CheckStatus::Fail,
            true,
            "remote report returned a different run identity",
            json!({"expected": channel.run_id, "actual": report.run_id}),
            vec![],
        )
    });
    report
        .checks
        .push(match repository_fingerprint(&channel.local_candidate) {
            Ok(fingerprint) => check(
                "local.source.fingerprint",
                CheckStatus::Pass,
                false,
                "local source fingerprint computed",
                json!({"path": channel.local_candidate, "repositoryFingerprint": fingerprint}),
                vec![],
            ),
            Err(error) => check(
                "local.source.fingerprint",
                CheckStatus::Warn,
                false,
                "local source fingerprint failed",
                json!({"error": error.to_string()}),
                vec![],
            ),
        });
    report.overall_status = aggregate(&report.checks);
    Ok(report)
}

fn check(
    id: &str,
    status: CheckStatus,
    required: bool,
    summary: impl Into<String>,
    evidence: Value,
    remediation: Vec<Remediation>,
) -> DoctorCheck {
    DoctorCheck {
        id: id.into(),
        status,
        required,
        summary: summary.into(),
        evidence,
        remediation,
    }
}

fn remediation(description: &str, command: Vec<&str>) -> Remediation {
    Remediation {
        description: description.into(),
        command: command.into_iter().map(str::to_owned).collect(),
    }
}

fn aggregate(checks: &[DoctorCheck]) -> OverallStatus {
    if checks
        .iter()
        .any(|check| check.required && check.status == CheckStatus::Fail)
    {
        OverallStatus::Unusable
    } else if checks.iter().any(|check| {
        matches!(
            check.status,
            CheckStatus::Warn | CheckStatus::Unknown | CheckStatus::Fail
        )
    }) {
        OverallStatus::Degraded
    } else {
        OverallStatus::Ready
    }
}

fn shell_display(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| {
            if argument
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_./:".contains(&byte))
            {
                argument.clone()
            } else {
                format!("'{}'", argument.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregation_distinguishes_ready_degraded_and_unusable() {
        assert_eq!(
            aggregate(&[check(
                "ok",
                CheckStatus::Pass,
                true,
                "ok",
                json!({}),
                vec![]
            )]),
            OverallStatus::Ready
        );
        assert_eq!(
            aggregate(&[check(
                "warn",
                CheckStatus::Warn,
                false,
                "warn",
                json!({}),
                vec![]
            )]),
            OverallStatus::Degraded
        );
        assert_eq!(
            aggregate(&[check(
                "bad",
                CheckStatus::Fail,
                true,
                "bad",
                json!({}),
                vec![]
            )]),
            OverallStatus::Unusable
        );
    }

    #[test]
    fn remediation_rendering_quotes_arguments() {
        assert_eq!(shell_display(&["avm".into(), "a b".into()]), "avm 'a b'");
    }
}
