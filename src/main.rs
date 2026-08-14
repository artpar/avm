use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use avm::audio::{
    AudioInterpretationKind, CommandAudioAdapter, CommandAudioAdapterConfig,
    DEFAULT_AUDIO_EVENT_PROMPT, DEFAULT_TRANSCRIPTION_PROMPT, interpret_audio_event,
};

use anyhow::{Context, Result, bail, ensure};
use avm::storage::ArtifactStore;
use avm::{
    accessibility::observe_accessibility,
    browser::{
        BrowserObserverOptions, correlate_viewport_png, diagnose_double_submit_failure,
        run_browser_observer,
    },
    codex::{
        AppServerOptions, ApprovalMode, ExecOptions, run_app_server_turn, run_codex_exec_json,
        run_policy_app_server_turn,
    },
    event::{EventSink, Provenance, RawEvent, monotonic_ns},
    experience::ExperienceStore,
    policy::{
        BrowserEvidenceOptions, DevelopmentDeclarationInput, DiagnosisPlanInput,
        EvidenceCommandOptions, EvidenceStore, PolicyConfig, PolicyPhase, PolicyState,
        acquire_browser_evidence, acquire_command_evidence,
    },
    query::{ExperienceQuery, execute_query},
    remote::{
        RemoteChannelConfig, TeeEventSink, apply_transfer, prepare_transfer, serve_event_relay,
    },
    runtime::import_runtime_jsonl,
    session::ExperimentSession,
    temporal::{TemporalConfig, analyze_temporal},
    timeline::{ExperienceEventSink, TimelineStore},
    vlm::{CommandVlmAdapter, CommandVlmConfig, DEFAULT_VLM_PROMPT, observe_temporal_event},
    vm::{RunConfig, VmController},
};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;

#[derive(Parser)]
#[command(name = "avm", about = "Host-owned instrumented virtual computer")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    SessionCreate {
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long)]
        state_root: PathBuf,
    },
    CreateRun {
        #[arg(long)]
        base_image: PathBuf,
        #[arg(long)]
        candidate: PathBuf,
        #[arg(long)]
        state_root: PathBuf,
    },
    Start {
        #[arg(long)]
        run: PathBuf,
    },
    Status {
        #[arg(long)]
        run: PathBuf,
    },
    Reset {
        #[arg(long)]
        run: PathBuf,
    },
    Checkpoint {
        #[arg(long)]
        run: PathBuf,
    },
    RestoreCheckpoint {
        #[arg(long)]
        run: PathBuf,
    },
    Stop {
        #[arg(long)]
        run: PathBuf,
    },
    DestroyRun {
        #[arg(long)]
        run: PathBuf,
    },
    Observe {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, default_value_t = 20)]
        recent_limit: usize,
    },
    History {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        start_ns: Option<u64>,
        #[arg(long)]
        end_ns: Option<u64>,
        #[arg(long)]
        last_duration_ms: Option<u64>,
        #[arg(long)]
        source: Vec<String>,
    },
    Frame {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        at_ns: Option<u64>,
        #[arg(long)]
        event_id: Option<uuid::Uuid>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Replay {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        start_ns: Option<u64>,
        #[arg(long)]
        end_ns: Option<u64>,
        #[arg(long)]
        last_duration_ms: Option<u64>,
    },
    TemporalAnalyze {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        start_ns: Option<u64>,
        #[arg(long)]
        end_ns: Option<u64>,
        #[arg(long)]
        last_duration_ms: Option<u64>,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    ExperienceQuery {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        input: PathBuf,
    },
    VlmObserve {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        adapter_config: PathBuf,
        #[arg(long)]
        temporal_event_id: Option<uuid::Uuid>,
        #[arg(long)]
        observation_index: Option<usize>,
        #[arg(long)]
        prompt: Option<String>,
    },
    AccessibilityObserve {
        #[arg(long)]
        run: PathBuf,
        #[arg(long, default_value_t = 10_000)]
        duration_ms: u64,
    },
    RuntimeImport {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        input: PathBuf,
    },
    BrowserObserve {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        endpoint: String,
        #[arg(long, default_value = "supervisor/browser/observer.mjs")]
        script: PathBuf,
        #[arg(long, default_value_t = 30_000)]
        duration_ms: u64,
        #[arg(long)]
        trace: Option<PathBuf>,
    },
    BrowserCorrelate {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        browser_event_id: Option<uuid::Uuid>,
    },
    BrowserDiagnoseFailure {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        click_event_id: Option<uuid::Uuid>,
    },
    CodexTurn {
        #[arg(
            long,
            conflicts_with_all = ["session", "candidate", "state_root"],
            required_unless_present_any = ["session", "candidate"]
        )]
        run: Option<PathBuf>,
        #[arg(
            long,
            conflicts_with_all = ["run", "candidate", "state_root"],
            required_unless_present_any = ["run", "candidate"]
        )]
        session: Option<PathBuf>,
        #[arg(long)]
        #[arg(required_unless_present_any = ["run", "session"], requires = "state_root")]
        candidate: Option<PathBuf>,
        #[arg(long, requires = "candidate")]
        state_root: Option<PathBuf>,
        #[arg(long)]
        prompt: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_enum, default_value_t = ApprovalArgument::Decline)]
        approval: ApprovalArgument,
        #[arg(long, value_enum, default_value_t = ApprovalPolicyArgument::OnRequest)]
        approval_policy: ApprovalPolicyArgument,
        #[arg(long)]
        channel: Option<PathBuf>,
        #[arg(long, requires = "channel")]
        publish_after_turn: bool,
        #[arg(long)]
        policy: Option<PathBuf>,
    },
    CodexExec {
        #[arg(
            long,
            conflicts_with_all = ["session", "candidate", "state_root"],
            required_unless_present_any = ["session", "candidate"]
        )]
        run: Option<PathBuf>,
        #[arg(
            long,
            conflicts_with_all = ["run", "candidate", "state_root"],
            required_unless_present_any = ["run", "candidate"]
        )]
        session: Option<PathBuf>,
        #[arg(long)]
        #[arg(required_unless_present_any = ["run", "session"], requires = "state_root")]
        candidate: Option<PathBuf>,
        #[arg(long, requires = "candidate")]
        state_root: Option<PathBuf>,
        #[arg(long)]
        prompt: String,
    },
    RemoteChannelCreate {
        #[arg(long)]
        local_candidate: PathBuf,
        #[arg(long)]
        state_root: PathBuf,
        #[arg(long)]
        project: String,
        #[arg(long)]
        zone: String,
        #[arg(long)]
        instance: String,
        #[arg(long)]
        remote_run: PathBuf,
        #[arg(long, default_value = "/home/artpar/avm/target/release/avm")]
        remote_avm: PathBuf,
    },
    RemotePublish {
        #[arg(long)]
        channel: PathBuf,
    },
    PolicyInit {
        #[arg(long)]
        target: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    PolicyDeclare {
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        input: PathBuf,
    },
    PolicyStatus {
        #[arg(long)]
        policy: PathBuf,
    },
    PolicyDiagnose {
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        input: PathBuf,
    },
    EvidenceCommand {
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        expected_exit_code: i32,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    EvidenceList {
        #[arg(long)]
        policy: PathBuf,
    },
    EvidenceBrowser {
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        experience_root: PathBuf,
        #[arg(long)]
        experience_session_id: uuid::Uuid,
        #[arg(long)]
        start_ns: Option<u64>,
        #[arg(long)]
        end_ns: Option<u64>,
        #[arg(long)]
        expected_before_text: String,
        #[arg(long)]
        expected_after_text: String,
    },
    #[command(hide = true)]
    EventRelay {
        #[arg(long)]
        run: PathBuf,
    },
    #[command(hide = true)]
    PrepareTransfer {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        transfer_id: uuid::Uuid,
    },
    #[command(hide = true)]
    ApplyTransfer {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        transfer_id: uuid::Uuid,
        #[arg(long)]
        sha256: String,
    },
    #[cfg(target_os = "linux")]
    Capture {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    #[cfg(target_os = "linux")]
    AudioObserve {
        #[arg(long)]
        run: PathBuf,
        #[arg(long, default_value_t = 10_000)]
        duration_ms: u64,
    },
    #[cfg(target_os = "linux")]
    AudioInterpret {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        adapter_config: PathBuf,
        #[arg(long)]
        audio_event_id: uuid::Uuid,
        #[arg(long)]
        prompt: Option<String>,
    },
    #[cfg(target_os = "linux")]
    Smoke {
        #[arg(long)]
        run: PathBuf,
        #[arg(long, default_value = "http://10.0.2.2:3000")]
        url: String,
        #[arg(long)]
        screenshot: PathBuf,
    },
    #[cfg(target_os = "linux")]
    ActClick {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        x: u32,
        #[arg(long)]
        y: u32,
        #[arg(long, default_value_t = 1_000)]
        wait_after_ms: u64,
    },
    #[cfg(target_os = "linux")]
    ActKey {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        keycode: u32,
        #[arg(long, value_enum, default_value_t = KeyMode::Press)]
        mode: KeyMode,
    },
    #[cfg(target_os = "linux")]
    ActType {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        text: String,
    },
    #[cfg(target_os = "linux")]
    DragProof {
        #[arg(long)]
        run: PathBuf,
        #[arg(long)]
        from_x: u32,
        #[arg(long)]
        from_y: u32,
        #[arg(long)]
        to_x: u32,
        #[arg(long)]
        to_y: u32,
        #[arg(long, default_value_t = 12)]
        steps: u32,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ApprovalArgument {
    Accept,
    Decline,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ApprovalPolicyArgument {
    Untrusted,
    OnRequest,
    Never,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum KeyMode {
    Press,
    Down,
    Up,
}

impl ApprovalPolicyArgument {
    fn as_wire_value(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::OnRequest => "on-request",
            Self::Never => "never",
        }
    }
}

impl From<ApprovalArgument> for ApprovalMode {
    fn from(value: ApprovalArgument) -> Self {
        match value {
            ApprovalArgument::Accept => Self::Accept,
            ApprovalArgument::Decline => Self::Decline,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::SessionCreate {
            candidate,
            state_root,
        } => {
            let session = ExperimentSession::create(&candidate, &state_root)?;
            println!("{}", session.paths().config.display());
        }
        Command::CreateRun {
            base_image,
            candidate,
            state_root,
        } => {
            let config = RunConfig::new(&base_image, &candidate, &state_root)?;
            let controller = VmController::new(config);
            controller.create_overlay()?;
            println!("{}", controller.config().paths().config.display());
        }
        Command::Start { run } => {
            let config = load_run(&run)?;
            lifecycle(&config, "vm.start.requested", json!({}))?;
            VmController::new(config.clone()).start().await?;
            lifecycle(&config, "vm.started", json!({}))?;
        }
        Command::Status { run } => {
            let config = load_run(&run)?;
            let running = VmController::new(config.clone()).is_running();
            println!(
                "{}",
                json!({"runId": config.id, "running": running, "stateDir": config.state_dir})
            );
        }
        Command::Reset { run } => {
            let config = load_run(&run)?;
            lifecycle(&config, "vm.reset.requested", json!({}))?;
            VmController::new(config.clone()).reset().await?;
            lifecycle(&config, "vm.reset.completed", json!({}))?;
        }
        Command::Checkpoint { run } => {
            let config = load_run(&run)?;
            lifecycle(
                &config,
                "vm.checkpoint.requested",
                json!({"tag": "avm-clean"}),
            )?;
            VmController::new(config.clone()).checkpoint().await?;
            lifecycle(
                &config,
                "vm.checkpoint.completed",
                json!({"tag": "avm-clean"}),
            )?;
        }
        Command::RestoreCheckpoint { run } => {
            let config = load_run(&run)?;
            lifecycle(
                &config,
                "vm.checkpoint.restore_requested",
                json!({"tag": "avm-clean"}),
            )?;
            VmController::new(config.clone())
                .restore_checkpoint_in_place()
                .await?;
            lifecycle(
                &config,
                "vm.checkpoint.restore_completed",
                json!({"tag": "avm-clean"}),
            )?;
        }
        Command::Stop { run } => {
            let config = load_run(&run)?;
            lifecycle(&config, "vm.stop.requested", json!({}))?;
            VmController::new(config.clone()).stop().await?;
            lifecycle(&config, "vm.stopped", json!({}))?;
        }
        Command::DestroyRun { run } => destroy_run(&run).await?,
        Command::Observe {
            run,
            output,
            recent_limit,
        } => observe(&run, output.as_deref(), recent_limit).await?,
        Command::History {
            run,
            start_ns,
            end_ns,
            last_duration_ms,
            source,
        } => history(&run, start_ns, end_ns, last_duration_ms, &source)?,
        Command::Frame {
            run,
            at_ns,
            event_id,
            output,
        } => frame(&run, at_ns, event_id, output.as_deref())?,
        Command::Replay {
            run,
            start_ns,
            end_ns,
            last_duration_ms,
        } => replay(&run, start_ns, end_ns, last_duration_ms)?,
        Command::TemporalAnalyze {
            run,
            start_ns,
            end_ns,
            last_duration_ms,
            config,
        } => temporal_analyze(&run, start_ns, end_ns, last_duration_ms, config.as_deref())?,
        Command::ExperienceQuery { run, input } => experience_query(&run, &input)?,
        Command::VlmObserve {
            run,
            adapter_config,
            temporal_event_id,
            observation_index,
            prompt,
        } => vlm_observe(
            &run,
            &adapter_config,
            temporal_event_id,
            observation_index,
            prompt.as_deref().unwrap_or(DEFAULT_VLM_PROMPT),
        )?,
        Command::AccessibilityObserve { run, duration_ms } => {
            accessibility_observe(&run, duration_ms)?
        }
        Command::RuntimeImport { run, input } => runtime_import(&run, &input)?,
        Command::BrowserObserve {
            run,
            endpoint,
            script,
            duration_ms,
            trace,
        } => {
            let config = load_run(&run)?;
            config.ensure_current_host_boot()?;
            let paths = config.paths();
            let sink: Arc<dyn EventSink> = Arc::new(ExperienceEventSink::open_dynamic(
                &paths.timeline,
                &paths.events,
                &config.candidate_workspace,
            )?);
            let artifacts = Arc::new(ArtifactStore::new(&paths.artifacts)?);
            let trace = trace.unwrap_or_else(|| {
                config
                    .state_dir
                    .join("browser")
                    .join(format!("trace-{}.zip", uuid::Uuid::new_v4()))
            });
            let sensor_artifacts_dir = config
                .state_dir
                .join("browser")
                .join(format!("sensor-artifacts-{}", uuid::Uuid::new_v4()));
            let result = run_browser_observer(
                config.id,
                sink,
                artifacts,
                BrowserObserverOptions::playwright(
                    script,
                    endpoint,
                    trace,
                    sensor_artifacts_dir,
                    duration_ms,
                ),
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::BrowserCorrelate {
            run,
            browser_event_id,
        } => browser_correlate(&run, browser_event_id)?,
        Command::BrowserDiagnoseFailure {
            run,
            click_event_id,
        } => browser_diagnose_failure(&run, click_event_id)?,
        Command::CodexTurn {
            run,
            session,
            candidate,
            state_root,
            prompt,
            model,
            approval,
            approval_policy,
            channel,
            publish_after_turn,
            policy,
        } => {
            let context = AgentEventContext::resolve(
                run.as_deref(),
                session.as_deref(),
                candidate.as_deref(),
                state_root.as_deref(),
            )?;
            let local_sink: Arc<dyn EventSink> = Arc::new(ExperienceEventSink::open_dynamic(
                &context.timeline,
                &context.events,
                &context.candidate_workspace,
            )?);
            let channel = channel
                .as_deref()
                .map(RemoteChannelConfig::load)
                .transpose()?;
            if let Some(channel) = &channel {
                ensure!(
                    channel.local_candidate.canonicalize()?
                        == context.candidate_workspace.canonicalize()?,
                    "channel local candidate differs from Codex candidate"
                );
            }
            let sink: Arc<dyn EventSink> = match &channel {
                Some(channel) => {
                    Arc::new(TeeEventSink::new(local_sink, channel.connect_event_sink()?))
                }
                None => local_sink,
            };
            let mut options = AppServerOptions::codex(&context.candidate_workspace, prompt);
            options.model = model;
            options.approval_mode = approval.into();
            options.approval_policy = approval_policy.as_wire_value().into();
            let result = match policy {
                Some(policy_path) => {
                    let mut policy = PolicyState::load(&policy_path)?;
                    ensure!(
                        policy.candidate_workspace.canonicalize()?
                            == context.candidate_workspace.canonicalize()?,
                        "policy candidate differs from Codex candidate"
                    );
                    let result =
                        run_policy_app_server_turn(context.id, sink.clone(), &mut policy, options)
                            .await?;
                    serde_json::to_value(result)?
                }
                None => serde_json::to_value(
                    run_app_server_turn(context.id, sink.clone(), options).await?,
                )?,
            };
            let published_repository_fingerprint = match (&channel, publish_after_turn) {
                (Some(channel), true) => {
                    Some(channel.publish_workspace(context.id, sink.as_ref())?)
                }
                _ => None,
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "session": context.session,
                    "run": context.run,
                    "channel": channel,
                    "publishedRepositoryFingerprint": published_repository_fingerprint,
                    "result": result,
                }))?
            );
        }
        Command::CodexExec {
            run,
            session,
            candidate,
            state_root,
            prompt,
        } => {
            let context = AgentEventContext::resolve(
                run.as_deref(),
                session.as_deref(),
                candidate.as_deref(),
                state_root.as_deref(),
            )?;
            let sink: Arc<dyn EventSink> = Arc::new(ExperienceEventSink::open_dynamic(
                &context.timeline,
                &context.events,
                &context.candidate_workspace,
            )?);
            let options = ExecOptions::codex(&context.candidate_workspace, prompt);
            let result = run_codex_exec_json(context.id, sink, options).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "session": context.session,
                    "run": context.run,
                    "result": result,
                }))?
            );
        }
        Command::RemoteChannelCreate {
            local_candidate,
            state_root,
            project,
            zone,
            instance,
            remote_run,
            remote_avm,
        } => {
            let channel = RemoteChannelConfig::create(
                &local_candidate,
                &state_root,
                project,
                zone,
                instance,
                remote_run,
                remote_avm,
            )?;
            println!("{}", channel.config_path().display());
        }
        Command::RemotePublish { channel } => {
            let channel = RemoteChannelConfig::load(channel)?;
            let sink = channel.connect_event_sink()?;
            let fingerprint = channel.publish_workspace(channel.run_id, sink.as_ref())?;
            println!(
                "{}",
                json!({"runId": channel.run_id, "repositoryFingerprint": fingerprint})
            );
        }
        Command::PolicyInit { target, config } => policy_init(&target, config.as_deref())?,
        Command::PolicyDeclare { policy, input } => policy_declare(&policy, &input)?,
        Command::PolicyStatus { policy } => policy_status(&policy)?,
        Command::PolicyDiagnose { policy, input } => policy_diagnose(&policy, &input)?,
        Command::EvidenceCommand {
            policy,
            cwd,
            expected_exit_code,
            command,
        } => evidence_command(&policy, cwd.as_deref(), expected_exit_code, command).await?,
        Command::EvidenceList { policy } => evidence_list(&policy)?,
        Command::EvidenceBrowser {
            policy,
            experience_root,
            experience_session_id,
            start_ns,
            end_ns,
            expected_before_text,
            expected_after_text,
        } => evidence_browser(
            &policy,
            &experience_root,
            experience_session_id,
            start_ns,
            end_ns,
            expected_before_text,
            expected_after_text,
        )?,
        Command::EventRelay { run } => serve_event_relay(&run)?,
        Command::PrepareTransfer { run, transfer_id } => {
            println!("{}", prepare_transfer(&run, transfer_id)?.display());
        }
        Command::ApplyTransfer {
            run,
            transfer_id,
            sha256,
        } => println!("{}", apply_transfer(&run, transfer_id, &sha256)?),
        #[cfg(target_os = "linux")]
        Command::Capture { run, output } => {
            let config = load_run(&run)?;
            let computer = connect_computer(&config).await?;
            computer
                .wait_for_stable_scanout(Duration::from_secs(10), Duration::from_millis(250))
                .await?;
            let hash = computer.save_screenshot(&output)?;
            println!("{}", json!({"screenshot": output, "frameSha256": hash}));
        }
        #[cfg(target_os = "linux")]
        Command::AudioObserve { run, duration_ms } => audio_observe(&run, duration_ms).await?,
        #[cfg(target_os = "linux")]
        Command::AudioInterpret {
            run,
            adapter_config,
            audio_event_id,
            prompt,
        } => audio_interpret(&run, &adapter_config, audio_event_id, prompt.as_deref())?,
        #[cfg(target_os = "linux")]
        Command::Smoke {
            run,
            url,
            screenshot,
        } => smoke(&run, &url, &screenshot).await?,
        #[cfg(target_os = "linux")]
        Command::ActClick {
            run,
            x,
            y,
            wait_after_ms,
        } => act_click(&run, x, y, wait_after_ms).await?,
        #[cfg(target_os = "linux")]
        Command::ActKey { run, keycode, mode } => act_key(&run, keycode, mode).await?,
        #[cfg(target_os = "linux")]
        Command::ActType { run, text } => act_type(&run, &text).await?,
        #[cfg(target_os = "linux")]
        Command::DragProof {
            run,
            from_x,
            from_y,
            to_x,
            to_y,
            steps,
        } => drag_proof(&run, from_x, from_y, to_x, to_y, steps).await?,
    }
    Ok(())
}

fn load_run(path: &Path) -> Result<RunConfig> {
    let config_path = if path.is_dir() {
        path.join("run.json")
    } else {
        path.to_owned()
    };
    RunConfig::load(&config_path).with_context(|| format!("load run {}", config_path.display()))
}

struct AgentEventContext {
    id: uuid::Uuid,
    candidate_workspace: PathBuf,
    timeline: PathBuf,
    events: PathBuf,
    session: Option<ExperimentSession>,
    run: Option<RunConfig>,
}

impl AgentEventContext {
    fn resolve(
        run: Option<&Path>,
        session: Option<&Path>,
        candidate: Option<&Path>,
        state_root: Option<&Path>,
    ) -> Result<Self> {
        match (run, session, candidate, state_root) {
            (Some(run), None, None, None) => {
                let run = load_run(run)?;
                let paths = run.paths();
                Ok(Self {
                    id: run.id,
                    candidate_workspace: run.candidate_workspace.clone(),
                    timeline: paths.timeline,
                    events: paths.events,
                    session: None,
                    run: Some(run),
                })
            }
            (None, Some(session), None, None) => {
                let session = ExperimentSession::load(session)?;
                let paths = session.paths();
                Ok(Self {
                    id: session.id,
                    candidate_workspace: session.candidate_workspace.clone(),
                    timeline: paths.timeline,
                    events: paths.events,
                    session: Some(session),
                    run: None,
                })
            }
            (None, None, Some(candidate), Some(state_root)) => {
                let session = ExperimentSession::create(candidate, state_root)?;
                let paths = session.paths();
                Ok(Self {
                    id: session.id,
                    candidate_workspace: session.candidate_workspace.clone(),
                    timeline: paths.timeline,
                    events: paths.events,
                    session: Some(session),
                    run: None,
                })
            }
            _ => bail!(
                "use exactly one target: --run, --session, or both --candidate and --state-root"
            ),
        }
    }
}

fn lifecycle(config: &RunConfig, kind: &str, payload: serde_json::Value) -> Result<()> {
    config.ensure_current_host_boot()?;
    let paths = config.paths();
    let sink =
        ExperienceEventSink::open(paths.timeline, paths.events, &config.candidate_workspace)?;
    sink.record(RawEvent::observed(config.id, "vm", kind, payload))
}

async fn destroy_run(path: &Path) -> Result<()> {
    let config = load_run(path)?;
    let controller = VmController::new(config.clone());
    if controller.is_running() {
        controller.stop().await?;
    }
    let saved = std::fs::canonicalize(config.paths().config)?;
    let state = std::fs::canonicalize(&config.state_dir)?;
    ensure!(
        saved.parent() == Some(state.as_path()),
        "run.json is not directly inside the state directory"
    );
    ensure!(
        state.file_name().and_then(|name| name.to_str()) == Some(&config.id.to_string()),
        "state directory does not match run ID"
    );
    std::fs::remove_dir_all(&state).with_context(|| format!("destroy run {}", state.display()))?;
    println!("destroyed {}", config.id);
    Ok(())
}

fn experience(config: &RunConfig) -> Result<ExperienceStore> {
    let paths = config.paths();
    ExperienceStore::open(config.id, paths.timeline, paths.artifacts)
}

fn record_canonical(config: &RunConfig, event: RawEvent) -> Result<()> {
    let paths = config.paths();
    ExperienceEventSink::open(paths.timeline, paths.events, &config.candidate_workspace)?
        .record(event)
}

struct PolicyTarget {
    id: uuid::Uuid,
    candidate_workspace: PathBuf,
    state_dir: PathBuf,
}

impl PolicyTarget {
    fn load(path: &Path) -> Result<Self> {
        let path = if path.is_dir() {
            let run = path.join("run.json");
            let session = path.join("session.json");
            if run.is_file() { run } else { session }
        } else {
            path.to_owned()
        };
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        if value.get("baseImage").is_some() {
            let run = RunConfig::load(&path)?;
            Ok(Self {
                id: run.id,
                candidate_workspace: run.candidate_workspace,
                state_dir: run.state_dir,
            })
        } else {
            let session = ExperimentSession::load(&path)?;
            Ok(Self {
                id: session.id,
                candidate_workspace: session.candidate_workspace,
                state_dir: session.state_dir,
            })
        }
    }
}

fn policy_init(target: &Path, config_path: Option<&Path>) -> Result<()> {
    let target = PolicyTarget::load(target)?;
    let config = match config_path {
        Some(path) => serde_json::from_slice(&std::fs::read(path)?)?,
        None => PolicyConfig::default(),
    };
    let policy_dir = target.state_dir.join("policy");
    ensure!(
        !policy_dir.join("policy-state.json").exists(),
        "policy is already initialized for this target"
    );
    let state = PolicyState::create(target.id, &target.candidate_workspace, &policy_dir, config)?;
    let mut event = RawEvent::observed(
        target.id,
        "evidence",
        "policy.initialized",
        json!({
            "policyState": state.state_path(),
            "phase": state.phase,
            "config": state.config,
        }),
    );
    event.provenance = Provenance::Observed;
    record_policy_event(&state, event)?;
    println!("{}", state.state_path().display());
    Ok(())
}

fn policy_declare(policy_path: &Path, input_path: &Path) -> Result<()> {
    let mut state = PolicyState::load(policy_path)?;
    let input: DevelopmentDeclarationInput = serde_json::from_slice(&std::fs::read(input_path)?)?;
    let declaration = input.bind(state.current_repository_fingerprint.clone())?;
    state.declare(declaration.clone())?;
    let mut event = RawEvent::observed(
        state.session_id,
        "agent",
        "policy.declaration.accepted",
        serde_json::to_value(declaration)?,
    );
    event.provenance = Provenance::AgentClaim;
    record_policy_event(&state, event)?;
    policy_status(policy_path)
}

fn policy_status(policy_path: &Path) -> Result<()> {
    let state = PolicyState::load(policy_path)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "state": state,
            "writeAllowed": state.phase == PolicyPhase::MutationAllowed,
            "writeBlockReasons": state.block_reasons(),
        }))?
    );
    Ok(())
}

fn policy_diagnose(policy_path: &Path, input_path: &Path) -> Result<()> {
    let mut state = PolicyState::load(policy_path)?;
    let input: DiagnosisPlanInput = serde_json::from_slice(&std::fs::read(input_path)?)?;
    let plan = input.into_plan();
    state.register_diagnosis(plan.clone())?;
    let mut event = RawEvent::observed(
        state.session_id,
        "agent",
        "policy.diagnosis.accepted",
        serde_json::to_value(plan)?,
    );
    event.provenance = Provenance::AgentClaim;
    record_policy_event(&state, event)?;
    policy_status(policy_path)
}

async fn evidence_command(
    policy_path: &Path,
    cwd: Option<&Path>,
    expected_exit_code: i32,
    command: Vec<String>,
) -> Result<()> {
    let mut state = PolicyState::load(policy_path)?;
    ensure!(
        state.phase != PolicyPhase::EvidenceFailed,
        "failed evidence requires policy-diagnose before another observation"
    );
    let target_state = state
        .state_dir
        .parent()
        .context("policy state has no supervisor target directory")?;
    let artifacts = ArtifactStore::new(target_state.join("artifacts"))?;
    let record = acquire_command_evidence(
        &state,
        &artifacts,
        EvidenceCommandOptions {
            command,
            cwd: cwd.unwrap_or(&state.candidate_workspace).to_owned(),
            expected_exit_code,
        },
    )
    .await?;
    EvidenceStore::open(&state.evidence_path())?.insert(&record)?;
    state.record_evidence(&record)?;
    let mut event = RawEvent::observed(
        state.session_id,
        "evidence",
        "evidence.command.completed",
        serde_json::to_value(&record)?,
    );
    event.provenance = Provenance::Observed;
    event.artifact_refs = record.raw_artifacts.clone();
    record_policy_event(&state, event)?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(())
}

fn evidence_list(policy_path: &Path) -> Result<()> {
    let state = PolicyState::load(policy_path)?;
    let records = EvidenceStore::open(&state.evidence_path())?.all(state.session_id)?;
    println!("{}", serde_json::to_string_pretty(&records)?);
    Ok(())
}

fn evidence_browser(
    policy_path: &Path,
    experience_root: &Path,
    experience_session_id: uuid::Uuid,
    start_ns: Option<u64>,
    end_ns: Option<u64>,
    expected_before_text: String,
    expected_after_text: String,
) -> Result<()> {
    let mut state = PolicyState::load(policy_path)?;
    let experience_root = experience_root.canonicalize()?;
    ensure!(
        !experience_root.starts_with(&state.candidate_workspace),
        "browser evidence source must be outside the candidate workspace"
    );
    let timeline = TimelineStore::open(experience_root.join("timeline.sqlite3"))?;
    let events = timeline.range(experience_session_id, start_ns, end_ns)?;
    let source_artifacts = ArtifactStore::new(experience_root.join("artifacts"))?;
    let record = acquire_browser_evidence(
        &state,
        &events,
        &source_artifacts,
        BrowserEvidenceOptions {
            expected_before_text,
            expected_after_text,
        },
    )?;
    let target_state = state
        .state_dir
        .parent()
        .context("policy state has no supervisor target directory")?;
    let target_artifacts = ArtifactStore::new(target_state.join("artifacts"))?;
    for artifact_ref in &record.raw_artifacts {
        let copied = target_artifacts.put(&source_artifacts.read(artifact_ref)?)?;
        ensure!(
            copied == *artifact_ref,
            "copied browser artifact changed content address"
        );
    }
    EvidenceStore::open(&state.evidence_path())?.insert(&record)?;
    state.record_evidence(&record)?;
    let mut event = RawEvent::observed(
        state.session_id,
        "evidence",
        "evidence.browser.completed",
        serde_json::to_value(&record)?,
    );
    event.provenance = Provenance::Observed;
    event.artifact_refs = record.raw_artifacts.clone();
    record_policy_event(&state, event)?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(())
}

fn record_policy_event(state: &PolicyState, event: RawEvent) -> Result<()> {
    let target_state = state
        .state_dir
        .parent()
        .context("policy state has no supervisor target directory")?;
    ExperienceEventSink::open_dynamic(
        target_state.join("timeline.sqlite3"),
        target_state.join("events.jsonl"),
        &state.candidate_workspace,
    )?
    .record(event)
}

fn resolve_interval(
    store: &ExperienceStore,
    start_ns: Option<u64>,
    end_ns: Option<u64>,
    last_duration_ms: Option<u64>,
) -> Result<(u64, u64)> {
    if start_ns.is_some() && last_duration_ms.is_some() {
        bail!("--start-ns and --last-duration-ms are mutually exclusive");
    }
    let end = end_ns
        .or(store.latest_ns()?)
        .context("timeline contains no events")?;
    let start = match last_duration_ms {
        Some(milliseconds) => end.saturating_sub(milliseconds.saturating_mul(1_000_000)),
        None => start_ns.unwrap_or(0),
    };
    ensure!(start <= end, "history start must not exceed end");
    Ok((start, end))
}

async fn observe(run: &Path, output: Option<&Path>, recent_limit: usize) -> Result<()> {
    let config = load_run(run)?;
    refresh_current_frame(&config).await?;
    let store = experience(&config)?;
    let latest = store.latest_ns()?.context("timeline contains no events")?;
    let current_screen = store.frame(latest, output)?;
    let events = store.history(None, Some(latest), &[])?;

    let mut recent_events = events
        .iter()
        .rev()
        .filter(|event| event.source != "display")
        .take(recent_limit)
        .cloned()
        .collect::<Vec<_>>();
    recent_events.reverse();
    let mut recent_display_events = events
        .iter()
        .rev()
        .filter(|event| event.source == "display")
        .take(3)
        .cloned()
        .collect::<Vec<_>>();
    recent_display_events.reverse();
    let mut unresolved_notable_events = events
        .iter()
        .rev()
        .filter(|event| {
            event.kind.contains("rejected")
                || event.kind.contains("unsupported")
                || event.kind.contains("failed")
        })
        .take(10)
        .cloned()
        .collect::<Vec<_>>();
    unresolved_notable_events.reverse();
    let repository_fingerprint = events
        .iter()
        .rev()
        .find_map(|event| event.repository_fingerprint.clone());

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "sessionId": config.id,
            "currentTime": {
                "hostMonotonicNs": monotonic_ns(),
                "wallClockTime": chrono::Utc::now().to_rfc3339(),
            },
            "repositoryFingerprint": repository_fingerprint,
            "currentScreen": current_screen,
            "focusedWindowOrApplication": null,
            "recentEvents": recent_events,
            "recentDisplayEvents": recent_display_events,
            "unresolvedNotableEvents": unresolved_notable_events,
        }))?
    );
    Ok(())
}

fn history(
    run: &Path,
    start_ns: Option<u64>,
    end_ns: Option<u64>,
    last_duration_ms: Option<u64>,
    sources: &[String],
) -> Result<()> {
    let config = load_run(run)?;
    let store = experience(&config)?;
    let (start, end) = resolve_interval(&store, start_ns, end_ns, last_duration_ms)?;
    let events = store.history(Some(start), Some(end), sources)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "sessionId": config.id,
            "startNs": start,
            "endNs": end,
            "sources": sources,
            "events": events,
        }))?
    );
    Ok(())
}

fn frame(
    run: &Path,
    at_ns: Option<u64>,
    event_id: Option<uuid::Uuid>,
    output: Option<&Path>,
) -> Result<()> {
    if at_ns.is_some() && event_id.is_some() {
        bail!("--at-ns and --event-id are mutually exclusive");
    }
    let config = load_run(run)?;
    let store = experience(&config)?;
    let target = if let Some(event_id) = event_id {
        let event = store
            .event(event_id)?
            .with_context(|| format!("event {event_id} is not in the timeline"))?;
        ensure!(
            event.session_id == config.id,
            "event belongs to another session"
        );
        event.host_monotonic_ns
    } else {
        at_ns
            .or(store.latest_ns()?)
            .context("timeline contains no events")?
    };
    let stored = store.frame(target, output)?;
    println!("{}", serde_json::to_string_pretty(&stored)?);
    Ok(())
}

fn replay(
    run: &Path,
    start_ns: Option<u64>,
    end_ns: Option<u64>,
    last_duration_ms: Option<u64>,
) -> Result<()> {
    let config = load_run(run)?;
    let store = experience(&config)?;
    let (start, end) = resolve_interval(&store, start_ns, end_ns, last_duration_ms)?;
    let replay = store.replay(start, end)?;
    let mut event = RawEvent::observed(
        config.id,
        "evidence",
        "experience.replay.created",
        json!({
            "startNs": start,
            "endNs": end,
            "inputEventCount": replay.input_events.len(),
            "keyframeCount": replay.keyframes.len(),
        }),
    );
    event.provenance = Provenance::Derived;
    event.artifact_refs = replay
        .keyframes
        .iter()
        .map(|frame| frame.artifact_ref.clone())
        .collect();
    record_canonical(&config, event)?;
    println!("{}", serde_json::to_string_pretty(&replay)?);
    Ok(())
}

fn temporal_analyze(
    run: &Path,
    start_ns: Option<u64>,
    end_ns: Option<u64>,
    last_duration_ms: Option<u64>,
    config_path: Option<&Path>,
) -> Result<()> {
    let config = load_run(run)?;
    let store = experience(&config)?;
    let (start, end) = resolve_interval(&store, start_ns, end_ns, last_duration_ms)?;
    let temporal_config = match config_path {
        Some(path) => serde_json::from_slice(&std::fs::read(path)?)?,
        None => TemporalConfig::default(),
    };
    let events = store.history(None, Some(end), &[])?;
    let artifacts = ArtifactStore::new(config.paths().artifacts)?;
    let analysis = analyze_temporal(&events, &artifacts, start, end, temporal_config)?;
    let mut event = RawEvent::observed(
        config.id,
        "perception",
        "perception.temporal.analysis",
        serde_json::to_value(&analysis)?,
    );
    event.provenance = Provenance::Derived;
    record_canonical(&config, event)?;
    println!("{}", serde_json::to_string_pretty(&analysis)?);
    Ok(())
}

fn experience_query(run: &Path, input: &Path) -> Result<()> {
    let config = load_run(run)?;
    let store = experience(&config)?;
    let query: ExperienceQuery = serde_json::from_slice(
        &std::fs::read(input)
            .with_context(|| format!("read experience query {}", input.display()))?,
    )?;
    let result = execute_query(&store, query)?;
    let mut event = RawEvent::observed(
        config.id,
        "experience",
        "experience.query.executed",
        json!({
            "query": result.query,
            "relation": result.relation,
            "interval": result.interval,
            "observedEventCount": result.observed_events.len(),
            "derivedEventCount": result.derived_events.len(),
            "modelInterpretationCount": result.model_interpretations.len(),
            "agentClaimCount": result.agent_claims.len(),
            "frameCount": result.frames.len(),
        }),
    );
    event.provenance = Provenance::Derived;
    event.artifact_refs = result.artifact_refs.clone();
    record_canonical(&config, event)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn vlm_observe(
    run: &Path,
    adapter_config_path: &Path,
    temporal_event_id: Option<uuid::Uuid>,
    observation_index: Option<usize>,
    prompt: &str,
) -> Result<()> {
    let config = load_run(run)?;
    let store = experience(&config)?;
    let temporal_event = match temporal_event_id {
        Some(event_id) => store
            .event(event_id)?
            .with_context(|| format!("temporal event {event_id} is not in the timeline"))?,
        None => store
            .history(None, None, &["perception".to_owned()])?
            .into_iter()
            .rev()
            .find(|event| event.kind == "perception.temporal.analysis")
            .context("timeline contains no temporal analysis event")?,
    };
    ensure!(
        temporal_event.session_id == config.id,
        "temporal event belongs to another run"
    );
    let adapter_config: CommandVlmConfig =
        serde_json::from_slice(&std::fs::read(adapter_config_path).with_context(|| {
            format!(
                "read VLM adapter configuration {}",
                adapter_config_path.display()
            )
        })?)?;
    let adapter = CommandVlmAdapter::new(adapter_config)?;
    let artifacts = ArtifactStore::new(config.paths().artifacts)?;
    let (event, observation) = observe_temporal_event(
        &store,
        &artifacts,
        &temporal_event,
        observation_index,
        prompt,
        &adapter,
    )?;
    record_canonical(&config, event)?;
    println!("{}", serde_json::to_string_pretty(&observation)?);
    Ok(())
}

fn accessibility_observe(run: &Path, duration_ms: u64) -> Result<()> {
    let config = load_run(run)?;
    config.ensure_current_host_boot()?;
    ensure!(
        VmController::new(config.clone()).is_running(),
        "VM must be running to observe guest accessibility"
    );
    let paths = config.paths();
    let sink: Arc<dyn EventSink> = Arc::new(ExperienceEventSink::open_dynamic(
        &paths.timeline,
        &paths.events,
        &config.candidate_workspace,
    )?);
    let result = observe_accessibility(
        config.id,
        &paths.accessibility_socket,
        sink,
        std::time::Duration::from_millis(duration_ms),
    )?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn runtime_import(run: &Path, input: &Path) -> Result<()> {
    let config = load_run(run)?;
    let bytes = std::fs::read(input)
        .with_context(|| format!("read runtime telemetry {}", input.display()))?;
    let artifacts = ArtifactStore::new(config.paths().artifacts)?;
    let (events, result) = import_runtime_jsonl(config.id, &bytes, &artifacts)?;
    let paths = config.paths();
    let sink = ExperienceEventSink::open_dynamic(
        paths.timeline,
        paths.events,
        &config.candidate_workspace,
    )?;
    for event in events {
        sink.record(event)?;
    }
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn browser_correlate(run: &Path, browser_event_id: Option<uuid::Uuid>) -> Result<()> {
    let config = load_run(run)?;
    let store = experience(&config)?;
    let browser_event = match browser_event_id {
        Some(event_id) => store
            .event(event_id)?
            .with_context(|| format!("browser event {event_id} is not in the timeline"))?,
        None => store
            .history(None, None, &["browser".into()])?
            .into_iter()
            .rev()
            .find(|event| event.kind == "browser.page.snapshot" && !event.artifact_refs.is_empty())
            .context("timeline contains no browser snapshot artifact")?,
    };
    ensure!(
        browser_event.session_id == config.id,
        "browser event belongs to another run"
    );
    ensure!(
        browser_event.kind == "browser.page.snapshot",
        "selected event is not a browser page snapshot"
    );
    let browser_artifact_ref = browser_event
        .artifact_refs
        .first()
        .context("browser snapshot has no viewport artifact")?;
    let frame = store.frame(browser_event.host_monotonic_ns, None)?;
    let artifacts = ArtifactStore::new(config.paths().artifacts)?;
    let correlation = correlate_viewport_png(
        &artifacts.read(&frame.artifact_ref)?,
        &artifacts.read(browser_artifact_ref)?,
    )?;
    let mut event = RawEvent::observed(
        config.id,
        "browser",
        "browser.coordinate_correlation",
        json!({
            "browserSnapshotEventId": browser_event.id,
            "displayFrameEventId": frame.event_id,
            "correlation": correlation,
        }),
    );
    event.provenance = Provenance::Derived;
    event.artifact_refs = vec![browser_artifact_ref.clone(), frame.artifact_ref.clone()];
    record_canonical(&config, event)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "runId": config.id,
            "browserSnapshotEventId": browser_event.id,
            "displayFrameEventId": frame.event_id,
            "browserArtifactRef": browser_artifact_ref,
            "displayArtifactRef": frame.artifact_ref,
            "correlation": correlation,
        }))?
    );
    Ok(())
}

fn browser_diagnose_failure(run: &Path, click_event_id: Option<uuid::Uuid>) -> Result<()> {
    let config = load_run(run)?;
    config.ensure_current_host_boot()?;
    let store = experience(&config)?;
    let events = store.history(None, None, &[])?;
    let diagnosis = diagnose_double_submit_failure(&events, click_event_id)?;
    let mut event = RawEvent::observed(
        config.id,
        "browser",
        "browser.failure.diagnosed",
        serde_json::to_value(&diagnosis)?,
    );
    event.provenance = Provenance::Derived;
    event.artifact_refs = diagnosis.artifact_refs.clone();
    record_canonical(&config, event)?;
    println!("{}", serde_json::to_string_pretty(&diagnosis)?);
    Ok(())
}

#[cfg(target_os = "linux")]
async fn refresh_current_frame(config: &RunConfig) -> Result<()> {
    if VmController::new(config.clone()).is_running() {
        let computer = connect_computer(config).await?;
        computer
            .wait_for_stable_scanout(Duration::from_secs(10), Duration::from_millis(250))
            .await?;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
async fn refresh_current_frame(_config: &RunConfig) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
async fn connect_computer(config: &RunConfig) -> Result<avm::display::HostComputer> {
    let paths = config.paths();
    let sink: Arc<dyn EventSink> = Arc::new(ExperienceEventSink::open(
        &paths.timeline,
        &paths.events,
        &config.candidate_workspace,
    )?);
    let artifacts = Arc::new(ArtifactStore::new(&paths.artifacts)?);
    avm::display::HostComputer::connect(&paths.display_socket, config.id, sink, artifacts).await
}

#[cfg(target_os = "linux")]
async fn audio_observe(run: &Path, duration_ms: u64) -> Result<()> {
    ensure!(
        duration_ms > 0,
        "audio observation duration must be positive"
    );
    let config = load_run(run)?;
    config.ensure_current_host_boot()?;
    ensure!(
        VmController::new(config.clone()).is_running(),
        "VM must be running to observe audio"
    );
    let paths = config.paths();
    let sink: Arc<dyn EventSink> = Arc::new(ExperienceEventSink::open_dynamic(
        &paths.timeline,
        &paths.events,
        &config.candidate_workspace,
    )?);
    let artifacts = Arc::new(ArtifactStore::new(&paths.artifacts)?);
    let capture =
        avm::audio::AudioCapture::connect(&paths.display_socket, config.id, sink, artifacts)
            .await?;
    let result = capture.observe(Duration::from_millis(duration_ms)).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

#[cfg(target_os = "linux")]
fn audio_interpret(
    run: &Path,
    adapter_config_path: &Path,
    audio_event_id: uuid::Uuid,
    prompt: Option<&str>,
) -> Result<()> {
    let config = load_run(run)?;
    let store = experience(&config)?;
    let raw_event = store
        .event(audio_event_id)?
        .with_context(|| format!("audio event {audio_event_id} is not in the timeline"))?;
    ensure!(
        raw_event.session_id == config.id,
        "audio event belongs to another run"
    );
    let adapter_config: CommandAudioAdapterConfig =
        serde_json::from_slice(&std::fs::read(adapter_config_path).with_context(|| {
            format!(
                "read audio adapter configuration {}",
                adapter_config_path.display()
            )
        })?)?;
    let default_prompt = match adapter_config.kind {
        AudioInterpretationKind::Transcription => DEFAULT_TRANSCRIPTION_PROMPT,
        AudioInterpretationKind::AudioEvent => DEFAULT_AUDIO_EVENT_PROMPT,
    };
    let adapter = CommandAudioAdapter::new(adapter_config)?;
    let artifacts = ArtifactStore::new(config.paths().artifacts)?;
    let (event, interpretation) = interpret_audio_event(
        &artifacts,
        &raw_event,
        prompt.unwrap_or(default_prompt),
        &adapter,
    )?;
    record_canonical(&config, event)?;
    println!("{}", serde_json::to_string_pretty(&interpretation)?);
    Ok(())
}

#[cfg(target_os = "linux")]
async fn smoke(run: &Path, url: &str, screenshot: &Path) -> Result<()> {
    let config = load_run(run)?;
    let computer = connect_computer(&config).await?;
    computer
        .wait_for_stable_frame_size(
            Duration::from_secs(60),
            Duration::from_millis(250),
            config.width,
            config.height,
        )
        .await?;

    // Ctrl+L opens Chromium's address bar through the guest's real keyboard path.
    let address_bar_started = computer.key_down(0x1d).await?;
    computer.key_press(0x26).await?;
    computer.key_up(0x1d).await?;
    computer
        .wait_for_display_after(address_bar_started.started_ns, Duration::from_secs(15))
        .await?;
    computer
        .wait_for_stable_scanout(Duration::from_secs(15), Duration::from_millis(250))
        .await?;
    computer.type_text(url).await?;
    let enter = computer.key_press(0x1c).await?;
    computer
        .wait_for_display_after(enter.started_ns, Duration::from_secs(15))
        .await?;
    computer
        .wait_for_stable_scanout(Duration::from_secs(15), Duration::from_millis(250))
        .await?;
    let hash = computer.save_screenshot(screenshot)?;
    println!(
        "{}",
        json!({
            "accepted": true, "inputActionId": enter.action_id, "postInputDisplayUpdate": true,
            "screenshot": screenshot, "frameSha256": hash
        })
    );
    Ok(())
}

#[cfg(target_os = "linux")]
async fn act_click(run: &Path, x: u32, y: u32, wait_after_ms: u64) -> Result<()> {
    let config = load_run(run)?;
    let computer = connect_computer(&config).await?;
    computer
        .wait_for_stable_frame_size(
            Duration::from_secs(60),
            Duration::from_millis(250),
            config.width,
            config.height,
        )
        .await?;
    let moved = computer.move_pointer(x, y).await?;
    let down = computer.mouse_down(avm::display::MouseButton::Left).await?;
    let up = computer.mouse_up(avm::display::MouseButton::Left).await?;
    tokio::time::sleep(Duration::from_millis(wait_after_ms)).await;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "runId": config.id,
            "coordinates": {"x": x, "y": y},
            "move": moved,
            "pointerDown": down,
            "pointerUp": up,
            "waitAfterMs": wait_after_ms,
        }))?
    );
    Ok(())
}

#[cfg(target_os = "linux")]
async fn act_key(run: &Path, keycode: u32, mode: KeyMode) -> Result<()> {
    let config = load_run(run)?;
    let computer = connect_computer(&config).await?;
    computer
        .wait_for_stable_frame_size(
            Duration::from_secs(60),
            Duration::from_millis(250),
            config.width,
            config.height,
        )
        .await?;
    let receipt = match mode {
        KeyMode::Press => computer.key_press(keycode).await?,
        KeyMode::Down => computer.key_down(keycode).await?,
        KeyMode::Up => computer.key_up(keycode).await?,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "runId": config.id,
            "keycode": keycode,
            "mode": format!("{mode:?}").to_lowercase(),
            "receipt": receipt,
        }))?
    );
    Ok(())
}

#[cfg(target_os = "linux")]
async fn act_type(run: &Path, text: &str) -> Result<()> {
    ensure!(!text.is_empty(), "text must not be empty");
    let config = load_run(run)?;
    let computer = connect_computer(&config).await?;
    computer
        .wait_for_stable_frame_size(
            Duration::from_secs(60),
            Duration::from_millis(250),
            config.width,
            config.height,
        )
        .await?;
    computer.type_text(text).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "runId": config.id,
            "characterCount": text.chars().count(),
        }))?
    );
    Ok(())
}

#[cfg(target_os = "linux")]
async fn drag_proof(
    run: &Path,
    from_x: u32,
    from_y: u32,
    to_x: u32,
    to_y: u32,
    steps: u32,
) -> Result<()> {
    ensure!(steps >= 2, "drag proof needs at least two trajectory steps");
    let config = load_run(run)?;
    let computer = connect_computer(&config).await?;
    computer
        .wait_for_stable_frame_size(
            Duration::from_secs(60),
            Duration::from_millis(250),
            config.width,
            config.height,
        )
        .await?;
    computer.move_pointer(from_x, from_y).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let down = computer.mouse_down(avm::display::MouseButton::Left).await?;
    computer
        .wait_for_display_after(down.started_ns, Duration::from_secs(5))
        .await?;
    for step in 1..=steps {
        let x = from_x as i64 + (to_x as i64 - from_x as i64) * step as i64 / steps as i64;
        let y = from_y as i64 + (to_y as i64 - from_y as i64) * step as i64 / steps as i64;
        computer.move_pointer(x as u32, y as u32).await?;
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let up = computer.mouse_up(avm::display::MouseButton::Left).await?;
    let update_count = computer.display_updates_between(down.started_ns, up.started_ns);
    ensure!(
        update_count > 0,
        "no display update occurred while the pointer was down"
    );
    println!(
        "{}",
        json!({
            "accepted": true, "pointerDownActionId": down.action_id,
            "pointerUpActionId": up.action_id, "trajectorySteps": steps,
            "displayUpdatesWhilePointerDown": update_count
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use avm::timeline::TimelineStore;

    #[test]
    fn codex_target_cli_requires_exactly_one_target_form() {
        assert!(
            Cli::try_parse_from([
                "avm",
                "codex-turn",
                "--run",
                "/tmp/run.json",
                "--prompt",
                "test"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "avm",
                "codex-turn",
                "--session",
                "/tmp/session.json",
                "--prompt",
                "test"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "avm",
                "codex-turn",
                "--candidate",
                "/tmp/candidate",
                "--state-root",
                "/tmp/state",
                "--prompt",
                "test"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "avm",
                "codex-turn",
                "--candidate",
                "/tmp/candidate",
                "--prompt",
                "test"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "avm",
                "codex-turn",
                "--run",
                "/tmp/run.json",
                "--session",
                "/tmp/session.json",
                "--prompt",
                "test"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "avm",
                "codex-turn",
                "--run",
                "/tmp/run.json",
                "--candidate",
                "/tmp/candidate",
                "--state-root",
                "/tmp/state",
                "--prompt",
                "test"
            ])
            .is_err()
        );
    }

    #[test]
    fn run_target_uses_vm_identity_and_canonical_event_files() {
        let temp = tempfile::tempdir().unwrap();
        let candidate = temp.path().join("candidate");
        let state = temp.path().join("state");
        let base = temp.path().join("base.qcow2");
        std::fs::create_dir(&candidate).unwrap();
        std::fs::write(&base, b"fixture").unwrap();
        let run = RunConfig::new(&base, &candidate, &state).unwrap();
        run.save().unwrap();

        let context =
            AgentEventContext::resolve(Some(&run.paths().config), None, None, None).unwrap();
        assert_eq!(context.id, run.id);
        assert_eq!(context.candidate_workspace, run.candidate_workspace);
        assert_eq!(context.timeline, run.paths().timeline);
        assert_eq!(context.events, run.paths().events);
        assert!(context.run.is_some());
        assert!(context.session.is_none());

        let sink = ExperienceEventSink::open_dynamic(
            &context.timeline,
            &context.events,
            &context.candidate_workspace,
        )
        .unwrap();
        sink.record(RawEvent::observed(
            context.id,
            "agent",
            "test.run_target",
            json!({}),
        ))
        .unwrap();
        let events = TimelineStore::open(&context.timeline)
            .unwrap()
            .all(run.id)
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session_id, run.id);
        assert!(events[0].repository_fingerprint.is_some());
    }

    #[test]
    fn session_target_reuses_identity_and_canonical_event_files() {
        let candidate = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(candidate.path())
            .status()
            .unwrap();
        let state = tempfile::tempdir().unwrap();
        let session = ExperimentSession::create(candidate.path(), state.path()).unwrap();
        let context =
            AgentEventContext::resolve(None, Some(&session.paths().config), None, None).unwrap();
        assert_eq!(context.id, session.id);
        assert_eq!(context.candidate_workspace, session.candidate_workspace);
        assert_eq!(context.timeline, session.paths().timeline);
        assert_eq!(context.events, session.paths().events);
        assert!(context.session.is_some());
        assert!(context.run.is_none());
    }
}
