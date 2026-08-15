# AVM backlog

This is the canonical project work tracker. Status is one of `planned`,
`active`, `blocked`, or `done`; priority is `P0` through `P3`.

| ID | Priority | Status | Work | Acceptance |
| --- | --- | --- | --- | --- |
| UI-001 | P0 | done | Read-only semantic run inspector | `avm start` always returns a live loopback WebUI; trace, event table, evidence modes, filters, density, gaps, and keyboard flow work; no mutation routes exist. |
| DEV-001 | P0 | done | AVM-on-AVM development loop | A dedicated development host and fresh guest validate the candidate without altering `cando`; captured evidence is reviewed. |
| DOC-001 | P0 | done | Product, interface, architecture, and dogfood documentation | README, contributing guide, wiki, `PRODUCT.md`, `DESIGN.md`, and focused docs agree. |
| UI-002 | P1 | planned | Large-run semantic query language | Typed fields, stable query grammar, pagination/virtualization, and explicit result completeness support runs beyond the initial 20,000-event response bound. |
| UI-003 | P1 | planned | Deeper relationship model | Persist schema-backed correlation edges with named basis and confidence; never infer causality from proximity alone. |
| UI-004 | P1 | planned | Accessibility and browser acceptance suite | Automated keyboard, 200% zoom, reduced-motion, semantic landmark, contrast, and guest-Chromium checks run in CI or the KVM gate. |
| UI-005 | P2 | planned | Archived-run inspector lifecycle | Define a supported, still-read-only workflow for inspecting relocated or stopped run archives without exposing a new control surface. |
| DEV-002 | P1 | planned | Configurable per-run host forwarding | Remove fixed SSH/CDP host-port collisions so multiple AVM runs can coexist safely on one supervisor host. |
| DEV-003 | P2 | planned | Durable remote publish manifest | Preserve the tracked/non-ignored publish manifest and explain inclusions/exclusions in evidence. |
| DEV-004 | P1 | planned | Guest-safe development build output | Define a supervisor-owned UID mapping or explicit build-output channel so guest builds never require weakening candidate mount permissions; until then use guest-local `CARGO_TARGET_DIR` and copy artifacts out explicitly. |
| OBS-001 | P1 | planned | Display stability under repeated Chromium scanouts | Reproduce the final dogfood timeout where identical full scanouts kept refreshing stability, determine whether the later QEMU exit is related, and make capture either converge or return a precise non-destructive diagnosis. |
| OBS-002 | P1 | planned | Chromium CDP reachability after guest restart | Ensure the guest browser listener and QEMU forwarding agree on a reachable address; verify `/json/version` from the supervisor after both first boot and restart. |

## Session notes

- 2026-08-15: dedicated `avm-dev-20260815` GCE host created for AVM development;
  active `cando` host/run deliberately left untouched.
- 2026-08-15: remote publishing was changed to exclude ignored build output after
  dogfooding exposed an attempted multi-gigabyte `target/` transfer.
- 2026-08-15: guest Linux testing exposed and fixed GNU/BSD tar option-order
  divergence in remote publishing and the valid empty-artifact-run WebUI case.
- 2026-08-15: inspector review hardening added token-validated process teardown,
  failed-start child reaping, empty-source semantics, replacement-only relation
  rendering, and active-frame reload regression coverage.
