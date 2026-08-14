# Controlled experiment results

## Result

This eight-trial experiment does **not** support the claim that rich workstation
perception improved software outcomes on the retry-duplicate task. The estimated
rich-perception main effect was 0.0 functional defects, while rich trials took
about 206 seconds longer and made 13.5 more tool calls on average. Evidence
gating was associated with 0.5 fewer functional defects and 0.5 fewer
regressions, but two repetitions per condition are far too few for a reliable
causal conclusion and the factorial interaction was large.

No condition produced a defect-free mean, no trial received hidden-defect or
diagnosis credit, and no catastrophic implementation or human intervention was
recorded. The correct conclusion is that this implementation established the
measurement path but did not demonstrate the core product-quality hypothesis.

## Design and integrity

The run used Codex `gpt-5.6-terra`, Codex CLI 0.146.0, pinned repository commit
`63424d89a78d5a3d94d153fa6cca3f92beca6119`, dependency digest
`sha256:65a394dd805b23e716f69a7cab6d10d23dee6d7b0c25740e23011c5857980e62`,
and VM image digest
`sha256:8e519f7b40bb5ebb3adb5dc7e44069d63bdbc77686ac4e124d4ca44c5273cfe8`.
The deterministic shuffled schedule contained two repetitions of each condition:

- A: ordinary Codex
- B: rich perception plus evidence gating
- C: rich perception without evidence gating
- D: evidence gating without rich perception

All eight agents and all eight blind evaluators exited 0 without a timeout. Each
condition has exactly two records. Candidate code ran in a fresh workspace; rich
trials used a fresh nested-guest overlay. Codex and its credentials remained on
the local host. The GCE instance was terminated after the batch.

Three earlier batches are excluded. They exposed, respectively, preflight
lifecycle defects, a GCE SSH-readiness race, and an unsupported Codex App Server
flag. The final run began only after isolated no-model and rich+gated preflights
passed, and the runner was changed to abort rather than advance after a failed
trial.

## Condition means

| Condition | Functional defects | Regressions | Time (s) | Tool calls | Product interactions | Model tokens |
|---|---:|---:|---:|---:|---:|---:|
| A | 2.0 | 0.5 | 144.3 | 7.5 | 0.0 | 614,657 |
| B | 1.5 | 0.0 | 362.5 | 20.5 | 53.0 | unavailable |
| C | 1.5 | 0.5 | 335.6 | 20.5 | 85.5 | 1,017,528.5 |
| D | 1.0 | 0.0 | 141.6 | 6.5 | 0.0 | unavailable |

The App Server event path did not expose model-token usage in B/D, so token
effects are intentionally reported as unavailable rather than estimated from
only A/C.

## Factor estimates

Effects are enabled minus disabled. Negative defect values favor the enabled
factor.

| Estimate | Functional defects | Regressions | Time (s) | Tool calls | Product interactions |
|---|---:|---:|---:|---:|---:|
| Rich-perception main effect | 0.0 | 0.0 | +206.1 | +13.5 | +69.25 |
| Evidence-gating main effect | -0.5 | -0.5 | +12.1 | -0.5 | -16.25 |
| B − C − D + A interaction | +1.0 | 0.0 | +29.6 | +1.0 | -32.5 |

The interaction and the per-condition trajectories show why the gating estimate
must not be generalized: D had the lowest mean defect count, while combining
gating with rich perception did not preserve that apparent advantage.

## Reproduction and limitations

The environment-specific manifest and raw evidence remain outside the candidate
repository. Recompute the summaries with:

```sh
node experiments/analyze.mjs /path/to/run/results.jsonl
```

The primary limitations are the sample size (`n=2` per condition), one task and
one model, no complete App Server token accounting, and a scorer whose two
functional checks do not provide a graded measure of implementation quality.
These results justify improving measurement coverage and repeating across more
tasks; they do not justify claiming a workstation benefit.
