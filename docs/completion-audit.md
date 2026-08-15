# Completion audit

This document maps the project specification to executable implementation and external evidence. Both final completion gates now have accepted, immutable evidence.

Status meanings:

- **Verified**: implementation plus a focused test or preserved real-system artifact exists.
- **Implemented**: code and automated tests exist; the capability participates in a broader real-system gate.
- **Open**: completion evidence is not yet valid or complete.

## Architecture and sensory truth

| Requirement | Status | Evidence |
| --- | --- | --- |
| Codex remains on the local host; candidate runs in a QEMU/KVM guest reached through a reliable channel | Verified | `README.md`; `src/remote.rs`; `/Users/artpar/workspace/code/avm-evidence/experiments/rich-demo-1`; GCE/KVM run evidence below |
| qcow2 base/overlay, QMP lifecycle and snapshot reset, virtiofs candidate mount | Verified | `src/vm.rs`, `src/qmp.rs`; VM contract and QMP tests; `/Users/artpar/workspace/code/avm-evidence/gcloud-full-run/passed` |
| Host-owned QEMU D-Bus display, framebuffer reconstruction, cursor and input timestamps before send | Verified | `src/display.rs`, `src/framebuffer.rs`; display/framebuffer tests; full-run event ledger and screenshots |
| Candidate cannot own or modify supervisor truth | Verified | state-path rejection tests, workspace-gate tests, external run directories, evaluator deployment boundary |
| Deterministic useful guest without a custom OS | Verified | `vm/image`; `vm/image/README.md`; `noble-v7` real acceptance runs |

## Experience API and preservation

| Requirement | Status | Evidence |
| --- | --- | --- |
| Canonical ordered timeline with provenance | Verified | `src/event.rs`, `src/timeline.rs`; monotonic/round-trip/independent-sink tests |
| Immutable content-addressed raw artifacts | Verified | `src/storage.rs`; tamper/deduplication tests; SHA-256 artifact trees in milestone evidence |
| Complete repository fingerprint including staged, unstaged, untracked, deletion, symlink and mode state | Verified | `src/fingerprint.rs`; comprehensive fingerprint mutation test |
| `observe`, bounded meaningful history, frame reconstruction and compact replay after execution | Verified | `src/experience.rs`, CLI commands, MCP `avm_experience`; offline reconstruction/replay tests; `/Users/artpar/workspace/code/avm-evidence/gcloud-milestone-2/caf19906-908a-479c-9f84-8cb2df873651` |
| Full action vocabulary through the real VM input path, with one action ID per request | Verified | `src/display.rs`, CLI and MCP `avm_act`; `/Users/artpar/workspace/code/avm-evidence/action-api-fbd4901`; `docs/action-api-acceptance.md` |
| Structural inspection without invented semantics | Verified | nine typed cross-source MCP query variants, `avm_experience inspect`, browser hit-testing with explicit correlation distance, fresh AT-SPI observation; accepted final loop used a canonical typed `aroundEvent` query |
| Query examples from the specification | Verified | `src/query.rs` focused tests; `/Users/artpar/workspace/code/avm-evidence/gcloud-milestone-7/cde8c1a8-a54c-4f56-94d1-3bc4aac197c1` |

## Milestone demonstrations

| Requirement | Status | Evidence |
| --- | --- | --- |
| Start, browse, inject input, capture, drag with display updates while held, reset | Verified | `scripts/linux-smoke.sh`; `/Users/artpar/workspace/code/avm-evidence/gcloud-full-run/passed` |
| Multi-second interaction reconstructed offline without repeating actions | Verified | experience replay test and milestone-2 timeline/artifacts |
| Codex App Server plus Codex Exec JSONL under an external supervisor | Verified | `src/codex.rs`; App Server/Exec tests; `/Users/artpar/workspace/code/avm-evidence/gcloud-milestone-3/run` |
| Agent/tool/repository events associated with fingerprints outside candidate | Verified | canonical Codex tests and milestone-3 external ledger |
| CDP navigation/DOM/focus/geometry/accessibility/console/errors/network/WebSocket/performance/trace | Verified | `supervisor/browser/observer.mjs`; observer tests; milestone-5 browser ledger; repaired transport acceptance in `docs/browser-transport-acceptance.md` |
| Pixel-verified display/browser correlation and combined failure diagnosis | Verified | `src/browser.rs` tests; `/Users/artpar/workspace/code/avm-evidence/gcloud-milestone-5/055846c2-16a3-4e37-9bc5-b4c61d392c3e` |
| Deterministic temporal perception for delay, no response, repetition, flicker/reversion and motion | Verified | `src/temporal.rs` tests; `/Users/artpar/workspace/code/avm-evidence/gcloud-milestone-6/cde8c1a8-a54c-4f56-94d1-3bc4aac197c1` |
| Event-triggered VLM with raw-frame/model/prompt/version provenance | Verified | `src/vlm.rs`; trigger, adapter and provenance tests; provider-neutral adapter fixture evidence |
| AT-SPI sensor for Chromium, terminal and native GUI app over isolated guest-host transport | Verified | `src/accessibility.rs`, `vm/sensor`; bounded/reconnection tests; `/Users/artpar/workspace/code/avm-evidence/gcloud-milestone-8/e234f308-ead4-4096-b73a-008094a6f00d` |
| Selective runtime logs/spans/process/profiling and trace-context queries | Verified | `src/runtime.rs`; validation/causal-basis tests; runtime query test and milestone-8 ledger |
| Host-timestamped bounded audio plus separate optional interpretation | Verified | `src/audio.rs`; raw interval/waveform/provenance tests; real 344,064-byte PCM interval recorded in milestone-8 evidence |
| Compositor integration evaluated only after higher-level mechanisms | Verified | `docs/compositor-decision.md` records the decision not to add a custom compositor |

## Evidence policy and trust boundary

| Requirement | Status | Evidence |
| --- | --- | --- |
| Structured pre-edit hypothesis, discriminating observation and prediction | Verified | `src/policy.rs`; declaration validation tests |
| Independent process/browser evidence with raw artifacts and fingerprints | Verified | command/browser evidence implementations and tests; milestone-5 external policy ledger |
| Completed evidence is insert-only | Verified | SQLite evidence store and insert-only test |
| OS-level staging gate outside candidate control | Verified | `src/workspace_gate.rs`; isolation, promotion and index-tamper tests |
| Explicit `EVIDENCE_FAILED` state blocks edits until diagnosis plus new observation | Verified | policy transition test covering failure, blocked mutation, diagnosis and recovery |
| Explainable fingerprint-bound evidence debt | Verified | policy debt test and status reasons |
| Evaluator-owned subsystem rules | Verified | configurable path/subsystem rules and policy tests; actual experiment config remains outside candidate |
| HTTP and benchmark observations retain required raw detail | Verified | evaluator-owned direct commands record command, environment, timing, stdout/stderr artifacts, exit status and fingerprint; accepted experiment and final hidden scorer preserve raw results outside the candidate |
| Host state, evidence, artifacts, secrets, base image, policy and hidden tests are unavailable to candidate | Verified | path checks, staging architecture, guest-only `/workspace`, host-private evaluator proxy, external hidden scorer; no Codex authentication on GCE/guest |
| Hidden final checks use evaluator-owned fresh state | Verified | `benchmarks/evaluator`, `experiments/runner.mjs`, blind labels and fresh workspace/overlay design |

## Evaluator and measurement

| Requirement | Status | Evidence |
| --- | --- | --- |
| Medium project board rather than trivial Todo app | Verified | `benchmarks/target-app`; target check |
| Eight fault classes: temporal, double action, focus, visual, state order, flicker, runtime-only, hidden behavior | Verified | `benchmarks/evaluator/profiles`; evaluator check |
| Host/guest CPU, memory, write bandwidth, processing/input latency, event/storage/model volume | Verified | `src/performance.rs`; performance tests; real paired GCE measurement summarized in `README.md` |
| Balanced repeated A/B/C/D runner with fixed model/task/repository/snapshot/dependencies/resources and blind evaluation | Verified | `experiments/runner.mjs`, fixture eight-trial check, real-agent capability-plan check |
| Complete time/tool/model/interaction/rework/failed-attempt measurement | Verified | `experiments/agent-metrics.mjs`; Exec/App Server parser check; ordinary JSONL persistence and evaluator merge tests; accepted run 5 and final loop contain numeric measurements |
| Valid controlled outcome experiment using the complete repaired rich stack | Verified | Accepted balanced run `/Users/artpar/workspace/code/avm-evidence/experiments/controlled-v1/run-5`; all seven rich browser calls succeeded; complete metrics and negative result reported in `docs/experiment-results.md` |

## Completion gates

| Requirement | Status | Required proof |
| --- | --- | --- |
| One coding agent completes the entire task→edit→guest run→GUI action→temporal revisit→richer query→browser/runtime correlation→diagnosis→fix→independent evidence→repeat→corrected observation loop | **Verified** | Accepted run `/Users/artpar/workspace/code/avm-evidence/final-experience-loop/run-3`; 1,696-event canonical timeline, pre/post POST/200 pairs, published fingerprint, zero-defect independent score, before/after framebuffer artifacts and passing machine audit; see `docs/final-experience-loop.md` |
| Experiment shows whether the environment improves outcomes versus the same model without it | Verified | Run 5 found zero rich-perception functional-defect main effect with substantially higher time/tool/token cost; see `docs/experiment-results.md` |

The scoped research system satisfies the specification's completion criteria. The experiment's negative result and limitations remain part of the conclusion rather than being reinterpreted as evidence of benefit.
