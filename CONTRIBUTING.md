# Contributing to AVM

Thank you for helping improve AVM. The project welcomes focused bug fixes,
documentation improvements, and changes that preserve the supervisor-owned
trust boundary.

## Development setup

Install Rust 1.87 or newer, Node.js 22 or newer, GNU Make, and Git. A Linux host
with QEMU 6+, KVM, D-Bus, and virtiofsd is required only for VM integration
tests.

```sh
git clone https://github.com/artpar/avm.git
cd avm
make setup
make check
```

`make check` is the same quality gate used by pull requests. The full KVM path
is intentionally separate; see `vm/image/README.md` and run
`scripts/linux-smoke.sh RUN_CONFIG` on a suitable host.

## Changes and pull requests

1. Open an issue first for changes to persisted schemas, the trust boundary, or
   the release process.
2. Keep a pull request focused and include tests for changed behavior.
3. Update user-facing documentation when commands or configuration change.
4. Run `make check` and report any Linux/KVM checks that could not be run.
5. Do not commit credentials, VM disks, run state, or private experiment data.

Commits use [Conventional Commits](https://www.conventionalcommits.org/):

- `fix:` produces a patch release;
- `feat:` produces a minor release;
- `feat!:` or a `BREAKING CHANGE:` footer produces a major release;
- `docs:`, `test:`, `build:`, `ci:`, `refactor:`, and `chore:` do not release by
  themselves.

Release Please collects eligible commits into a version PR. Merging that PR
updates `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, creates the version tag and
GitHub Release, and attaches the verified Linux binary archive.

## Reporting security issues

Do not open a public issue for a vulnerability. Follow [SECURITY.md](SECURITY.md)
instead.
