# AVM

AVM is a host-owned, instrumented virtual computer for software-development agents. Codex runs and authenticates on the local supervisor machine. A durable, explicit remote channel connects that supervisor to a GCE Linux/KVM host and its nested guest; Codex does not need to run or log in on the VM.

Implemented now:

- qcow2 base plus per-run overlays;
- QMP lifecycle (`create-run`, `start`, `status`, clean snapshot-backed `reset`, `stop`, `destroy-run`);
- candidate repository mounted with virtiofs while supervisor state stays outside it;
- a private D-Bus bus per VM and QEMU D-Bus display listener;
- host framebuffer reconstruction from `Scanout` and rectangular `Update` calls;
- cursor events and keyboard/mouse injection through QEMU console interfaces;
- monotonic timestamps recorded before input is sent;
- PNG capture, browser-navigation smoke gate, and a drag gate that requires display updates between pointer-down and pointer-up.
- a canonical SQLite timeline and immutable SHA-256 artifact store outside the candidate workspace;
- deterministic repository fingerprints covering Git HEAD, index, worktree, untracked files, deletions, symlinks, and executable mode;
- offline `observe`, `history`, `frame`, and `replay` commands backed by raw scanout/update artifacts;
- a Codex App Server stdio client that records thread, turn, message, command, file-change, MCP, and approval traffic with repository fingerprints;
- a `codex exec --json` baseline recorder;
- browser navigation, DOM, accessibility, console, network, performance, screenshot, and trace observation through a supervisor-owned CDP tunnel;
- pixel-matched browser-to-framebuffer coordinate correlation and deterministic duplicate-submit failure diagnosis;
- bounded temporal analysis of full scanouts and rectangular updates, including delayed or absent application response, repeated regions, A-B-A reversions, and exact pixel translations;
- QEMU-backed click actuation with separately timestamped move, pointer-down, and pointer-up receipts;
- event-triggered, provider-neutral VLM observations over content-addressed before/during/after frames, retained separately from direct evidence;
- evaluator-owned policy phases, evidence debt, structured declarations and diagnoses, immutable evidence records, and fingerprint-bound workspace promotion;
- supervisor-owned staging so Codex mutations cannot reach the candidate before policy-controlled promotion.

## Local supervisor and remote VM

The supported deployment keeps Codex local. Use authenticated `gcloud compute ssh` with an explicit project, zone, and instance as the control channel. AVM's `remote-channel-create` command records that channel, and `remote-publish` transfers a fingerprinted candidate through it. Guest SSH and Chromium CDP are tunneled through the GCE host when required. Device-code authorization on the VM is neither required nor recommended for this architecture.

The run configuration is boot-bound: after the outer GCE host reboots, create a new run instead of appending events to a run associated with the previous host boot. Stop nested QEMU and the outer GCE instance when they are not in use.

Build the guest on the Linux host as described in [vm/image/README.md](vm/image/README.md), then:

```sh
cargo build --release
target/release/avm create-run \
  --base-image /var/lib/avm/images/noble-v1/avm-base.qcow2 \
  --candidate /srv/candidates/example \
  --state-root /var/lib/avm/runs
target/release/avm start --run /var/lib/avm/runs/RUN_ID/run.json
target/release/avm capture --run /var/lib/avm/runs/RUN_ID/run.json --output /tmp/current.png
```

QEMU starts paused without the non-migratable vhost-user filesystem device, records an internal full-VM snapshot named `avm-clean` on the writable `os` qcow2 node, hot-adds the candidate virtiofs device, and then starts the guest. `reset` restarts QEMU paused, restores that snapshot through QMP, hot-adds virtiofs again, and resumes execution; it does not preserve guest disk mutations. QEMU 6.0 or newer is required for the native `snapshot-save` and `snapshot-load` commands.

Run `scripts/linux-smoke.sh RUN_CONFIG` for the acceptance sequence. It does not report success unless QEMU provides a real framebuffer and a display update occurs after injected input; its drag check additionally requires an update while the pointer is held down.

After an interaction, the experience remains queryable even when QEMU is stopped:

```sh
target/release/avm observe --run /var/lib/avm/runs/RUN_ID/run.json
target/release/avm history --run /var/lib/avm/runs/RUN_ID/run.json --source input
target/release/avm frame --run /var/lib/avm/runs/RUN_ID/run.json --at-ns MONOTONIC_NS --output /tmp/frame.png
target/release/avm replay --run /var/lib/avm/runs/RUN_ID/run.json --last-duration-ms 10000
target/release/avm act-click --run /var/lib/avm/runs/RUN_ID/run.json --x 640 --y 360 --wait-after-ms 1000
target/release/avm temporal-analyze \
  --run /var/lib/avm/runs/RUN_ID/run.json \
  --start-ns ACTION_START_NS --end-ns OBSERVATION_END_NS
```

`temporal-analyze` reconstructs consecutive same-sized full scanouts as well as rectangular display updates. It records its derived result back into the run timeline as `perception.temporal.analysis`. Small connected pixel changes near a recent pointer destination are retained as `display.cursor_only_change` evidence but cannot satisfy an application visual-response claim. Exact translated components are reported with source and destination bounds, displacement, and pixel match ratio.

To interpret an interesting temporal result with a multimodal model, configure a direct executable adapter:

```json
{
  "program": "/opt/avm/bin/my-vlm-adapter",
  "args": [],
  "model": "provider/model-name",
  "modelVersion": "pinned-version-or-snapshot"
}
```

```sh
target/release/avm vlm-observe \
  --run /var/lib/avm/runs/RUN_ID/run.json \
  --adapter-config /etc/avm/vlm-adapter.json \
  --temporal-event-id TEMPORAL_ANALYSIS_EVENT_ID
```

The adapter is started directly without a shell. It receives one JSON request on stdin containing the constrained prompt plus three PNG artifact paths and hashes, and must return `{"output": ...}` on stdout. AVM invokes it only for eligible temporal observations such as delayed response, repeated updates, state reversion, or pixel translation. The resulting `perception.vlm.observation` event records the model and version, prompt, output, trigger, timestamp, and portable input artifact hashes with `model_interpreted` provenance. It neither alters nor replaces the underlying `derived` temporal event.

Create an externally owned session and run Codex under its supervisor store:

```sh
SESSION_JSON=$(target/release/avm session-create \
  --candidate /srv/candidates/example \
  --state-root /var/lib/avm/supervisor)

target/release/avm codex-turn \
  --session /var/lib/avm/supervisor/SESSION_ID/session.json \
  --approval decline \
  --approval-policy on-request \
  --prompt 'Inspect this application and report what you observe.'

target/release/avm codex-exec \
  --candidate /srv/candidates/example \
  --state-root /var/lib/avm/baselines \
  --prompt 'Run the targeted tests and report the result.'
```

To record Codex into the same canonical timeline as a VM run, select the run instead of creating a standalone supervisor session:

```sh
target/release/avm codex-turn \
  --run /var/lib/avm/runs/RUN_ID/run.json \
  --approval decline \
  --approval-policy on-request \
  --prompt 'Modify the mounted application, verify it, and summarize the result.'
```

`--run`, `--session`, and the standalone `--candidate`/`--state-root` pair are mutually exclusive. Supervisor state must be outside the candidate workspace. App Server wire schemas generated from the pinned CLI are under `supervisor/codex/schema`; see its README before upgrading Codex.

For a policy-controlled session, initialize policy against the session, submit a structured declaration before mutation, and collect evidence afterward:

```sh
target/release/avm policy-init --target /var/lib/avm/supervisor/SESSION_ID/session.json
target/release/avm policy-declare \
  --policy /var/lib/avm/supervisor/SESSION_ID/policy/policy-state.json \
  --input declaration.json
target/release/avm codex-turn \
  --session /var/lib/avm/supervisor/SESSION_ID/session.json \
  --policy /var/lib/avm/supervisor/SESSION_ID/policy/policy-state.json \
  --approval decline \
  --approval-policy on-request \
  --prompt 'Make the declared change and run the targeted verifier.'
target/release/avm evidence-command \
  --policy /var/lib/avm/supervisor/SESSION_ID/policy/policy-state.json \
  --expected-exit-code 0 -- ./check.sh
target/release/avm policy-status \
  --policy /var/lib/avm/supervisor/SESSION_ID/policy/policy-state.json
```

Contradictory evidence moves policy to `EVIDENCE_FAILED`. Further declarations and edits remain blocked until `policy-diagnose` records a causal diagnosis and a new discriminating observation is acquired. `evidence-list` exposes the immutable evidence ledger. For UI claims, use `browser-observe`, inject input through AVM, run `browser-correlate`, then register the bounded before/input/display/after proof with `evidence-browser`. Browser window metrics are recorded only as unverified estimates; the authoritative coordinate mapping is the pixel correlation against the QEMU framebuffer.

`scripts/qemu-protocol-smoke.sh [EVIDENCE_DIRECTORY]` is a smaller integration check for development hosts without KVM. It boots a 512-byte VGA fixture under QEMU TCG and requires a real D-Bus scanout followed by injected keyboard events, guest acknowledgments/repaints, a changed host framebuffer, and an exact return to the pre-input framebuffer after QMP snapshot restore. When an empty evidence directory is supplied, the raw timeline, screenshots, disk, logs, exact QEMU arguments, environment, result, and a complete checksum manifest are retained there. Passing it validates the protocol bridge and reset mechanism, but it is not a substitute for the full KVM/Weston/Chromium acceptance sequence.

## Current boundary

This is not the completed research system. Milestones one through six have passed their current acceptance gates on a real GCE Linux/KVM host: VM lifecycle/reset, persistent experience, local Codex App Server supervision over a remote channel, authoritative browser observation/failure diagnosis, evaluator-owned policy plus evidence enforcement, and temporal perception. The temporal gate used a deterministic guest fixture to prove cursor movement without application response, delayed response, repeated region changes with A-B-A reversions, and an exact 160-pixel translation. A provider-neutral, event-triggered VLM adapter is implemented and verified with reconstructable frame provenance; a live provider call is deployment configuration rather than a fixed dependency. Broader experience queries, guest AT-SPI, runtime/audio sources, the evaluator application, and the controlled experiment remain later milestones.
