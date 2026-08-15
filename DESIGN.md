# AVM interface system

AVM is a dense, read-only instrument panel for understanding the complete
recorded machine experience. Its interface is evidence, not decoration: every
mark should identify activity, an event, artifact, relationship, boundary, gap,
or collection state.

## Visual language

- Grayscale only. Meaning must never depend on hue.
- System sans for interface copy; system monospace for time, IDs, hashes,
  source names, and payloads.
- A compact 4 px spacing rhythm with clear section boundaries and restrained
  use of borders.
- Solid dark selection, hatched inferred state, outline-only recorded state,
  and dotted unknown or missing state.
- No ornamental gradients, shadows, glass effects, illustrations, or motion.
- Minimum 4.5:1 text contrast and visible keyboard focus. Respect reduced
  motion and preserve useful layouts at 200% zoom.

## Interaction language

The primary workspace is a multiscale causal trace with one shared time axis and
equal-status lanes for every collected source. A switch exposes the same
evidence as an event ledger. Selecting a time, event, or artifact synchronizes
all lanes plus the persistent logs, network, artifacts, and structured evidence
regions below. Filters change what is shown; they never change the run.

Display is one evidence source among CPU, disk, network, logs, browser, input,
audio, VM lifecycle, and future collectors. It must not dominate the default
layout. No basic evidence is hidden behind mutually exclusive tabs.

The interface has no VM, input, policy, deletion, or artifact mutation controls.
It labels raw observation, deterministic derivation, model interpretation, and
agent claim explicitly and never draws a causal edge without a recorded basis.

Keyboard conventions: `/` focuses search, `T` selects trace, `A` selects the
event ledger, left/right moves between meaningful evidence times, plus/minus
zooms, and Escape restores a maximized evidence region. Every shortcut has a
visible equivalent.

## Product test

A user should be able to answer three questions without consulting raw files:

1. What was the whole machine doing across this time range?
2. What did every collected source show around this selected moment?
3. Where is evidence absent, truncated, derived, corrupt, or uncertain?

See [docs/human-interface.md](docs/human-interface.md) for product behavior and
[docs/webui-architecture.md](docs/webui-architecture.md) for trust boundaries.
