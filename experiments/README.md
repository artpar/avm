# Controlled experiment runner

`runner.mjs` enforces one global model, model settings, repository, task, starting VM snapshot, dependency digest, and resource allowance. It requires exactly these capability conditions:

- A: ordinary Codex, no rich perception and no evidence gating;
- B: rich workstation perception plus evidence gating;
- C: rich perception without gating;
- D: gating without rich perception.

At least two repetitions are required. Trials use deterministic randomized order, fresh repository copies, direct executable commands, blind labels, the same evaluator command, and structured results covering functional/hidden/user-facing/temporal defects, requirement errors, regressions, rework, failed attempts, time/tool/model cost, interactions, diagnosis accuracy, interventions, and catastrophic implementations. The evaluator receives a workspace and blind label but no condition label.

`manifest.fixture.json` uses mock commands only. Run `node experiments/check.mjs` from the repository root to prove eight trial executions and the A/B/C/D design. A real manifest must point every condition at the same pinned local Codex build/model settings and use condition-specific supervisor flags or prompts only to expose the assigned capabilities. Do not authenticate Codex inside the VM; agent commands run on the local supervisor and reach the GCE/KVM guest through the explicit channel.
