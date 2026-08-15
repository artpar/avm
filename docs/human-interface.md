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

The default causal-trace workspace uses horizontal time and semantic source
lanes. Events are compact marks; honest relations connect events only when AVM
has a recorded reference or shared artifact. A density strip provides whole-run
orientation and exposes quiet or missing spans.

The **Events** perspective presents the same filtered evidence as a compact,
scannable table. Selecting an event in either perspective opens the persistent
evidence panel, whose modes are:

- Summary: type, source, time, provenance, fingerprint, and relationship basis.
- Frame: the reconstructed display at that point in time, when display evidence
  permits it.
- Payload: structured event data with stable keys.
- Artifacts: content-addressed objects, MIME hints, and verified downloads.
- Provenance: recorded/derived/inferred status and source metadata.

The source rail, provenance filter, and semantic text search narrow the view
without rewriting the underlying record. Disabling every source is an explicit
empty selection rather than an implicit reset to all sources. The UI states
event limits and truncation rather than silently omitting data.

## Semantics and accessibility

The interface is strictly grayscale. Shape, texture, border, typography, and
labels carry state, so no distinction relies on color. Native controls,
landmarks, table semantics, visible focus, reduced-motion support, and keyboard
shortcuts target WCAG 2.2 AA. Dense does not mean cryptic: abbreviated data has
an accessible name or nearby explanation.

## Non-goals

The WebUI does not start, stop, reset, type into, click, approve, publish, or
otherwise control a run. It is not a general dashboard, live IDE, or replacement
for machine-readable AVM commands. Writable workflows remain explicit CLI/MCP
operations outside the inspector process.
