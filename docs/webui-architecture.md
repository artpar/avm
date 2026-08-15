# WebUI architecture and trust boundary

The WebUI is embedded in the AVM binary and is started automatically by
`avm start`. The parent launches a hidden internal server process bound to a
random `127.0.0.1` port. Its endpoint and PID are supervisor lifecycle state;
the browser receives no direct filesystem access.

The lifecycle record also contains a fresh UUID passed as an exact child-process
argument. AVM verifies that non-reusable token in the live process before
sending SIGTERM, so a stale or recycled PID cannot target an unrelated process.
If startup fails or times out, the parent terminates and reaps the child before
removing its endpoint files.

## Read path

```text
browser -> loopback Axum server -> read-only SQLite + verified artifact store
                                     -> frame reconstruction in memory
```

SQLite is opened with read-only flags and no schema or journal changes. An
absent artifact tree is represented as an empty store without creating it;
each present artifact is SHA-256 verified when read. Reconstructed PNG frames
remain in memory and are not inserted as derived artifacts. API responses are
bounded, and truncation is explicit.

The server supplies a run summary with per-source coverage, filtered events,
whole-run aggregates, verified artifacts, selected-time display status, and
reconstructed frames. `GET /api/frame-status?timeNs=…` distinguishes no
collection, a time before capture, an available persisted display state, and a
disabled or invalidated display state, and a reconstruction defect. Only
accepted scanout/update evidence contributes framebuffer state; disable and
full-scanout recovery boundaries clear it. `GET /api/frame-at?timeNs=…` returns
the reconstruction at that exact machine time. Relationship edges are emitted only for
explicit event references or shared artifact references. Temporal proximity is
not presented as causality.

Every failed API response uses the same JSON problem shape: stable `code`, human
`message`, diagnostic `detail`, `requestId`, and `recoverable`. The server writes
the same request ID and code as structured stderr so the non-modal problem shelf
can be correlated with supervisor logs without obscuring unaffected evidence.

## Browser boundary

Static HTML, CSS, and JavaScript are compiled into the binary. The browser maps
raw collectors into equal-status semantic lanes while preserving unknown
collectors as additional lanes; this mapping is presentation only and never
rewrites evidence. Trace and Events share focus range, cursor, filters, and URL
state. Responses carry
a restrictive Content Security Policy and defensive headers. The interface has
GET routes only and binds to loopback; it has no mutation API. The WebUI uses no
third-party browser runtime dependencies, remote fonts, analytics, or external
requests.

Loopback binding protects against network exposure, not hostile software already
running as the same host user. AVM remains a pre-1.0 research system, not a
multi-tenant evidence service.

## Lifecycle

- `start`: starts QEMU, records lifecycle evidence, then starts the inspector.
- `status`: reports VM state and the live inspector URL.
- `stop` / `destroy-run`: terminate the inspector and remove its ephemeral
  endpoint files.
- `reset`: keeps the inspector endpoint stable while the run restarts.

Server logs and endpoint metadata live in the supervisor-owned run directory.
The evidence database and content-addressed artifacts remain read-only to the
server.
