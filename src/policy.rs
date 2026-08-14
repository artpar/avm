use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    event::{Provenance, RawEvent},
    fingerprint::repository_fingerprint,
    storage::ArtifactStore,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyConfig {
    pub maximum_mutation_events_without_evidence: u32,
    pub maximum_evidence_debt: u32,
    pub rules: Vec<SubsystemRule>,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            maximum_mutation_events_without_evidence: 3,
            maximum_evidence_debt: 6,
            rules: vec![
                SubsystemRule::new(
                    "ui_interaction",
                    &["html", "css", "tsx", "jsx"],
                    &["browser_interaction"],
                    2,
                ),
                SubsystemRule::new(
                    "typed_domain",
                    &["ts", "tsx"],
                    &["targeted_test", "type_check"],
                    1,
                ),
                SubsystemRule::new("database", &["sql"], &["migration_round_trip"], 2),
            ],
        }
    }
}

impl PolicyConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.maximum_mutation_events_without_evidence > 0,
            "maximum mutation events must be positive"
        );
        ensure!(
            self.maximum_evidence_debt > 0,
            "maximum evidence debt must be positive"
        );
        for rule in &self.rules {
            ensure!(!rule.name.trim().is_empty(), "policy rule name is empty");
            ensure!(
                !rule.required_observations.is_empty(),
                "policy rule {} has no required observation",
                rule.name
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsystemRule {
    pub name: String,
    #[serde(default)]
    pub path_prefixes: Vec<String>,
    #[serde(default)]
    pub extensions: Vec<String>,
    pub required_observations: Vec<String>,
    pub debt: u32,
}

impl SubsystemRule {
    fn new(name: &str, extensions: &[&str], observations: &[&str], debt: u32) -> Self {
        Self {
            name: name.into(),
            path_prefixes: Vec::new(),
            extensions: extensions.iter().map(|value| (*value).into()).collect(),
            required_observations: observations.iter().map(|value| (*value).into()).collect(),
            debt,
        }
    }

    fn matches(&self, path: &str) -> bool {
        self.path_prefixes.iter().any(|prefix| {
            path == prefix
                || path
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
        }) || Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| self.extensions.iter().any(|value| value == extension))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevelopmentDeclaration {
    pub id: Uuid,
    pub intended_behavior_change: String,
    pub causal_hypothesis: String,
    pub observation_type: String,
    pub instrument: String,
    pub action_or_command: String,
    pub predicted_result: String,
    pub contradiction_meaning: String,
    pub repository_fingerprint: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevelopmentDeclarationInput {
    pub intended_behavior_change: String,
    pub causal_hypothesis: String,
    pub observation_type: String,
    pub instrument: String,
    pub action_or_command: String,
    pub predicted_result: String,
    pub contradiction_meaning: String,
}

impl DevelopmentDeclarationInput {
    pub fn bind(self, repository_fingerprint: String) -> Result<DevelopmentDeclaration> {
        DevelopmentDeclaration::new(
            self.intended_behavior_change,
            self.causal_hypothesis,
            self.observation_type,
            self.instrument,
            self.action_or_command,
            self.predicted_result,
            self.contradiction_meaning,
            repository_fingerprint,
        )
    }
}

impl DevelopmentDeclaration {
    // The constructor mirrors the externally persisted declaration schema.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        intended_behavior_change: String,
        causal_hypothesis: String,
        observation_type: String,
        instrument: String,
        action_or_command: String,
        predicted_result: String,
        contradiction_meaning: String,
        repository_fingerprint: String,
    ) -> Result<Self> {
        let declaration = Self {
            id: Uuid::new_v4(),
            intended_behavior_change,
            causal_hypothesis,
            observation_type,
            instrument,
            action_or_command,
            predicted_result,
            contradiction_meaning,
            repository_fingerprint,
            created_at: now(),
        };
        declaration.validate()?;
        Ok(declaration)
    }

    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("intended behavior change", &self.intended_behavior_change),
            ("causal hypothesis", &self.causal_hypothesis),
            ("observation type", &self.observation_type),
            ("instrument", &self.instrument),
            ("action or command", &self.action_or_command),
            ("predicted result", &self.predicted_result),
            ("contradiction meaning", &self.contradiction_meaning),
        ] {
            ensure!(
                value.trim().len() >= 4,
                "declaration {name} is missing or too vague"
            );
        }
        let normalized = self.action_or_command.trim().to_ascii_lowercase();
        ensure!(
            !matches!(
                normalized.as_str(),
                "verify" | "verify it works" | "test it" | "check it" | "make sure it works"
            ),
            "action or command must name a discriminating external observation"
        );
        ensure!(
            self.repository_fingerprint.starts_with("sha256:"),
            "declaration has no repository fingerprint"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosisPlan {
    pub suspected_layer: String,
    pub causal_hypothesis: String,
    pub observation_type: String,
    pub instrument: String,
    pub action_or_command: String,
    pub predicted_outcomes: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosisPlanInput {
    pub suspected_layer: String,
    pub causal_hypothesis: String,
    pub observation_type: String,
    pub instrument: String,
    pub action_or_command: String,
    pub predicted_outcomes: String,
}

impl DiagnosisPlanInput {
    pub fn into_plan(self) -> DiagnosisPlan {
        DiagnosisPlan {
            suspected_layer: self.suspected_layer,
            causal_hypothesis: self.causal_hypothesis,
            observation_type: self.observation_type,
            instrument: self.instrument,
            action_or_command: self.action_or_command,
            predicted_outcomes: self.predicted_outcomes,
            created_at: now(),
        }
    }
}

impl DiagnosisPlan {
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("suspected layer", &self.suspected_layer),
            ("causal hypothesis", &self.causal_hypothesis),
            ("observation type", &self.observation_type),
            ("instrument", &self.instrument),
            ("action or command", &self.action_or_command),
            ("predicted outcomes", &self.predicted_outcomes),
        ] {
            ensure!(
                value.trim().len() >= 4,
                "diagnosis {name} is missing or too vague"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PolicyPhase {
    AwaitingDeclaration,
    MutationAllowed,
    EvidenceRequired,
    EvidenceFailed,
    DiagnosticObservationRequired,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyState {
    pub session_id: Uuid,
    pub candidate_workspace: PathBuf,
    pub state_dir: PathBuf,
    pub phase: PolicyPhase,
    pub current_repository_fingerprint: String,
    pub last_evidence_fingerprint: Option<String>,
    pub debt: u32,
    pub debt_reasons: Vec<String>,
    pub mutation_events_since_evidence: u32,
    pub required_observations: BTreeSet<String>,
    pub satisfied_observations: BTreeSet<String>,
    pub active_declaration: Option<DevelopmentDeclaration>,
    #[serde(default)]
    pub pending_evidence_declaration: Option<DevelopmentDeclaration>,
    pub diagnosis_plan: Option<DiagnosisPlan>,
    pub config: PolicyConfig,
}

impl PolicyState {
    pub fn create(
        session_id: Uuid,
        candidate_workspace: &Path,
        state_dir: &Path,
        config: PolicyConfig,
    ) -> Result<Self> {
        config.validate()?;
        let candidate_workspace = candidate_workspace.canonicalize()?;
        std::fs::create_dir_all(state_dir)?;
        let state_dir = state_dir.canonicalize()?;
        ensure!(
            !state_dir.starts_with(&candidate_workspace),
            "policy state must be outside the candidate workspace"
        );
        let state = Self {
            session_id,
            current_repository_fingerprint: repository_fingerprint(&candidate_workspace)?,
            candidate_workspace,
            state_dir,
            phase: PolicyPhase::AwaitingDeclaration,
            last_evidence_fingerprint: None,
            debt: 0,
            debt_reasons: Vec::new(),
            mutation_events_since_evidence: 0,
            required_observations: BTreeSet::new(),
            satisfied_observations: BTreeSet::new(),
            active_declaration: None,
            pending_evidence_declaration: None,
            diagnosis_plan: None,
            config,
        };
        state.save()?;
        Ok(state)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let state: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        ensure!(
            state.state_path().canonicalize()? == path.canonicalize()?,
            "policy state path does not match its owned directory"
        );
        state.config.validate()?;
        Ok(state)
    }

    pub fn state_path(&self) -> PathBuf {
        self.state_dir.join("policy-state.json")
    }

    pub fn evidence_path(&self) -> PathBuf {
        self.state_dir.join("evidence.sqlite3")
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(&self.state_dir)?;
        let temporary = self
            .state_dir
            .join(format!(".policy-state-{}.tmp", Uuid::new_v4()));
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(temporary, self.state_path())?;
        Ok(())
    }

    pub fn declare(&mut self, declaration: DevelopmentDeclaration) -> Result<()> {
        ensure!(
            self.phase == PolicyPhase::AwaitingDeclaration,
            "declaration is not accepted while policy phase is {:?}",
            self.phase
        );
        declaration.validate()?;
        let current = repository_fingerprint(&self.candidate_workspace)?;
        ensure!(
            current == self.current_repository_fingerprint,
            "candidate changed outside the supervisor: expected {}, observed {}",
            self.current_repository_fingerprint,
            current
        );
        ensure!(
            declaration.repository_fingerprint == current,
            "declaration belongs to repository {}, current repository is {}",
            declaration.repository_fingerprint,
            current
        );
        self.active_declaration = Some(declaration);
        self.phase = PolicyPhase::MutationAllowed;
        self.save()
    }

    pub fn ensure_mutation_allowed(&self) -> Result<()> {
        ensure!(
            self.phase == PolicyPhase::MutationAllowed && self.active_declaration.is_some(),
            "write blocked because: {}",
            self.block_reasons().join("; ")
        );
        Ok(())
    }

    pub fn record_mutation(
        &mut self,
        before_fingerprint: &str,
        after_fingerprint: String,
        changed_files: &[ChangedFile],
    ) -> Result<()> {
        self.ensure_mutation_allowed()?;
        ensure!(
            before_fingerprint == self.current_repository_fingerprint,
            "mutation baseline fingerprint changed outside the supervisor"
        );
        ensure!(
            after_fingerprint != before_fingerprint,
            "mutation batch did not change the repository"
        );
        ensure!(
            !changed_files.is_empty(),
            "mutation batch has no changed files"
        );

        self.mutation_events_since_evidence += 1;
        self.debt = self.debt.saturating_add(1);
        self.debt_reasons.push(format!(
            "mutation event {} occurred after the last evidence",
            self.mutation_events_since_evidence
        ));
        if changed_files.iter().any(|file| file.status == "added") {
            self.debt = self.debt.saturating_add(1);
            self.debt_reasons
                .push("new files appeared after the last evidence".into());
        }
        let mut touched_subsystems = BTreeSet::new();
        for file in changed_files {
            for rule in self
                .config
                .rules
                .iter()
                .filter(|rule| rule.matches(&file.path))
            {
                if touched_subsystems.insert(rule.name.clone()) {
                    self.debt = self.debt.saturating_add(rule.debt);
                    self.debt_reasons.push(format!(
                        "{} changed and requires {}",
                        rule.name,
                        rule.required_observations.join(" + ")
                    ));
                    self.required_observations
                        .extend(rule.required_observations.iter().cloned());
                }
            }
        }
        if touched_subsystems.len() > 1 {
            self.debt = self
                .debt
                .saturating_add((touched_subsystems.len() - 1) as u32);
            self.debt_reasons
                .push("more than one subsystem changed in the batch".into());
        }
        self.current_repository_fingerprint = after_fingerprint;
        self.pending_evidence_declaration = self.active_declaration.take();
        self.satisfied_observations.clear();
        self.phase = if self.mutation_events_since_evidence
            >= self.config.maximum_mutation_events_without_evidence
            || self.debt >= self.config.maximum_evidence_debt
        {
            PolicyPhase::EvidenceRequired
        } else {
            PolicyPhase::AwaitingDeclaration
        };
        self.save()
    }

    pub fn register_diagnosis(&mut self, plan: DiagnosisPlan) -> Result<()> {
        ensure!(
            self.phase == PolicyPhase::EvidenceFailed,
            "diagnosis is only accepted in EVIDENCE_FAILED"
        );
        plan.validate()?;
        self.diagnosis_plan = Some(plan);
        self.phase = PolicyPhase::DiagnosticObservationRequired;
        self.save()
    }

    pub fn record_evidence(&mut self, record: &EvidenceRecord) -> Result<()> {
        ensure!(
            record.repository_fingerprint == self.current_repository_fingerprint,
            "evidence fingerprint {} does not match current repository {}",
            record.repository_fingerprint,
            self.current_repository_fingerprint
        );
        ensure!(
            record.provenance == Provenance::Observed,
            "only independently observed evidence can change policy state"
        );
        let diagnostic = self.phase == PolicyPhase::DiagnosticObservationRequired;
        if self.phase == PolicyPhase::EvidenceFailed {
            bail!("write blocked because failed evidence requires a diagnosis plan first");
        }
        self.last_evidence_fingerprint = Some(record.repository_fingerprint.clone());
        self.satisfied_observations
            .insert(record.observation_type.clone());

        if diagnostic {
            self.diagnosis_plan = None;
            self.pending_evidence_declaration = None;
            self.debt = self.debt.saturating_sub(1);
            if !self.debt_reasons.is_empty() {
                self.debt_reasons.remove(0);
            }
            self.phase = PolicyPhase::AwaitingDeclaration;
        } else {
            match record.verdict {
                EvidenceVerdict::Contradicted => {
                    self.debt = self
                        .debt
                        .max(self.config.maximum_evidence_debt)
                        .saturating_add(1);
                    self.debt_reasons.push(format!(
                        "evidence {} contradicted the declared prediction",
                        record.id
                    ));
                    self.phase = PolicyPhase::EvidenceFailed;
                }
                EvidenceVerdict::Inconclusive => {
                    self.debt_reasons
                        .push(format!("evidence {} was inconclusive", record.id));
                    self.phase = PolicyPhase::EvidenceRequired;
                }
                EvidenceVerdict::Supported => {
                    if self
                        .required_observations
                        .is_subset(&self.satisfied_observations)
                    {
                        self.debt = 0;
                        self.debt_reasons.clear();
                        self.mutation_events_since_evidence = 0;
                        self.required_observations.clear();
                        self.satisfied_observations.clear();
                        self.pending_evidence_declaration = None;
                        self.phase = PolicyPhase::AwaitingDeclaration;
                    } else {
                        self.debt = self.debt.saturating_sub(1);
                        self.phase = PolicyPhase::EvidenceRequired;
                    }
                }
            }
        }
        self.save()
    }

    pub fn block_reasons(&self) -> Vec<String> {
        let mut reasons = Vec::new();
        match self.phase {
            PolicyPhase::AwaitingDeclaration => {
                reasons.push("no structured pre-edit declaration is active".into())
            }
            PolicyPhase::MutationAllowed => {}
            PolicyPhase::EvidenceRequired => reasons.push(format!(
                "evidence debt is {} and required observations are missing: {}",
                self.debt,
                self.required_observations
                    .difference(&self.satisfied_observations)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            PolicyPhase::EvidenceFailed => reasons.push(
                "policy is EVIDENCE_FAILED; submit a causal diagnosis plan before observing again"
                    .into(),
            ),
            PolicyPhase::DiagnosticObservationRequired => reasons.push(
                "a diagnosis plan exists but its discriminating observation has not been acquired"
                    .into(),
            ),
        }
        reasons.extend(self.debt_reasons.iter().cloned());
        reasons
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceVerdict {
    Supported,
    Contradicted,
    Inconclusive,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRecord {
    pub id: Uuid,
    pub session_id: Uuid,
    pub repository_fingerprint: String,
    pub claim: String,
    pub prediction: String,
    pub contradiction_meaning: String,
    pub observation_type: String,
    pub instrument: String,
    pub started_at: String,
    pub completed_at: String,
    pub action_or_command: String,
    pub working_directory: Option<String>,
    pub environment_identity: Option<String>,
    pub exit_code: Option<i32>,
    pub raw_artifacts: Vec<String>,
    #[serde(default)]
    pub supporting_event_ids: Vec<Uuid>,
    pub observed_result: String,
    pub verdict: EvidenceVerdict,
    pub provenance: Provenance,
}

#[derive(Clone, Debug)]
pub struct EvidenceCommandOptions {
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub expected_exit_code: i32,
}

#[derive(Clone, Debug)]
pub struct BrowserEvidenceOptions {
    pub expected_before_text: String,
    pub expected_after_text: String,
}

pub fn acquire_browser_evidence(
    state: &PolicyState,
    events: &[RawEvent],
    artifacts: &ArtifactStore,
    options: BrowserEvidenceOptions,
) -> Result<EvidenceRecord> {
    ensure!(
        state.phase != PolicyPhase::EvidenceFailed,
        "failed evidence requires a diagnosis plan before another observation"
    );
    let declaration = state
        .pending_evidence_declaration
        .as_ref()
        .or(state.active_declaration.as_ref())
        .context("browser evidence has no declaration to test")?;
    let mut ordered = events
        .iter()
        .filter(|event| {
            event.repository_fingerprint.as_deref() == Some(&state.current_repository_fingerprint)
        })
        .collect::<Vec<_>>();
    ordered.sort_by_key(|event| (event.host_monotonic_ns, event.id));
    let snapshots = ordered
        .iter()
        .copied()
        .filter(|event| event.kind == "browser.page.snapshot")
        .collect::<Vec<_>>();
    let contains = |event: &RawEvent, expected: &str| {
        event
            .payload
            .get("dom")
            .is_some_and(|dom| dom.to_string().contains(expected))
    };
    let before = snapshots
        .iter()
        .copied()
        .find(|event| contains(event, &options.expected_before_text));
    let after_before_ns = before.map(|event| event.host_monotonic_ns).unwrap_or(0);
    let pointer_down = ordered
        .iter()
        .copied()
        .find(|event| event.kind == "pointer.down" && event.host_monotonic_ns > after_before_ns);
    let pointer_up = pointer_down.and_then(|down| {
        ordered.iter().copied().find(|event| {
            event.kind == "pointer.up" && event.host_monotonic_ns >= down.host_monotonic_ns
        })
    });
    let after_expected = pointer_down.and_then(|down| {
        snapshots.iter().copied().find(|event| {
            event.host_monotonic_ns > down.host_monotonic_ns
                && contains(event, &options.expected_after_text)
        })
    });
    let final_after = pointer_down.and_then(|down| {
        snapshots
            .iter()
            .rev()
            .copied()
            .find(|event| event.host_monotonic_ns > down.host_monotonic_ns)
    });
    let selected_after = after_expected.or(final_after);
    let display = pointer_down.and_then(|down| {
        ordered.iter().copied().find(|event| {
            matches!(event.kind.as_str(), "display.scanout" | "display.update")
                && event.host_monotonic_ns > down.host_monotonic_ns
                && selected_after
                    .is_none_or(|after| event.host_monotonic_ns <= after.host_monotonic_ns)
        })
    });
    let correlation = selected_after.and_then(|after| {
        let expected = after.id.to_string();
        ordered.iter().copied().find(|event| {
            event.kind == "browser.coordinate_correlation"
                && event
                    .payload
                    .get("browserSnapshotEventId")
                    .and_then(serde_json::Value::as_str)
                    == Some(expected.as_str())
        })
    });

    let observations = [
        ("before snapshot", before),
        ("pointer down", pointer_down),
        ("pointer up", pointer_up),
        ("post-click snapshot", selected_after),
        ("display response", display),
        ("coordinate correlation", correlation),
    ];
    let missing = observations
        .iter()
        .filter_map(|(name, event)| event.is_none().then_some(*name))
        .collect::<Vec<_>>();
    let verdict = if missing.is_empty() && after_expected.is_some() {
        EvidenceVerdict::Supported
    } else if before.is_some()
        && pointer_down.is_some()
        && pointer_up.is_some()
        && final_after.is_some()
        && after_expected.is_none()
    {
        EvidenceVerdict::Contradicted
    } else {
        EvidenceVerdict::Inconclusive
    };
    let supporting_event_ids = observations
        .iter()
        .filter_map(|(_, event)| event.map(|event| event.id))
        .collect::<Vec<_>>();
    let raw_artifacts = supporting_event_ids
        .iter()
        .filter_map(|id| events.iter().find(|event| event.id == *id))
        .flat_map(|event| event.artifact_refs.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    for artifact_ref in &raw_artifacts {
        artifacts
            .read(artifact_ref)
            .with_context(|| format!("verify browser evidence artifact {artifact_ref}"))?;
    }
    let started_at = before
        .map(|event| event.wall_clock_time.clone())
        .unwrap_or_else(now);
    let completed_at = selected_after
        .map(|event| event.wall_clock_time.clone())
        .unwrap_or_else(now);
    Ok(EvidenceRecord {
        id: Uuid::new_v4(),
        session_id: state.session_id,
        repository_fingerprint: state.current_repository_fingerprint.clone(),
        claim: declaration.intended_behavior_change.clone(),
        prediction: format!(
            "{}; visible transition {:?} -> {:?}",
            declaration.predicted_result, options.expected_before_text, options.expected_after_text
        ),
        contradiction_meaning: declaration.contradiction_meaning.clone(),
        observation_type: "browser_interaction".into(),
        instrument: "QEMU D-Bus display plus Playwright/CDP browser observer".into(),
        started_at,
        completed_at,
        action_or_command: "one physical pointer click through the QEMU input path".into(),
        working_directory: None,
        environment_identity: Some("instrumented QEMU/KVM guest browser".into()),
        exit_code: None,
        raw_artifacts,
        supporting_event_ids,
        observed_result: serde_json::to_string(&serde_json::json!({
            "expectedBeforeText": options.expected_before_text,
            "expectedAfterText": options.expected_after_text,
            "afterTextObserved": after_expected.is_some(),
            "missingObservations": missing,
        }))?,
        verdict,
        provenance: Provenance::Observed,
    })
}

pub async fn acquire_command_evidence(
    state: &PolicyState,
    artifacts: &ArtifactStore,
    options: EvidenceCommandOptions,
) -> Result<EvidenceRecord> {
    ensure!(!options.command.is_empty(), "evidence command is empty");
    let cwd = options.cwd.canonicalize()?;
    ensure!(
        cwd.starts_with(&state.candidate_workspace),
        "evidence command working directory must be inside the candidate"
    );
    let before = repository_fingerprint(&state.candidate_workspace)?;
    ensure!(
        before == state.current_repository_fingerprint,
        "candidate changed outside the supervisor before evidence acquisition"
    );
    let (claim, prediction, contradiction_meaning, observation_type, instrument) =
        if state.phase == PolicyPhase::DiagnosticObservationRequired {
            let plan = state
                .diagnosis_plan
                .as_ref()
                .context("diagnostic observation has no diagnosis plan")?;
            (
                format!("diagnose suspected layer: {}", plan.suspected_layer),
                plan.predicted_outcomes.clone(),
                format!(
                    "the observed outcome discriminates: {}",
                    plan.causal_hypothesis
                ),
                plan.observation_type.clone(),
                plan.instrument.clone(),
            )
        } else {
            let declaration = state
                .pending_evidence_declaration
                .as_ref()
                .or(state.active_declaration.as_ref())
                .context("evidence acquisition has no declaration to test")?;
            (
                declaration.intended_behavior_change.clone(),
                declaration.predicted_result.clone(),
                declaration.contradiction_meaning.clone(),
                declaration.observation_type.clone(),
                declaration.instrument.clone(),
            )
        };
    let action_or_command = serde_json::to_string(&options.command)?;
    let started_at = now();
    let output = tokio::process::Command::new(&options.command[0])
        .args(&options.command[1..])
        .current_dir(&cwd)
        .output()
        .await
        .with_context(|| format!("execute evidence command {}", options.command[0]))?;
    let completed_at = now();
    let stdout_ref = artifacts.put(&output.stdout)?;
    let stderr_ref = artifacts.put(&output.stderr)?;
    let after = repository_fingerprint(&state.candidate_workspace)?;
    let exit_code = output.status.code();
    let verdict = if after != before || exit_code.is_none() {
        EvidenceVerdict::Inconclusive
    } else if exit_code == Some(options.expected_exit_code) {
        EvidenceVerdict::Supported
    } else {
        EvidenceVerdict::Contradicted
    };
    let mutation_note = if after == before {
        "repository fingerprint remained unchanged".to_owned()
    } else {
        format!("repository changed during observation from {before} to {after}")
    };
    Ok(EvidenceRecord {
        id: Uuid::new_v4(),
        session_id: state.session_id,
        repository_fingerprint: before,
        claim,
        prediction,
        contradiction_meaning,
        observation_type,
        instrument,
        started_at,
        completed_at,
        action_or_command,
        working_directory: Some(cwd.display().to_string()),
        environment_identity: Some(format!(
            "os={};arch={};program={}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            options.command[0]
        )),
        exit_code,
        raw_artifacts: vec![stdout_ref, stderr_ref],
        supporting_event_ids: Vec::new(),
        observed_result: format!(
            "exitCode={exit_code:?}; stdoutBytes={}; stderrBytes={}; {mutation_note}",
            output.stdout.len(),
            output.stderr.len()
        ),
        verdict,
        provenance: Provenance::Observed,
    })
}

impl EvidenceRecord {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.repository_fingerprint.starts_with("sha256:"),
            "evidence has no repository fingerprint"
        );
        ensure!(
            self.provenance == Provenance::Observed,
            "completed evidence must be independently observed"
        );
        for (name, value) in [
            ("claim", &self.claim),
            ("prediction", &self.prediction),
            ("contradiction meaning", &self.contradiction_meaning),
            ("observation type", &self.observation_type),
            ("instrument", &self.instrument),
            ("action or command", &self.action_or_command),
            ("observed result", &self.observed_result),
        ] {
            ensure!(!value.trim().is_empty(), "evidence {name} is empty");
        }
        Ok(())
    }
}

pub struct EvidenceStore {
    connection: Connection,
}

impl EvidenceStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS evidence_records (
               id TEXT PRIMARY KEY NOT NULL,
               session_id TEXT NOT NULL,
               repository_fingerprint TEXT NOT NULL,
               record_json TEXT NOT NULL,
               completed_at TEXT NOT NULL
             );
             CREATE TRIGGER IF NOT EXISTS evidence_records_no_update
               BEFORE UPDATE ON evidence_records
               BEGIN SELECT RAISE(ABORT, 'completed evidence records are immutable'); END;
             CREATE TRIGGER IF NOT EXISTS evidence_records_no_delete
               BEFORE DELETE ON evidence_records
               BEGIN SELECT RAISE(ABORT, 'completed evidence records are immutable'); END;",
        )?;
        Ok(Self { connection })
    }

    pub fn insert(&self, record: &EvidenceRecord) -> Result<()> {
        record.validate()?;
        self.connection.execute(
            "INSERT INTO evidence_records
             (id, session_id, repository_fingerprint, record_json, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.id.to_string(),
                record.session_id.to_string(),
                record.repository_fingerprint,
                serde_json::to_string(record)?,
                record.completed_at,
            ],
        )?;
        Ok(())
    }

    pub fn all(&self, session_id: Uuid) -> Result<Vec<EvidenceRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT record_json FROM evidence_records
             WHERE session_id = ?1 ORDER BY completed_at, id",
        )?;
        let rows = statement.query_map([session_id.to_string()], |row| {
            let value: String = row.get(0)?;
            serde_json::from_str(&value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git_candidate() -> tempfile::TempDir {
        let candidate = tempfile::tempdir().unwrap();
        std::fs::write(candidate.path().join("app.tsx"), "export const x = 1;\n").unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "avm@example.invalid"],
            vec!["config", "user.name", "AVM"],
            vec!["add", "."],
            vec!["commit", "-qm", "baseline"],
        ] {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(candidate.path())
                    .status()
                    .unwrap()
                    .success()
            );
        }
        candidate
    }

    fn declaration(fingerprint: String) -> DevelopmentDeclaration {
        DevelopmentDeclaration::new(
            "disable Save while its request is pending".into(),
            "duplicate dispatch occurs because both handlers remain active".into(),
            "browser_interaction".into(),
            "Playwright plus network observer".into(),
            "click Save once and count POST /api/document requests".into(),
            "one POST occurs and the button remains disabled until response".into(),
            "two POSTs mean duplicate event dispatch remains".into(),
            fingerprint,
        )
        .unwrap()
    }

    fn evidence(state: &PolicyState, verdict: EvidenceVerdict) -> EvidenceRecord {
        EvidenceRecord {
            id: Uuid::new_v4(),
            session_id: state.session_id,
            repository_fingerprint: state.current_repository_fingerprint.clone(),
            claim: "Save dispatch is single-shot".into(),
            prediction: "one POST".into(),
            contradiction_meaning: "two POSTs indicate duplicate dispatch".into(),
            observation_type: "browser_interaction".into(),
            instrument: "Playwright network observer".into(),
            started_at: now(),
            completed_at: now(),
            action_or_command: "click Save once".into(),
            working_directory: None,
            environment_identity: Some("test".into()),
            exit_code: None,
            raw_artifacts: vec!["sha256:raw".into()],
            supporting_event_ids: Vec::new(),
            observed_result: "two POST requests".into(),
            verdict,
            provenance: Provenance::Observed,
        }
    }

    #[test]
    fn declaration_is_required_and_vague_observations_are_rejected() {
        let candidate = git_candidate();
        let state_dir = tempfile::tempdir().unwrap();
        let mut state = PolicyState::create(
            Uuid::new_v4(),
            candidate.path(),
            state_dir.path(),
            PolicyConfig::default(),
        )
        .unwrap();
        assert!(state.ensure_mutation_allowed().is_err());
        let vague = DevelopmentDeclaration::new(
            "change behavior".into(),
            "a handler is wrong".into(),
            "test".into(),
            "test runner".into(),
            "verify it works".into(),
            "it works".into(),
            "it does not".into(),
            state.current_repository_fingerprint.clone(),
        );
        assert!(vague.is_err());
        state
            .declare(declaration(state.current_repository_fingerprint.clone()))
            .unwrap();
        state.ensure_mutation_allowed().unwrap();
    }

    #[test]
    fn debt_is_fingerprint_bound_and_subsystem_requirements_are_explainable() {
        let candidate = git_candidate();
        let state_dir = tempfile::tempdir().unwrap();
        let mut state = PolicyState::create(
            Uuid::new_v4(),
            candidate.path(),
            state_dir.path(),
            PolicyConfig::default(),
        )
        .unwrap();
        let before = state.current_repository_fingerprint.clone();
        state.declare(declaration(before.clone())).unwrap();
        std::fs::write(candidate.path().join("app.tsx"), "export const x = 2;\n").unwrap();
        let after = repository_fingerprint(candidate.path()).unwrap();
        state
            .record_mutation(
                &before,
                after.clone(),
                &[ChangedFile {
                    path: "app.tsx".into(),
                    status: "modified".into(),
                }],
            )
            .unwrap();
        assert_eq!(state.current_repository_fingerprint, after);
        assert!(state.debt >= 4);
        assert!(state.required_observations.contains("browser_interaction"));
        assert!(state.required_observations.contains("targeted_test"));
        assert!(state.required_observations.contains("type_check"));
        assert!(
            state
                .block_reasons()
                .iter()
                .any(|reason| reason.contains("ui_interaction"))
        );
    }

    #[test]
    fn failed_evidence_requires_diagnosis_and_new_observation_before_editing() {
        let candidate = git_candidate();
        let state_dir = tempfile::tempdir().unwrap();
        let mut state = PolicyState::create(
            Uuid::new_v4(),
            candidate.path(),
            state_dir.path(),
            PolicyConfig::default(),
        )
        .unwrap();
        let contradicted = evidence(&state, EvidenceVerdict::Contradicted);
        state.record_evidence(&contradicted).unwrap();
        assert_eq!(state.phase, PolicyPhase::EvidenceFailed);
        assert!(state.record_evidence(&contradicted).is_err());
        state
            .register_diagnosis(DiagnosisPlan {
                suspected_layer: "browser event dispatch".into(),
                causal_hypothesis: "both click handlers remain registered".into(),
                observation_type: "browser_interaction".into(),
                instrument: "Playwright network observer".into(),
                action_or_command: "click Save once and count requests".into(),
                predicted_outcomes: "one request refutes duplication; two supports it".into(),
                created_at: now(),
            })
            .unwrap();
        assert_eq!(state.phase, PolicyPhase::DiagnosticObservationRequired);
        let diagnostic = evidence(&state, EvidenceVerdict::Inconclusive);
        state.record_evidence(&diagnostic).unwrap();
        assert_eq!(state.phase, PolicyPhase::AwaitingDeclaration);
    }

    #[test]
    fn completed_evidence_rows_are_insert_only() {
        let candidate = git_candidate();
        let state_dir = tempfile::tempdir().unwrap();
        let state = PolicyState::create(
            Uuid::new_v4(),
            candidate.path(),
            state_dir.path(),
            PolicyConfig::default(),
        )
        .unwrap();
        let store = EvidenceStore::open(&state.evidence_path()).unwrap();
        let record = evidence(&state, EvidenceVerdict::Supported);
        store.insert(&record).unwrap();
        assert_eq!(store.all(state.session_id).unwrap().len(), 1);
        drop(store);
        let connection = Connection::open(state.evidence_path()).unwrap();
        let update = connection.execute(
            "UPDATE evidence_records SET completed_at = 'changed' WHERE id = ?1",
            [record.id.to_string()],
        );
        assert!(update.is_err());
        let delete = connection.execute(
            "DELETE FROM evidence_records WHERE id = ?1",
            [record.id.to_string()],
        );
        assert!(delete.is_err());
    }

    #[tokio::test]
    async fn process_evidence_uses_exit_status_and_detects_repository_mutation() {
        let candidate = git_candidate();
        let state_dir = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(state_dir.path().join("artifacts")).unwrap();
        let mut state = PolicyState::create(
            Uuid::new_v4(),
            candidate.path(),
            state_dir.path(),
            PolicyConfig::default(),
        )
        .unwrap();
        state
            .declare(declaration(state.current_repository_fingerprint.clone()))
            .unwrap();
        let supported = acquire_command_evidence(
            &state,
            &artifacts,
            EvidenceCommandOptions {
                command: vec!["sh".into(), "-c".into(), "printf observed".into()],
                cwd: candidate.path().into(),
                expected_exit_code: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(supported.verdict, EvidenceVerdict::Supported);
        assert_eq!(supported.exit_code, Some(0));
        assert_eq!(supported.raw_artifacts.len(), 2);

        let contradicted = acquire_command_evidence(
            &state,
            &artifacts,
            EvidenceCommandOptions {
                command: vec!["sh".into(), "-c".into(), "exit 9".into()],
                cwd: candidate.path().into(),
                expected_exit_code: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(contradicted.verdict, EvidenceVerdict::Contradicted);
        assert_eq!(contradicted.exit_code, Some(9));

        let mutated = acquire_command_evidence(
            &state,
            &artifacts,
            EvidenceCommandOptions {
                command: vec!["sh".into(), "-c".into(), "printf changed > app.tsx".into()],
                cwd: candidate.path().into(),
                expected_exit_code: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(mutated.verdict, EvidenceVerdict::Inconclusive);
        assert!(mutated.observed_result.contains("repository changed"));
    }

    #[test]
    fn browser_evidence_requires_input_display_snapshots_and_correlation() {
        let candidate = git_candidate();
        let state_dir = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(state_dir.path().join("artifacts")).unwrap();
        let mut state = PolicyState::create(
            Uuid::new_v4(),
            candidate.path(),
            state_dir.path(),
            PolicyConfig::default(),
        )
        .unwrap();
        state
            .declare(declaration(state.current_repository_fingerprint.clone()))
            .unwrap();
        let session = Uuid::new_v4();
        let fingerprint = state.current_repository_fingerprint.clone();
        let make = |at: u64, source: &str, kind: &str, payload: serde_json::Value| {
            let mut event = RawEvent::observed_at(session, at, source, kind, payload);
            event.repository_fingerprint = Some(fingerprint.clone());
            event
        };
        let mut before = make(
            10,
            "browser",
            "browser.page.snapshot",
            serde_json::json!({"dom":{"strings":["Presses: 0"]}}),
        );
        before.artifact_refs.push(artifacts.put(b"before").unwrap());
        let down = make(20, "input", "pointer.down", serde_json::json!({}));
        let up = make(21, "input", "pointer.up", serde_json::json!({}));
        let mut display = make(22, "display", "display.update", serde_json::json!({}));
        display
            .artifact_refs
            .push(artifacts.put(b"display").unwrap());
        let mut after = make(
            30,
            "browser",
            "browser.page.snapshot",
            serde_json::json!({"dom":{"strings":["Presses: 1"]}}),
        );
        after.artifact_refs.push(artifacts.put(b"after").unwrap());
        let mut correlation = make(
            31,
            "browser",
            "browser.coordinate_correlation",
            serde_json::json!({"browserSnapshotEventId":after.id}),
        );
        correlation
            .artifact_refs
            .push(artifacts.put(b"correlation").unwrap());
        let events = vec![
            before.clone(),
            down,
            up,
            display,
            after.clone(),
            correlation,
        ];
        let options = BrowserEvidenceOptions {
            expected_before_text: "Presses: 0".into(),
            expected_after_text: "Presses: 1".into(),
        };
        let supported =
            acquire_browser_evidence(&state, &events, &artifacts, options.clone()).unwrap();
        assert_eq!(supported.verdict, EvidenceVerdict::Supported);
        assert_eq!(supported.supporting_event_ids.len(), 6);

        let wrong_after = make(
            30,
            "browser",
            "browser.page.snapshot",
            serde_json::json!({"dom":{"strings":["Count: 1"]}}),
        );
        let contradicted = acquire_browser_evidence(
            &state,
            &[before, events[1].clone(), events[2].clone(), wrong_after],
            &artifacts,
            options.clone(),
        )
        .unwrap();
        assert_eq!(contradicted.verdict, EvidenceVerdict::Contradicted);

        let inconclusive =
            acquire_browser_evidence(&state, &events[..5], &artifacts, options).unwrap();
        assert_eq!(inconclusive.verdict, EvidenceVerdict::Inconclusive);
    }
}
