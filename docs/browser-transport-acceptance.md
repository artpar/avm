# Browser transport acceptance

The controlled-experiment browser channel was diagnosed and retested on the real Linux/KVM gate on 2026-08-14 UTC (2026-08-15 IST).

## Diagnosis

On a fresh nested guest, `curl http://127.0.0.1:9222/json/version` succeeded inside the guest. The same request through QEMU's `hostfwd=tcp:127.0.0.1:9222-:9222` connected and then reset. The preserved failed-trial timelines recorded Playwright's exact error as `connectOverCDP: socket hang up`.

The guest Chromium instance accepted CDP on guest loopback. QEMU user networking forwarded to the guest NIC address, so its fixed host-forward could not reach that listener. A guest SSH tunnel from GCE loopback port 9223 to guest loopback port 9222 succeeded and returned Chrome 151 browser metadata with a `ws://127.0.0.1:9223/...` debugger URL.

## Fix and runtime result

Rich experiment conditions now create a dedicated SSH CDP tunnel with `ExitOnForwardFailure=yes`, require `/json/version` readiness before starting Codex, configure the MCP observer for port 9223, and terminate the tunnel during cleanup. Browser-observer failures also surface recorded stderr through the command/MCP error instead of returning only an exit status.

Run `f6d36f1a-0566-4cf2-9edd-683c65d8def0` then exercised the real Playwright observer through the new transport. AVM returned five canonical events:

- `browser.page.snapshot`
- `browser.performance.metrics`
- `browser.observer.started`
- `browser.observer.completed`
- `browser.trace.stored`

The stored trace artifact is `sha256:425b3ec7186bd60dae9f79d486b19a7e6bd7394544669fecb0641d6a7eb8552c`; the viewport artifact is `sha256:8ccba555b78102278abc6eeac45ef786faca9684eb6f9259344774550a0e1515`.

External evidence is stored in `/Users/artpar/workspace/code/avm-evidence/browser-transport-9223`:

| File | SHA-256 |
| --- | --- |
| `browser-history.json` | `001a1fd414f1a3f25a825ecf481171ae6995cb00180b26f051d55310bcedb292` |
| `events.jsonl` | `77e1dcd1457e703b19308158de662b1abb4f29a615dc2c801ea9ef075211c872` |
| `run.json` | `8dddea7c79d44df604d9e18e38d2fae946a77c31f4d7d1df8879cc5725c2acfc` |
| trace artifact | `425b3ec7186bd60dae9f79d486b19a7e6bd7394544669fecb0641d6a7eb8552c` |
| viewport artifact | `8ccba555b78102278abc6eeac45ef786faca9684eb6f9259344774550a0e1515` |

The tunnel and nested QEMU process were verified absent after the run. The outer GCE instance was stopped and verified terminated.
