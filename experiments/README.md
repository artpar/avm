# Controlled experiment runner

`runner.mjs` enforces one global model, model settings, repository, task, starting VM snapshot, dependency digest, and resource allowance. It requires exactly these capability conditions:

- A: ordinary Codex, no rich perception and no evidence gating;
- B: rich workstation perception plus evidence gating;
- C: rich perception without gating;
- D: gating without rich perception.

At least two repetitions are required. Trials use deterministic randomized order, fresh repository copies, direct executable commands, blind labels, the same evaluator command, and structured results covering functional/hidden/user-facing/temporal defects, requirement errors, regressions, rework, failed attempts, time/tool/model cost, interactions, diagnosis accuracy, interventions, and catastrophic implementations. The evaluator receives a workspace and blind label but no condition label.

`manifest.fixture.json` uses mock commands only. Run `node experiments/check.mjs` from the repository root to prove eight trial executions and the A/B/C/D design. A real manifest must point every condition at the same pinned local Codex build/model settings and use condition-specific supervisor flags or prompts only to expose the assigned capabilities. Do not authenticate Codex inside the VM; agent commands run on the local supervisor and reach the GCE/KVM guest through the explicit channel.

`real-agent.mjs` is the evaluator-owned command for a real manifest. It accepts an
external deployment config, condition, fresh workspace, trial directory, and
task. A/C use Codex Exec; B/D use the App Server workspace gate with a structured
pre-edit declaration and an independently recorded post-batch command. B/C
receive the narrow AVM MCP tools. For those rich conditions, the wrapper starts
a fresh QEMU overlay, publishes the candidate, runs the candidate server inside
the nested guest at `/workspace`, and keeps the private fault proxy on the GCE
host behind an SSH tunnel. A separate SSH tunnel connects GCE loopback port
9223 to Chromium's guest-loopback CDP port, and a `/json/version` readiness gate
must pass before Codex starts. Candidate code therefore cannot read host supervisor
state, evaluator tests, credentials, or the base image. Cleanup stops QEMU and
both tunnels, then stops GCE even after an agent failure.

The wrapper preserves ordinary Codex Exec JSONL as `codex-events.jsonl`; gated
trials already retain the complete App Server stream in their supervisor event
log. Token cost uses each stream's authoritative cumulative total. Failed
implementation attempts are failed command executions after the first file
mutation, while rework counts later file-change completions after such a
failure. MCP sensor failures are reported separately as `toolFailures` and are
not mislabeled as failed implementations.

Run `node experiments/real-agent-check.mjs` to validate the four capability
plans without contacting GCE. `real-evaluator.mjs` combines a blind hidden score
with externally recorded duration, tool/model use when available, and product
interactions; `node experiments/real-evaluator-check.mjs` verifies that merge.
The deployment config and real manifest are environment-specific evidence and
must stay outside the candidate repository.

Before a real batch, run one rich condition with `--preflight`. This performs
the exact guest, target, proxy, application tunnel, CDP tunnel, browser observer,
timeline and cleanup path without starting Codex, and writes
`preflight-result.json`. A batch should not start unless the result contains a
positive `browserEventCount` and a content-addressed browser trace.

Run `node experiments/analyze.mjs PATH/TO/results.jsonl` to validate a completed
balanced run and calculate per-condition means, rich-perception and
evidence-gating main effects, and the factorial interaction. Null measurements
remain unavailable rather than being coerced to zero.
