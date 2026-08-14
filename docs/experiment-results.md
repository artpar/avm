# Controlled experiment results

## Result

The accepted eight-trial experiment does **not** support the claim that rich workstation perception improved software outcomes on the retry-duplicate task. The rich-perception main effect was exactly 0.0 functional defects. Rich conditions reduced the operational rework proxy by 0.75 file-change events and regressions by 0.25, but took about 197 seconds longer, made 12.25 more tool calls, consumed about 619,785 more tokens, and used 81 more product interactions on average.

Condition C (rich perception without gating) had the lowest mean functional-defect count at 1.0, but B (rich perception plus gating) had the highest at 2.0. That produced a +1.0 factorial interaction and no general rich-perception benefit. Evidence gating increased the functional-defect mean by 0.5 in this sample. No condition earned hidden-defect or diagnosis credit. The correct conclusion is that the complete workstation was real and measurable, but did not improve functional outcomes on this task with this model and two repetitions per condition.

## Design and integrity

The accepted run is `/Users/artpar/workspace/code/avm-evidence/experiments/controlled-v1/run-5`. It used:

- Codex `gpt-5.6-terra` and Codex CLI 0.146.0;
- pinned repository commit `63424d89a78d5a3d94d153fa6cca3f92beca6119`;
- dependency digest `sha256:65a394dd805b23e716f69a7cab6d10d23dee6d7b0c25740e23011c5857980e62`;
- VM image digest `sha256:8e519f7b40bb5ebb3adb5dc7e44069d63bdbc77686ac4e124d4ca44c5273cfe8`;
- the same task, model settings, sandbox, approval policy and resource allowance in all conditions;
- two deterministic randomized repetitions of A, B, C and D.

All eight agents and all eight blind evaluators exited 0 without a timeout. Every requested metric, including model tokens, rework and failed attempts, was numeric in every trial. Ordinary A/C Codex Exec JSONL streams were preserved, while B/D retained their complete App Server streams in the external supervisor store. Candidate workspaces were fresh; rich trials used fresh nested-guest overlays. Codex and its credentials remained local.

The repaired CDP transport was proven in a no-model preflight that recorded 21 browser events, 47 guest interactions and trace `sha256:0564595503e8e03b8b7aeb910e6bcbbd0b52fe68d491941b1d07440fc435921c`. Across the four rich trials, all seven agent-requested `avm_browser_observe` calls completed successfully and returned content-addressed traces. The GCE instance was `TERMINATED` after the batch.

Integrity hashes:

| File | SHA-256 |
| --- | --- |
| `run-5/manifest.json` | `7a1a633a6d702eb0c52327bf1ce5d15aa4e5e3955049f6ad23312db776ac24ba` |
| `run-5/schedule.json` | `e62de4361db0932a820de4950e9573525a28261700c621d5d6724824bd3663d1` |
| `run-5/results.jsonl` | `63ec3f73bd9239601f4e59b086aa3a1676ecfa9613f2ba03b125814f523a15d5` |
| `run-5/analysis.json` | `459f8c723a84760c5fe7f9c7dcbc449fd3eab01a6591f25f6699d308f49b12e5` |
| preflight result | `78de6bd8790306780edf8ca9fab08011cd7ca5688e7681cdd7a28d90dff4bccd` |

## Condition means

| Condition | Functional defects | Regressions | Rework | Failed attempts | Time (s) | Tool calls | Model tokens | Product interactions |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A | 1.5 | 0.0 | 2.0 | 1.0 | 157.5 | 8.0 | 403,589 | 0 |
| B | 2.0 | 0.5 | 0.5 | 1.0 | 307.8 | 14.0 | 741,035.5 | 53 |
| C | 1.0 | 0.0 | 0.5 | 1.0 | 374.3 | 24.0 | 1,113,753 | 109 |
| D | 1.5 | 1.0 | 0.5 | 1.0 | 129.6 | 5.5 | 211,629 | 0 |

`rework` is the documented operational proxy: file-change completions after a failed post-mutation command. `failed attempts` counts failed command executions after the first mutation. Sensory MCP failures are not mislabeled as implementation failures.

## Factor estimates

Effects are enabled minus disabled. Negative defect/rework values favor the enabled factor.

| Estimate | Functional defects | Regressions | Rework | Failed attempts | Time (s) | Tool calls | Model tokens | Product interactions |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Rich-perception main effect | 0.0 | -0.25 | -0.75 | 0.0 | +197.5 | +12.25 | +619,785.25 | +81 |
| Evidence-gating main effect | +0.5 | +0.75 | -0.75 | 0.0 | -47.2 | -6.25 | -282,338.75 | -28 |
| B − C − D + A interaction | +1.0 | -0.5 | +1.5 | 0.0 | -38.5 | -7.5 | -180,757.5 | -56 |

## Exclusions and limitations

Runs 1–3 exposed lifecycle, SSH-readiness and App Server configuration defects and were aborted or excluded. Run 4 completed, but post-run audit proved its rich browser calls failed because QEMU forwarded to the guest NIC while Chromium accepted CDP on guest loopback. It remains historical evidence only. Run 5 followed the dedicated guest-loopback SSH tunnel, readiness gate, successful no-model preflight and complete-metric fixes.

The accepted result still has only two repetitions per condition, one task, one model and a scorer with two binary functional checks. Diagnosis accuracy remained zero across all conditions. The operational rework metric is a deterministic event proxy, not a human judgment of edit quality. These results answer the required experiment honestly for the tested scope; they do not establish a universal negative result for workstation perception.

Recompute the report with:

```sh
node experiments/analyze.mjs \
  /Users/artpar/workspace/code/avm-evidence/experiments/controlled-v1/run-5/results.jsonl
```
