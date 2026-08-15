# Releasing

AVM uses Release Please, Conventional Commits, and GitHub Actions. Maintainers do
not manually edit versions for routine releases.

## Version selection

- `fix:` increments the patch version.
- `feat:` increments the minor version.
- `feat!:` or a `BREAKING CHANGE:` footer increments the major version.
- Documentation, tests, CI, build, refactor, and chore commits do not cause a
  release by themselves.

Because AVM is pre-1.0, a breaking change increments the minor version. This is
configured explicitly in `release-please-config.json`.

## Release flow

1. Eligible commits land on `main` through reviewed pull requests.
2. Release Please opens or updates a release PR with `Cargo.toml`, `Cargo.lock`,
   `.release-please-manifest.json`, and `CHANGELOG.md` changes.
3. CI must pass on the release PR.
4. A maintainer reviews and merges the release PR.
5. Release Please creates the version tag and a draft GitHub Release.
6. The release job checks out the exact release commit and runs `make ci`.
7. It builds `avm-vVERSION-x86_64-unknown-linux-gnu.tar.gz`, generates a
   `.sha256` file, and emits GitHub build provenance.
8. The artifacts are attached and only then is the draft published.

If build or tests fail, the release remains a draft rather than presenting an
unverified artifact as complete.

## Verify a release

```sh
gh release download vVERSION --repo artpar/avm
shasum -a 256 -c avm-vVERSION-x86_64-unknown-linux-gnu.tar.gz.sha256
gh attestation verify avm-vVERSION-x86_64-unknown-linux-gnu.tar.gz \
  --repo artpar/avm
```

Extract the archive and run `avm --version`. It must match the release tag.

## Emergency correction

Do not move an existing version tag or silently replace an artifact. Fix the
problem on `main` with an appropriate Conventional Commit and publish a new
patch release. A compromised release should be called out in the release notes
and security advisory as appropriate.
