# Human interface

## Audience and purpose

The AVM WebUI is for people using AVM while developing their own software. It
lets them inspect the complete collected timeline and its artifacts as one
semantic record. It does not control the guest or mutate evidence.

`avm start` always starts the inspector beside the VM and returns its loopback
URL as `webUi`. `avm status` reports the same URL while the process is alive.
`avm stop` and `destroy-run` stop it. There is no separate user-facing WebUI
command.

## Information architecture

The default **Trace** workspace is a multiscale, shared-time view of the whole
machine experience. CPU, disk, network, logs, browser, input, display, audio, VM
lifecycle, and future collectors are equal-status evidence lanes. No source is
the default center of the product. Honest relations appear only when AVM has a
recorded reference or shared artifact.

A whole-run ruler, focus range, and selection cursor synchronize every lane.
Below the lanes, logs, network activity, artifacts, and structured selected-time
evidence remain visible together. Their relative size may be adjusted or one
region temporarily maximized, but basic inspection never requires evidence tabs.

The **Events** perspective presents the same evidence as a compact chronological
ledger. It shares the trace's time range, selection, filters, and URL state; it
is an alternate representation rather than a separate workflow.

Display evidence appears as compact thumbnails and artifacts alongside other
sources. When no display evidence exists at the selected time, AVM shows a black
frame explicitly labeled as absent evidence. It does not substitute a nearby
frame. Recorded black output remains distinguishable from absence through its
timestamp, artifact identity, and provenance.

The source rail, provenance filter, and semantic text search narrow the view
without rewriting the underlying record. Disabling every source is an explicit
empty selection rather than an implicit reset to all sources. The UI states
event limits and truncation rather than silently omitting data.

Absence is specific rather than generic for every source. The interface
distinguishes not collected, outside capture bounds, a collection gap, a true
zero/silence/black value, a missing or corrupt artifact, truncation, and a query
or reconstruction defect. Failed API requests expose a stable code, request ID,
HTTP status, and copyable diagnostic detail correlated with structured logs.

## Semantics and accessibility

The interface is strictly grayscale. Shape, texture, border, typography, and
labels carry state, so no distinction relies on color. Native controls,
landmarks, table semantics, visible focus, reduced-motion support, and keyboard
shortcuts target WCAG 2.2 AA. Dense does not mean cryptic: abbreviated data has
an accessible name or nearby explanation.

The read-only trust boundary is a system property, not a recurring status badge.
The interface demonstrates it by offering no mutation controls; architecture and
help documentation explain the guarantee when a user needs it.

## Non-goals

The WebUI does not start, stop, reset, type into, click, approve, publish, or
otherwise control a run. It is not a general dashboard, live IDE, or replacement
for machine-readable AVM commands. Writable workflows remain explicit CLI/MCP
operations outside the inspector process.
