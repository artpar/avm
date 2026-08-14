use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    fingerprint::repository_fingerprint,
    policy::{ChangedFile, PolicyState},
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceGate {
    pub declaration_id: Uuid,
    pub staging_workspace: PathBuf,
    pub before_repository_fingerprint: String,
    #[serde(skip)]
    before_index_tree: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromotionResult {
    pub before_repository_fingerprint: String,
    pub after_repository_fingerprint: String,
    pub changed_files: Vec<ChangedFile>,
    pub staging_workspace: PathBuf,
}

impl WorkspaceGate {
    pub fn prepare(state: &PolicyState) -> Result<Self> {
        state.ensure_mutation_allowed()?;
        let declaration = state
            .active_declaration
            .as_ref()
            .context("mutation policy has no active declaration")?;
        let actual = repository_fingerprint(&state.candidate_workspace)?;
        ensure!(
            actual == state.current_repository_fingerprint,
            "candidate changed outside the supervisor before staging"
        );
        let git_dir = state.candidate_workspace.join(".git");
        ensure!(
            git_dir.is_dir(),
            "workspace gate currently requires a standard Git directory, not a linked worktree"
        );
        let staging_root = state.state_dir.join("staging");
        std::fs::create_dir_all(&staging_root)?;
        let staging_workspace = staging_root.join(declaration.id.to_string());
        ensure!(
            staging_workspace.parent() == Some(staging_root.as_path()),
            "staging workspace escaped its owned root"
        );
        if staging_workspace.exists() {
            std::fs::remove_dir_all(&staging_workspace)?;
        }
        std::fs::create_dir(&staging_workspace)?;
        rsync_tree(&state.candidate_workspace, &staging_workspace, false)?;
        let staged = repository_fingerprint(&staging_workspace)?;
        ensure!(
            staged == actual,
            "staging fingerprint {staged} differs from candidate {actual}"
        );
        Ok(Self {
            declaration_id: declaration.id,
            staging_workspace,
            before_repository_fingerprint: actual,
            before_index_tree: index_tree(&state.candidate_workspace)?,
        })
    }

    pub fn promote(self, state: &mut PolicyState) -> Result<PromotionResult> {
        state.ensure_mutation_allowed()?;
        ensure!(
            state.active_declaration.as_ref().map(|value| value.id) == Some(self.declaration_id),
            "active declaration changed after staging"
        );
        let actual_before = repository_fingerprint(&state.candidate_workspace)?;
        ensure!(
            actual_before == self.before_repository_fingerprint,
            "candidate changed outside the supervisor while Codex used staging"
        );
        let staging_index_tree = index_tree(&self.staging_workspace)?;
        ensure!(
            staging_index_tree == self.before_index_tree,
            "staging Git index tree changed; promotion refuses staged content mutations"
        );
        let changed_files = changed_files(&self.staging_workspace)?;
        ensure!(
            !changed_files.is_empty(),
            "staging workspace contains no mutation to promote"
        );
        let staged_fingerprint = repository_fingerprint(&self.staging_workspace)?;

        let backup_root = state.state_dir.join("promotion-backups");
        std::fs::create_dir_all(&backup_root)?;
        let backup = backup_root.join(self.declaration_id.to_string());
        ensure!(
            backup.parent() == Some(backup_root.as_path()),
            "promotion backup escaped its owned root"
        );
        if backup.exists() {
            std::fs::remove_dir_all(&backup)?;
        }
        std::fs::create_dir(&backup)?;
        rsync_tree(&state.candidate_workspace, &backup, true)?;

        if let Err(error) = rsync_tree(&self.staging_workspace, &state.candidate_workspace, true) {
            restore_backup(&backup, &state.candidate_workspace)?;
            return Err(error).context("promote staging worktree");
        }
        let promoted_fingerprint = repository_fingerprint(&state.candidate_workspace)?;
        if promoted_fingerprint != staged_fingerprint {
            restore_backup(&backup, &state.candidate_workspace)?;
            bail!(
                "promoted fingerprint {promoted_fingerprint} differs from staging {staged_fingerprint}; candidate was rolled back"
            );
        }

        let previous_state = state.clone();
        if let Err(error) = state.record_mutation(
            &self.before_repository_fingerprint,
            promoted_fingerprint.clone(),
            &changed_files,
        ) {
            restore_backup(&backup, &state.candidate_workspace)?;
            *state = previous_state;
            state.save()?;
            return Err(error).context("record promoted mutation in policy state");
        }
        Ok(PromotionResult {
            before_repository_fingerprint: self.before_repository_fingerprint,
            after_repository_fingerprint: promoted_fingerprint,
            changed_files,
            staging_workspace: self.staging_workspace,
        })
    }
}

fn restore_backup(backup: &Path, candidate: &Path) -> Result<()> {
    rsync_tree(backup, candidate, true).context("restore candidate after failed promotion")
}

fn index_tree(repository: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["write-tree"])
        .current_dir(repository)
        .output()?;
    ensure!(
        output.status.success(),
        "git write-tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn rsync_tree(source: &Path, destination: &Path, exclude_git: bool) -> Result<()> {
    let mut command = Command::new("rsync");
    command.args(["--archive", "--delete"]);
    if exclude_git {
        command.arg("--exclude=.git");
    }
    command
        .arg(format!("{}/", source.display()))
        .arg(format!("{}/", destination.display()));
    let output = command.output().context("launch rsync workspace gate")?;
    ensure!(
        output.status.success(),
        "rsync workspace gate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

pub fn changed_files(repository: &Path) -> Result<Vec<ChangedFile>> {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .current_dir(repository)
        .output()?;
    ensure!(
        output.status.success(),
        "git status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fields = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let field = fields[index];
        ensure!(
            field.len() >= 4 && field[2] == b' ',
            "invalid Git status record"
        );
        let status_bytes = &field[..2];
        let path = std::str::from_utf8(&field[3..])?.to_owned();
        let status = if status_bytes.contains(&b'D') {
            "deleted"
        } else if status_bytes.contains(&b'A') || status_bytes == b"??" {
            "added"
        } else if status_bytes.contains(&b'R') {
            "renamed"
        } else {
            "modified"
        };
        files.push(ChangedFile {
            path,
            status: status.into(),
        });
        index += 1;
        if status_bytes.contains(&b'R') || status_bytes.contains(&b'C') {
            ensure!(index < fields.len(), "Git rename status has no source path");
            index += 1;
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{DevelopmentDeclaration, PolicyConfig};

    fn candidate_and_state() -> (tempfile::TempDir, tempfile::TempDir, PolicyState) {
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
                Command::new("git")
                    .args(args)
                    .current_dir(candidate.path())
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let state_dir = tempfile::tempdir().unwrap();
        let state = PolicyState::create(
            Uuid::new_v4(),
            candidate.path(),
            state_dir.path(),
            PolicyConfig::default(),
        )
        .unwrap();
        (candidate, state_dir, state)
    }

    fn declare(state: &mut PolicyState) {
        state
            .declare(
                DevelopmentDeclaration::new(
                    "change the visible counter label".into(),
                    "the existing label is rendered from static HTML".into(),
                    "browser_interaction".into(),
                    "real browser and framebuffer".into(),
                    "open the application and inspect the counter label".into(),
                    "the new label appears in the running product".into(),
                    "the old label means the rendered source was not changed".into(),
                    state.current_repository_fingerprint.clone(),
                )
                .unwrap(),
            )
            .unwrap();
    }

    #[test]
    fn staging_isolated_until_verified_promotion() {
        let (candidate, _state_dir, mut state) = candidate_and_state();
        assert!(WorkspaceGate::prepare(&state).is_err());
        declare(&mut state);
        let gate = WorkspaceGate::prepare(&state).unwrap();
        std::fs::write(gate.staging_workspace.join("index.html"), "after\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(candidate.path().join("index.html")).unwrap(),
            "before\n"
        );
        let staged_fingerprint = repository_fingerprint(&gate.staging_workspace).unwrap();
        let result = gate.promote(&mut state).unwrap();
        assert_eq!(result.after_repository_fingerprint, staged_fingerprint);
        assert_eq!(
            std::fs::read_to_string(candidate.path().join("index.html")).unwrap(),
            "after\n"
        );
        assert_eq!(result.changed_files[0].path, "index.html");
        assert_eq!(state.current_repository_fingerprint, staged_fingerprint);
    }

    #[test]
    fn staging_index_tampering_is_not_promoted() {
        let (candidate, _state_dir, mut state) = candidate_and_state();
        declare(&mut state);
        let gate = WorkspaceGate::prepare(&state).unwrap();
        std::fs::write(gate.staging_workspace.join("index.html"), "after\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "index.html"])
                .current_dir(&gate.staging_workspace)
                .status()
                .unwrap()
                .success()
        );
        let error = gate.promote(&mut state).unwrap_err();
        assert!(error.to_string().contains("index tree changed"));
        assert_eq!(
            std::fs::read_to_string(candidate.path().join("index.html")).unwrap(),
            "before\n"
        );
    }

    #[test]
    fn harmless_index_stat_refresh_does_not_block_promotion() {
        let (candidate, _state_dir, mut state) = candidate_and_state();
        declare(&mut state);
        let gate = WorkspaceGate::prepare(&state).unwrap();
        std::fs::write(gate.staging_workspace.join("index.html"), "after\n").unwrap();
        assert!(
            Command::new("git")
                .args(["status", "--short"])
                .current_dir(&gate.staging_workspace)
                .status()
                .unwrap()
                .success()
        );
        gate.promote(&mut state).unwrap();
        assert_eq!(
            std::fs::read_to_string(candidate.path().join("index.html")).unwrap(),
            "after\n"
        );
    }
}
