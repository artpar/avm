use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout, Command},
    time::timeout,
};
use uuid::Uuid;

use crate::{
    event::{EventSink, RawEvent},
    policy::PolicyState,
    workspace_gate::{PromotionResult, WorkspaceGate},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalMode {
    Accept,
    Decline,
}

impl ApprovalMode {
    fn modern_decision(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Decline => "decline",
        }
    }

    fn legacy_decision(self) -> Value {
        match self {
            Self::Accept => json!("approved"),
            Self::Decline => json!({"denied": {"rejection": "declined by AVM supervisor"}}),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppServerOptions {
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub prompt: String,
    pub model: Option<String>,
    pub approval_mode: ApprovalMode,
    pub approval_policy: String,
    pub sandbox: String,
}

impl AppServerOptions {
    pub fn codex(cwd: impl AsRef<Path>, prompt: impl Into<String>) -> Self {
        Self {
            command: vec!["codex".into(), "app-server".into(), "--stdio".into()],
            cwd: cwd.as_ref().to_owned(),
            prompt: prompt.into(),
            model: None,
            approval_mode: ApprovalMode::Decline,
            approval_policy: "on-request".into(),
            sandbox: "workspace-write".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppServerResult {
    pub thread_id: String,
    pub turn_id: String,
    pub turn_status: String,
    pub received_messages: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyAppServerResult {
    pub app_server: AppServerResult,
    pub promotion: PromotionResult,
}

pub async fn run_policy_app_server_turn(
    session_id: Uuid,
    sink: Arc<dyn EventSink>,
    state: &mut PolicyState,
    mut options: AppServerOptions,
) -> Result<PolicyAppServerResult> {
    let gate = WorkspaceGate::prepare(state)?;
    options.cwd = gate.staging_workspace.clone();
    let app_server = run_app_server_turn(session_id, sink.clone(), options).await?;
    let promotion = gate.promote(state)?;
    sink.record(RawEvent::observed(
        session_id,
        "repository",
        "workspace.promotion.completed",
        serde_json::to_value(&promotion)?,
    ))?;
    Ok(PolicyAppServerResult {
        app_server,
        promotion,
    })
}

#[derive(Clone, Debug)]
pub struct ExecOptions {
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub prompt: String,
}

impl ExecOptions {
    pub fn codex(cwd: impl AsRef<Path>, prompt: impl Into<String>) -> Self {
        Self {
            command: vec!["codex".into()],
            cwd: cwd.as_ref().to_owned(),
            prompt: prompt.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResult {
    pub exit_code: Option<i32>,
    pub event_count: u64,
}

pub async fn run_codex_exec_json(
    session_id: Uuid,
    sink: Arc<dyn EventSink>,
    options: ExecOptions,
) -> Result<ExecResult> {
    ensure!(!options.command.is_empty(), "Codex exec command is empty");
    let mut command = Command::new(&options.command[0]);
    command.args(&options.command[1..]);
    if options.command == ["codex"] {
        command.args([
            "exec",
            "--json",
            "--skip-git-repo-check",
            "--sandbox",
            "workspace-write",
            "-C",
        ]);
        command.arg(&options.cwd).arg(&options.prompt);
    }
    command
        .current_dir(&options.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().context("launch codex exec --json")?;
    let stdout = child.stdout.take().context("codex exec has no stdout")?;
    let stderr = child.stderr.take().context("codex exec has no stderr")?;
    let sequence = Arc::new(AtomicU64::new(0));
    let stderr_task = {
        let sink = sink.clone();
        let sequence = sequence.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Some(line) = lines.next_line().await? {
                let mut event = RawEvent::observed(
                    session_id,
                    "agent",
                    "codex.exec.stderr",
                    json!({"line": line}),
                );
                event.source_sequence = Some(sequence.fetch_add(1, Ordering::SeqCst));
                sink.record(event)?;
            }
            Ok::<(), anyhow::Error>(())
        })
    };
    let mut lines = BufReader::new(stdout).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let message: Value =
            serde_json::from_str(&line).context("decode codex exec JSONL event")?;
        let (source, kind) = classify_exec_message(&message);
        let mut event = RawEvent::observed(session_id, source, &kind, message);
        event.source_sequence = Some(sequence.fetch_add(1, Ordering::SeqCst));
        sink.record(event)?;
    }
    let status = child.wait().await?;
    stderr_task
        .await
        .context("join codex exec stderr recorder")??;
    let mut completed = RawEvent::observed(
        session_id,
        "agent",
        "codex.exec.process_completed",
        json!({"exitCode": status.code(), "success": status.success()}),
    );
    completed.source_sequence = Some(sequence.fetch_add(1, Ordering::SeqCst));
    sink.record(completed)?;
    Ok(ExecResult {
        exit_code: status.code(),
        event_count: sequence.load(Ordering::SeqCst),
    })
}

fn classify_exec_message(message: &Value) -> (&'static str, String) {
    let event_type = message
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let item_type = message.pointer("/item/type").and_then(Value::as_str);
    let source = match item_type {
        Some("command_execution") | Some("commandExecution") => "process",
        Some("file_change") | Some("fileChange") => "repository",
        _ => "agent",
    };
    (
        source,
        format!("codex.exec.{}", event_type.replace('.', "_")),
    )
}

pub async fn run_app_server_turn(
    session_id: Uuid,
    sink: Arc<dyn EventSink>,
    options: AppServerOptions,
) -> Result<AppServerResult> {
    ensure!(!options.command.is_empty(), "app-server command is empty");
    ensure!(
        matches!(
            options.approval_policy.as_str(),
            "untrusted" | "on-request" | "never"
        ),
        "unsupported App Server approval policy"
    );
    let mut command = Command::new(&options.command[0]);
    command
        .args(&options.command[1..])
        .current_dir(&options.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().context("launch Codex App Server")?;
    let mut stdin = child
        .stdin
        .take()
        .context("Codex App Server has no stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("Codex App Server has no stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("Codex App Server has no stderr")?;
    let mut reader = BufReader::new(stdout).lines();
    let sequence = Arc::new(AtomicU64::new(0));
    let stderr_task = {
        let sink = sink.clone();
        let sequence = sequence.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Some(line) = lines.next_line().await? {
                let mut event = RawEvent::observed(
                    session_id,
                    "agent",
                    "codex.app_server.stderr",
                    json!({"line": line}),
                );
                event.source_sequence = Some(sequence.fetch_add(1, Ordering::SeqCst));
                sink.record(event)?;
            }
            Ok::<(), anyhow::Error>(())
        })
    };

    send_wire(
        &mut stdin,
        &sink,
        session_id,
        &sequence,
        json!({
            "method": "initialize",
            "id": 1,
            "params": {"clientInfo": {
                "name": "avm_supervisor", "title": "AVM Supervisor", "version": env!("CARGO_PKG_VERSION")
            }}
        }),
    )
    .await?;
    response_for(
        &mut reader,
        &mut stdin,
        &sink,
        session_id,
        &sequence,
        json!(1),
        options.approval_mode,
    )
    .await?;
    send_wire(
        &mut stdin,
        &sink,
        session_id,
        &sequence,
        json!({"method": "initialized", "params": {}}),
    )
    .await?;

    let mut thread_params = json!({
        "cwd": options.cwd,
        "approvalPolicy": options.approval_policy,
        "sandbox": options.sandbox,
        "serviceName": "avm_supervisor"
    });
    if let Some(model) = &options.model {
        thread_params["model"] = json!(model);
    }
    send_wire(
        &mut stdin,
        &sink,
        session_id,
        &sequence,
        json!({"method": "thread/start", "id": 2, "params": thread_params}),
    )
    .await?;
    let thread_response = response_for(
        &mut reader,
        &mut stdin,
        &sink,
        session_id,
        &sequence,
        json!(2),
        options.approval_mode,
    )
    .await?;
    let thread_id = thread_response
        .pointer("/result/thread/id")
        .and_then(Value::as_str)
        .context("thread/start response has no thread id")?
        .to_owned();
    let turn_sandbox = sandbox_policy_type(&options.sandbox)?;

    send_wire(
        &mut stdin,
        &sink,
        session_id,
        &sequence,
        json!({
            "method": "turn/start",
            "id": 3,
            "params": {
                "threadId": thread_id,
                "input": [{"type": "text", "text": options.prompt}],
                "cwd": options.cwd,
                "approvalPolicy": options.approval_policy,
                "sandboxPolicy": {
                    "type": turn_sandbox,
                    "writableRoots": [options.cwd],
                    "networkAccess": false
                }
            }
        }),
    )
    .await?;
    let turn_response = response_for(
        &mut reader,
        &mut stdin,
        &sink,
        session_id,
        &sequence,
        json!(3),
        options.approval_mode,
    )
    .await?;
    let turn_id = turn_response
        .pointer("/result/turn/id")
        .and_then(Value::as_str)
        .context("turn/start response has no turn id")?
        .to_owned();

    let turn_status = loop {
        let message = read_wire(&mut reader).await?;
        record_wire(&sink, session_id, &sequence, "inbound", message.clone())?;
        if is_server_request(&message) {
            respond_to_server_request(
                &mut stdin,
                &sink,
                session_id,
                &sequence,
                &message,
                options.approval_mode,
            )
            .await?;
            continue;
        }
        if message.get("method").and_then(Value::as_str) == Some("turn/completed")
            && message.pointer("/params/turn/id").and_then(Value::as_str) == Some(&turn_id)
        {
            break message
                .pointer("/params/turn/status")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned();
        }
    };

    drop(stdin);
    let status = match timeout(Duration::from_secs(5), child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            child.kill().await?;
            child.wait().await?
        }
    };
    stderr_task
        .await
        .context("join app-server stderr recorder")??;
    ensure!(
        status.success() || status.code().is_none(),
        "Codex App Server exited with {status}"
    );
    Ok(AppServerResult {
        thread_id,
        turn_id,
        turn_status,
        received_messages: sequence.load(Ordering::SeqCst),
    })
}

fn sandbox_policy_type(sandbox: &str) -> Result<&'static str> {
    match sandbox {
        "workspace-write" => Ok("workspaceWrite"),
        "read-only" => Ok("readOnly"),
        "danger-full-access" => Ok("dangerFullAccess"),
        other => bail!("unsupported App Server sandbox mode {other}"),
    }
}

async fn response_for(
    reader: &mut tokio::io::Lines<BufReader<ChildStdout>>,
    stdin: &mut ChildStdin,
    sink: &Arc<dyn EventSink>,
    session_id: Uuid,
    sequence: &Arc<AtomicU64>,
    request_id: Value,
    approval_mode: ApprovalMode,
) -> Result<Value> {
    loop {
        let message = read_wire(reader).await?;
        record_wire(sink, session_id, sequence, "inbound", message.clone())?;
        if is_server_request(&message) {
            respond_to_server_request(stdin, sink, session_id, sequence, &message, approval_mode)
                .await?;
            continue;
        }
        if message.get("id") == Some(&request_id) {
            if let Some(error) = message.get("error") {
                bail!("Codex App Server request failed: {error}");
            }
            return Ok(message);
        }
    }
}

async fn read_wire(reader: &mut tokio::io::Lines<BufReader<ChildStdout>>) -> Result<Value> {
    loop {
        let line = reader
            .next_line()
            .await?
            .context("Codex App Server closed stdout before turn completion")?;
        if !line.trim().is_empty() {
            return serde_json::from_str(&line).context("decode Codex App Server JSONL message");
        }
    }
}

async fn send_wire(
    stdin: &mut ChildStdin,
    sink: &Arc<dyn EventSink>,
    session_id: Uuid,
    sequence: &Arc<AtomicU64>,
    message: Value,
) -> Result<()> {
    record_wire(sink, session_id, sequence, "outbound", message.clone())?;
    stdin
        .write_all(serde_json::to_string(&message)?.as_bytes())
        .await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;
    Ok(())
}

fn record_wire(
    sink: &Arc<dyn EventSink>,
    session_id: Uuid,
    sequence: &Arc<AtomicU64>,
    direction: &str,
    message: Value,
) -> Result<()> {
    let (source, kind) = classify_message(direction, &message);
    let mut event = RawEvent::observed(
        session_id,
        source,
        &kind,
        json!({"direction": direction, "message": message}),
    );
    event.source_sequence = Some(sequence.fetch_add(1, Ordering::SeqCst));
    sink.record(event)
}

fn classify_message<'a>(direction: &str, message: &'a Value) -> (&'a str, String) {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return ("agent", format!("codex.{direction}.response"));
    };
    if method.starts_with("item/") {
        let item_type = message.pointer("/params/item/type").and_then(Value::as_str);
        let phase = method.rsplit('/').next().unwrap_or("event");
        return match item_type {
            Some("commandExecution") => ("process", format!("codex.command.{phase}")),
            Some("fileChange") => ("repository", format!("codex.file_change.{phase}")),
            Some("mcpToolCall") => ("agent", format!("codex.mcp_tool_call.{phase}")),
            _ if method.contains("requestApproval") => ("agent", "codex.approval.requested".into()),
            _ => ("agent", format!("codex.{}", method.replace('/', "."))),
        };
    }
    ("agent", format!("codex.{}", method.replace('/', ".")))
}

fn is_server_request(message: &Value) -> bool {
    message.get("id").is_some() && message.get("method").is_some()
}

async fn respond_to_server_request(
    stdin: &mut ChildStdin,
    sink: &Arc<dyn EventSink>,
    session_id: Uuid,
    sequence: &Arc<AtomicU64>,
    request: &Value,
    approval_mode: ApprovalMode,
) -> Result<()> {
    let id = request
        .get("id")
        .context("server request has no id")?
        .clone();
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .context("server request has no method")?;
    let response = match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            json!({"id": id, "result": {"decision": approval_mode.modern_decision()}})
        }
        "execCommandApproval" | "applyPatchApproval" => {
            json!({"id": id, "result": {"decision": approval_mode.legacy_decision()}})
        }
        _ => json!({
            "id": id,
            "error": {"code": -32601, "message": format!("AVM supervisor cannot resolve {method}")}
        }),
    };
    send_wire(stdin, sink, session_id, sequence, response).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::MemoryEventSink;

    #[tokio::test]
    async fn supervises_handshake_items_approval_and_turn_completion() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("fake.py");
        std::fs::write(
            &script,
            r#"import json,sys
def read(): return json.loads(sys.stdin.readline())
def send(x): print(json.dumps(x), flush=True)
m=read(); assert m['method']=='initialize'; send({'id':m['id'],'result':{'userAgent':'fake'}})
assert read()['method']=='initialized'
m=read(); assert m['method']=='thread/start'; send({'id':m['id'],'result':{'thread':{'id':'thr_test'}}}); send({'method':'thread/started','params':{'thread':{'id':'thr_test'}}})
m=read(); assert m['method']=='turn/start'; send({'id':m['id'],'result':{'turn':{'id':'turn_test','status':'inProgress'}}}); send({'method':'turn/started','params':{'turn':{'id':'turn_test','status':'inProgress'}}})
send({'method':'item/started','params':{'item':{'id':'cmd','type':'commandExecution','status':'inProgress'}}})
send({'id':99,'method':'item/commandExecution/requestApproval','params':{'threadId':'thr_test','turnId':'turn_test','itemId':'cmd'}})
approval=read(); assert approval['id']==99 and approval['result']['decision']=='accept'
send({'method':'item/completed','params':{'item':{'id':'cmd','type':'commandExecution','status':'completed','exitCode':0}}})
send({'method':'item/started','params':{'item':{'id':'patch','type':'fileChange','status':'inProgress','changes':[]}}})
send({'method':'item/completed','params':{'item':{'id':'patch','type':'fileChange','status':'completed','changes':[]}}})
send({'method':'item/completed','params':{'item':{'id':'msg','type':'agentMessage','text':'done'}}})
send({'method':'turn/completed','params':{'turn':{'id':'turn_test','status':'completed'}}})
sys.stdin.read()
"#,
        )
        .unwrap();
        let sink = Arc::new(MemoryEventSink::default());
        let options = AppServerOptions {
            command: vec!["python3".into(), script.display().to_string()],
            cwd: temp.path().to_owned(),
            prompt: "test".into(),
            model: None,
            approval_mode: ApprovalMode::Accept,
            approval_policy: "untrusted".into(),
            sandbox: "workspace-write".into(),
        };
        let result = run_app_server_turn(Uuid::new_v4(), sink.clone(), options)
            .await
            .unwrap();
        assert_eq!(result.thread_id, "thr_test");
        assert_eq!(result.turn_id, "turn_test");
        assert_eq!(result.turn_status, "completed");
        let kinds = sink
            .events()
            .into_iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>();
        for expected in [
            "codex.thread.started",
            "codex.turn.started",
            "codex.command.started",
            "codex.approval.requested",
            "codex.outbound.response",
            "codex.command.completed",
            "codex.file_change.started",
            "codex.file_change.completed",
            "codex.turn.completed",
        ] {
            assert!(
                kinds.iter().any(|kind| kind == expected),
                "missing {expected}"
            );
        }
    }

    #[tokio::test]
    async fn records_codex_exec_jsonl_baseline_events() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("fake-exec.py");
        std::fs::write(
            &script,
            r#"import json
print(json.dumps({'type':'thread.started','thread_id':'thr_exec'}), flush=True)
print(json.dumps({'type':'item.completed','item':{'type':'command_execution','exit_code':0}}), flush=True)
print(json.dumps({'type':'turn.completed','usage':{'input_tokens':1}}), flush=True)
"#,
        )
        .unwrap();
        let sink = Arc::new(MemoryEventSink::default());
        let result = run_codex_exec_json(
            Uuid::new_v4(),
            sink.clone(),
            ExecOptions {
                command: vec!["python3".into(), script.display().to_string()],
                cwd: temp.path().to_owned(),
                prompt: "ignored".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, Some(0));
        let kinds = sink
            .events()
            .into_iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>();
        assert!(kinds.contains(&"codex.exec.thread_started".into()));
        assert!(kinds.contains(&"codex.exec.item_completed".into()));
        assert!(kinds.contains(&"codex.exec.turn_completed".into()));
        assert!(kinds.contains(&"codex.exec.process_completed".into()));
    }

    #[tokio::test]
    async fn policy_turn_isolates_unapproved_app_server_write_until_promotion() {
        let candidate = tempfile::tempdir().unwrap();
        std::fs::write(candidate.path().join("index.html"), "before\n").unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "avm@example.invalid"],
            vec!["config", "user.name", "AVM"],
            vec!["add", "."],
            vec!["commit", "-qm", "baseline"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(candidate.path())
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let supervisor = tempfile::tempdir().unwrap();
        let mut policy = PolicyState::create(
            Uuid::new_v4(),
            candidate.path(),
            supervisor.path(),
            crate::policy::PolicyConfig::default(),
        )
        .unwrap();
        policy
            .declare(
                crate::policy::DevelopmentDeclaration::new(
                    "change the visible label".into(),
                    "the label is static HTML".into(),
                    "browser_interaction".into(),
                    "browser observer".into(),
                    "open the product and inspect the label".into(),
                    "the new label is visible".into(),
                    "the old label means promotion failed".into(),
                    policy.current_repository_fingerprint.clone(),
                )
                .unwrap(),
            )
            .unwrap();
        let script = supervisor.path().join("fake-policy-app-server.py");
        std::fs::write(
            &script,
            r#"import json,pathlib,sys
def read(): return json.loads(sys.stdin.readline())
def send(x): print(json.dumps(x), flush=True)
m=read(); send({'id':m['id'],'result':{'userAgent':'fake'}})
read()
m=read(); send({'id':m['id'],'result':{'thread':{'id':'thr'}}})
m=read(); send({'id':m['id'],'result':{'turn':{'id':'turn','status':'inProgress'}}})
pathlib.Path('index.html').write_text('after\n')
send({'method':'item/started','params':{'item':{'id':'patch','type':'fileChange','status':'inProgress','changes':[{'path':'index.html'}]}}})
send({'method':'item/completed','params':{'item':{'id':'patch','type':'fileChange','status':'completed','changes':[{'path':'index.html'}]}}})
send({'method':'turn/completed','params':{'turn':{'id':'turn','status':'completed'}}})
sys.stdin.read()
"#,
        )
        .unwrap();
        let sink = Arc::new(MemoryEventSink::default());
        let result = run_policy_app_server_turn(
            policy.session_id,
            sink.clone(),
            &mut policy,
            AppServerOptions {
                command: vec!["python3".into(), script.display().to_string()],
                cwd: candidate.path().into(),
                prompt: "change label".into(),
                model: None,
                approval_mode: ApprovalMode::Decline,
                approval_policy: "on-request".into(),
                sandbox: "workspace-write".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(candidate.path().join("index.html")).unwrap(),
            "after\n"
        );
        assert_eq!(
            result.promotion.after_repository_fingerprint,
            policy.current_repository_fingerprint
        );
        assert!(policy.required_observations.contains("browser_interaction"));
        let promotion = sink
            .events()
            .into_iter()
            .find(|event| event.kind == "workspace.promotion.completed")
            .unwrap();
        assert_eq!(
            promotion.payload["afterRepositoryFingerprint"],
            policy.current_repository_fingerprint
        );
    }
}
