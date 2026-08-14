use std::{
    fs::{OpenOptions, Permissions},
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const PREFIX: &str = "sha256:";

#[derive(Clone, Debug)]
pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_owned();
        std::fs::create_dir_all(root.join("sha256"))
            .with_context(|| format!("create artifact store {}", root.display()))?;
        Ok(Self { root })
    }

    pub fn put(&self, bytes: &[u8]) -> Result<String> {
        let digest = hex::encode(Sha256::digest(bytes));
        let artifact_ref = format!("{PREFIX}{digest}");
        let path = self.path_for_digest(&digest)?;
        if path.exists() {
            self.verify_existing(&path, &digest)?;
            return Ok(artifact_ref);
        }

        let parent = path.parent().context("artifact path has no parent")?;
        std::fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(".tmp-{}", Uuid::new_v4()));
        let write_result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            make_read_only(&temporary)?;
            match std::fs::hard_link(&temporary, &path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    self.verify_existing(&path, &digest)
                }
                Err(error) => Err(error.into()),
            }
        })();
        let _ = std::fs::remove_file(&temporary);
        write_result?;
        Ok(artifact_ref)
    }

    pub fn read(&self, artifact_ref: &str) -> Result<Vec<u8>> {
        let digest = parse_ref(artifact_ref)?;
        let path = self.path_for_digest(digest)?;
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read artifact {artifact_ref} at {}", path.display()))?;
        ensure!(
            hex::encode(Sha256::digest(&bytes)) == digest,
            "artifact {artifact_ref} failed content-hash verification"
        );
        Ok(bytes)
    }

    pub fn path(&self, artifact_ref: &str) -> Result<PathBuf> {
        self.path_for_digest(parse_ref(artifact_ref)?)
    }

    fn path_for_digest(&self, digest: &str) -> Result<PathBuf> {
        validate_digest(digest)?;
        Ok(self.root.join("sha256").join(&digest[..2]).join(digest))
    }

    fn verify_existing(&self, path: &Path, digest: &str) -> Result<()> {
        let bytes = std::fs::read(path)?;
        ensure!(
            hex::encode(Sha256::digest(bytes)) == digest,
            "existing artifact {} does not match its content address",
            path.display()
        );
        Ok(())
    }
}

fn parse_ref(artifact_ref: &str) -> Result<&str> {
    artifact_ref
        .strip_prefix(PREFIX)
        .context("artifact reference must start with sha256:")
}

fn validate_digest(digest: &str) -> Result<()> {
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid SHA-256 artifact digest");
    }
    Ok(())
}

fn make_read_only(path: &Path) -> Result<()> {
    #[cfg(unix)]
    let permissions = Permissions::from_mode(0o444);
    #[cfg(not(unix))]
    let permissions = {
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_readonly(true);
        permissions
    };
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_addressed_writes_deduplicate_and_verify() {
        let temp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(temp.path()).unwrap();
        let first = store.put(b"raw frame bytes").unwrap();
        let second = store.put(b"raw frame bytes").unwrap();
        assert_eq!(first, second);
        assert_eq!(store.read(&first).unwrap(), b"raw frame bytes");
        assert_eq!(
            std::fs::read_dir(store.path(&first).unwrap().parent().unwrap())
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn tampered_artifact_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(temp.path()).unwrap();
        let reference = store.put(b"trusted").unwrap();
        let path = store.path(&reference).unwrap();
        make_writable(&path);
        std::fs::write(path, b"tampered").unwrap();
        assert!(
            store
                .read(&reference)
                .unwrap_err()
                .to_string()
                .contains("verification")
        );
    }

    fn make_writable(path: &Path) {
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o200);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        std::fs::set_permissions(path, permissions).unwrap();
    }
}
