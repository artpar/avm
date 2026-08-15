# Changelog

All notable changes to AVM are documented here. The project follows
[Semantic Versioning](https://semver.org/) and uses
[Conventional Commits](https://www.conventionalcommits.org/) to automate version
selection and release notes.

## [0.3.1](https://github.com/artpar/avm/compare/v0.3.0...v0.3.1) (2026-08-15)


### Bug Fixes

* tolerate healthy guest image provisioning ([#25](https://github.com/artpar/avm/issues/25)) ([b6b0a7f](https://github.com/artpar/avm/commit/b6b0a7f1aae55a40433a3a0a6b8f1625ef4f97fa))

## [0.3.0](https://github.com/artpar/avm/compare/v0.2.2...v0.3.0) (2026-08-15)


### Features

* redesign inspector as whole-machine evidence workspace ([f52e739](https://github.com/artpar/avm/commit/f52e7395c58292f4f93898bc978e3a65cb56788c))

## [0.2.2](https://github.com/artpar/avm/compare/v0.2.1...v0.2.2) (2026-08-15)


### Bug Fixes

* stabilize guest sensors and enable WebGL2 ([#21](https://github.com/artpar/avm/issues/21)) ([3fe6388](https://github.com/artpar/avm/commit/3fe638896edbdfd08d3a39a2d43b2615befde37b))

## [0.2.1](https://github.com/artpar/avm/compare/v0.2.0...v0.2.1) (2026-08-15)


### Bug Fixes

* wait for inspector process identity in lifecycle test ([d0fb4ce](https://github.com/artpar/avm/commit/d0fb4ce7536e0e808487eeb47779b877fc2e8ddd))

## [0.2.0](https://github.com/artpar/avm/compare/v0.1.3...v0.2.0) (2026-08-15)


### Features

* add read-only semantic run inspector ([a6cdd7d](https://github.com/artpar/avm/commit/a6cdd7de910eeaac1c8b87bffabd86e51831b365))

## [0.1.3](https://github.com/artpar/avm/compare/v0.1.2...v0.1.3) (2026-08-15)


### Bug Fixes

* bound remote publishes to Git-visible files ([a7dd42b](https://github.com/artpar/avm/commit/a7dd42b9625f23b53341506c007da995c1c22fa6))
* keep tar options portable across release hosts ([5dee357](https://github.com/artpar/avm/commit/5dee3572fb7331b0e243c91aeabff9d8e8a08647))
* map guest workspace ownership through virtiofs ([69eeefc](https://github.com/artpar/avm/commit/69eeefc23422a4a343236d5c6a0940c28dc79981))
* ship release-native Linux smoke tooling ([5d239e5](https://github.com/artpar/avm/commit/5d239e5fc6e24bbdd3a21f048419f4a5b65f739d))

## [0.1.2](https://github.com/artpar/avm/compare/v0.1.1...v0.1.2) (2026-08-15)


### Bug Fixes

* keep draft releases discoverable ([525b5c9](https://github.com/artpar/avm/commit/525b5c9f14d96b8d2f34229f538ca0b94a825b74))

## [0.1.1](https://github.com/artpar/avm/compare/v0.1.0...v0.1.1) (2026-08-15)


### Bug Fixes

* resolve recorded pointer coordinates in queries ([cd5ae42](https://github.com/artpar/avm/commit/cd5ae42b73a4cf93dc2087470b78950b497aedda))
* target bootstrap releases without moving tags ([b8cb108](https://github.com/artpar/avm/commit/b8cb10854523407c6350274269cc824165d82bf8))
* type shifted US keyboard symbols ([e48c39a](https://github.com/artpar/avm/commit/e48c39a6586540dd0a302f8457403fb31fd9c198))

## 0.1.0 (2026-08-15)

Initial public release of the host-owned instrumented virtual computer,
including QEMU/KVM lifecycle control, recorded guest input, persistent
experience storage and replay, browser and accessibility observation, local
Codex supervision over a remote channel, evidence policy, and the controlled
experiment harness.
