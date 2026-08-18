use std::{fs::File, os::fd::AsRawFd, path::Path};

use anyhow::{Context, Result, ensure};

/// An advisory, process-wide coordination lock for operations that change a run.
/// Every AVM lifecycle, publication, promotion, and guest-command mutation must
/// take this lock before inspecting mutable run state.
pub struct RunLock {
    file: File,
}

impl RunLock {
    pub fn exclusive(state_dir: &Path, operation: &str) -> Result<Self> {
        std::fs::create_dir_all(state_dir)?;
        let path = state_dir.join("run.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open run coordination lock {}", path.display()))?;
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        ensure!(
            result == 0,
            "run is busy with another coordinated operation; retry {operation} after it completes"
        );
        Ok(Self { file })
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_second_mutator() {
        let temp = tempfile::tempdir().unwrap();
        let first = RunLock::exclusive(temp.path(), "first").unwrap();
        assert!(RunLock::exclusive(temp.path(), "second").is_err());
        drop(first);
        RunLock::exclusive(temp.path(), "third").unwrap();
    }
}
