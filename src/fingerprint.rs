use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(unix)]
use std::os::unix::{ffi::OsStringExt, fs::PermissionsExt};

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};

pub fn repository_fingerprint(path: impl AsRef<Path>) -> Result<String> {
    let requested = path
        .as_ref()
        .canonicalize()
        .context("canonicalize repository")?;
    ensure!(requested.is_dir(), "repository path must be a directory");
    match git_output(&requested, &["rev-parse", "--show-toplevel"]) {
        Ok(output) => {
            let root = PathBuf::from(String::from_utf8(output)?.trim()).canonicalize()?;
            fingerprint_git(&root)
        }
        Err(_) => fingerprint_directory(&requested),
    }
}

fn fingerprint_git(root: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    tagged(&mut digest, b"format", b"avm-working-tree-v1");
    let head = git_output(root, &["rev-parse", "--verify", "HEAD"])
        .unwrap_or_else(|_| b"UNBORN\n".to_vec());
    tagged(&mut digest, b"head", &head);
    tagged(
        &mut digest,
        b"index",
        &git_output(root, &["ls-files", "--stage", "-z"])?,
    );

    let listed = git_output(
        root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?;
    let mut paths = BTreeSet::new();
    for bytes in listed
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        paths.insert(path_bytes(bytes));
    }
    for relative in paths {
        fingerprint_entry(&mut digest, root, &relative)?;
    }
    Ok(format!("sha256:{}", hex::encode(digest.finalize())))
}

fn fingerprint_directory(root: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    tagged(&mut digest, b"format", b"avm-directory-v1");
    let mut paths = Vec::new();
    collect_paths(root, root, &mut paths)?;
    paths.sort();
    for relative in paths {
        fingerprint_entry(&mut digest, root, &relative)?;
    }
    Ok(format!("sha256:{}", hex::encode(digest.finalize())))
}

fn collect_paths(root: &Path, directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root)?.to_owned();
        if relative == Path::new(".git") {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            collect_paths(root, &path, output)?;
        } else {
            output.push(relative);
        }
    }
    Ok(())
}

fn fingerprint_entry(digest: &mut Sha256, root: &Path, relative: &Path) -> Result<()> {
    tagged(digest, b"path", &path_raw_bytes(relative));
    let path = root.join(relative);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tagged(digest, b"type", b"deleted");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        tagged(digest, b"type", b"symlink");
        tagged(
            digest,
            b"target",
            &path_raw_bytes(&std::fs::read_link(path)?),
        );
    } else if metadata.is_file() {
        tagged(digest, b"type", b"file");
        #[cfg(unix)]
        tagged(
            digest,
            b"executable",
            if metadata.permissions().mode() & 0o111 == 0 {
                b"0"
            } else {
                b"1"
            },
        );
        #[cfg(not(unix))]
        tagged(digest, b"executable", b"unknown");
        let mut file = File::open(path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        tagged(digest, b"content", &bytes);
    } else {
        tagged(digest, b"type", b"other");
    }
    Ok(())
}

fn tagged(digest: &mut Sha256, name: &[u8], value: &[u8]) {
    digest.update((name.len() as u64).to_be_bytes());
    digest.update(name);
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn git_output(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    ensure!(output.status.success(), "git {} failed", args.join(" "));
    Ok(output.stdout)
}

#[cfg(unix)]
fn path_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes.to_vec()))
}

#[cfg(not(unix))]
fn path_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(unix)]
fn path_raw_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_raw_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_changes_for_staged_unstaged_untracked_deleted_and_mode_state() {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-q"]);
        git(
            temp.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(temp.path(), &["config", "user.name", "AVM Test"]);
        std::fs::write(temp.path().join("tracked.txt"), "one\n").unwrap();
        std::fs::write(temp.path().join("mode.sh"), "#!/bin/sh\n").unwrap();
        git(temp.path(), &["add", "."]);
        git(temp.path(), &["commit", "-qm", "base"]);

        let base = repository_fingerprint(temp.path()).unwrap();
        assert_eq!(base, repository_fingerprint(temp.path()).unwrap());

        std::fs::write(temp.path().join("tracked.txt"), "two\n").unwrap();
        let unstaged = repository_fingerprint(temp.path()).unwrap();
        assert_ne!(base, unstaged);
        git(temp.path(), &["add", "tracked.txt"]);
        let staged = repository_fingerprint(temp.path()).unwrap();
        assert_ne!(unstaged, staged);

        std::fs::write(temp.path().join("new.txt"), "new\n").unwrap();
        let untracked = repository_fingerprint(temp.path()).unwrap();
        assert_ne!(staged, untracked);
        std::fs::remove_file(temp.path().join("new.txt")).unwrap();
        std::fs::remove_file(temp.path().join("mode.sh")).unwrap();
        let deleted = repository_fingerprint(temp.path()).unwrap();
        assert_ne!(staged, deleted);

        git(temp.path(), &["checkout", "--", "mode.sh"]);
        #[cfg(unix)]
        {
            let path = temp.path().join("mode.sh");
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(path, permissions).unwrap();
            assert_ne!(staged, repository_fingerprint(temp.path()).unwrap());
        }
    }

    fn git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {}", args.join(" "));
    }
}
