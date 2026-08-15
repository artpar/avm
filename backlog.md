# AVM backlog

This is the canonical project work tracker. Status is one of `planned`,
`active`, `blocked`, or `done`; priority is `P0` through `P3`.

| ID | Priority | Status | Work | Acceptance |
| --- | --- | --- | --- | --- |
| UI-001 | P0 | done | Read-only semantic run inspector | `avm start` always returns a live loopback WebUI; synchronized trace and event perspectives, filters, selected-time evidence, and keyboard flow work; no mutation routes exist. |
| DEV-001 | P0 | done | AVM-on-AVM development loop | A dedicated development host and fresh guest validate the candidate without altering `cando`; captured evidence is reviewed. |
| DOC-001 | P0 | done | Product, interface, architecture, and dogfood documentation | README, contributing guide, wiki, `PRODUCT.md`, `DESIGN.md`, and focused docs agree. |
| UI-006 | P0 | active | Cross-source coverage and availability semantics | Every source exposes collected spans, gaps, absence, corruption, and truncation without treating missing evidence as zero; display absence renders as a black frame at the selected time, while recorded black output remains distinguishable through evidence metadata. |
| UI-007 | P0 | active | Inspector request diagnostics | Every failed request has a stable error code and request ID, the UI exposes copyable endpoint/status/detail without hiding the rest of the workspace, and structured server logs correlate the same ID; provenance-filter regressions are covered against empty, mixed, and large runs. |
| UI-008 | P0 | done | Consolidated whole-machine evidence workspace | Implement confirmed direction C: one shared-time trace with equal-status CPU, disk, network, logs, browser, input, display, audio, VM, and future collector lanes; keep logs, network, artifacts, and selected evidence visible together below without evidence tabs. |
| UI-009 | P0 | active | Multiscale synchronized time navigation | Replace collection density with a whole-run ruler, focus range, selection cursor, zoom/pan, source coverage, and scale-aware aggregation; trace lanes, lower evidence regions, event ledger, filters, and URL remain synchronized. |
| UI-010 | P1 | done | Inspector hierarchy and interaction cleanup | Remove the `Read-only` badge, mutually exclusive evidence tabs, inert panels, and screen-centric hierarchy; standardize lane controls, counts, filters, selection, maximization, keyboard navigation, and honest zero/absent states. |
| UI-011 | P1 | active | Real-run WebUI usability gate | Dogfood the inspector through AVM on representative application-development runs, including the audited 506-event run and a 20,000+ event run; verify one-glance evidence comprehension, frame gap states, query failures, keyboard flow, and 200% zoom. |
| UI-002 | P1 | planned | Large-run semantic query language | Typed fields, stable query grammar, pagination/virtualization, and explicit result completeness support runs beyond the initial 20,000-event response bound. |
| UI-003 | P1 | planned | Deeper relationship model | Persist schema-backed correlation edges with named basis and confidence; never infer causality from proximity alone. |
| UI-004 | P1 | planned | Accessibility and browser acceptance suite | Automated keyboard, 200% zoom, reduced-motion, semantic landmark, contrast, and guest-Chromium checks run in CI or the KVM gate. |
| UI-005 | P2 | planned | Archived-run inspector lifecycle | Define a supported, still-read-only workflow for inspecting relocated or stopped run archives without exposing a new control surface. |
| DEV-002 | P1 | planned | Configurable per-run host forwarding | Remove fixed SSH/CDP host-port collisions so multiple AVM runs can coexist safely on one supervisor host. |
| DEV-003 | P2 | planned | Durable remote publish manifest | Preserve the tracked/non-ignored publish manifest and explain inclusions/exclusions in evidence. |
| DEV-004 | P1 | planned | Guest-safe development build output | Define a supervisor-owned UID mapping or explicit build-output channel so guest builds never require weakening candidate mount permissions; until then use guest-local `CARGO_TARGET_DIR` and copy artifacts out explicitly. |
| OBS-001 | P1 | planned | Display stability under repeated Chromium scanouts | Reproduce the final dogfood timeout where identical full scanouts kept refreshing stability, determine whether the later QEMU exit is related, and make capture either converge or return a precise non-destructive diagnosis. |
| OBS-002 | P1 | planned | Chromium CDP reachability after guest restart | Ensure the guest browser listener and QEMU forwarding agree on a reachable address; verify `/json/version` from the supervisor after both first boot and restart. |
| DOC-002 | P2 | planned | Initialize the GitHub Wiki backing repository | Create the first Wiki page through an authenticated GitHub browser session so the existing source-controlled Wiki workflow can publish `wiki/*.md`. |

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
- 2026-08-15: the `v0.2.0` release gate exposed a Linux spawn/exec race in the
  process-identity regression test; production readiness already polls, and the
  test now waits for the tokenized command line before exercising teardown.
- 2026-08-15: audited the inspector against application-development run
  `6723825b-e9fa-447d-a8df-85cd69cfe642` (506 events). The selected transport
  event precedes the first display scanout by about 19m55s, so its frame failure
  is a capture-coverage gap that the UI currently misreports as a generic
  reconstruction failure. Provenance query failures were not reproducible in
  the same session; UI-007 preserves them as a P0 diagnostics/regression issue.
- 2026-08-15: record/replay UX research established persistent point-in-time
  evidence and an interactive temporal navigator as the redesign anchors. The
  implementation brief is `docs/webui-redesign-plan.md`.
- 2026-08-15: visual direction C was confirmed for the redesign. AVM is a
  modality-neutral whole-machine evidence inspector, not a screen-first replay
  tool. The selected probe is preserved at
  `docs/assets/webui-direction-c.png`; its topology, not generated detail, is the
  implementation reference.
- 2026-08-15: implemented direction C through dedicated AVM development run
  `d26596d8-3b5a-4a23-ac48-853ab51a1f36` without touching `cando`. The
  guest-built candidate rendered 575 records, five raw sources, 49 artifacts,
  and a 66m25s run as synchronized CPU, disk, network, logs, browser, input,
  display, audio, and VM lanes with persistent logs/network/artifacts/selection
  regions and an Events ledger. Dogfood verified zoom, whole-run navigation,
  source/provenance filters (including empty selection), text search, region
  maximize/restore, exact-event deep links, and request-error visibility.
- 2026-08-15: the same run begins at monotonic `714418542688` while display
  capture begins at `1382843783377`. Selecting the earlier time now produces a
  black tile labeled `before capture`; selecting a later time reconstructs the
  last recorded display state at the exact cursor without treating a nearby
  frame as a substitute. UI-006 remains active for equivalent gap/corruption
  depth across every non-display collector, and UI-011 remains active until the
  20,000+ record and 200% zoom gates pass.
- 2026-08-15: review hardening made whole-run cursor jumps relocate the current
  zoom window and fetch fresh synchronized evidence; browser collector identity
  now outranks URL keywords; display thumbnails use reconstructed PNGs while raw
  artifacts remain downloadable; and log/network rows select by pointer, Enter,
  or Space. Display availability and reconstruction now ignore unsupported or
  rejected records, begin only at an accepted scanout, and invalidate persisted
  state on disable or an update rejection that awaits a new full scanout. The
  fixes passed the full local gate and guest-side tests, then were verified in
  the dedicated AVM development run without touching `cando`.
