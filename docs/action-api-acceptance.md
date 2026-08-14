# Guest action API acceptance

The complete guest action API added in commit `fbd4901` was exercised on the real Linux/KVM gate on 2026-08-14 UTC (2026-08-15 IST).

- GCE host: `avm-kvm-gate-20260814`, project `agent4-471206`, zone `asia-south1-a`
- deployed source: Git archive of `fbd4901`, SHA-256 `8bafcc5bf01376fbe1b5a003231d485aa065cbd037705f309e43a73227f3a898`
- run ID: `8e04ff5e-b43e-484b-84ee-c6e5629ede07`
- host boot ID: `bda77ce9-87cf-4de3-93ab-c51226f252f9`
- base image: `/home/artpar/avm-state/images/noble-v7/avm-base.qcow2`
- candidate: `/home/artpar/experiment-candidates/rich-demo-1`

The Linux release build completed successfully. The host did not have the optional Clippy component installed, so strict Clippy was run locally instead; `cargo clippy --all-targets --all-features -- -D warnings` passed on the same source. The local Rust suite also passed all 68 tests and the MCP check reported 10 tools and all 12 action variants.

The runtime acceptance invoked pointer movement, mouse down, mouse up, click, double-click, drag, scroll, key down, key up, key press, text typing, and wait through QEMU input. All 12 calls returned distinct completed action receipts. Canonical history contains exactly 12 corresponding `input.action.completed` events. Compound actions retained one action ID across their low-level phases; in particular:

- click: move, down, up, completion
- double-click: move, two down/up pairs, completion
- drag: initial move, down, four trajectory moves, up, completion
- key press: down, up, completion
- text typing: type marker, six key transitions for `avm`, completion
- scroll: scroll marker, two wheel steps, completion

External immutable evidence is stored in `/Users/artpar/workspace/code/avm-evidence/action-api-fbd4901`:

| File | SHA-256 |
| --- | --- |
| `action-history.json` | `a41de2a42fe6fb064a4c4039e6f55eb150863131b30d9a5c7c4defd8bb4eca64` |
| `events.jsonl` | `6df25123f19201bafe9b2f7386ca0c02c45d57533ee3125be8bacc80ce42aeaf` |
| `run.json` | `d6c63436cae95ea7594145d2cc5afeb99bb72d1e02c9cc0f0138b5c49db725cd` |

The first `capture` readiness probe timed out because this guest produced changing full scanouts roughly every 180 ms and therefore never satisfied the stable-frame heuristic. The raw display stream remained healthy and recorded 1280×800 observed scanouts. This was diagnosed before action execution and does not weaken the input-only result; it remains a caveat for capture UX on continuously animated screens.

Nested QEMU was stopped and verified absent with a full-command-line process match. The outer GCE instance was then stopped and verified `TERMINATED`.
