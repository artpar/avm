use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentSession {
    pub id: Uuid,
    pub candidate_workspace: PathBuf,
    pub state_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ExperimentPaths {
    pub config: PathBuf,
    pub events: PathBuf,
    pub timeline: PathBuf,
    pub artifacts: PathBuf,
}

impl ExperimentSession {
    pub fn create(candidate_workspace: &Path, state_root: &Path) -> Result<Self> {
        let candidate_workspace = candidate_workspace
            .canonicalize()
            .context("canonicalize candidate workspace")?;
        ensure!(
            candidate_workspace.is_dir(),
            "candidate must be a directory"
        );
        std::fs::create_dir_all(state_root)?;
        let state_root = state_root
            .canonicalize()
            .context("canonicalize state root")?;
        ensure!(
            !state_root.starts_with(&candidate_workspace),
            "experiment state must be outside the candidate workspace"
        );
        let id = Uuid::new_v4();
        let session = Self {
            id,
            candidate_workspace,
            state_dir: state_root.join(id.to_string()),
        };
        session.save()?;
        Ok(session)
    }

    pub fn paths(&self) -> ExperimentPaths {
        ExperimentPaths {
            config: self.state_dir.join("session.json"),
            events: self.state_dir.join("events.jsonl"),
            timeline: self.state_dir.join("timeline.sqlite3"),
            artifacts: self.state_dir.join("artifacts"),
        }
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(&self.state_dir)?;
        std::fs::write(self.paths().config, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = std::fs::read(path.as_ref())?;
        serde_json::from_slice(&bytes).context("decode experiment session")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_state_outside_candidate_and_rejects_inside_state() {
        let candidate = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let session = ExperimentSession::create(candidate.path(), external.path()).unwrap();
        assert!(session.paths().config.is_file());
        assert!(!session.state_dir.starts_with(candidate.path()));

        let error = ExperimentSession::create(candidate.path(), &candidate.path().join("state"))
            .unwrap_err();
        assert!(error.to_string().contains("outside"));
    }
}
